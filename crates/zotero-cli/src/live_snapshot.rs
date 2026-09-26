//! Live, WAL-consistent read snapshot for when a running Zotero holds the database lock.
//!
//! A running Zotero 10 keeps `zotero.sqlite` in WAL mode under an exclusive lock, so
//! [`crate::db::connect_readonly`] cannot open it and correctly refuses rather than reading a
//! stale `immutable=1` view. Every catalog read (`collection *`, `item get/children/...`,
//! `note get`, `tag *`, `search *`, `session use-library`, item resolution for rendering) used
//! to fail in exactly the state a user is most often in.
//!
//! When that refusal happens and an owned CLI Bridge answers, this module asks the running
//! Zotero for a consistent copy of its database written with `VACUUM INTO` through Zotero's
//! *own* connection -- the same mechanism `Zotero.DBConnection#vacuum` uses, which sees every
//! committed WAL frame. `connect_readonly` then hands back a read-only connection to that copy,
//! so the existing SQL in `db.rs` runs unchanged and produces byte-identical output to the
//! offline path.
//!
//! The copy is kept on disk and reused by later CLI processes for as long as Zotero reports the
//! same **change key**: a token for Zotero's current database connection, its transaction
//! commit counter (`Zotero.DB._commitCount`), and SQLite's `total_changes()` for that
//! connection, which counts every row change including writes made outside a transaction.
//! Zotero is the only writer while it holds the lock, so an unchanged key means an unchanged
//! database. A warm read therefore costs one Bridge round trip; only a read after a change pays
//! for a new copy (~0.2 s for a 25 MB library).
//!
//! Properties:
//!
//! - **Never speculative.** Only reached after `mode=ro` has actually refused with
//!   [`crate::db::DatabaseLocked`] on a WAL database. Offline runs issue no extra requests.
//! - **Never stale.** Every process asks Zotero for the current key before reusing a copy; any
//!   write, rollback, or reopened connection changes the key and forces a new copy.
//! - **Read-only.** The Bridge template only reads the change key and runs `VACUUM INTO` a new
//!   file; no Zotero object is loaded or saved. The copy is opened read-only with `query_only`.
//! - **Private.** Copies live under the CLI state directory (`0700` directory, `0600` files on
//!   Unix) and only the latest one is kept per Zotero data directory.
//! - **Honest failure.** If no source is registered or the Bridge cannot answer, the caller
//!   returns the original refusal verbatim.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};

/// Read-only by construction: reads the change key and copies the database with `VACUUM INTO`
/// a path Rust chose. Zotero's own `vacuum()` waits for open transactions the same way, because
/// `VACUUM` cannot run inside one.
const T_SNAPSHOT: &str = r#"
var tokens = globalThis.__zoteroCliSnapshotTokens ||
  (globalThis.__zoteroCliSnapshotTokens = new WeakMap());
