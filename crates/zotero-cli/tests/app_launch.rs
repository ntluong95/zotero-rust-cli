//! `app launch` tests (`core/discovery.py::launch_zotero`), pinned at
//! `PiaoyangGuohai1/cli-anything-zotero@e42a930e`.
//!
//! **No test here ever spawns a real Zotero process.** Two independent layers make that
//! guarantee:
//!
//! - The library-level tests call `zotero_cli::app_launch::launch_zotero` directly (no
//!   subprocess at all) against a hand-built `RuntimeContext` and an injected `FakeSpawner`
//!   (below) that only records the constructed [`LaunchCommand`] and returns a fabricated PID --
//!   it never calls `std::process::Command::spawn`. Readiness polling (`/connector/ping`,
//!   `/api/`) is driven against a local `ScriptedServer` or a genuinely closed port, never a real
//!   Zotero HTTP surface.
//! - The CLI-subprocess-level tests (real `zotero-cli` binary) only exercise the paths that
//!   return *before* any process would be spawned at all -- an explicit `--executable` pointing
//!   at a path that doesn't exist, or the default discovery finding nothing on this CI machine.
//!   Both are proven safe by inspecting the exact error message, not by trusting that no spawn
//!   happened.
//!
//! No SQLite writes, no item/collection/config mutation anywhere in this file.

#[path = "common/mod.rs"]
mod common;

use common::{run_cli, ScriptedResponse, ScriptedServer};
use serde_json::json;
use std::path::PathBuf;
use zotero_cli::app_launch::{launch_zotero, LaunchCommand, ProcessSpawner};
use zotero_cli::paths::ZoteroEnvironment;
use zotero_cli::runtime::RuntimeContext;

/// Records every constructed [`LaunchCommand`] it's asked to spawn and returns a fixed
/// (`Ok`/`Err`) result, without ever touching `std::process::Command`.
struct FakeSpawner {
    result: Result<u32, String>,
    calls: Vec<LaunchCommand>,
}

impl FakeSpawner {
    fn ok(pid: u32) -> Self {
        FakeSpawner {
            result: Ok(pid),
            calls: Vec::new(),
        }
    }

    fn err(message: &str) -> Self {
        FakeSpawner {
            result: Err(message.to_string()),
            calls: Vec::new(),
        }
    }
}

impl ProcessSpawner for FakeSpawner {
    fn spawn(&mut self, command: &LaunchCommand) -> anyhow::Result<u32> {
        self.calls.push(command.clone());
        match &self.result {
            Ok(pid) => Ok(*pid),
            Err(message) => Err(anyhow::anyhow!("{message}")),
        }
    }
}

/// Mirrors `write_router_integration.rs`'s `test_runtime` fixture: every path field except the
/// ones a given test actually varies points at a placeholder that provably doesn't exist, so no
/// test here can accidentally resolve onto a real Zotero installation on the machine running it.
fn test_runtime(
    port: u16,
    executable: Option<PathBuf>,
    executable_exists: bool,
    local_api_enabled_configured: bool,
) -> RuntimeContext {
    RuntimeContext {
        environment: ZoteroEnvironment {
            executable,
            executable_exists,
            install_dir: None,
            version: "7.0.24".to_string(),
            profile_root: PathBuf::from("/tmp/does-not-exist-profile-root"),
            profile_dir: None,
            data_dir: PathBuf::from("/tmp/does-not-exist-data-dir"),
            data_dir_exists: false,
            sqlite_path: PathBuf::from("/tmp/does-not-exist.sqlite"),
            sqlite_exists: false,
            styles_dir: PathBuf::from("/tmp/does-not-exist-styles"),
            styles_exists: false,
            storage_dir: PathBuf::from("/tmp/does-not-exist-storage"),
            storage_exists: false,
            translators_dir: PathBuf::from("/tmp/does-not-exist-translators"),
            translators_exists: false,
            port,
            local_api_enabled_configured,
        },
        backend: "auto".to_string(),
        connector_available: false,
        connector_message: String::new(),
        local_api_available: false,
        local_api_message: String::new(),
        server_id: None,
        local_api_writes_available: false,
    }
}

/// A placeholder "executable" that exists on disk (so `executable_exists` checks can be real)
/// but is never actually spawned -- `FakeSpawner` intercepts before any real process starts.
fn fixture_executable_path(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "zotero-cli-app-launch-fixture-{}-{}",
        std::process::id(),
        label
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("zotero-decoy");
    std::fs::write(&path, b"not a real Zotero binary").unwrap();
    path
}

// ── library-level: readiness polling (real HTTP, fake spawn) ──────────────

#[test]
fn launch_zotero_success_polls_both_connector_and_local_api() {
    let executable = fixture_executable_path("success");
    let server = ScriptedServer::start(vec![
        ScriptedResponse::json(200, json!({})),
        ScriptedResponse::json(200, json!({})),
    ]);
    let runtime = test_runtime(server.port, Some(executable.clone()), true, true);
    let mut spawner = FakeSpawner::ok(4242);

    let payload = launch_zotero(&runtime, 5, &mut spawner).expect("launch_zotero must succeed");
    let requests = server.finish();

    assert_eq!(payload["action"], "launch");
    assert_eq!(payload["pid"], 4242);
    assert_eq!(payload["connector_ready"], true);
    assert_eq!(payload["local_api_ready"], true);
    assert_eq!(payload["wait_timeout"], 5);
    assert_eq!(
        payload["executable"],
        executable.to_string_lossy().into_owned()
    );
    assert_eq!(
        requests.len(),
        2,
        "connector then local API, one request each"
    );
    assert_eq!(requests[0].path, "/connector/ping");
    assert!(requests[1].path.starts_with("/api/"));
    assert_eq!(spawner.calls.len(), 1);

    std::fs::remove_dir_all(executable.parent().unwrap()).ok();
}

