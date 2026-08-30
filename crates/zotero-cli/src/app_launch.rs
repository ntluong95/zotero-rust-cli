//! Port of `core/discovery.py::launch_zotero` (`app launch`). Command construction
//! (`build_launch_command`, pure and platform-parameterized) is kept strictly separate from
//! process spawning (the `ProcessSpawner` trait) so tests can exercise every platform branch --
//! and the full readiness-poll flow -- without ever spawning a real process or touching a real
//! Zotero installation.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::error::DomainError;
use crate::http;
use crate::runtime::RuntimeContext;

/// A platform-resolved argv, not yet spawned: `program` plus its `args`. Two shapes exist in the
/// pinned Python source -- spawn the Zotero executable directly, or (macOS with a resolvable
/// `.app` bundle) `open <bundle>` -- see `build_launch_command`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// `_macos_app_bundle_for_executable()` (`discovery.py:54-58`): walks `executable` and each of
/// its ancestors (itself first, matching Python's `(executable, *executable.parents)`, which
/// `Path::ancestors()` already yields in exactly that order) for the first path whose final
/// component ends in `.app`.
fn macos_app_bundle_for_executable(executable: &Path) -> Option<PathBuf> {
    executable
        .ancestors()
        .find(|candidate| candidate.extension().and_then(|ext| ext.to_str()) == Some("app"))
        .map(Path::to_path_buf)
}

/// `launch_zotero()`'s `launch_command` selection (`discovery.py:68-72`), as a pure function of
/// `executable` and whether this is running on macOS -- `is_macos` is an explicit parameter
/// (rather than reading `cfg!(target_os = "macos")` internally) precisely so tests can exercise
/// all three platform branches (macOS-with-bundle, macOS-without-a-resolvable-bundle, and
/// non-macOS) from any single CI runner, matching Python's `sys.platform == "darwin"` branch
/// without needing three different host OSes to prove it.
///
/// Non-macOS (Windows and Linux both) always spawns the executable directly with no arguments --
/// Python has no OS-specific branch for either, so neither does this port.
pub fn build_launch_command(executable: &Path, is_macos: bool) -> LaunchCommand {
    if is_macos {
        if let Some(bundle) = macos_app_bundle_for_executable(executable) {
            if bundle.exists() {
                return LaunchCommand {
                    program: "open".to_string(),
                    args: vec![bundle.to_string_lossy().into_owned()],
                };
            }
        }
    }
    LaunchCommand {
        program: executable.to_string_lossy().into_owned(),
        args: Vec::new(),
    }
}

/// `subprocess.Popen(launch_command, stdout=DEVNULL, stderr=DEVNULL)` (`discovery.py:73`), as an
/// injectable seam -- the real implementation ([`RealProcessSpawner`]) is the only thing in this
/// crate that ever spawns a real OS process for `app launch`; every test substitutes a fake.
pub trait ProcessSpawner {
    fn spawn(&mut self, command: &LaunchCommand) -> anyhow::Result<u32>;
}

/// Fire-and-forget, matching Python: never `.wait()`s or reaps the child, relying (as Python
/// does) on the OS to handle it once this short-lived CLI process exits.
pub struct RealProcessSpawner;

impl ProcessSpawner for RealProcessSpawner {
    fn spawn(&mut self, command: &LaunchCommand) -> anyhow::Result<u32> {
        let child = std::process::Command::new(&command.program)
            .args(&command.args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        Ok(child.id())
    }
}

/// `launch_zotero()` (`discovery.py:61-96`).
///
/// Two of Python's raised exceptions here are never caught by `dispatch()` (no handler exists
/// for `FileNotFoundError` or a bare `OSError` from `subprocess.Popen`), so they'd propagate as
/// raw, unformatted tracebacks in the reference implementation -- the same category of accidental
/// behavior `error.rs` already documents for a missing `zotero.sqlite`. Both are ports here as a
/// clean [`DomainError`] (exit 1, readable message) instead of reproduced as a divergence: a
/// deliberate, minor improvement over an implementation accident, not an intentional Python
/// contract.
pub fn launch_zotero(
    runtime: &RuntimeContext,
    wait_timeout: i64,
    spawner: &mut dyn ProcessSpawner,
) -> anyhow::Result<Value> {
    let Some(executable) = runtime.environment.executable.clone() else {
        return Err(DomainError::new("Zotero executable could not be resolved").into());
    };
    if !runtime.environment.executable_exists {
        return Err(DomainError::new(format!(
            "Zotero executable not found: {}",
            executable.display()
        ))
        .into());
    }

    let command = build_launch_command(&executable, cfg!(target_os = "macos"));
    let pid = spawner
        .spawn(&command)
        .map_err(|err| DomainError::new(format!("Failed to launch Zotero: {err}")))?;

    let timeout = Duration::from_secs(wait_timeout.max(0) as u64);
    let connector_ready = http::wait_for_endpoint(
        runtime.environment.port,
        "/connector/ping",
        timeout,
        Duration::from_millis(500),
        &[],
        &[200],
    );
    let mut local_api_ready = false;
    if runtime.environment.local_api_enabled_configured {
        local_api_ready = http::wait_for_endpoint(
            runtime.environment.port,
            "/api/",
            timeout,
            Duration::from_millis(500),
            &[("Zotero-API-Version", http::LOCAL_API_VERSION)],
            &[200],
        );
    }

    Ok(json!({
        "action": "launch",
        "pid": pid,
        "connector_ready": connector_ready,
        "local_api_ready": local_api_ready,
        "wait_timeout": wait_timeout,
        "executable": executable.to_string_lossy(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_with_resolvable_app_bundle_uses_open() {
        let dir = std::env::temp_dir().join(format!(
            "zotero-cli-app-launch-bundle-test-{}",
            std::process::id()
        ));
        let bundle = dir.join("Zotero.app");
        let contents = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&contents).unwrap();
        let executable = contents.join("zotero");
        std::fs::write(&executable, b"").unwrap();

        let command = build_launch_command(&executable, true);
        assert_eq!(command.program, "open");
        assert_eq!(command.args, vec![bundle.to_string_lossy().into_owned()]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn macos_without_a_resolvable_bundle_falls_back_to_raw_executable() {
        // No ancestor directory actually ends in ".app" and exists.
        let executable = std::env::temp_dir().join("zotero-cli-nonexistent-bundle-test/zotero");
        let command = build_launch_command(&executable, true);
        assert_eq!(command.program, executable.to_string_lossy().into_owned());
        assert!(command.args.is_empty());
    }

    #[test]
    fn non_macos_always_spawns_the_executable_directly() {
        let dir = std::env::temp_dir().join(format!(
            "zotero-cli-app-launch-nonmacos-test-{}",
            std::process::id()
        ));
        let bundle = dir.join("Zotero.app");
        std::fs::create_dir_all(&bundle).unwrap();
        let executable = bundle.join("zotero");
        std::fs::write(&executable, b"").unwrap();

        // Even with a resolvable, existing .app bundle in the ancestry, non-macOS never
        // rewrites to `open` -- Python has no such branch outside `sys.platform == "darwin"`.
        let command = build_launch_command(&executable, false);
        assert_eq!(command.program, executable.to_string_lossy().into_owned());
        assert!(command.args.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }
}
