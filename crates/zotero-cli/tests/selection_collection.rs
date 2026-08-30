//! Selection / Collection slice CLI integration tests: exercises the actual `zotero-cli` binary
//! against scripted mock Connector/Bridge servers for the three commands this slice ports --
//! `collection use-selected`, `session use-selected` (both `catalog::use_selected_collection`,
//! the Connector's `/connector/getSelectedCollection`), and `collection stats` (`core/jsbridge.py`'s
//! `collection_stats`, JS Bridge only).
//!
//! No live Zotero, no SQLite writes, no item/collection mutation anywhere in this file. The only
//! state mutation under test is the CLI-owned `session.json` file, in an isolated temp directory
//! per test via `CLI_ANYTHING_ZOTERO_STATE_DIR`.

#[path = "common/mod.rs"]
mod common;

use common::{build_fixture_sqlite, run_cli, ScriptedResponse, ScriptedServer, TestDir};
use serde_json::json;
use std::path::Path;
use std::process::Command;

fn connector_ping_ok() -> ScriptedResponse {
    ScriptedResponse::json(200, json!({}))
}

fn connector_ping_unavailable() -> ScriptedResponse {
    ScriptedResponse::Http {
        status: 500,
        headers: Vec::new(),
        body: b"internal error".to_vec(),
    }
}

fn local_api_probe_available() -> ScriptedResponse {
    ScriptedResponse::json(200, json!({}))
}

fn bridge_ownership_ok() -> ScriptedResponse {
    ScriptedResponse::json(
        200,
        json!({"fork": "zotero-rust-cli", "id": "cli-bridge@cli-anything-rust.dev"}),
    )
}

/// A 200 response that fails the Bridge's fork/id ownership check -- `bridge_endpoint_active()`
/// treats this the same as no plugin installed at all.
fn bridge_ownership_wrong_fork() -> ScriptedResponse {
    ScriptedResponse::json(200, json!({"fork": "someone-else", "id": "not-ours"}))
}

fn text_response(status: u16, body: &str) -> ScriptedResponse {
    ScriptedResponse::Http {
        status,
        headers: Vec::new(),
        body: body.as_bytes().to_vec(),
    }
}

/// Runs the CLI without forcing `--json`, for human-mode output assertions.
fn run_cli_human(
    data_dir: &Path,
    port: u16,
    extra_env: &[(&str, &str)],
    args: &[&str],
) -> (i32, String) {
    let mut command = Command::new(common::bin_path());
    command
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .env("ZOTERO_HTTP_PORT", port.to_string())
        .env_remove("ZOTERO_LOCAL_API_KEY")
        .env_remove("CLI_ANYTHING_ZOTERO_STATE_DIR");
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let output = command.output().expect("failed to run zotero-cli binary");
    let code = output.status.code().unwrap_or(-1);
    (code, String::from_utf8_lossy(&output.stdout).into_owned())
}

fn read_session_state(state_dir: &Path) -> serde_json::Value {
    let raw = std::fs::read_to_string(state_dir.join("session.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

// ── collection use-selected ───────────────────────────────────────────────

#[test]
fn collection_use_selected_success_persists_session_and_emits_raw_selection() {
    let dir = TestDir::new("coll-use-selected-success");
    build_fixture_sqlite(dir.path());
    let state_dir = dir.path().join("state");

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        ScriptedResponse::json(
            200,
            json!({
                "libraryID": 1,
                "libraryName": "My Library",
                "id": 42,
                "name": "Sample Collection",
                "targets": [{"id": "L1", "name": "My Library", "filesEditable": true, "level": 0}],
            }),
        ),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[("CLI_ANYTHING_ZOTERO_STATE_DIR", state_dir.to_str().unwrap())],
        &["collection", "use-selected"],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["id"], 42);
    assert_eq!(value["libraryID"], 1);
    assert_eq!(value["name"], "Sample Collection");
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2].method, "POST");
    assert!(requests[2]
        .path
        .starts_with("/connector/getSelectedCollection"));

    let state = read_session_state(&state_dir);
    assert_eq!(state["current_library"], 1);
    assert_eq!(state["current_collection"], 42);
    assert_eq!(state["command_history"], json!(["collection use-selected"]));
}

