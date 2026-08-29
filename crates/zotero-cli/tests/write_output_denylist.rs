//! Standing backend-identity denylist test (§3.5/Testing Strategy): every write command Phase 6
//! implements must never leak `backend`/`server_id` into stdout JSON, and a Local-API-routed
//! write's compatibility renderer must never leak the raw Local API `version` field either
//! (`write_router::LocalApiItemSummary.version` is internal `If-Unmodified-Since-Version`
//! plumbing, not domain data -- unlike a SQLite-backed `Item`/`Collection`'s own legitimate
//! `version` field, which this test does not flag for Bridge-routed commands since that value
//! already appears in every existing read command's output).

#[path = "common/mod.rs"]
mod common;

use common::{
    assert_no_forbidden_keys, build_fixture_sqlite, run_cli, ScriptedResponse, ScriptedServer,
    TestDir,
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

fn bridge_ownership_ok() -> ScriptedResponse {
    ScriptedResponse::json(
        200,
        json!({"fork": "zotero-rust-cli", "id": "cli-bridge@cli-anything-rust.dev"}),
    )
}

#[test]
fn local_api_item_update_output_never_leaks_backend_identity_or_raw_local_api_version() {
    let dir = TestDir::new("denylist-item-update-local-api");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        ScriptedResponse::json(
            200,
            json!({
                "key": "ITEM0001", "version": 5, "library": {"id": 0},
                "data": {"itemType": "document", "title": "Test Item One", "collections": [], "tags": []},
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
    server.finish();

    assert_eq!(code, 0, "payload: {payload}");
    assert_no_forbidden_keys(&payload, &["backend", "server_id", "version"], "$");
}

#[test]
fn local_api_collection_create_output_never_leaks_backend_identity_or_raw_local_api_version() {
    let dir = TestDir::new("denylist-collection-create-local-api");
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
                "key": "NEWCOL01", "version": 1, "library": {"id": 0},
                "data": {"name": "Denylist Collection"},
            }),
        ),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_LOCAL_API_KEY", "env-supplied-key")],
        &["collection", "create", "Denylist Collection"],
    );
    server.finish();

    assert_eq!(code, 0, "payload: {payload}");
    assert_no_forbidden_keys(&payload, &["backend", "server_id", "version"], "$");
}

/// Bridge-routed output is a plain SQLite-backed `Item`/`Collection`, which legitimately carries
/// its own domain `version` field (present in every existing read command's output already) --
/// only `backend`/`server_id` are checked here, deliberately not `version`.
#[test]
fn bridge_item_update_output_never_leaks_backend_identity() {
    let dir = TestDir::new("denylist-item-update-bridge");
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
    server.finish();

    assert_eq!(code, 0, "payload: {payload}");
    assert_no_forbidden_keys(&payload, &["backend", "server_id"], "$");
}

#[test]
fn sync_output_never_leaks_backend_identity() {
    let dir = TestDir::new("denylist-sync");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::bridge_string(200, "Sync completed"),
    ]);

    let (code, payload) = run_cli(dir.path(), server.port, &[], &["sync"]);
    server.finish();

    assert_eq!(code, 0, "payload: {payload}");
    assert_no_forbidden_keys(&payload, &["backend", "server_id"], "$");
}
