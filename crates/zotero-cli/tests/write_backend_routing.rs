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

fn select_library(dir: &std::path::Path, library_id: i64) {
    let state_dir = dir.join("cli-state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(
        state_dir.join("session.json"),
        json!({"current_library": library_id}).to_string(),
    )
    .unwrap();
}

fn group_resolution(key: &str, object: &str) -> ScriptedResponse {
    let mut resolved = json!({
        "found": true,
        "key": key,
        "libraryID": 2,
        "libraryType": "group",
        "groupID": 4597652,
    });
    match object {
        "item" => {
            resolved["itemType"] = json!("document");
            resolved["itemID"] = json!(21);
        }
        "collection" => {
            resolved["name"] = json!("Group Collection");
            resolved["collectionID"] = json!(31);
        }
        "library" => {}
        _ => panic!("unexpected object"),
    }
    ScriptedResponse::json(200, json!(resolved.to_string()))
}

fn group_object_response(key: &str, name: &str, version: i64) -> ScriptedResponse {
    ScriptedResponse::json(
        200,
        json!({
            "key": key,
            "version": version,
            "library": {"id": 4597652, "type": "group"},
            "data": {"name": name},
        }),
    )
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

/// The live target resolution every write command performs before it writes.
///
/// Write commands no longer resolve their target from SQLite: a running Zotero holds an
/// exclusive lock on its WAL-mode database, so a SQLite lookup made the command fail during
/// *target lookup* in exactly the situation it exists for. Resolution now goes through the same
/// live backend the write itself will use, which is why each of these tests carries one
/// resolution response per resolved object.
///
/// Local-API-routed commands resolve with `GET {scope}/items/{key}` -- the same response shape
/// as `item_get_response`.
fn local_api_resolve_item() -> ScriptedResponse {
    item_get_response(5, vec![])
}

fn local_api_resolve_collection(key: &str, name: &str) -> ScriptedResponse {
    ScriptedResponse::json(
        200,
        json!({
            "key": key,
            "version": 1,
            "library": {"id": 0},
            "data": {"key": key, "name": name},
        }),
    )
}

/// Bridge-routed commands resolve through an eval whose template returns a JSON *string*, so the
/// transport body is a quoted string the resolver parses once more.
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

fn bridge_resolve_collection(key: &str, name: &str, collection_id: i64) -> ScriptedResponse {
    ScriptedResponse::json(
        200,
        json!(json!({
            "found": true,
            "key": key,
            "libraryID": 1,
            "libraryType": "user",
            "name": name,
            "collectionID": collection_id,
        })
        .to_string()),
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
        local_api_resolve_item(),
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
    assert_eq!(
        requests.len(),
        6,
        "ping, probe, live-resolve GET, GET, PATCH, GET(verify)"
    );
    assert_eq!(requests[4].method, "PATCH");
    assert_eq!(requests[4].path, "/api/users/0/items/ITEM0001");

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
        local_api_resolve_item(),
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
        4,
        "ping, probe, live-resolve GET, and the pre-write GET only -- no PATCH attempt"
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
        local_api_resolve_item(),
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
        5,
        "ping, probe, live-resolve GET, GET, and exactly one PATCH -- never /api/local/authorize"
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
        local_api_resolve_item(),
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
        5,
        "ping, probe, live-resolve GET, GET, and exactly one (dropped) PATCH attempt -- no retry"
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
        bridge_resolve_item("ITEM0001", 1),
        ScriptedResponse::bridge_string(200, "OK: updated Test Item One"),
        // Post-write live Zotero-runtime readback (never SQLite) -- the positive ownership
        // probe is cached for the rest of this process, so this costs exactly one more
        // request, not a second ownership ping.
        ScriptedResponse::json(
            200,
            json!({
                "found": true,
                "key": "ITEM0001",
                "libraryID": 1,
                "data": {"itemType": "document", "title": "New Title"},
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
        requests.len(),
        6,
        "ping, probe, bridge-ownership-ping, live-resolve-eval, write-eval, live-readback-eval"
    );
    for request in &requests[3..] {
        assert_eq!(request.path, "/cli-bridge/eval");
    }

    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(
        payload["key"], "ITEM0001",
        "Bridge path re-reads live through the Bridge, never SQLite"
    );
    assert_eq!(payload["data"]["title"], "New Title");
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
        local_api_resolve_collection("COLLE001", "Test Collection"),
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
    assert_eq!(
        requests.len(),
        7,
        "ping, probe, live-resolve item, live-resolve collection, GET, PATCH, GET(verify)"
    );
    let patch_body = requests[5].body_json();
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
        // The duplicate-name guard reads the collection list live through the Local API
        // rather than from SQLite, which a running Zotero holds locked.
        ScriptedResponse::json(200, json!([])),
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
        ScriptedResponse::json(200, json!([])),
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

    let requests = server.finish();

    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(payload["key"], "NEWCOL01");
    // Zotero's Local API rejects a bare object with HTTP 400 "Uploaded data must be a JSON
    // array": creation always posts the Web API's array-of-objects shape.
    let create = requests
        .iter()
        .find(|request| request.method == "POST")
        .expect("a create request");
    assert_eq!(
        create.body_json(),
        json!([{"name": "Brand New Collection"}]),
        "the create body must be a one-element JSON array"
    );
    assert_no_forbidden_keys(&payload, &["backend", "server_id", "version"], "$");
}

#[test]
fn selected_group_item_key_resolves_in_group_before_local_api_write() {
    let dir = TestDir::new("selected-group-item");
    build_fixture_sqlite(dir.path()); // Also contains ITEM0001 in My Library.
    select_library(dir.path(), 2);
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        bridge_ownership_ok(),
        group_resolution("ITEM0001", "item"),
        ScriptedResponse::json(
            200,
            json!({
                "key": "ITEM0001", "version": 5,
                "library": {"id": 4597652, "type": "group"},
                "data": {"itemType": "document", "title": "Old Title", "collections": [], "tags": []},
            }),
        ),
        ScriptedResponse::Http {
            status: 204,
            headers: vec![("Last-Modified-Version".into(), "6".into())],
            body: Vec::new(),
        },
        ScriptedResponse::json(
            200,
            json!({
                "key": "ITEM0001", "version": 6,
                "library": {"id": 4597652, "type": "group"},
                "data": {"itemType": "document", "title": "New Title", "collections": [], "tags": []},
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
    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(requests.len(), 7);
    assert_eq!(requests[3].method, "POST"); // Bridge resolution precedes any item GET.
    assert_eq!(requests[5].path, "/api/groups/4597652/items/ITEM0001");
    assert_eq!(requests[5].method, "PATCH");
}

#[test]
fn selected_group_collection_key_resolves_in_group_before_local_api_write() {
    let dir = TestDir::new("selected-group-collection");
    build_fixture_sqlite(dir.path()); // Also contains COLLE001 in My Library.
    select_library(dir.path(), 2);
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        bridge_ownership_ok(),
        group_resolution("COLLE001", "collection"),
        group_object_response("COLLE001", "Group Collection", 1),
        ScriptedResponse::Http {
            status: 204,
            headers: vec![("Last-Modified-Version".into(), "2".into())],
            body: Vec::new(),
        },
        group_object_response("COLLE001", "Renamed Collection", 2),
    ]);
    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_LOCAL_API_KEY", "env-supplied-key")],
        &[
            "collection",
            "rename",
            "COLLE001",
            "--name",
            "Renamed Collection",
        ],
    );
    let requests = server.finish();
    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(requests.len(), 7);
    assert_eq!(requests[5].path, "/api/groups/4597652/collections/COLLE001");
    assert_eq!(requests[5].method, "PATCH");
}

#[test]
fn selected_group_collection_create_uses_group_id_scope() {
    let dir = TestDir::new("selected-group-create");
    build_fixture_sqlite(dir.path());
    select_library(dir.path(), 2);
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        bridge_ownership_ok(),
        group_resolution("", "library"),
        ScriptedResponse::json(200, json!([])),
        ScriptedResponse::json(
            201,
            json!({
                "successful": {"0": {"key": "NEWCOL01", "version": 1}}
            }),
        ),
        group_object_response("NEWCOL01", "New Group Collection", 1),
    ]);
    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_LOCAL_API_KEY", "env-supplied-key")],
        &["collection", "create", "New Group Collection"],
    );
    let requests = server.finish();
    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(requests.len(), 7);
    assert_eq!(
        requests[4].path,
        "/api/groups/4597652/collections?format=json"
    );
    assert_eq!(requests[5].path, "/api/groups/4597652/collections");
    assert_eq!(requests[5].method, "POST");
}

#[test]
fn selected_personal_library_create_keeps_user_scope() {
    let dir = TestDir::new("selected-personal-create");
    build_fixture_sqlite(dir.path());
    select_library(dir.path(), 1);
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!(json!({
                "found": true,
                "libraryID": 1,
                "libraryType": "user",
                "groupID": null,
            })
            .to_string()),
        ),
        ScriptedResponse::json(200, json!([])),
        ScriptedResponse::json(
            201,
            json!({"successful": {"0": {"key": "NEWCOL01", "version": 1}}}),
        ),
        local_api_resolve_collection("NEWCOL01", "New Personal Collection"),
    ]);
    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_LOCAL_API_KEY", "env-supplied-key")],
        &["collection", "create", "New Personal Collection"],
    );
    let requests = server.finish();
    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(requests[5].path, "/api/users/0/collections");
}