async function currentKey() {
  var changes = await Zotero.DB.valueQueryAsync('SELECT total_changes()');
  var conn = Zotero.DB._connection;
  var keyed = !!conn && typeof conn === 'object';
  var token = keyed ? tokens.get(conn) : null;
  if (!token) {
    token = Zotero.Utilities.randomString(12);
    if (keyed) { tokens.set(conn, token); }
  }
  return token + '-' + Zotero.DB._commitCount + '-' + changes;
}
var key = await currentKey();
if (P.have.indexOf(key) !== -1) { return JSON.stringify({key: key, reused: true}); }
var lastError = null;
for (var attempt = 0; attempt < 5; attempt++) {
  if (Zotero.DB.inTransaction()) { await Zotero.DB.waitForTransaction(); }
  // Read before copying: a write that lands in between makes the copy newer than its key,
  // which only ever causes one extra rebuild later, never a stale reuse.
  key = await currentKey();
  try {
    await Zotero.DB.queryAsync("VACUUM INTO '" + P.path.replace(/'/g, "''") + "'");
    return JSON.stringify({key: key, reused: false});
  } catch (e) {
    lastError = e;
  }
}
throw new Error('live snapshot: ' + (lastError && lastError.message ? lastError.message : lastError));
"#;

/// Where snapshot data comes from. The production source is the owned Bridge; tests supply a
/// fake that answers from a fixture database.
pub trait SnapshotSource {
    /// Answers `{op: "snapshot", have: [keys], path}` with `{key, reused}`: `reused: true` when
    /// the current key is one of `have`, otherwise after writing a consistent copy of the live
    /// database to `path`. `None` means the source cannot answer at all.
    fn request(&self, request: &Value) -> Option<Value>;
}

/// The production source: the owned CLI Bridge on `port`, whose ownership handshake runs before
/// any code is sent.
pub struct BridgeSnapshotSource {
    port: u16,
}

impl BridgeSnapshotSource {
    pub fn new(port: u16) -> Self {
        Self { port }
    }
}

impl SnapshotSource for BridgeSnapshotSource {
    fn request(&self, request: &Value) -> Option<Value> {
        let code = crate::bridge::templates::render(T_SNAPSHOT, request).ok()?;
        let resp = crate::bridge::JSBridgeClient::new(self.port).execute_js(&code, 60);
        if !resp.ok {
            // Preserve the caller's original locked-database refusal when the Bridge is absent.
            // A copy rejected inside Zotero must retain its explicit reason.
            let error = resp.error?;
            return error
                .contains("live snapshot:")
                .then(|| json!({"error": error}));
        }
        match resp.data? {
            Value::String(text) => serde_json::from_str(&text).ok(),
            other => Some(other),
        }
    }
}

struct Registration {
    source: Box<dyn SnapshotSource>,
    cache_root: PathBuf,
}

thread_local! {
    static SOURCE: RefCell<Option<Registration>> = const { RefCell::new(None) };
    /// The copy this process already validated against Zotero's key; later reads in the same
    /// invocation reuse it without another round trip.
    /// Keyed by the database path the copy was made for.
    static BUILT: RefCell<Option<(PathBuf, PathBuf)>> = const { RefCell::new(None) };
    static SUPPRESSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f` with the snapshot fallback switched off, so a locked database is refused as before.
///
/// For callers that own a cheaper, purpose-built live path (`search.rs`: `item find` and
/// `library list` answer with a single Bridge query) and need the refusal to trigger it.
pub fn without_snapshot<T>(f: impl FnOnce() -> T) -> T {
    let previous = SUPPRESSED.with(|s| s.replace(true));
    let result = f();
    SUPPRESSED.with(|s| s.set(previous));
    result
}

static INCOMING_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Registers the source used when the database is locked, and the directory copies are cached
/// under. Per-thread, so the CLI's single dispatch thread gets it and parallel tests stay
/// isolated. Forgets any copy this process already validated.
pub fn register_source(source: Box<dyn SnapshotSource>, cache_root: PathBuf) {
    SOURCE.with(|s| *s.borrow_mut() = Some(Registration { source, cache_root }));
    BUILT.with(|b| *b.borrow_mut() = None);
}

/// Removes the registered source and any validated copy (tests).
pub fn clear_source() {
    SOURCE.with(|s| *s.borrow_mut() = None);
    BUILT.with(|b| *b.borrow_mut() = None);
}

/// The cache directory for one Zotero database under `cache_root`. Keyed by the database path so
/// two data directories never share a copy.
pub fn cache_dir(cache_root: &Path, sqlite_path: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(sqlite_path).unwrap_or_else(|_| sqlite_path.into());
    // FNV-1a: stable across Rust releases, unlike `DefaultHasher`, so upgrades keep the cache.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in canonical.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    cache_root.join(format!("{hash:016x}"))
}

/// Whether this process already found `sqlite_path` locked and validated a live copy of it, so
/// later reads can skip the lock probe. Always `false` inside [`without_snapshot`].
pub fn is_validated(sqlite_path: &Path) -> bool {
    !SUPPRESSED.with(|s| s.get()) && built_for(sqlite_path).is_some()
}

fn built_for(sqlite_path: &Path) -> Option<PathBuf> {
    BUILT.with(|b| {
        b.borrow()
            .as_ref()
            .filter(|(source, _)| source == sqlite_path)
            .map(|(_, copy)| copy.clone())
    })
}

/// A read-only connection to a live snapshot of `sqlite_path`. `Ok(None)` when no source is
/// registered or the source cannot answer -- the caller then reports its refusal.
pub fn connect(sqlite_path: &Path) -> anyhow::Result<Option<Connection>> {
    if SUPPRESSED.with(|s| s.get()) {
        return Ok(None);
    }
    let path = match built_for(sqlite_path) {
        Some(path) => path,
        None => {
            let resolved = SOURCE.with(|s| {
                s.borrow()
                    .as_ref()
                    .map(|registration| resolve(registration, sqlite_path))
            });
            match resolved {
                Some(Ok(Some(path))) => {
                    BUILT.with(|b| *b.borrow_mut() = Some((sqlite_path.into(), path.clone())));
                    path
                }
                Some(Err(err)) => return Err(err),
                Some(Ok(None)) | None => return Ok(None),
            }
        }
    };
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    // Belt and braces on top of the read-only open: any write statement fails.
    conn.pragma_update(None, "query_only", true)?;
    conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |_| Ok(()))?;
    Ok(Some(conn))
}

/// Asks the source for the current key and returns the matching cached copy, creating it when
/// the key changed. Retries once without offering cached keys if a reused copy vanished (another
/// process replaced it between the answer and the open).
fn resolve(registration: &Registration, sqlite_path: &Path) -> anyhow::Result<Option<PathBuf>> {
    let dir = cache_dir(&registration.cache_root, sqlite_path);
    create_private_dir(&dir)?;
    for offer_cached in [true, false] {
        let have = if offer_cached {
            cached_keys(&dir)
        } else {
            Vec::new()
        };
        let incoming = dir.join(format!(
            "incoming-{}-{}.sqlite",
            std::process::id(),
            INCOMING_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&incoming);
        let Some(answer) = registration.source.request(&json!({
            "op": "snapshot",
            "have": have,
            "path": incoming.to_string_lossy(),
        })) else {
            let _ = std::fs::remove_file(&incoming);
            return Ok(None);
        };
        let accepted = accept(&dir, &incoming, &answer);
        let _ = std::fs::remove_file(&incoming);
        if let Some(path) = accepted? {
            return Ok(Some(path));
        }
    }
    anyhow::bail!("live snapshot: the cached copy kept disappearing while it was being opened")
}

/// Turns one source answer into the path of a complete copy. `Ok(None)` means the answer reused
/// a copy that no longer exists.
fn accept(dir: &Path, incoming: &Path, answer: &Value) -> anyhow::Result<Option<PathBuf>> {
    if let Some(error) = answer.get("error").and_then(Value::as_str) {
        anyhow::bail!("{error}");
    }
    let key = answer
        .get("key")
        .and_then(Value::as_str)
        .filter(|key| is_valid_key(key))
        .ok_or_else(|| anyhow::anyhow!("live snapshot: unexpected response: {answer}"))?;
    let target = snapshot_path(dir, key);
    if answer.get("reused").and_then(Value::as_bool) == Some(true) {
        return Ok(target.exists().then_some(target));
    }
    if !incoming.exists() {
        anyhow::bail!("live snapshot: Zotero reported a copy but wrote no file");
    }
    restrict_to_owner(incoming);
    std::fs::rename(incoming, &target)?;
    prune(dir, &target);
    Ok(Some(target))
}

/// Keys become file names, so accept only what the template produces.
fn is_valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn snapshot_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("snapshot-{key}.sqlite"))
}

fn cached_keys(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let key = name.strip_prefix("snapshot-")?.strip_suffix(".sqlite")?;
            is_valid_key(key).then(|| key.to_string())
        })
        .collect()
}

/// Keeps only `current`. Old copies are removed; a process still reading one keeps its open
/// handle (Unix), and a removal that fails (Windows, file in use) is retried next time.
/// Abandoned incoming files from interrupted runs are removed after an hour.
fn prune(dir: &Path, current: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path == current {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let stale_incoming = name.starts_with("incoming-")
            && entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age.as_secs() > 3600);
        if name.starts_with("snapshot-") || stale_incoming {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn create_private_dir(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn restrict_to_owner(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    /// Answers like the Bridge template, from a real SQLite file standing in for Zotero's own
    /// connection: `VACUUM INTO` when the key is not offered, reuse when it is.
    pub(crate) struct FixtureSource {
        pub path: PathBuf,
        pub key: RefCell<String>,
        pub copies: Cell<usize>,
    }

    impl FixtureSource {
        pub(crate) fn new(path: PathBuf) -> Self {
            Self {
                path,
                key: RefCell::new("fixture-1-0".to_string()),
                copies: Cell::new(0),
            }
        }
    }

    impl SnapshotSource for FixtureSource {
        fn request(&self, request: &Value) -> Option<Value> {
            let key = self.key.borrow().clone();
            let offered = request["have"]
                .as_array()?
                .iter()
                .any(|k| k.as_str() == Some(key.as_str()));
            if offered {
                return Some(json!({"key": key, "reused": true}));
            }
            let target = request["path"].as_str()?;
            let conn = Connection::open(&self.path).ok()?;
            conn.execute("VACUUM INTO ?1", [target]).ok()?;
            self.copies.set(self.copies.get() + 1);
            Some(json!({"key": key, "reused": false}))
        }
    }

    /// Delegates to a shared fixture so a test can observe it across re-registrations, which
    /// stand in for separate CLI processes.
    struct Shared(Rc<FixtureSource>);
    impl SnapshotSource for Shared {
        fn request(&self, request: &Value) -> Option<Value> {
            self.0.request(request)
        }
    }

    struct Answer(Value);
    impl SnapshotSource for Answer {
        fn request(&self, _request: &Value) -> Option<Value> {
            Some(self.0.clone())
        }
    }

    struct Unavailable;
    impl SnapshotSource for Unavailable {
        fn request(&self, _request: &Value) -> Option<Value> {
            None
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zotero-cli-live-snapshot-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fixture(dir: &Path) -> PathBuf {
        let path = dir.join("zotero.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE libraries (libraryID INTEGER PRIMARY KEY, type TEXT, editable INT);
             INSERT INTO libraries VALUES (1, 'user', 1), (2, 'group', 0);
             CREATE TABLE tags (tagID INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
             CREATE INDEX tags_name ON tags(name);
             INSERT INTO tags VALUES (1, 'alpha'), (2, 'beta'), (3, 'gamma');",
        )
        .unwrap();
        path
    }

    fn tag_names(conn: &Connection) -> Vec<String> {
        conn.prepare("SELECT name FROM tags ORDER BY tagID")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn register_shared(source: &Rc<FixtureSource>, cache: &Path) {
        register_source(Box::new(Shared(source.clone())), cache.to_path_buf());
    }

    #[test]
    fn first_read_copies_the_database_read_only() {
        let dir = scratch("cold");
        let db = fixture(&dir);
        let cache = dir.join("cache");
        let source = Rc::new(FixtureSource::new(db.clone()));
        register_shared(&source, &cache);

        let conn = connect(&db).unwrap().expect("snapshot built");
        assert_eq!(tag_names(&conn), ["alpha", "beta", "gamma"]);
        assert_eq!(source.copies.get(), 1);
        assert!(
            conn.execute("DELETE FROM tags", []).is_err(),
            "the handed-out connection must be read-only"
        );
        let copy = cache_dir(&cache, &db).join("snapshot-fixture-1-0.sqlite");
        assert!(copy.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&copy).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "the copy must be private to its owner");
        }
        clear_source();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_later_process_reuses_the_copy_while_the_key_is_unchanged() {
        let dir = scratch("warm");
        let db = fixture(&dir);
        let cache = dir.join("cache");
        let source = Rc::new(FixtureSource::new(db.clone()));
        register_shared(&source, &cache);
        drop(connect(&db).unwrap().unwrap());

        // A new registration stands in for the next CLI invocation.
        register_shared(&source, &cache);
        let conn = connect(&db).unwrap().expect("cached snapshot");
        assert_eq!(
            source.copies.get(),
            1,
            "an unchanged key must not copy again"
        );
        assert_eq!(tag_names(&conn), ["alpha", "beta", "gamma"]);
        clear_source();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_changed_key_rebuilds_and_drops_the_old_copy() {
        let dir = scratch("changed");
        let db = fixture(&dir);
        let cache = dir.join("cache");
        let source = Rc::new(FixtureSource::new(db.clone()));
        register_shared(&source, &cache);
        drop(connect(&db).unwrap().unwrap());

        Connection::open(&db)
            .unwrap()
            .execute("INSERT INTO tags VALUES (4, 'delta')", [])
            .unwrap();
        *source.key.borrow_mut() = "fixture-2-1".to_string();
        register_shared(&source, &cache);
        let conn = connect(&db).unwrap().unwrap();
        assert_eq!(source.copies.get(), 2);
        assert_eq!(tag_names(&conn), ["alpha", "beta", "gamma", "delta"]);
        assert_eq!(cached_keys(&cache_dir(&cache, &db)), ["fixture-2-1"]);
        clear_source();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reused_copy_that_vanished_is_rebuilt() {
        struct ThenFixture(Rc<FixtureSource>);
        impl SnapshotSource for ThenFixture {
            fn request(&self, request: &Value) -> Option<Value> {
                if request["have"].as_array()?.is_empty() {
                    self.0.request(request)
                } else {
                    // Another process pruned this copy after it was offered.
                    Some(json!({"key": "gone-1-0", "reused": true}))
                }
            }
        }
        let dir = scratch("vanished");
        let db = fixture(&dir);
        let cache = dir.join("cache");
        let source = Rc::new(FixtureSource::new(db.clone()));
        register_shared(&source, &cache);
        drop(connect(&db).unwrap().unwrap());

        register_source(Box::new(ThenFixture(source.clone())), cache.clone());
        let conn = connect(&db).unwrap().expect("rebuilt snapshot");
        assert_eq!(tag_names(&conn).len(), 3);
        assert_eq!(source.copies.get(), 2);
        clear_source();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn errors_and_malformed_answers_are_errors_not_snapshots() {
        let dir = scratch("errors");
        let db = fixture(&dir);
        for answer in [
            json!({"error": "live snapshot: disk full"}),
            json!({"key": "../escape", "reused": false}),
            json!({"key": "ok-1-0", "reused": false}),
        ] {
            register_source(Box::new(Answer(answer.clone())), dir.join("cache"));
            assert!(connect(&db).is_err(), "{answer} must not yield a snapshot");
        }
        clear_source();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_source_or_unavailable_source_yields_none() {
        let dir = scratch("none");
        let db = fixture(&dir);
        clear_source();
        assert!(connect(&db).unwrap().is_none());
        register_source(Box::new(Unavailable), dir.join("cache"));
        assert!(connect(&db).unwrap().is_none());
        clear_source();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_template_is_read_only() {
        for verb in [
            "saveTx",
            "eraseTx",
            "merge",
            "trash",
            "setField",
            "INSERT",
            "UPDATE",
            "DELETE",
            "DROP",
            "ALTER",
            "executeTransaction",
        ] {
            assert!(
                !T_SNAPSHOT.contains(verb),
                "snapshot template must not contain `{verb}`"
            );
        }
    }

    #[test]
    fn a_locked_read_is_remembered_per_database_and_suppressible() {
        let dir = scratch("remembered");
        let db = fixture(&dir);
        register_source(Box::new(FixtureSource::new(db.clone())), dir.join("cache"));
        assert!(
            !is_validated(&db),
            "nothing is known before the first locked read"
        );
        drop(connect(&db).unwrap().unwrap());
        assert!(is_validated(&db));
        assert!(!is_validated(&dir.join("other.sqlite")));
        assert!(!without_snapshot(|| is_validated(&db)));
        clear_source();
        assert!(!is_validated(&db));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_snapshot_suppresses_and_restores() {
        let dir = scratch("suppressed");
        let db = fixture(&dir);
        register_source(Box::new(FixtureSource::new(db.clone())), dir.join("cache"));
        assert!(without_snapshot(|| connect(&db).unwrap().is_none()));
        assert!(
            connect(&db).unwrap().is_some(),
            "suppression must end with the closure"
        );
        clear_source();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
