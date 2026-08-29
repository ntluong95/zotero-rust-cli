//! Local API write-credential resolution and storage
//! (`phase-06-js-bridge-and-injection-hardening.md` §3.4a, added post-Slice-0 after
//! `zotero-10-impact-on-rust-port.md` §8.7 found that `POST /api/local/authorize` re-prompts for
//! human consent on every call, even with an existing valid "Always Allow" grant -- so a
//! stateless, per-invocation CLI process must persist the key itself to write unattended).
//!
//! Two sources, checked in order, never mixed:
//! 1. `ZOTERO_LOCAL_API_KEY` env var -- operator-owned. This module never writes, modifies, or
//!    deletes it; on rejection the caller (`write_router`) only reports the failure.
//! 2. The CLI-owned local file store, scoped to a specific `Zotero-Server-ID` (LIVE VERIFIED
//!    stable across one restart only, not universal permanence -- do not treat a stored entry as
//!    valid forever without also handling the server's own 401 rejection).
//!
//! The file lives beside `session.json` (same `session_state_dir()`/
//! `CLI_ANYTHING_ZOTERO_STATE_DIR` convention) but is a **separate file** -- never added to
//! `session.json`'s own byte-for-byte-locked 4-key schema (`session.rs`'s own documented
//! constraint). Same threat model as an SSH private key or `~/.netrc`: a restrictive-permission
//! local file, not an OS keychain (see §3.4a for why a keychain backend is out of v1 scope).

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const ENV_KEY: &str = "ZOTERO_LOCAL_API_KEY";
const CREDENTIALS_FILE_NAME: &str = "local_api_credentials.json";
const SCHEMA_VERSION: u32 = 1;

/// A Local API write credential. `Debug`/`Display` deliberately never expose `key` in full --
/// only its length, so accidental `{:?}`/log output can't leak the secret.
#[derive(Clone, Deserialize, Serialize)]
pub struct LocalApiCredential {
    pub app_name: String,
    pub key: String,
    pub remember: bool,
    pub issued_at: String,
}

impl fmt::Debug for LocalApiCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalApiCredential")
            .field("app_name", &self.app_name)
            .field("key", &format!("<redacted, {} bytes>", self.key.len()))
            .field("remember", &self.remember)
            .field("issued_at", &self.issued_at)
            .finish()
    }
}

/// Where a resolved credential came from, or that none was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSource {
    Environment,
    Store,
    None,
}

impl From<CredentialSource> for crate::write::CredentialSource {
    fn from(source: CredentialSource) -> Self {
        match source {
            CredentialSource::Environment => crate::write::CredentialSource::Environment,
            CredentialSource::Store => crate::write::CredentialSource::Store,
            CredentialSource::None => crate::write::CredentialSource::None,
        }
    }
}

#[derive(Default, Deserialize, Serialize)]
struct CredentialFile {
    version: u32,
    #[serde(default)]
    credentials: HashMap<String, LocalApiCredential>,
}

fn credentials_path() -> PathBuf {
    crate::session::session_state_dir().join(CREDENTIALS_FILE_NAME)
}

fn ensure_private_dir(dir: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        if !dir.exists() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
    }
    #[cfg(not(unix))]
    {
        // Windows: rely on the per-user profile directory's inherited ACL (same trust boundary
        // `session_state_dir()` already assumes for session.json) -- documented in
        // docs/SECURITY.md alongside the rest of this store's threat model.
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}

fn read_credential_file() -> anyhow::Result<CredentialFile> {
    let path = credentials_path();
    if !path.exists() {
        return Ok(CredentialFile {
            version: SCHEMA_VERSION,
            credentials: HashMap::new(),
        });
    }
    let raw = std::fs::read_to_string(&path)?;
    if raw.trim().is_empty() {
        return Ok(CredentialFile {
            version: SCHEMA_VERSION,
            credentials: HashMap::new(),
        });
    }
    Ok(serde_json::from_str(&raw)?)
}