// ── O. bare `zotero-cli` -> help, exit 0 ──

#[test]
fn bare_invocation_prints_help_and_exits_zero() {
    let output = common::cli_command().output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Agent-native Zotero CLI"));
}

// ── Review Blocker 1: backend-neutral output schema ──

fn sorted_keys(payload: &serde_json::Value) -> Vec<String> {
    let mut keys: Vec<String> = payload.as_object().unwrap().keys().cloned().collect();
    keys.sort_unstable();
    keys
}

#[test]
fn item_update_output_schema_is_identical_across_local_api_and_bridge_backends() {
    let local_dir = TestDir::new("equivalence-item-update-local-api");
    build_fixture_sqlite(local_dir.path());
    let local_server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        local_api_resolve_item(),
        item_get_response(5, vec![]),
        ScriptedResponse::Http {
            status: 204,
            headers: vec![("Last-Modified-Version".to_string(), "6".to_string())],
            body: Vec::new(),
        },
        ScriptedResponse::json(
            200,
            json!({
                "key": "ITEM0001", "version": 6, "library": {"id": 0},
                "data": {"itemType": "document", "title": "New Title", "collections": [], "tags": []},
            }),
        ),
    ]);
    let (local_code, local_payload) = run_cli(
        local_dir.path(),
        local_server.port,
        &[("ZOTERO_LOCAL_API_KEY", "env-supplied-key")],
        &["item", "update", "ITEM0001", "--field", "title=New Title"],
    );
    local_server.finish();
    assert_eq!(local_code, 0, "local API payload: {local_payload}");

    let bridge_dir = TestDir::new("equivalence-item-update-bridge");
    build_fixture_sqlite(bridge_dir.path());
    let bridge_server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        bridge_resolve_item("ITEM0001", 1),
        ScriptedResponse::bridge_string(200, "OK: updated Test Item One"),
        ScriptedResponse::json(
            200,
            json!({
                "found": true,
                "key": "ITEM0001",
                "libraryID": 1,
                "data": {"itemType": "document", "title": "New Title"},
            }),
        ),
    ]);
    let (bridge_code, bridge_payload) = run_cli(
        bridge_dir.path(),
        bridge_server.port,
        &[],
        &["item", "update", "ITEM0001", "--field", "title=New Title"],
    );
    bridge_server.finish();
    assert_eq!(bridge_code, 0, "bridge payload: {bridge_payload}");

    assert_eq!(
        sorted_keys(&local_payload),
        sorted_keys(&bridge_payload),
        "the same command must produce the same JSON schema regardless of backend: \
         local={local_payload} bridge={bridge_payload}"
    );
    assert_eq!(local_payload["outcome"], bridge_payload["outcome"]);
    assert_eq!(local_payload["key"], bridge_payload["key"]);
    assert_eq!(
        local_payload["data"]["title"],
        bridge_payload["data"]["title"]
    );
}

