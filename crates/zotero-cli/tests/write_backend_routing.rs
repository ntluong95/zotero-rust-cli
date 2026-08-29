//! Phase 6 Slice 6 CLI + routing integration tests: exercises the actual `zotero-cli` binary
//! (not `write_router`/`bridge` unit functions directly, which already have their own dedicated
//! test files) against a scripted mock server standing in for Zotero's HTTP surface, proving the
//! *routing decisions* Slice 6 wires -- which backend gets selected, how each `WriteOutcome`
//! variant surfaces at the CLI boundary, and the data-integrity invariants (full-array-replace,
//! no-fake-success) that must hold at that boundary.
//!
//! Connector-routed test cases (I/J in the required matrix) are intentionally absent: per
//! `phase-06-js-bridge-and-injection-hardening.md`'s Overview/§3.6, Phase 6 owns zero
//! Connector-routed commands -- every command this phase implements is Local-API-or-JS-Bridge.
//! Phase 7 owns the Connector-routed import commands and should adopt this same harness pattern.

#[path = "common/mod.rs"]
mod common;

use common::{
    assert_no_forbidden_keys, build_fixture_sqlite, read_stored_credentials, run_cli,
    write_stored_credential, ScriptedResponse, ScriptedServer, TestDir,
};
use serde_json::json;

const SERVER_ID: &str = "TEST-SERVER-1";

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

fn connector_ping_ok() -> ScriptedResponse {
    ScriptedResponse::json(200, json!({}))
}

fn item_get_response(version: i64, collections: Vec<&str>) -> ScriptedResponse {
    ScriptedResponse::json(
        200,
        json!({
            "key": "ITEM0001",
            "version": version,
            "library": {"id": 0},
            "data": {
                "itemType": "document",
                "title": "Test Item One",
                "collections": collections,
                "tags": [],
            },
        }),
    )
}

fn bridge_ownership_ok() -> ScriptedResponse {
    ScriptedResponse::json(
        200,
        json!({"fork": "zotero-rust-cli", "id": "cli-bridge@cli-anything-rust.dev"}),
    )
}

fn bridge_ownership_foreign() -> ScriptedResponse {
    ScriptedResponse::json(
        200,
        json!({"fork": "some-other-fork", "id": "unknown@example.dev"}),
    )
}

// ── A. local_api_writes_available == true + valid authorization -> Local API selected ──

#[test]
fn local_api_write_with_env_credential_is_applied_and_matches_the_stable_output_contract() {
    let dir = TestDir::new("scenario-a");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        item_get_response(5, vec![]),
        ScriptedResponse::Http {
            status: 204,
            headers: vec![("Last-Modified-Version".to_string(), "6".to_string())],
            body: Vec::new(),
        },
        ScriptedResponse::json(
            200,
            json!({
                "key": "ITEM0001",
                "version": 6,
                "library": {"id": 0},
                "data": {
                    "itemType": "document",
                    "title": "New Title",
                    "collections": [],
                    "tags": [],
                },
            }),
        ),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_LOCAL_API_KEY", "env-supplied-key")],
        &["item", "update", "ITEM0001", "--field", "title=New Title"],
    );

    let requests = server.finish();
    assert_eq!(requests.len(), 5, "ping, probe, GET, PATCH, GET(verify)");
    assert_eq!(requests[3].method, "PATCH");
    assert_eq!(requests[3].path, "/api/users/0/items/ITEM0001");

    assert_eq!(code, 0, "payload: {payload}");
    // N: representative Phase 6 command JSON output remains stable -- exact top-level key set,
    // no `field_mismatches` (title matched what was requested), no version/backend/server_id.
    let mut keys: Vec<&str> = payload
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["data", "item_type", "key", "library_id", "outcome"]
    );
    assert_eq!(payload["outcome"], "applied");
    assert_eq!(payload["key"], "ITEM0001");
}

// ── B. authorization required -> AuthorizationFailed::Required, zero write attempts ──

