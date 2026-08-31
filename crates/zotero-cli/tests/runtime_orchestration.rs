//! Cross-backend composition tests for the RC2 stabilization.
//!
//! RC1 shipped with every component independently proven -- SQLite safety, Local API, Bridge,
//! `app launch`, `doctor`, `note add` -- and still failed against a real Zotero, because nothing
//! exercised the components *in combination*: Zotero running, SQLite intentionally locked, a
//! healthy live backend, and a typed write command on top. That combination is the dimension
//! this file owns.
//!
//! **No test here may launch or mutate a real Zotero.** Every launch is performed through the
//! injectable [`ProcessSpawner`] seam against a mock HTTP server, and every subprocess run sets
//! `ZOTERO_CLI_NO_AUTOLAUNCH=1` (see `common::run_cli`) so the real spawn path is unreachable.

#[path = "common/mod.rs"]
mod common;

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{build_fixture_sqlite, run_cli, ScriptedResponse, ScriptedServer, TestDir};
use serde_json::json;

use zotero_cli::app_launch::{LaunchCommand, ProcessSpawner};
use zotero_cli::lifecycle::{self, Backend};
use zotero_cli::paths::ZoteroEnvironment;
use zotero_cli::runtime::RuntimeContext;

const SERVER_ID: &str = "TEST-SERVER-1";

// ── Fixtures ───────────────────────────────────────────────────────────────

fn connector_ping_ok() -> ScriptedResponse {
    ScriptedResponse::json(200, json!({}))
}

fn local_api_probe_available() -> ScriptedResponse {
    ScriptedResponse::json_with_headers(
        200,
        vec![("Zotero-Server-ID".to_string(), SERVER_ID.to_string())],
        json!({}),
    )
}

fn local_api_probe_unavailable() -> ScriptedResponse {
    ScriptedResponse::json(403, json!({"message": "local API disabled"}))
}

fn bridge_ownership_ok() -> ScriptedResponse {
    ScriptedResponse::json(
        200,
        json!({"fork": "zotero-rust-cli", "id": "cli-bridge@cli-anything-rust.dev"}),
    )
}

fn bridge_resolve_item(key: &str, item_id: i64) -> ScriptedResponse {
    ScriptedResponse::json(
        200,
        json!(json!({
            "found": true,
            "key": key,
            "libraryID": 1,
            "libraryType": "user",
            "itemType": "document",
            "itemID": item_id,
        })
        .to_string()),
    )
}

/// A runtime describing a Zotero that is **not** running: nothing answered either probe. The
/// executable is a real file so `launch_zotero`'s existence check passes without ever spawning
/// it -- the spawner is a fake.
fn closed_runtime(port: u16, executable: PathBuf) -> RuntimeContext {
    RuntimeContext {
        environment: ZoteroEnvironment {
            executable_exists: executable.exists(),
            executable: Some(executable),
            install_dir: None,
            version: "10.0.1".to_string(),
            profile_root: PathBuf::from("/nonexistent"),
            profile_dir: None,
            data_dir: PathBuf::from("/nonexistent"),
            data_dir_exists: false,
            sqlite_path: PathBuf::from("/nonexistent/zotero.sqlite"),
            sqlite_exists: false,
            styles_dir: PathBuf::from("/nonexistent/styles"),
            styles_exists: false,
            storage_dir: PathBuf::from("/nonexistent/storage"),
            storage_exists: false,
            translators_dir: PathBuf::from("/nonexistent/translators"),
            translators_exists: false,
            port,
            local_api_enabled_configured: true,
        },
        backend: "auto".to_string(),
        connector_available: false,
        connector_message: "HTTP request failed for /connector/ping".to_string(),
        local_api_available: false,
        local_api_message: "HTTP request failed for /api/".to_string(),
        server_id: None,
        local_api_writes_available: false,
    }
}

/// A runtime describing a Zotero that *is* running with a reachable, Zotero-10 Local API.
fn running_runtime(port: u16) -> RuntimeContext {
    let mut runtime = closed_runtime(port, PathBuf::from("/nonexistent/zotero"));
    runtime.connector_available = true;
    runtime.connector_message = "connector available".to_string();
    runtime.local_api_available = true;
    runtime.local_api_message = "local API available".to_string();
    runtime.server_id = Some(SERVER_ID.to_string());
    runtime.local_api_writes_available = true;
    runtime
}