#[test]
fn collection_rename_output_schema_is_identical_across_local_api_and_bridge_backends() {
    let local_dir = TestDir::new("equivalence-collection-rename-local-api");
    build_fixture_sqlite(local_dir.path());
    let local_server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        local_api_resolve_collection("COLLE001", "Test Collection"),
        ScriptedResponse::json(
            200,
            json!({
                "key": "COLLE001", "version": 1, "library": {"id": 0},
                "data": {"name": "Test Collection"},
            }),
        ),
        ScriptedResponse::Http {
            status: 204,
            headers: vec![("Last-Modified-Version".to_string(), "2".to_string())],
            body: Vec::new(),
        },
        ScriptedResponse::json(
            200,
            json!({
                "key": "COLLE001", "version": 2, "library": {"id": 0},
                "data": {"name": "Renamed Collection"},
            }),
        ),
    ]);
    let (local_code, local_payload) = run_cli(
        local_dir.path(),
        local_server.port,
        &[("ZOTERO_LOCAL_API_KEY", "env-supplied-key")],
        &[
            "collection",
            "rename",
            "COLLE001",
            "--name",
            "Renamed Collection",
        ],
    );
    local_server.finish();
    assert_eq!(local_code, 0, "local API payload: {local_payload}");

    let bridge_dir = TestDir::new("equivalence-collection-rename-bridge");
    build_fixture_sqlite(bridge_dir.path());
    let bridge_server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        bridge_resolve_collection("COLLE001", "Test Collection", 1),
        ScriptedResponse::bridge_string(200, "OK: updated collection Renamed Collection"),
        ScriptedResponse::json(
            200,
            json!({
                "found": true,
                "key": "COLLE001",
                "libraryID": 1,
                "data": {"name": "Renamed Collection"},
            }),
        ),
    ]);
    let (bridge_code, bridge_payload) = run_cli(
        bridge_dir.path(),
        bridge_server.port,
        &[],
        &[
            "collection",
            "rename",
            "COLLE001",
            "--name",
            "Renamed Collection",
        ],
    );
    bridge_server.finish();
    assert_eq!(bridge_code, 0, "bridge payload: {bridge_payload}");

    assert_eq!(
        sorted_keys(&local_payload),
        sorted_keys(&bridge_payload),
        "local={local_payload} bridge={bridge_payload}"
    );
    assert_eq!(
        local_payload["data"]["name"],
        bridge_payload["data"]["name"]
    );
}