#[test]
fn collection_use_selected_no_collection_selected_persists_null_collection() {
    let dir = TestDir::new("coll-use-selected-empty");
    build_fixture_sqlite(dir.path());
    let state_dir = dir.path().join("state");

    // Root "My Library" selected, no specific collection -- Zotero's `getSelectedCollection`
    // omits `id` entirely in this case.
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        ScriptedResponse::json(200, json!({"libraryID": 1, "libraryName": "My Library"})),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[("CLI_ANYTHING_ZOTERO_STATE_DIR", state_dir.to_str().unwrap())],
        &["collection", "use-selected"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert!(value.get("id").is_none());

    let state = read_session_state(&state_dir);
    assert_eq!(state["current_library"], 1);
    assert_eq!(state["current_collection"], serde_json::Value::Null);
}

#[test]
fn collection_use_selected_connector_unavailable_is_domain_error_with_no_further_calls() {
    let dir = TestDir::new("coll-use-selected-connector-down");
    build_fixture_sqlite(dir.path());
    let state_dir = dir.path().join("state");

    let server = ScriptedServer::start(vec![
        connector_ping_unavailable(),
        local_api_probe_available(),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[("CLI_ANYTHING_ZOTERO_STATE_DIR", state_dir.to_str().unwrap())],
        &["collection", "use-selected"],
    );
    let requests = server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert!(value["error"]
        .as_str()
        .unwrap_or_default()
        .starts_with("Zotero connector is not available"));
    assert_eq!(
        requests.len(),
        2,
        "no getSelectedCollection call should fire when the connector is unavailable"
    );
    assert!(
        !state_dir.join("session.json").exists(),
        "session state must not be written on failure"
    );
}

#[test]
fn collection_use_selected_malformed_response_is_a_clean_error() {
    let dir = TestDir::new("coll-use-selected-malformed");
    build_fixture_sqlite(dir.path());
    let state_dir = dir.path().join("state");

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        text_response(200, "not valid json"),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[("CLI_ANYTHING_ZOTERO_STATE_DIR", state_dir.to_str().unwrap())],
        &["collection", "use-selected"],
    );
    server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert!(!value["error"].as_str().unwrap_or_default().is_empty());
    assert!(!state_dir.join("session.json").exists());
}

#[test]
fn collection_use_selected_human_mode_matches_json_mode_for_dict_payload() {
    // `emit()`'s dict branch (`zotero_cli.py:311-313`) always renders `_json_text`, regardless
    // of `--json` -- a dict payload is never given special "human" formatting. Both invocations
    // must therefore produce byte-identical stdout.
    let dir = TestDir::new("coll-use-selected-human");
    build_fixture_sqlite(dir.path());

    let selection = json!({"libraryID": 1, "libraryName": "My Library", "id": 7, "name": "X"});

    let server_json = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        ScriptedResponse::json(200, selection.clone()),
    ]);
    let (json_code, json_value) = run_cli(
        dir.path(),
        server_json.port,
        &[(
            "CLI_ANYTHING_ZOTERO_STATE_DIR",
            dir.path().join("state-json").to_str().unwrap(),
        )],
        &["collection", "use-selected"],
    );
    server_json.finish();

    let server_human = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        ScriptedResponse::json(200, selection),
    ]);
    let (human_code, human_stdout) = run_cli_human(
        dir.path(),
        server_human.port,
        &[(
            "CLI_ANYTHING_ZOTERO_STATE_DIR",
            dir.path().join("state-human").to_str().unwrap(),
        )],
        &["collection", "use-selected"],
    );
    server_human.finish();

    assert_eq!(json_code, 0);
    assert_eq!(human_code, 0);
    assert_eq!(
        serde_json::to_string_pretty(&json_value).unwrap(),
        human_stdout.trim()
    );
}

// ── session use-selected ────────────────────────────────────────────────

