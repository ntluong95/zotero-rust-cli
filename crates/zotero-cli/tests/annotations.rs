//! Annotation search/retrieval core tests (Phase 7 Slice 5): `core/jsbridge.py::search_annotations`
//! / `get_annotations`, ported to `src/annotations.rs`. Not yet a real crate module (no
//! CLI-dispatch slice has registered it in `lib.rs`), so it's `#[path]`-included here exactly
//! like Phase 7 Slice 3/4's own test files already do for `pdf_fetch.rs`/`notes.rs`.

#![allow(dead_code)]

#[path = "../src/annotations.rs"]
mod annotations;

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

// ── search_annotations: request shape ──

#[test]
fn search_annotations_hardcodes_library_id_1() {
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(200, json!([])),
    ]);
    let client = JSBridgeClient::new(server.port);

    annotations::search_annotations(&client, "", None, 20);

    let requests = server.finish();
    let body = String::from_utf8(requests[1].body.clone()).unwrap();
    assert!(body.contains(r#"\"libraryID\":1"#), "body was: {body}");
}

#[test]
fn empty_query_and_colors_are_forwarded_as_empty_string_and_null() {
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(200, json!([])),
    ]);
    let client = JSBridgeClient::new(server.port);

    annotations::search_annotations(&client, "", None, 20);

    let requests = server.finish();
    let body = String::from_utf8(requests[1].body.clone()).unwrap();
    assert!(body.contains(r#"\"query\":\"\""#), "body was: {body}");
    assert!(body.contains(r#"\"colors\":null"#), "body was: {body}");
}

#[test]
fn nonempty_query_and_colors_are_forwarded_verbatim() {
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(200, json!([])),
    ]);
    let client = JSBridgeClient::new(server.port);

    let colors = vec!["yellow".to_string(), "#ffd400".to_string()];
    annotations::search_annotations(&client, "NAFLD", Some(&colors), 5);

    let requests = server.finish();
    let body = String::from_utf8(requests[1].body.clone()).unwrap();
    assert!(body.contains(r#"\"query\":\"NAFLD\""#), "body was: {body}");
    assert!(body.contains("yellow"));
    assert!(body.contains("#ffd400"));
    assert!(body.contains(r#"\"limit\":5"#));
}

// ── search_annotations: template preserves Python's exact query/color/limit semantics ──

#[test]
fn template_uses_annotation_text_contains_not_annotation_comment() {
    let template = zotero_cli::bridge::templates::T_SEARCH_ANNOTATIONS;
    assert!(template.contains("annotationText"));
    assert!(
        !template.contains("annotationComment', 'contains'")
            && !template.contains("addCondition('annotationComment'"),
        "annotationComment must never be searched -- only read back on results"
    );
}

#[test]
fn template_falls_back_to_itemtype_annotation_condition_for_empty_query() {
    let template = zotero_cli::bridge::templates::T_SEARCH_ANNOTATIONS;
    assert!(template.contains("itemType', 'is', 'annotation'"));
}

#[test]
fn template_applies_color_filter_before_the_limit_slice() {
    let template = zotero_cli::bridge::templates::T_SEARCH_ANNOTATIONS;
    let filter_pos = template
        .find("P.colors.includes")
        .expect("color filter must be present");
    let slice_pos = template
        .find(".slice(0, P.limit)")
        .expect("limit slice must be present");
    assert!(
        filter_pos < slice_pos,
        "color filtering must happen before the limit slice, matching Python's \
         `filtered.slice(0, limit)` ordering"
    );
}

#[test]
fn template_never_queries_fts_sqlite() {
    let template = zotero_cli::bridge::templates::T_SEARCH_ANNOTATIONS;
    assert!(template.contains("Zotero.Search"));
    assert!(!template.to_lowercase().contains("select "));
}

// ── get_annotations: request shape ──

#[test]
fn get_annotations_hardcodes_library_id_1() {
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(200, json!({"count": 0, "annotations": []})),
    ]);
    let client = JSBridgeClient::new(server.port);

    annotations::get_annotations(&client, "EMI3S3GJ");

    let requests = server.finish();
    let body = String::from_utf8(requests[1].body.clone()).unwrap();
    assert!(body.contains(r#"\"libraryID\":1"#), "body was: {body}");
    assert!(body.contains(r#"\"key\":\"EMI3S3GJ\""#), "body was: {body}");
}

#[test]
fn get_annotations_forwards_raw_key_unconditionally_attachment_or_parent() {
    // The core function has no notion of "attachment vs. bibliographic parent" -- it forwards
    // whatever key it's given as-is. The attachment->parent walk happens entirely inside the JS
    // template at Zotero runtime.
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(200, json!({"count": 0, "annotations": []})),
    ]);
    let client = JSBridgeClient::new(server.port);

    annotations::get_annotations(&client, "RZ694UHL");

    let requests = server.finish();
    let body = String::from_utf8(requests[1].body.clone()).unwrap();
    assert!(body.contains(r#"\"key\":\"RZ694UHL\""#), "body was: {body}");
}

// ── get_annotations: success payload pass-through ──

#[test]
fn get_annotations_success_payload_is_passed_through_unmodified() {
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({"count": 1, "annotations": [{"type": "highlight", "text": "some text", "comment": "", "color": "#ffd400", "page": "3"}]}),
        ),
    ]);
    let client = JSBridgeClient::new(server.port);

    let (payload, is_success) = annotations::get_annotations(&client, "RZ694UHL");

    server.finish();
    assert!(is_success);
    assert_eq!(payload["count"], 1);
    assert_eq!(payload["annotations"][0]["type"], "highlight");
}

// ── get_annotations: the "ERROR: ..." string is still a *successful* Bridge payload ──

#[test]
fn not_found_error_string_is_success_exit_code_matching_python_emit_js() {
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::bridge_string(200, "ERROR: item MISSING01 not found"),
    ]);
    let client = JSBridgeClient::new(server.port);

    let (payload, is_success) = annotations::get_annotations(&client, "MISSING01");

    server.finish();
    // This looks wrong but is deliberate: `zotero_cli.py::emit_js` treats any transport-level
    // success whose `data` is a bare string -- even one starting with "ERROR:" -- as a successful
    // exit (0). Only a transport failure, or a `data` object explicitly carrying `"ok": false`,
    // is an error exit.
    assert!(is_success);
    assert_eq!(payload, json!("ERROR: item MISSING01 not found"));
}

#[test]
fn attachment_with_no_resolvable_parent_error_string_is_also_success() {
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::bridge_string(200, "ERROR: attachment has no parent item"),
    ]);
    let client = JSBridgeClient::new(server.port);

    let (payload, is_success) = annotations::get_annotations(&client, "ORPHANATT");

    server.finish();
    assert!(is_success);
    assert_eq!(payload, json!("ERROR: attachment has no parent item"));
}