// ── Array-preservation tests (review-required) ──

#[test]
fn item_tag_add_preserves_unrelated_existing_tags() {
    let dir = TestDir::new("array-tag-add");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        local_api_resolve_item(),
        ScriptedResponse::json(
            200,
            json!({
                "key": "ITEM0001", "version": 5, "library": {"id": 0},
                "data": {"itemType": "document", "title": "Test Item One", "collections": [],
                         "tags": [{"tag": "keep-me"}]},
            }),
        ),
        ScriptedResponse::Http {
            status: 204,
            headers: vec![("Last-Modified-Version".to_string(), "6".to_string())],
            body: Vec::new(),
        },
        ScriptedResponse::json(
            200,
            json!({
                "key": "ITEM0001", "version": 6, "library": {"id": 0},
                "data": {"itemType": "document", "title": "Test Item One", "collections": [],
                         "tags": [{"tag": "keep-me"}, {"tag": "new-tag"}]},
            }),
        ),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_LOCAL_API_KEY", "env-supplied-key")],
        &["item", "tag", "ITEM0001", "--add", "new-tag"],
    );

    let requests = server.finish();
    let patch_body = requests[4].body_json();
    assert_eq!(
        patch_body["tags"],
        json!([{"tag": "keep-me"}, {"tag": "new-tag"}]),
        "adding a tag must not drop the item's existing tags"
    );

    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(payload.get("field_mismatches"), None);
}

