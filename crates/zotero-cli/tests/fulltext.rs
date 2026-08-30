//! Full-text search core tests (Phase 7 Slice 5): `core/jsbridge.py::search_fulltext` /
//! `zotero_cli.py::item_search_fulltext_command`, ported to `src/fulltext.rs`. Not yet a real
//! crate module (no CLI-dispatch slice has registered it in `lib.rs`), so it's `#[path]`-included
//! here exactly like Phase 7 Slice 3/4's own test files already do for `pdf_fetch.rs`/`notes.rs`.

#![allow(dead_code)]

#[path = "../src/fulltext.rs"]
mod fulltext;

pub mod bridge {
    pub use zotero_cli::bridge::*;
}

#[path = "common/mod.rs"]
mod common;

use bridge::JSBridgeClient;
use common::{ScriptedResponse, ScriptedServer};
use serde_json::json;

fn bridge_ownership_ok() -> ScriptedResponse {
    ScriptedResponse::json(
        200,
        json!({"fork": "zotero-rust-cli", "id": "cli-bridge@cli-anything-rust.dev"}),
    )
}

// ── happy path: exact attachment-level pass-through (frozen live evidence) ──

#[test]
fn search_fulltext_returns_the_bridge_array_unmodified() {
    // This is the exact frozen live-evidence projection for a `fulltextContent` match: Zotero
    // returns the PDF ATTACHMENT item, not its bibliographic parent. Rust must not resolve to a
    // parent, substitute parent metadata, or add snippets/scores -- it passes this through as-is.
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!([{"key": "RZ694UHL", "title": "PDF", "date": ""}]),
        ),
    ]);
    let client = JSBridgeClient::new(server.port);

    let (payload, is_success) = fulltext::search_fulltext(&client, "marker text", 10);

    let requests = server.finish();
    assert_eq!(requests.len(), 2, "ownership probe + eval");
    assert!(is_success);
    assert_eq!(
        payload,
        json!([{"key": "RZ694UHL", "title": "PDF", "date": ""}])
    );
}

#[test]
fn search_fulltext_empty_results_are_a_bare_empty_array_not_an_error() {
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(200, json!([])),
    ]);
    let client = JSBridgeClient::new(server.port);

    let (payload, is_success) = fulltext::search_fulltext(&client, "no such marker", 10);

    server.finish();
    assert!(is_success);
    assert_eq!(payload, json!([]));
}

// ── request shape: hardcoded libraryID, unclamped limit ──

#[test]
fn request_hardcodes_library_id_1_regardless_of_caller() {
    // Python's CLI layer never passes `--library` for this command -- `library_id` is hardcoded
    // to 1 inside `fulltext::search_fulltext`, not something this port's callers can override.
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(200, json!([])),
    ]);
    let client = JSBridgeClient::new(server.port);

    fulltext::search_fulltext(&client, "q", 10);

    let requests = server.finish();
    let body = String::from_utf8(requests[1].body.clone()).unwrap();
    assert!(body.contains(r#"\"libraryID\":1"#), "body was: {body}");
}

#[test]
fn zero_limit_is_sent_unclamped_for_zotero_js_slice_semantics() {
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(200, json!([])),
    ]);
    let client = JSBridgeClient::new(server.port);

    fulltext::search_fulltext(&client, "q", 0);

    let requests = server.finish();
    let body = String::from_utf8(requests[1].body.clone()).unwrap();
    // `items.slice(0, P.limit)` lives in the JS template itself; Rust must forward `limit`
    // unvalidated so Zotero's own JS engine -- not this port -- decides what `0` means.
    assert!(body.contains(r#"\"limit\":0"#), "body was: {body}");
}

#[test]
fn negative_limit_is_sent_unclamped_for_zotero_js_slice_semantics() {
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(200, json!([])),
    ]);
    let client = JSBridgeClient::new(server.port);

    fulltext::search_fulltext(&client, "q", -1);

    let requests = server.finish();
    let body = String::from_utf8(requests[1].body.clone()).unwrap();
    // `Array.slice(0, -1)` is a valid, meaningful JS expression (all but the last element) --
    // do not reject or clamp a negative limit at this layer.
    assert!(body.contains(r#"\"limit\":-1"#), "body was: {body}");
}

#[test]
fn query_is_bound_via_json_safe_parameter_not_string_interpolation() {
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(200, json!([])),
    ]);
    let client = JSBridgeClient::new(server.port);

    // A query containing a single quote and backslash would corrupt a naively-interpolated JS
    // string literal; the JSON.parse binding mechanism must escape it safely instead.
    fulltext::search_fulltext(&client, "O'Brien\\test", 10);

    let requests = server.finish();
    let body = String::from_utf8(requests[1].body.clone()).unwrap();
    assert!(body.starts_with("const P = JSON.parse("));
    assert!(body.contains("fulltextContent"));
    assert!(!body.contains("addCondition('fulltextContent', 'contains', 'O'Brien"));
}

// ── never queries SQLite FTS tables, never polls for indexing ──

#[test]
fn template_uses_live_zotero_search_api_not_fts_sqlite() {
    let template = zotero_cli::bridge::templates::T_SEARCH_FULLTEXT;
    assert!(template.contains("Zotero.Search"));
    assert!(template.contains("fulltextContent"));
    assert!(!template.to_lowercase().contains("fulltext.sqlite"));
    assert!(!template.to_lowercase().contains("select "));
}

#[test]
fn no_index_state_polling_or_retry_in_template() {
    let template = zotero_cli::bridge::templates::T_SEARCH_FULLTEXT;
    assert!(!template.contains("getIndexedState"));
    assert!(!template.contains("setTimeout"));
    assert!(!template.contains("while ("));
}

// ── transport/application failure classification ──

#[test]
fn transport_failure_is_not_success() {
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::Http {
            status: 500,
            headers: Vec::new(),
            body: br#"{"error":"boom"}"#.to_vec(),
        },
    ]);
    let client = JSBridgeClient::new(server.port);

    let (payload, is_success) = fulltext::search_fulltext(&client, "q", 10);

    server.finish();
    assert!(!is_success);
    assert_eq!(payload["ok"], false);
}

#[test]
fn nested_application_level_failure_object_is_not_success() {
    // `classify_bridge_payload`'s shared logic (exercised here via `fulltext::search_fulltext`,
    // but identical for `annotations::*`): a transport-level success whose `data` is an object
    // carrying `"ok": false` is still an application-level failure, matching `emit_js`.
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(200, json!({"ok": false, "error": "collection not found"})),
    ]);
    let client = JSBridgeClient::new(server.port);

    let (payload, is_success) = fulltext::search_fulltext(&client, "q", 10);

    server.finish();
    assert!(!is_success);
    assert_eq!(payload["error"], "collection not found");
}