#[test]
fn missing_credential_returns_authorization_required_without_ever_attempting_the_patch() {
    let dir = TestDir::new("scenario-b");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        item_get_response(5, vec![]),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "update", "ITEM0001", "--field", "title=New Title"],
    );

    let requests = server.finish();
    assert_eq!(
        requests.len(),
        3,
        "ping, probe, and the pre-write GET only -- no PATCH attempt"
    );

    assert_eq!(code, 3, "payload: {payload}");
    assert_eq!(payload["outcome"], "authorization_failed");
    assert_eq!(payload["reason"], "required");
    assert_eq!(payload["needs_human_action"], true);
}

// ── C. persisted credential revoked -> Revoked, stored credential removed, no dialog attempt ──

#[test]
fn revoked_stored_credential_is_removed_and_never_triggers_an_authorize_call() {
    let dir = TestDir::new("scenario-c");
    build_fixture_sqlite(dir.path());
    let state_dir = dir.path().join("state");
    write_stored_credential(&state_dir, SERVER_ID, "now-revoked-key");

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        item_get_response(5, vec![]),
        ScriptedResponse::json(401, json!("Invalid or expired API key")),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[("CLI_ANYTHING_ZOTERO_STATE_DIR", state_dir.to_str().unwrap())],
        &["item", "update", "ITEM0001", "--field", "title=New Title"],
    );

    let requests = server.finish();
    assert_eq!(
        requests.len(),
        4,
        "ping, probe, GET, and exactly one PATCH -- no /api/local/authorize call"
    );
    assert!(
        requests.iter().all(|r| r.path != "/api/local/authorize"),
        "must never silently attempt the authorize handshake"
    );

    assert_eq!(code, 3, "payload: {payload}");
    assert_eq!(payload["outcome"], "authorization_failed");
    assert_eq!(payload["reason"], "revoked");

    let remaining = read_stored_credentials(&state_dir);
    assert!(
        remaining["credentials"].get(SERVER_ID).is_none(),
        "the revoked stored credential must be removed: {remaining}"
    );
}

// ── D. Local API transport ambiguity -> TransportError, no automatic retry ──

#[test]
fn ambiguous_transport_failure_on_patch_maps_to_transport_error_with_exactly_one_attempt() {
    let dir = TestDir::new("scenario-d");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        item_get_response(5, vec![]),
        ScriptedResponse::Drop,
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_LOCAL_API_KEY", "env-supplied-key")],
        &["item", "update", "ITEM0001", "--field", "title=New Title"],
    );

    let requests = server.finish();
    assert_eq!(
        requests.len(),
        4,
        "ping, probe, GET, and exactly one (dropped) PATCH attempt -- no retry"
    );

    assert_eq!(code, 1, "payload: {payload}");
    assert_eq!(payload["outcome"], "transport_error");
    assert_eq!(payload["needs_human_action"], false);
}

// ── E. Local API writes unavailable + our Bridge active -> Bridge fallback ──

#[test]
fn local_api_unavailable_falls_back_to_our_owned_bridge() {
    let dir = TestDir::new("scenario-e");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        ScriptedResponse::bridge_string(200, "OK: updated Test Item One"),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "update", "ITEM0001", "--field", "title=New Title"],
    );

    let requests = server.finish();
    assert_eq!(
        requests.len(),
        4,
        "ping, probe, bridge-ownership-ping, bridge-eval"
    );
    assert_eq!(requests[3].path, "/cli-bridge/eval");

    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(
        payload["key"], "ITEM0001",
        "Bridge path re-reads via SQLite"
    );
}

// ── F. Local API writes unavailable + Bridge inactive -> deterministic failure ──

#[test]
fn local_api_unavailable_and_bridge_inactive_fails_deterministically() {
    let dir = TestDir::new("scenario-f");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_foreign(),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "update", "ITEM0001", "--field", "title=New Title"],
    );

    let requests = server.finish();
    assert_eq!(
        requests.len(),
        3,
        "ping, probe, and one ownership probe -- no second (eval) request once ownership fails"
    );

    assert_eq!(code, 1, "payload: {payload}");
    assert_eq!(payload["outcome"], "transport_error");
    assert!(payload["detail"]
        .as_str()
        .unwrap_or_default()
        .contains("CLI Bridge"));
}