#[test]
fn session_use_selected_success_emits_selected_and_session_payload() {
    let dir = TestDir::new("session-use-selected-success");
    build_fixture_sqlite(dir.path());
    let state_dir = dir.path().join("state");

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        ScriptedResponse::json(
            200,
            json!({"libraryID": 1, "libraryName": "My Library", "id": 99, "name": "Papers"}),
        ),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[("CLI_ANYTHING_ZOTERO_STATE_DIR", state_dir.to_str().unwrap())],
        &["session", "use-selected"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["selected"]["id"], 99);
    assert_eq!(value["selected"]["name"], "Papers");
    assert_eq!(value["session"]["current_library"], 1);
    assert_eq!(value["session"]["current_collection"], 99);
    // `history_count` here is 0, not 1: `append_command_history()` (`session.py:95-103`) reloads
    // its own fresh copy of state from disk and saves that -- it never mutates the in-memory
    // `state` object `session_use_selected` already holds and passes to `build_session_payload`.
    // The append lands on disk (see `session_use_selected_persists_across_invocations`) but this
    // command's own emitted payload under-reports its own history entry by one, matching
    // Python's `session_use_selected` (`zotero_cli.py:2371-2378`) byte-for-byte.
    assert_eq!(value["session"]["history_count"], 0);
}

#[test]
fn session_use_selected_empty_selection_leaves_collection_null() {
    let dir = TestDir::new("session-use-selected-empty");
    build_fixture_sqlite(dir.path());
    let state_dir = dir.path().join("state");

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        ScriptedResponse::json(200, json!({"libraryID": 3, "libraryName": "Group Lib"})),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[("CLI_ANYTHING_ZOTERO_STATE_DIR", state_dir.to_str().unwrap())],
        &["session", "use-selected"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["session"]["current_library"], 3);
    assert_eq!(
        value["session"]["current_collection"],
        serde_json::Value::Null
    );
}

#[test]
fn session_use_selected_connector_unavailable_is_domain_error() {
    let dir = TestDir::new("session-use-selected-connector-down");
    build_fixture_sqlite(dir.path());
    let state_dir = dir.path().join("state");

    let server = ScriptedServer::start(vec![
        connector_ping_unavailable(),
        local_api_probe_available(),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[("CLI_ANYTHING_ZOTERO_STATE_DIR", state_dir.to_str().unwrap())],
        &["session", "use-selected"],
    );
    let requests = server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert!(value["error"]
        .as_str()
        .unwrap_or_default()
        .starts_with("Zotero connector is not available"));
    assert_eq!(requests.len(), 2);
    assert!(!state_dir.join("session.json").exists());
}

#[test]
fn session_use_selected_malformed_response_is_a_clean_error() {
    let dir = TestDir::new("session-use-selected-malformed");
    build_fixture_sqlite(dir.path());
    let state_dir = dir.path().join("state");

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        text_response(200, "{not json"),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[("CLI_ANYTHING_ZOTERO_STATE_DIR", state_dir.to_str().unwrap())],
        &["session", "use-selected"],
    );
    server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert!(!value["error"].as_str().unwrap_or_default().is_empty());
}

#[test]
fn session_use_selected_persists_across_invocations() {
    let dir = TestDir::new("session-use-selected-persist");
    build_fixture_sqlite(dir.path());
    let state_dir = dir.path().join("state");

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        ScriptedResponse::json(
            200,
            json!({"libraryID": 1, "libraryName": "My Library", "id": 5, "name": "Reading"}),
        ),
    ]);
    let (code, _value) = run_cli(
        dir.path(),
        server.port,
        &[("CLI_ANYTHING_ZOTERO_STATE_DIR", state_dir.to_str().unwrap())],
        &["session", "use-selected"],
    );
    let port = server.port;
    server.finish();
    assert_eq!(code, 0);

    // `session status` never calls `build_runtime()` -- zero HTTP traffic expected.
    let second_server = ScriptedServer::start(vec![]);
    let (status_code, status_value) = run_cli(
        dir.path(),
        port,
        &[("CLI_ANYTHING_ZOTERO_STATE_DIR", state_dir.to_str().unwrap())],
        &["session", "status"],
    );
    let second_requests = second_server.finish();

    assert_eq!(status_code, 0, "stdout={status_value}");
    assert_eq!(status_value["current_library"], 1);
    assert_eq!(status_value["current_collection"], 5);
    assert!(second_requests.is_empty());
}