#[test]
fn item_tag_remove_removes_only_the_named_tag() {
    let dir = TestDir::new("array-tag-remove");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        local_api_resolve_item(),
        ScriptedResponse::json(
            200,
            json!({
                "key": "ITEM0001", "version": 5, "library": {"id": 0},
                "data": {"itemType": "document", "title": "Test Item One", "collections": [],
                         "tags": [{"tag": "keep-me"}, {"tag": "remove-me"}]},
            }),
        ),
        ScriptedResponse::Http {
            status: 204,
            headers: vec![("Last-Modified-Version".to_string(), "6".to_string())],
            body: Vec::new(),
        },
        ScriptedResponse::json(
            200,
            json!({
                "key": "ITEM0001", "version": 6, "library": {"id": 0},
                "data": {"itemType": "document", "title": "Test Item One", "collections": [],
                         "tags": [{"tag": "keep-me"}]},
            }),
        ),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_LOCAL_API_KEY", "env-supplied-key")],
        &["item", "tag", "ITEM0001", "--remove", "remove-me"],
    );

    let requests = server.finish();
    let patch_body = requests[4].body_json();
    assert_eq!(
        patch_body["tags"],
        json!([{"tag": "keep-me"}]),
        "removing one tag must not remove unrelated tags"
    );

    assert_eq!(code, 0, "payload: {payload}");
}

#[test]
fn item_move_to_collection_from_removes_only_the_named_source() {
    let dir = TestDir::new("array-move-from");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        local_api_resolve_item(),
        local_api_resolve_collection("COLLE001", "Test Collection"),
        item_get_response(5, vec!["EXISTC1", "EXISTC2"]),
        local_api_resolve_collection("EXISTC1", "Existing Collection One"),
        ScriptedResponse::Http {
            status: 204,
            headers: vec![("Last-Modified-Version".to_string(), "6".to_string())],
            body: Vec::new(),
        },
        item_get_response(6, vec!["EXISTC2", "COLLE001"]),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_LOCAL_API_KEY", "env-supplied-key")],
        &[
            "item",
            "move-to-collection",
            "ITEM0001",
            "COLLE001",
            "--from",
            "EXISTC1",
        ],
    );

    let requests = server.finish();
    let patch_body = requests[6].body_json();
    assert_eq!(
        patch_body["collections"],
        json!(["EXISTC2", "COLLE001"]),
        "must remove only the named --from source, matching the Python contract, and must \
         preserve EXISTC2"
    );

    assert_eq!(code, 0, "payload: {payload}");
}

#[test]
fn item_move_to_collection_all_other_collections_leaves_only_the_target() {
    let dir = TestDir::new("array-move-all-other");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        local_api_resolve_item(),
        local_api_resolve_collection("COLLE001", "Test Collection"),
        item_get_response(5, vec!["EXISTC1", "EXISTC2"]),
        ScriptedResponse::Http {
            status: 204,
            headers: vec![("Last-Modified-Version".to_string(), "6".to_string())],
            body: Vec::new(),
        },
        item_get_response(6, vec!["COLLE001"]),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_LOCAL_API_KEY", "env-supplied-key")],
        &[
            "item",
            "move-to-collection",
            "ITEM0001",
            "COLLE001",
            "--all-other-collections",
        ],
    );

    let requests = server.finish();
    let patch_body = requests[5].body_json();
    assert_eq!(
        patch_body["collections"],
        json!(["COLLE001"]),
        "--all-other-collections must leave the item in the target collection alone"
    );

    assert_eq!(code, 0, "payload: {payload}");
}

#[test]
fn collection_remove_item_preserves_unrelated_collection_memberships() {
    let dir = TestDir::new("array-collection-remove-item");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        local_api_resolve_collection("COLLE001", "Test Collection"),
        local_api_resolve_item(),
        item_get_response(5, vec!["EXISTC1", "COLLE001"]),
        ScriptedResponse::Http {
            status: 204,
            headers: vec![("Last-Modified-Version".to_string(), "6".to_string())],
            body: Vec::new(),
        },
        item_get_response(6, vec!["EXISTC1"]),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_LOCAL_API_KEY", "env-supplied-key")],
        &["collection", "remove-item", "COLLE001", "ITEM0001"],
    );

    let requests = server.finish();
    let patch_body = requests[5].body_json();
    assert_eq!(
        patch_body["collections"],
        json!(["EXISTC1"]),
        "removing from one collection must not disturb the item's other memberships"
    );

    assert_eq!(code, 0, "payload: {payload}");
}