fn temp_executable(name: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "zotero-cli-orchestration-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("zotero");
    std::fs::write(&exe, b"").unwrap();
    (dir, exe)
}

/// Counts spawns and, on the first one, "starts Zotero" by making the given endpoint answer --
/// exactly the observable behavior of a real launch, with no real process anywhere.
struct FakeSpawner {
    spawns: Arc<Mutex<Vec<LaunchCommand>>>,
    on_spawn: Box<dyn FnMut() + Send>,
    fail: bool,
}

impl FakeSpawner {
    fn new(on_spawn: impl FnMut() + Send + 'static) -> (Self, Arc<Mutex<Vec<LaunchCommand>>>) {
        let spawns = Arc::new(Mutex::new(Vec::new()));
        (
            FakeSpawner {
                spawns: Arc::clone(&spawns),
                on_spawn: Box::new(on_spawn),
                fail: false,
            },
            spawns,
        )
    }

    fn failing() -> (Self, Arc<Mutex<Vec<LaunchCommand>>>) {
        let (mut spawner, spawns) = Self::new(|| {});
        spawner.fail = true;
        (spawner, spawns)
    }
}

impl ProcessSpawner for FakeSpawner {
    fn spawn(&mut self, command: &LaunchCommand) -> anyhow::Result<u32> {
        self.spawns.lock().unwrap().push(command.clone());
        if self.fail {
            anyhow::bail!("simulated exec failure");
        }
        (self.on_spawn)();
        Ok(4242)
    }
}

/// Serves the owned-Bridge ownership handshake forever on `port`, starting only when `start` is
/// called -- the stand-in for "Zotero came up and registered /cli-bridge/eval".
struct DeferredBridge {
    listener: Option<TcpListener>,
    port: u16,
}

impl DeferredBridge {
    fn bind() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        DeferredBridge {
            listener: Some(listener),
            port,
        }
    }

    /// Begins answering. Returns a handle that keeps serving until dropped.
    fn start(&mut self) {
        let listener = self.listener.take().expect("start called twice");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                use std::io::{Read, Write};
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let body = r#"{"pong":true,"fork":"zotero-rust-cli","id":"cli-bridge@cli-anything-rust.dev"}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
    }
}

/// Each test gets its own port, and the probe cache is process-wide, so tests that assert on
/// probe counts serialize against each other.
fn reset_probe_cache() {
    zotero_cli::bridge::clear_probe_cache();
}

// ── CASE A / E: Zotero closed -> exactly one launch -> backend ready -> proceed ──