// ── get_annotations: template preserves per-PDF error swallowing and parent walk ──

#[test]
fn template_walks_attachment_to_bibliographic_parent() {
    let template = zotero_cli::bridge::templates::T_GET_ANNOTATIONS;
    assert!(template.contains("isAttachment"));
    assert!(template.contains("parentItemID"));
}

#[test]
fn template_swallows_per_attachment_get_annotations_errors_individually() {
    let template = zotero_cli::bridge::templates::T_GET_ANNOTATIONS;
    assert!(
        template.contains("try {") && template.contains("catch (e) {}"),
        "one bad PDF's getAnnotations() must not fail the whole call"
    );
}

// ── transport/application failure classification (shared with fulltext) ──

#[test]
fn search_annotations_transport_failure_is_not_success() {
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::Http {
            status: 500,
            headers: Vec::new(),
            body: br#"{"error":"boom"}"#.to_vec(),
        },
    ]);
    let client = JSBridgeClient::new(server.port);

    let (payload, is_success) = annotations::search_annotations(&client, "q", None, 20);

    server.finish();
    assert!(!is_success);
    assert_eq!(payload["ok"], false);
}

#[test]
fn get_annotations_transport_failure_is_not_success() {
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::Http {
            status: 500,
            headers: Vec::new(),
            body: br#"{"error":"boom"}"#.to_vec(),
        },
    ]);
    let client = JSBridgeClient::new(server.port);

    let (payload, is_success) = annotations::get_annotations(&client, "ITEM0001");

    server.finish();
    assert!(!is_success);
    assert_eq!(payload["ok"], false);
}