/// Atomic, private-from-creation write: a uniquely-named temp file in the same directory (never
/// world-readable at any point, no create-then-chmod window), then `rename` over the final path.
/// `rename` replaces the destination directory entry itself rather than following it, so this
/// does not follow a symlink planted at `credentials_path()` the way an in-place `write` would.
fn write_credential_file(file: &CredentialFile) -> anyhow::Result<()> {
    let path = credentials_path();
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("credentials path has no parent directory"))?;
    ensure_private_dir(dir)?;

    let body = serde_json::to_string_pretty(file)?;
    let tmp_name = format!(
        ".{CREDENTIALS_FILE_NAME}.tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let tmp_path = dir.join(tmp_name);

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true).mode(0o600);
        let mut f = opts.open(&tmp_path)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&tmp_path, body.as_bytes())?;
    }

    let rename_result = std::fs::rename(&tmp_path, &path);
    if rename_result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    rename_result.map_err(anyhow::Error::from)
}

/// Priority: `ZOTERO_LOCAL_API_KEY` env var, then the file store scoped to `server_id`, then
/// nothing. Never combines the two, never validates the env value against `server_id` (the
/// operator injecting it is responsible for correctness; a mismatched key simply fails the write
/// with a normal `401`, reported without mutating the environment).
pub fn resolve_credential(server_id: &str) -> (Option<LocalApiCredential>, CredentialSource) {
    if let Ok(raw) = std::env::var(ENV_KEY) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return (
                Some(LocalApiCredential {
                    app_name: "zotero-rust-cli".to_string(),
                    key: trimmed.to_string(),
                    remember: true,
                    issued_at: String::new(),
                }),
                CredentialSource::Environment,
            );
        }
    }

    match read_credential_file() {
        Ok(file) => match file.credentials.get(server_id) {
            Some(cred) => (Some(cred.clone()), CredentialSource::Store),
            None => (None, CredentialSource::None),
        },
        Err(_) => (None, CredentialSource::None),
    }
}

/// Persists a newly-issued credential for `server_id`, keeping every other server's entry
/// untouched (§3.4a: "the credential-store schema may support multiple server IDs").
pub fn store_credential(server_id: &str, credential: &LocalApiCredential) -> anyhow::Result<()> {
    let mut file = read_credential_file().unwrap_or(CredentialFile {
        version: SCHEMA_VERSION,
        credentials: HashMap::new(),
    });
    file.version = SCHEMA_VERSION;
    file.credentials
        .insert(server_id.to_string(), credential.clone());
    write_credential_file(&file)
}