// ── Review Blocker 2: item merge verified live through the Bridge, never SQLite ──

#[test]
fn item_merge_succeeds_when_survivor_resolves_live_and_merged_away_key_does_not() {
    let dir = TestDir::new("merge-success");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        bridge_resolve_item("ITEM0001", 1),
        bridge_resolve_item("ITEM0002", 2),
        ScriptedResponse::bridge_string(200, "OK: merged 1 items into Test Item One"),
        ScriptedResponse::bridge_json(
            json!({"found": true, "key": "ITEM0001", "libraryID": 1, "data": {"itemType": "document", "title": "Test Item One"}}),
        ),
        ScriptedResponse::bridge_json(json!({"found": false})),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "merge", "ITEM0001", "ITEM0002", "--confirm"],
    );

    let requests = server.finish();
    assert_eq!(
        requests.len(),
        8,
        "ping, probe, ownership-ping, two live target resolutions, merge-eval, \
         survivor-live-read, merged-away-live-read"
    );

    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(payload["outcome"], "applied");
    assert_eq!(payload["key"], "ITEM0001");
}

#[test]
fn item_merge_reports_conflict_when_a_merged_away_key_still_resolves_live() {
    let dir = TestDir::new("merge-conflict");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        bridge_resolve_item("ITEM0001", 1),
        bridge_resolve_item("ITEM0002", 2),
        ScriptedResponse::bridge_string(200, "OK: merged 1 items into Test Item One"),
        ScriptedResponse::bridge_json(
            json!({"found": true, "key": "ITEM0001", "libraryID": 1, "data": {"itemType": "document", "title": "Test Item One"}}),
        ),
        // The Bridge reported success, but a live re-read still finds the merged-away item --
        // this must never be silently reported as `applied`.
        ScriptedResponse::bridge_json(
            json!({"found": true, "key": "ITEM0002", "libraryID": 1, "data": {"itemType": "document", "title": "Test Item Two"}}),
        ),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "merge", "ITEM0001", "ITEM0002", "--confirm"],
    );

    server.finish();

    assert_eq!(code, 1, "payload: {payload}");
    assert_eq!(payload["outcome"], "conflict");
    assert_ne!(payload["outcome"], "applied");
}

// ── `item duplicates` is now implemented in the ANALYSIS / HYGIENE slice ──

#[test]
fn item_duplicates_is_now_a_recognized_subcommand() {
    let output = common::cli_command()
        .args(["item", "duplicates", "--help"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "item duplicates is now a supported command in the analysis-hygiene slice"
    );
}

// ── Trash-by-default delete ─────────────────────────────────────────────────

fn body_text(request: &common::CapturedRequest) -> String {
    String::from_utf8_lossy(&request.body).into_owned()
}

#[test]
fn delete_refusals_never_reach_zotero() {
    let dir = TestDir::new("delete-refusals");
    build_fixture_sqlite(dir.path());
    for args in [
        vec!["item", "delete", "ITEM0001"],
        vec!["item", "delete", "ITEM0001", "--permanent"],
        // `--confirm` never authorizes an erase, not even alongside --permanent.
        vec!["item", "delete", "ITEM0001", "--permanent", "--confirm"],
        vec!["item", "restore", "ITEM0001"],
        vec![
            "collection",
            "delete",
            "COLL0001",
            "--permanent",
            "--confirm",
        ],
    ] {
        let server = ScriptedServer::start(vec![]);
        let (code, payload) = run_cli(dir.path(), server.port, &[], &args);
        let requests = server.finish();
        assert_eq!(code, 1, "{args:?} must be refused: {payload}");
        assert!(
            requests.is_empty(),
            "{args:?}: a refused delete must issue no request at all"
        );
    }
}

#[test]
fn item_delete_confirm_trashes_through_the_local_api_and_never_deletes() {
    let dir = TestDir::new("delete-trash-local");
    build_fixture_sqlite(dir.path());
    let trashed = ScriptedResponse::json(
        200,
        json!({
            "key": "ITEM0001",
            "version": 6,
            "library": {"id": 0},
            "data": {"itemType": "document", "title": "Test Item One", "deleted": 1},
        }),
    );
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        local_api_resolve_item(),
        item_get_response(5, vec![]),
        ScriptedResponse::Http {
            status: 204,
            headers: vec![("Last-Modified-Version".to_string(), "6".to_string())],
            body: Vec::new(),
        },
        trashed,
    ]);
    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_LOCAL_API_KEY", "env-supplied-key")],
        &["item", "delete", "ITEM0001", "--confirm"],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(payload["action"], "item_trash");
    assert_eq!(payload["recoverable"], true);
    assert_eq!(requests[4].method, "PATCH");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body_text(&requests[4])).unwrap(),
        json!({"deleted": 1})
    );
    assert!(
        requests.iter().all(|r| r.method != "DELETE"),
        "--confirm must never send a DELETE (Zotero's DELETE erases permanently)"
    );
}