// ── G. privileged command (sync) + our Bridge active -> Bridge selected ──

#[test]
fn sync_routes_to_our_owned_bridge_and_never_probes_local_api_at_all() {
    let dir = TestDir::new("scenario-g");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::bridge_string(200, "Sync completed"),
    ]);

    let (code, payload) = run_cli(dir.path(), server.port, &[], &["sync"]);

    let requests = server.finish();
    assert_eq!(
        requests.len(),
        2,
        "sync never builds a RuntimeContext -- no connector/local-API probes at all"
    );

    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(payload, "Sync completed");
}

// ── H. foreign/wrong Bridge ownership -> privileged operation rejected ──

#[test]
fn foreign_bridge_ownership_rejects_the_privileged_sync_call() {
    let dir = TestDir::new("scenario-h");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![bridge_ownership_foreign()]);

    let (code, payload) = run_cli(dir.path(), server.port, &[], &["sync"]);

    let requests = server.finish();
    assert_eq!(
        requests.len(),
        1,
        "must reject before ever sending the privileged eval payload"
    );

    assert_eq!(code, 1, "payload: {payload}");
    assert!(payload["error"]
        .as_str()
        .unwrap_or_default()
        .contains("CLI Bridge"));
}

// ── K. add-to-collection preserves unrelated memberships (full-array-replace) ──

#[test]
fn add_to_collection_preserves_unrelated_existing_memberships() {
    let dir = TestDir::new("scenario-k");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        item_get_response(5, vec!["EXISTC1"]),
        ScriptedResponse::Http {
            status: 204,
            headers: vec![("Last-Modified-Version".to_string(), "6".to_string())],
            body: Vec::new(),
        },
        item_get_response(6, vec!["EXISTC1", "COLLE001"]),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_LOCAL_API_KEY", "env-supplied-key")],
        &["item", "add-to-collection", "ITEM0001", "COLLE001"],
    );

    let requests = server.finish();
    assert_eq!(requests.len(), 5);
    let patch_body = requests[3].body_json();
    assert_eq!(
        patch_body["collections"],
        json!(["EXISTC1", "COLLE001"]),
        "must submit the union, never a naive single-element array that would strip EXISTC1"
    );

    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(
        payload["data"]["collections"],
        json!(["EXISTC1", "COLLE001"])
    );
}

// ── L. malformed write success cannot become Applied ──

#[test]
fn malformed_create_response_never_becomes_applied() {
    let dir = TestDir::new("scenario-l");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        ScriptedResponse::Http {
            status: 201,
            headers: Vec::new(),
            body: b"not json at all".to_vec(),
        },
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_LOCAL_API_KEY", "env-supplied-key")],
        &["collection", "create", "Brand New Collection"],
    );

    server.finish();

    assert_eq!(code, 1, "payload: {payload}");
    assert_eq!(payload["outcome"], "transport_error");
    assert_ne!(payload["outcome"], "applied");
}

// ── M. affected_key is preserved where available ──

#[test]
fn collection_create_preserves_the_servers_affected_key() {
    let dir = TestDir::new("scenario-m");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        ScriptedResponse::json(
            201,
            json!({"successful": {"0": {"key": "NEWCOL01", "version": 1}}}),
        ),
        ScriptedResponse::json(
            200,
            json!({
                "key": "NEWCOL01",
                "version": 1,
                "library": {"id": 0},
                "data": {"name": "Brand New Collection"},
            }),
        ),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_LOCAL_API_KEY", "env-supplied-key")],
        &["collection", "create", "Brand New Collection"],
    );

    server.finish();

    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(payload["key"], "NEWCOL01");
    assert_no_forbidden_keys(&payload, &["backend", "server_id", "version"], "$");
}

// ── O. bare `zotero-cli` -> help, exit 0 ──

#[test]
fn bare_invocation_prints_help_and_exits_zero() {
    let output = std::process::Command::new(common::bin_path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Agent-native Zotero CLI"));
}