#[test]
fn case_a_e_closed_zotero_launches_exactly_once_and_waits_for_the_named_backend() {
    reset_probe_cache();
    let (dir, exe) = temp_executable("case-a");
    let bridge = DeferredBridge::bind();
    let port = bridge.port;
    // Nothing answers until the (fake) launch happens -- the same order a real cold start has.
    let bridge_cell = Arc::new(Mutex::new(Some(bridge)));
    let bridge_for_spawn = Arc::clone(&bridge_cell);
    let (mut spawner, spawns) = FakeSpawner::new(move || {
        if let Some(mut b) = bridge_for_spawn.lock().unwrap().take() {
            b.start();
        }
    });

    let runtime = closed_runtime(port, exe);
    let result = lifecycle::ensure_backend(runtime, Backend::Bridge, &mut spawner);

    assert!(result.is_ok(), "backend became ready: {:?}", result.err());
    assert_eq!(
        spawns.lock().unwrap().len(),
        1,
        "a closed Zotero is launched exactly once, never once per capability check"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ── CASE F: Zotero already running -> live command never spawns a second process ──

#[test]
fn case_f_running_zotero_is_never_launched_again() {
    reset_probe_cache();
    let server = ScriptedServer::start(vec![bridge_ownership_ok()]);
    let (mut spawner, spawns) = FakeSpawner::new(|| {});

    let runtime = running_runtime(server.port);
    let result = lifecycle::ensure_backend(runtime, Backend::Bridge, &mut spawner);

    assert!(result.is_ok());
    assert!(
        spawns.lock().unwrap().is_empty(),
        "an available backend must short-circuit before any launch decision"
    );
    server.finish();
}

#[test]
fn case_f_unavailable_capability_on_a_running_zotero_still_never_launches() {
    reset_probe_cache();
    // The Local API answers, so Zotero is demonstrably up; the connector is not available.
    // Launching a second Zotero cannot fix a capability problem in the one already running.
    let server = ScriptedServer::start(vec![]);
    let (mut spawner, spawns) = FakeSpawner::new(|| {});

    let mut runtime = running_runtime(server.port);
    runtime.connector_available = false;
    let result = lifecycle::ensure_backend(runtime, Backend::Connector, &mut spawner);

    assert!(
        result.is_ok(),
        "the command itself reports the missing capability; lifecycle only owns launching"
    );
    assert!(spawns.lock().unwrap().is_empty());
    server.finish();
}

// ── CASE G: launch succeeds, backend never becomes ready -> clean timeout error ──

#[test]
fn case_g_launch_succeeds_but_backend_never_readies_is_a_clean_timeout_error() {
    reset_probe_cache();
    let (dir, exe) = temp_executable("case-g");
    // Bind and immediately drop, so the port is dead for the whole wait.
    let dead_port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let (mut spawner, spawns) = FakeSpawner::new(|| {});

    // SAFETY: set before the call, and this test does not run concurrently with another that
    // reads this variable (each other test uses its own port and never launches).
    unsafe { std::env::set_var("ZOTERO_CLI_LAUNCH_TIMEOUT", "1") };
    let runtime = closed_runtime(dead_port, exe);
    let result = lifecycle::ensure_backend(runtime, Backend::Bridge, &mut spawner);
    unsafe { std::env::remove_var("ZOTERO_CLI_LAUNCH_TIMEOUT") };

    let message = match result {
        Ok(_) => panic!("a backend that never readies must not look like success"),
        Err(err) => err.to_string(),
    };
    assert!(
        message.contains("did not become ready"),
        "timeout must be reported as a readiness timeout, not a generic failure: {message}"
    );
    assert!(
        message.contains("CLI Bridge eval endpoint"),
        "the error must name the specific capability that timed out: {message}"
    );
    assert_eq!(
        spawns.lock().unwrap().len(),
        1,
        "no retry storm of launches"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn launch_failure_is_a_clean_domain_error_not_a_panic() {
    reset_probe_cache();
    let (dir, exe) = temp_executable("launch-fail");
    let dead_port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let (mut spawner, _spawns) = FakeSpawner::failing();

    let runtime = closed_runtime(dead_port, exe);
    let message = match lifecycle::ensure_backend(runtime, Backend::Bridge, &mut spawner) {
        Ok(_) => panic!("a failed spawn must surface as an error"),
        Err(err) => err.to_string(),
    };
    assert!(
        message.contains("Failed to launch Zotero"),
        "got: {message}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ── CASE H: missing Local API authorization -> reported, never bypassed ──

#[test]
fn case_h_missing_authorization_is_reported_and_never_auto_granted() {
    reset_probe_cache();
    let dir = TestDir::new("case-h");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        // Live target resolution succeeds -- the failure below is purely about authorization.
        ScriptedResponse::json(
            200,
            json!({
                "key": "ITEM0001", "version": 5, "library": {"id": 0},
                "data": {"itemType": "document", "title": "Test Item One", "collections": [], "tags": []},
            }),
        ),
        ScriptedResponse::json(
            200,
            json!({
                "key": "ITEM0001", "version": 5, "library": {"id": 0},
                "data": {"itemType": "document", "title": "Test Item One", "collections": [], "tags": []},
            }),
        ),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "update", "ITEM0001", "--field", "title=New Title"],
    );
    let requests = server.finish();

    assert_eq!(
        code, 3,
        "authorization gaps keep their own exit code: {payload}"
    );
    assert_eq!(payload["outcome"], "authorization_failed");
    assert_eq!(payload["reason"], "required");
    assert_eq!(payload["needs_human_action"], true);
    assert!(
        requests.iter().all(|r| r.path != "/api/local/authorize"),
        "an automatic launch must never turn into an automatic consent grant"
    );
    assert!(
        requests.iter().all(|r| r.method != "PATCH"),
        "no write may be attempted without authorization"
    );
    // Nothing resembling credential material may reach stdout.
    let text = payload.to_string();
    assert!(
        !text.contains("\"key\":\""),
        "payload leaked a key field: {text}"
    );
}

// ── CASE B: Zotero running, SQLite locked, live backend healthy -> typed write succeeds ──

/// Holds an exclusive SQLite lock on a WAL-mode fixture for as long as it lives, reproducing a
/// running Zotero. Any command that resolves its target from SQLite fails here; a command that
/// resolves through a live backend does not.
struct LockedWalDb {
    _conn: rusqlite::Connection,
}

impl LockedWalDb {
    fn hold(sqlite_path: &std::path::Path) -> Self {
        let conn = rusqlite::Connection::open(sqlite_path).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.pragma_update(None, "locking_mode", "EXCLUSIVE")
            .unwrap();
        // A write transaction is what actually takes the exclusive lock under `locking_mode`.
        conn.execute_batch("BEGIN EXCLUSIVE; CREATE TABLE IF NOT EXISTS _lock (x); COMMIT;")
            .unwrap();
        conn.query_row("SELECT 1", [], |_| Ok(())).unwrap();
        LockedWalDb { _conn: conn }
    }
}

#[test]
fn case_b_typed_write_succeeds_while_sqlite_is_locked_and_the_bridge_is_healthy() {
    reset_probe_cache();
    let dir = TestDir::new("case-b");
    let sqlite_path = build_fixture_sqlite(dir.path());
    let _lock = LockedWalDb::hold(&sqlite_path);

    // Sanity: SQLite really is unusable right now, so the write below cannot be passing by
    // accidentally reading the database.
    let (read_code, read_payload) = run_cli(dir.path(), 1, &[], &["item", "get", "ITEM0001"]);
    assert_eq!(read_code, 1, "SQLite must be refused while locked");
    assert!(
        read_payload["error"]
            .as_str()
            .unwrap_or_default()
            .contains("exclusive lock"),
        "expected the WAL safety refusal, got: {read_payload}"
    );

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        bridge_resolve_item("ITEM0001", 1),
        ScriptedResponse::json(
            200,
            json!({"key": "NEWNOTE1", "itemID": 999, "title": "Test Item One"}),
        ),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "note",
            "add",
            "ITEM0001",
            "--text",
            "written while sqlite was locked",
        ],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "typed write must not need SQLite: {payload}");
    assert_eq!(payload["action"], "note_add");
    assert_eq!(payload["parentItemKey"], "ITEM0001");
    assert_eq!(
        requests.len(),
        5,
        "ping, probe, ownership, live target resolution, note write"
    );
}

// ── CASE C / D: doctor and js agree about the Bridge, in both directions ──

#[test]
fn case_c_doctor_bridge_probe_success_is_followed_by_a_working_js_command() {
    reset_probe_cache();
    let dir = TestDir::new("case-c");
    build_fixture_sqlite(dir.path());

    let doctor_server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({"ok": true, "value": "cli-bridge-ok", "version": "10.0.1"}),
        ),
    ]);
    let doctor_port = doctor_server.port;
    let (_doctor_code, doctor) = run_cli(dir.path(), doctor_port, &[], &["app", "doctor"]);
    doctor_server.finish();

    assert_eq!(doctor["checks"]["bridge"]["ok"], true);
    assert_eq!(doctor["checks"]["bridge"]["state"], "healthy");
    assert_eq!(
        doctor["checks"]["bridge"]["port"], doctor_port,
        "doctor must report the port it actually probed, so a mismatch is visible here"
    );

    // A `js` command in the same environment must reach the same endpoint. Before the fix,
    // `doctor` used the runtime's (profile-derived) port while every other Bridge caller
    // hard-coded 23119 -- so this pair could contradict each other in the same second.
    let js_server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(200, json!({"two": 2})),
    ]);
    let (js_code, js_payload) = run_cli(
        dir.path(),
        js_server.port,
        &[],
        &["js", "return {two: 1 + 1};"],
    );
    js_server.finish();
    assert_eq!(js_code, 0, "js must agree with doctor: {js_payload}");
    assert_eq!(js_payload["two"], 2);
}