#[test]
fn item_delete_and_restore_through_the_bridge_flip_the_deleted_flag() {
    for (args, readback_deleted, action) in [
        (
            vec!["item", "delete", "ITEM0001", "--confirm"],
            json!(true),
            "item_trash",
        ),
        (
            vec!["item", "restore", "ITEM0001", "--confirm"],
            json!(null),
            "item_restore",
        ),
    ] {
        let dir = TestDir::new("delete-trash-bridge");
        build_fixture_sqlite(dir.path());
        let mut data = json!({"itemType": "document", "title": "Test Item One"});
        if !readback_deleted.is_null() {
            data["deleted"] = readback_deleted;
        }
        let server = ScriptedServer::start(vec![
            connector_ping_ok(),
            local_api_probe_unavailable(),
            bridge_ownership_ok(),
            bridge_resolve_item("ITEM0001", 1),
            ScriptedResponse::bridge_string(200, "OK: done ITEM0001"),
            ScriptedResponse::bridge_json(
                json!({"found": true, "key": "ITEM0001", "libraryID": 1, "data": data}),
            ),
        ]);
        let (code, payload) = run_cli(dir.path(), server.port, &[], &args);
        let requests = server.finish();

        assert_eq!(code, 0, "{args:?}: {payload}");
        assert_eq!(payload["action"], action);
        let write = body_text(&requests[4]);
        assert!(write.contains("item.deleted = "), "{write}");
        assert!(!write.contains("eraseTx"), "trash/restore must never erase");
    }
}

#[test]
fn trash_that_does_not_stick_is_reported_as_a_conflict() {
    let dir = TestDir::new("delete-trash-conflict");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        bridge_resolve_item("ITEM0001", 1),
        ScriptedResponse::bridge_string(200, "OK: trashed ITEM0001"),
        ScriptedResponse::bridge_json(
            json!({"found": true, "key": "ITEM0001", "libraryID": 1, "data": {"itemType": "document"}}),
        ),
    ]);
    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "delete", "ITEM0001", "--confirm"],
    );
    server.finish();
    assert_eq!(code, 1, "{payload}");
    assert_eq!(payload["outcome"], "conflict");
}

#[test]
fn permanent_yes_erase_is_the_only_path_to_an_erase() {
    let dir = TestDir::new("delete-erase-bridge");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        bridge_resolve_item("ITEM0001", 1),
        ScriptedResponse::bridge_string(200, "DELETED: Test Item One"),
        ScriptedResponse::bridge_json(json!({"found": false})),
    ]);
    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "delete", "ITEM0001", "--permanent", "--yes-erase"],
    );
    let requests = server.finish();
    assert_eq!(code, 0, "{payload}");
    assert_eq!(payload["action"], "item_erase");
    assert_eq!(payload["recoverable"], false);
    assert!(body_text(&requests[4]).contains("eraseTx"));
}