// ── collection stats ───────────────────────────────────────────────────

#[test]
fn collection_stats_normal_collection_returns_full_schema() {
    let dir = TestDir::new("collection-stats-normal");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({
                "total": 12,
                "withPDF": 9,
                "noPDF": 3,
                "byYear": {"2020": 4, "2021": 8},
                "topJournals": [{"journal": "Nature", "count": 5}, {"journal": "Science", "count": 3}],
            }),
        ),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["collection", "stats", "COLLE001"],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["total"], 12);
    assert_eq!(value["withPDF"], 9);
    assert_eq!(value["noPDF"], 3);
    assert_eq!(value["byYear"]["2020"], 4);
    assert_eq!(value["topJournals"][0]["journal"], "Nature");

    let eval_body = String::from_utf8_lossy(&requests[1].body);
    assert!(eval_body.contains("\\\"libraryID\\\":1"));
    assert!(eval_body.contains("\\\"collectionKey\\\":\\\"COLLE001\\\""));
    assert!(eval_body.contains("!i.isAttachment() && !i.isNote()"));
}

#[test]
fn collection_stats_empty_collection_returns_zeroed_schema() {
    let dir = TestDir::new("collection-stats-empty");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({"total": 0, "withPDF": 0, "noPDF": 0, "byYear": {}, "topJournals": []}),
        ),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["collection", "stats", "COLLE001"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["total"], 0);
    assert_eq!(value["byYear"], json!({}));
    assert_eq!(value["topJournals"], json!([]));
}

#[test]
fn collection_stats_missing_collection_is_exit_zero_with_error_string() {
    // `emit_js`'s quirk (`zotero_cli.py:317-349`): a bare `"ERROR: ..."` *string* return from the
    // JS itself is a transport-level *success*, not a `{"ok": false, ...}` object, so this is
    // exit code 0 -- not 1 -- exactly matching Python, however counter-intuitive.
    let dir = TestDir::new("collection-stats-missing");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(200, json!("ERROR: collection NOSUCHKEY not found")),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["collection", "stats", "NOSUCHKEY"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value, json!("ERROR: collection NOSUCHKEY not found"));
}

#[test]
fn collection_stats_malformed_bridge_result_is_transport_failure() {
    let dir = TestDir::new("collection-stats-malformed");
    build_fixture_sqlite(dir.path());

    // Ownership probe returns 200 but the wrong fork/id -- `bridge_endpoint_active()` treats
    // this as "no plugin installed", so `execute_js` never issues the real eval call at all.
    let server = ScriptedServer::start(vec![bridge_ownership_wrong_fork()]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["collection", "stats", "COLLE001"],
    );
    let requests = server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert_eq!(value["ok"], false);
    assert!(!value["error"].as_str().unwrap_or_default().is_empty());
    assert_eq!(
        requests.len(),
        1,
        "no real eval call after a failed ownership probe"
    );
}

#[test]
fn collection_stats_human_mode_matches_json_mode_for_dict_payload() {
    let dir = TestDir::new("collection-stats-human");
    build_fixture_sqlite(dir.path());
    let stats =
        json!({"total": 1, "withPDF": 0, "noPDF": 1, "byYear": {"2022": 1}, "topJournals": []});

    let server_json = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(200, stats.clone()),
    ]);
    let (json_code, json_value) = run_cli(
        dir.path(),
        server_json.port,
        &[],
        &["collection", "stats", "COLLE001"],
    );
    server_json.finish();

    let server_human = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(200, stats),
    ]);
    let (human_code, human_stdout) = run_cli_human(
        dir.path(),
        server_human.port,
        &[],
        &["collection", "stats", "COLLE001"],
    );
    server_human.finish();

    assert_eq!(json_code, 0);
    assert_eq!(human_code, 0);
    assert_eq!(
        serde_json::to_string_pretty(&json_value).unwrap(),
        human_stdout.trim()
    );
}