#[test]
fn case_d_bridge_unavailable_makes_doctor_and_js_agree_that_it_is_unavailable() {
    reset_probe_cache();
    let dir = TestDir::new("case-d");
    build_fixture_sqlite(dir.path());

    let doctor_server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        // No Bridge response scripted: the ownership probe is refused.
    ]);
    let (_code, doctor) = run_cli(dir.path(), doctor_server.port, &[], &["app", "doctor"]);
    doctor_server.finish();
    assert_eq!(doctor["checks"]["bridge"]["ok"], false);
    assert_eq!(doctor["checks"]["bridge"]["endpoint_active"], false);

    let js_server = ScriptedServer::start(vec![]);
    let (js_code, js_payload) = run_cli(dir.path(), js_server.port, &[], &["js", "return 1;"]);
    js_server.finish();
    assert_eq!(js_code, 1, "js must not claim success: {js_payload}");
    // Agreement is the invariant, not one exact sentence: doctor reported the Bridge as not ok,
    // and `js` must fail for a Bridge reason rather than silently succeeding or blaming
    // something unrelated.
    let js_error = js_payload["error"].as_str().unwrap_or_default();
    assert!(
        js_error.contains("Bridge"),
        "js must attribute the failure to the Bridge, as doctor did: {js_error}"
    );
}