fn collection_readback(deleted: Option<bool>) -> ScriptedResponse {
    let mut data = json!({"key": "COLL0001", "name": "Test Collection"});
    if let Some(deleted) = deleted {
        data["deleted"] = json!(deleted);
    }
    ScriptedResponse::bridge_json(
        json!({"found": true, "key": "COLL0001", "libraryID": 1, "data": data}),
    )
}

#[test]
fn collection_trash_and_restore_through_the_bridge_match_zotero_semantics() {
    for (args, readback, action) in [
        (
            vec!["collection", "delete", "COLL0001", "--confirm"],
            Some(true),
            "collection_trash",
        ),
        (
            vec![
                "collection",
                "delete",
                "COLL0001",
                "--delete-items",
                "--confirm",
            ],
            Some(true),
            "collection_trash",
        ),
        (
            vec!["collection", "restore", "COLL0001", "--confirm"],
            None,
            "collection_restore",
        ),
    ] {
        let dir = TestDir::new("collection-trash-bridge");
        build_fixture_sqlite(dir.path());
        let server = ScriptedServer::start(vec![
            connector_ping_ok(),
            local_api_probe_unavailable(),
            bridge_ownership_ok(),
            bridge_resolve_collection("COLL0001", "Test Collection", 1),
            ScriptedResponse::bridge_string(200, "OK: done COLL0001"),
            // The real Bridge returns the readback template's JSON.stringify() as a string.
            collection_readback(readback),
        ]);
        let (code, payload) = run_cli(dir.path(), server.port, &[], &args);
        let requests = server.finish();

        assert_eq!(code, 0, "{args:?}: {payload}");
        assert_eq!(payload["action"], action, "{args:?}");
        let write = requests
            .iter()
            .map(body_text)
            .find(|body| {
                body.contains("Zotero.Collections.getByLibraryAndKey(P.libraryID, P.collectionKey)")
            })
            .expect("the trash/restore template was sent");
        assert!(!write.contains("eraseTx"), "trash/restore must never erase");
        // Zotero's own trash path cascades to subcollections and, with deleteItems, to every
        // item in the subtree; restore brings back subcollections but never items.
        assert!(write.contains("await col.save({ deleteItems: !!P.includeItems })"));
        assert!(write.contains("getDescendents(false, 'collection', true)"));
        let params = write.contains(r#"\"includeItems\":true"#);
        assert_eq!(
            params,
            args.contains(&"--delete-items"),
            "{args:?}: {write}"
        );
    }
}

#[test]
fn collection_restore_no_longer_accepts_with_items() {
    let dir = TestDir::new("collection-restore-with-items");
    build_fixture_sqlite(dir.path());
    let output = common::cli_command()
        .args([
            "collection",
            "restore",
            "COLL0001",
            "--with-items",
            "--confirm",
        ])
        .env("ZOTERO_CLI_NO_AUTOLAUNCH", "1")
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "clap must reject the removed flag"
    );
}

#[test]
fn permanent_collection_erase_with_delete_items_erases_the_items_too() {
    let dir = TestDir::new("collection-erase-items");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        bridge_resolve_collection("COLL0001", "Test Collection", 1),
        ScriptedResponse::bridge_string(200, "DELETED: collection Test Collection"),
        ScriptedResponse::bridge_json(json!({"found": false})),
    ]);
    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "collection",
            "delete",
            "COLL0001",
            "--delete-items",
            "--permanent",
            "--yes-erase",
        ],
    );
    let requests = server.finish();
    assert_eq!(code, 0, "{payload}");
    assert_eq!(payload["action"], "collection_erase");
    let erase = requests
        .iter()
        .map(body_text)
        .find(|body| body.contains("eraseTx"))
        .expect("the erase template was sent");
    // Zotero's erase only trashes the items; the CLI promises to erase them.
    assert!(erase.contains("await col.eraseTx({ deleteItems: !!P.deleteItems })"));
    assert!(erase.contains("await Zotero.Items.erase(itemIDs)"));
    assert!(erase.contains(r#"\"deleteItems\":true"#), "{erase}");
}