#[test]
fn launch_zotero_local_api_not_configured_never_polls_it() {
    let executable = fixture_executable_path("no-local-api");
    let server = ScriptedServer::start(vec![ScriptedResponse::json(200, json!({}))]);
    let runtime = test_runtime(server.port, Some(executable.clone()), true, false);
    let mut spawner = FakeSpawner::ok(1);

    let payload = launch_zotero(&runtime, 5, &mut spawner).expect("launch_zotero must succeed");
    let requests = server.finish();

    assert_eq!(payload["connector_ready"], true);
    // Always false when Local API isn't configured, per Python -- never even attempted, not
    // just "attempted and failed".
    assert_eq!(payload["local_api_ready"], false);
    assert_eq!(
        requests.len(),
        1,
        "only /connector/ping should ever be requested"
    );
    assert_eq!(requests[0].path, "/connector/ping");

    std::fs::remove_dir_all(executable.parent().unwrap()).ok();
}

#[test]
fn launch_zotero_unreachable_endpoint_times_out_to_false() {
    // Bind then immediately drop a listener to obtain a port nothing is listening on --
    // connections fail fast (connection refused) so this stays quick even with real polling.
    let dead_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let executable = fixture_executable_path("timeout");
    let runtime = test_runtime(dead_port, Some(executable.clone()), true, true);
    let mut spawner = FakeSpawner::ok(1);

    let payload = launch_zotero(&runtime, 1, &mut spawner).expect("launch_zotero must succeed");

    assert_eq!(payload["connector_ready"], false);
    assert_eq!(payload["local_api_ready"], false);
    assert_eq!(
        spawner.calls.len(),
        1,
        "spawn still happens even if readiness never arrives"
    );

    std::fs::remove_dir_all(executable.parent().unwrap()).ok();
}

// ── library-level: pre-spawn error paths (never reach FakeSpawner) ────────

#[test]
fn launch_zotero_unresolved_executable_is_domain_error_and_never_spawns() {
    let runtime = test_runtime(0, None, false, false);
    let mut spawner = FakeSpawner::ok(1);

    let err = launch_zotero(&runtime, 5, &mut spawner).expect_err("must fail");

    assert_eq!(err.to_string(), "Zotero executable could not be resolved");
    assert!(spawner.calls.is_empty());
}

#[test]
fn launch_zotero_nonexistent_executable_path_is_domain_error_and_never_spawns() {
    let missing = PathBuf::from("/definitely/does/not/exist/zotero-binary");
    let runtime = test_runtime(0, Some(missing.clone()), false, false);
    let mut spawner = FakeSpawner::ok(1);

    let err = launch_zotero(&runtime, 5, &mut spawner).expect_err("must fail");

    assert_eq!(
        err.to_string(),
        format!("Zotero executable not found: {}", missing.display())
    );
    assert!(spawner.calls.is_empty());
}

#[test]
fn launch_zotero_spawn_failure_is_a_clean_domain_error() {
    let executable = fixture_executable_path("spawn-failure");
    let runtime = test_runtime(0, Some(executable.clone()), true, false);
    let mut spawner = FakeSpawner::err("permission denied");

    let err = launch_zotero(&runtime, 5, &mut spawner).expect_err("must fail");

    assert!(err.to_string().contains("Failed to launch Zotero"));
    assert!(err.to_string().contains("permission denied"));
    assert_eq!(
        spawner.calls.len(),
        1,
        "spawn was attempted exactly once, no retry"
    );

    std::fs::remove_dir_all(executable.parent().unwrap()).ok();
}

// ── CLI-subprocess-level: safe pre-spawn error paths only ─────────────────

#[test]
fn app_launch_cli_nonexistent_explicit_executable_is_a_clean_exit_one() {
    let dir = std::env::temp_dir().join(format!(
        "zotero-cli-app-launch-cli-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let missing = dir.join("definitely-not-zotero");

    // `build_runtime()`'s connector/Local-API prelude still dials this port (and fails fast,
    // nothing is listening) before `app launch`'s own executable check ever runs -- a genuinely
    // closed port (bind then immediately drop) rather than the literal `0`, to avoid any
    // platform-specific handling of port `0` as a connect target.
    let dead_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let (code, value) = run_cli(
        &dir,
        dead_port,
        &[],
        &["--executable", missing.to_str().unwrap(), "app", "launch"],
    );

    assert_eq!(code, 1, "stdout={value}");
    assert_eq!(
        value["error"],
        format!("Zotero executable not found: {}", missing.display())
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn app_launch_cli_help_documents_wait_timeout() {
    let output = std::process::Command::new(common::bin_path())
        .args(["app", "launch", "--help"])
        .output()
        .expect("failed to run zotero-cli binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("--wait-timeout"));
}