// ── CASE I: SQLite-only read with Zotero closed -> works, launches nothing ──

#[test]
fn case_i_offline_sqlite_read_works_with_zotero_closed_and_starts_nothing() {
    reset_probe_cache();
    let dir = TestDir::new("case-i");
    build_fixture_sqlite(dir.path());
    // Port 1 answers nothing: Zotero is closed as far as this invocation can tell.
    let (code, payload) = run_cli(dir.path(), 1, &[], &["item", "get", "ITEM0001"]);

    assert_eq!(
        code, 0,
        "an offline read must not require Zotero: {payload}"
    );
    assert_eq!(payload["key"], "ITEM0001");
}

// ── CASE J: `app doctor` with Zotero closed -> honest report, no launch ──

#[test]
fn case_j_doctor_with_zotero_closed_reports_state_and_never_launches() {
    reset_probe_cache();
    let dir = TestDir::new("case-j");
    build_fixture_sqlite(dir.path());
    let (code, doctor) = run_cli(dir.path(), 1, &[], &["app", "doctor"]);

    assert_eq!(code, 1, "a degraded environment is reported, not fixed");
    assert_eq!(doctor["ready"], false);
    assert_eq!(doctor["checks"]["connector"]["ok"], false);
    assert_eq!(doctor["checks"]["bridge"]["ok"], false);
    assert_eq!(
        doctor["write_ready"], false,
        "write_ready must not claim a backend that does not exist"
    );
    assert_eq!(
        doctor["write_backends"],
        json!({"bridge": false, "local_api": false}),
        "the diagnostic must say which write backends it means"
    );
}

// ── Diagnostics stay diagnostic even when auto-launch is enabled ──

#[test]
fn diagnostic_commands_never_auto_launch_even_without_the_opt_out() {
    reset_probe_cache();
    let dir = TestDir::new("diagnostics-observe-only");
    build_fixture_sqlite(dir.path());
    for args in [
        vec!["app", "doctor"],
        vec!["app", "status"],
        vec!["app", "ping"],
    ] {
        // Deliberately *not* setting the opt-out here: these commands must be launch-free by
        // construction, not merely because a test disabled launching. A real spawn would be
        // observable as a hang or a stray process; the assertion is that each returns promptly
        // with an honest report instead.
        let mut command = std::process::Command::new(common::bin_path());
        command
            .arg("--json")
            .arg("--data-dir")
            .arg(dir.path())
            .args(&args)
            .env("ZOTERO_HTTP_PORT", "1")
            .env(
                "CLI_ANYTHING_ZOTERO_STATE_DIR",
                dir.path().join("cli-state"),
            )
            .env_remove("ZOTERO_CLI_NO_AUTOLAUNCH")
            .env_remove("ZOTERO_LOCAL_API_KEY");
        let started = std::time::Instant::now();
        let output = command.output().expect("failed to run zotero-cli");
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "{args:?} took long enough to suggest it waited on a launch"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !stdout.is_empty(),
            "{args:?} must still report state: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

// ── Safety invariant: `item merge` stays dry-run by default and launches nothing ──

#[test]
fn bare_item_merge_remains_a_preview_and_requires_no_live_backend() {
    reset_probe_cache();
    let dir = TestDir::new("merge-safe-default");
    build_fixture_sqlite(dir.path());
    let (code, payload) = run_cli(
        dir.path(),
        1,
        &[],
        &["item", "merge", "ITEM0001", "ITEM0002"],
    );

    assert_eq!(payload["dry_run"], true, "payload: {payload}");
    assert_eq!(payload["status"], "dry_run");
    assert_eq!(code, 0);
}