/// Removes only the entry for `server_id`, leaving every other stored credential untouched. Must
/// only ever be called when the rejected credential's source was `CredentialSource::Store` --
/// never for an environment-sourced credential, which this module never mutates.
pub fn invalidate_stored(server_id: &str) -> anyhow::Result<()> {
    let mut file = match read_credential_file() {
        Ok(file) => file,
        Err(_) => return Ok(()),
    };
    if file.credentials.remove(server_id).is_some() {
        write_credential_file(&file)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::STATE_DIR_ENV_LOCK;

    const STATE_DIR_ENV: &str = "CLI_ANYTHING_ZOTERO_STATE_DIR";

    fn temp_state_dir(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "zotero-cli-test-credentials-{}-{n}-{label}",
            std::process::id()
        ))
    }

    fn sample_credential(app_name: &str) -> LocalApiCredential {
        LocalApiCredential {
            app_name: app_name.to_string(),
            key: "test-secret-key-value".to_string(),
            remember: true,
            issued_at: "2026-08-29T00:00:00Z".to_string(),
        }
    }

    /// Guards `ZOTERO_LOCAL_API_KEY` in addition to `STATE_DIR_ENV_LOCK` -- no other test in the
    /// crate touches this env var, but two tests *within this file* still could without a lock.
    static ENV_KEY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn resolve_credential_returns_none_when_nothing_is_configured() {
        let _state_guard = STATE_DIR_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env_guard = ENV_KEY_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_state_dir("none-configured");
        // SAFETY: serialized against every other STATE_DIR_ENV/ZOTERO_LOCAL_API_KEY-mutating
        // test by the two guards above, held for this whole function.
        unsafe {
            std::env::set_var(STATE_DIR_ENV, &dir);
            std::env::remove_var(ENV_KEY);
        }

        let (credential, source) = resolve_credential("some-server-id");
        assert!(credential.is_none());
        assert_eq!(source, CredentialSource::None);

        std::fs::remove_dir_all(&dir).ok();
        unsafe {
            std::env::remove_var(STATE_DIR_ENV);
        }
    }

    #[test]
    fn env_credential_takes_priority_over_a_stored_one_and_is_never_persisted() {
        let _state_guard = STATE_DIR_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env_guard = ENV_KEY_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_state_dir("env-priority");
        unsafe {
            std::env::set_var(STATE_DIR_ENV, &dir);
        }

        store_credential("server-a", &sample_credential("stored-app"))
            .expect("store_credential must succeed");

        unsafe {
            std::env::set_var(ENV_KEY, "env-provided-key");
        }
        let (credential, source) = resolve_credential("server-a");
        assert_eq!(source, CredentialSource::Environment);
        assert_eq!(credential.unwrap().key, "env-provided-key");

        // The stored entry must be untouched by resolving through the env var.
        unsafe {
            std::env::remove_var(ENV_KEY);
        }
        let (credential, source) = resolve_credential("server-a");
        assert_eq!(source, CredentialSource::Store);
        assert_eq!(credential.unwrap().app_name, "stored-app");

        std::fs::remove_dir_all(&dir).ok();
        unsafe {
            std::env::remove_var(STATE_DIR_ENV);
        }
    }

    #[test]
    fn store_and_invalidate_only_touch_the_matching_server_id() {
        let _state_guard = STATE_DIR_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env_guard = ENV_KEY_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_state_dir("scoped-invalidate");
        unsafe {
            std::env::set_var(STATE_DIR_ENV, &dir);
            std::env::remove_var(ENV_KEY);
        }

        store_credential("server-a", &sample_credential("app-a")).expect("store a must succeed");
        store_credential("server-b", &sample_credential("app-b")).expect("store b must succeed");

        invalidate_stored("server-a").expect("invalidate must succeed");

        let (credential_a, source_a) = resolve_credential("server-a");
        assert!(credential_a.is_none(), "server-a's entry must be gone");
        assert_eq!(source_a, CredentialSource::None);

        let (credential_b, source_b) = resolve_credential("server-b");
        assert_eq!(source_b, CredentialSource::Store);
        assert_eq!(
            credential_b.unwrap().app_name,
            "app-b",
            "server-b's entry must survive server-a's invalidation"
        );

        std::fs::remove_dir_all(&dir).ok();
        unsafe {
            std::env::remove_var(STATE_DIR_ENV);
        }
    }

    #[test]
    #[cfg(unix)]
    fn credentials_file_is_created_private_from_the_start() {
        use std::os::unix::fs::PermissionsExt;

        let _state_guard = STATE_DIR_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _env_guard = ENV_KEY_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_state_dir("private-perms");
        unsafe {
            std::env::set_var(STATE_DIR_ENV, &dir);
            std::env::remove_var(ENV_KEY);
        }

        store_credential("server-a", &sample_credential("app-a")).expect("store must succeed");

        let path = dir.join(CREDENTIALS_FILE_NAME);
        let mode = std::fs::metadata(&path)
            .expect("credentials file must exist")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "credentials file must be mode 0600, not {mode:o}"
        );

        std::fs::remove_dir_all(&dir).ok();
        unsafe {
            std::env::remove_var(STATE_DIR_ENV);
        }
    }

    #[test]
    fn debug_impl_never_exposes_the_raw_key() {
        let credential = sample_credential("app-a");
        let debug_output = format!("{credential:?}");
        assert!(
            !debug_output.contains("test-secret-key-value"),
            "Debug output must never contain the raw key: {debug_output}"
        );
        assert!(debug_output.contains("redacted"));
    }
}
