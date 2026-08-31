//! `item merge`'s dry-run preview parity restoration (Phase 10 blocker fix).
//!
//! Pinned Python authority: `PiaoyangGuohai1/cli-anything-zotero@e42a930e`,
//! `zotero_cli.py:1501-1520` + `core/hygiene.py:preview_merge`/`_preview_from_summaries`/
//! `merge_items`. Exact contract:
//!
//! - `--dry-run/--confirm` is a single Click boolean flag pair, `default=True` (dry-run). Bare
//!   invocation previews; `--confirm` is required to mutate. If both flags are given, the last
//!   one on the command line wins (`resolve_bool_flag` + clap's `overrides_with`).
//! - Self-refs (`merge_key == keep_key`, plain string equality) are silently dropped before
//!   dispatch; if that empties the merge-key list, `INVALID_ARGS` (`ok:false`, exit 1) --
//!   *without* the `plan`/`dry_run` envelope fields Python only adds once dispatch proceeds.
//! - A successful preview never mutates: it reports `ok:true, status:"dry_run", code:"DRY_RUN"`,
//!   exit 0, and lists any unresolved merge-away keys in `missing` without erroring.
//! - An unresolved *keep* key is the one preview-time error: `KEEP_NOT_FOUND`, exit 1.
//! - `--confirm` is unaffected: it dispatches to the existing, accepted `Zotero.Items.merge()`
//!   JS Bridge mutation path in `item_merge_command`, unchanged by this fix.
//!
//! - Bridge-first, SQLite-fallback: a read-only JS Bridge preview is attempted first
//!   (`preview_source: "bridge"`); on any failure (unreachable, ownership handshake fails, eval
//!   reports `ok:false` -- including a bridge-reported "keep item not found", which Python does
//!   *not* treat as terminal) it falls back to the SQLite-only preview (`preview_source:
//!   "sqlite"`), carrying the captured `bridge_error` forward exactly as Python does.

#[path = "common/mod.rs"]
mod common;

use common::{run_cli, ScriptedResponse, ScriptedServer, TestDir};
use serde_json::json;
use std::process::Command;

fn connector_ping_ok() -> ScriptedResponse {
    ScriptedResponse::json(200, json!({}))
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

/// A live target-resolution response, as `target::resolve_item` expects it: the resolution
/// template returns a JSON *string*, so the transport body is a quoted string that the resolver
/// parses once more. `--confirm` resolves its targets through the live Zotero runtime rather
/// than SQLite, so each resolved key costs one of these.
fn bridge_resolve_item_ok(key: &str, item_id: i64) -> ScriptedResponse {
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

/// Two items ("keep" + "other A") share a tag and a collection with a second "other B" item, so
/// dedup-on-accumulate (not naive set-union) is actually exercised. Schema matches
/// `common::build_fixture_sqlite`; see inline comments for what each row is used to prove.
fn build_merge_preview_fixture(dir: &std::path::Path) -> std::path::PathBuf {
    let sqlite_path = dir.join("zotero.sqlite");
    let conn = rusqlite::Connection::open(&sqlite_path).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE libraries (libraryID INTEGER PRIMARY KEY, type TEXT, editable INTEGER, filesEditable INTEGER, version INTEGER, storageVersion INTEGER, lastSync INTEGER, archived INTEGER);
        INSERT INTO libraries VALUES (1, 'user', 1, 1, 1, 1, 0, 0);
        CREATE TABLE itemTypes (itemTypeID INTEGER PRIMARY KEY, typeName TEXT, templateItemTypeID INTEGER, display INTEGER);
        INSERT INTO itemTypes VALUES (1, 'document', NULL, 1), (2, 'attachment', NULL, 1), (3, 'note', NULL, 1);
        CREATE TABLE items (itemID INTEGER PRIMARY KEY, itemTypeID INTEGER, dateAdded TEXT, dateModified TEXT, clientDateModified TEXT, libraryID INTEGER, key TEXT, version INTEGER, synced INTEGER);
        INSERT INTO items VALUES (1, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'KEEP0001', 1, 1);
        INSERT INTO items VALUES (2, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'OTHR0001', 1, 1);
        INSERT INTO items VALUES (3, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'OTHR0002', 1, 1);
        INSERT INTO items VALUES (101, 2, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'ATCH0001', 1, 1);
        INSERT INTO items VALUES (102, 3, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'NOTE0001', 1, 1);
        CREATE TABLE fields (fieldID INTEGER PRIMARY KEY, fieldName TEXT, fieldFormatID INTEGER);
        INSERT INTO fields VALUES (1, 'title', 0), (2, 'DOI', 0), (3, 'date', 0);
        CREATE TABLE itemDataValues (valueID INTEGER PRIMARY KEY, value TEXT);
        INSERT INTO itemDataValues VALUES (1, 'Keep Item'), (2, 'Other Item A'), (3, '10.1000/a'), (4, '2024-01-01'), (5, 'Other Item B');
        CREATE TABLE itemData (itemID INTEGER, fieldID INTEGER, valueID INTEGER);
        INSERT INTO itemData VALUES (1, 1, 1), (2, 1, 2), (2, 2, 3), (2, 3, 4), (3, 1, 5);
        CREATE TABLE creators (creatorID INTEGER PRIMARY KEY, firstName TEXT, lastName TEXT, fieldMode INTEGER);
        CREATE TABLE itemCreators (itemID INTEGER, creatorID INTEGER, creatorTypeID INTEGER, orderIndex INTEGER);
        CREATE TABLE tags (tagID INTEGER PRIMARY KEY, name TEXT);
        INSERT INTO tags VALUES (1, 'keep-tag'), (2, 'shared-tag'), (3, 'only-a-tag');
        CREATE TABLE itemTags (itemID INTEGER, tagID INTEGER, type INTEGER);
        -- keep has keep-tag; A has shared-tag + only-a-tag; B has shared-tag (same tag row as A).
        INSERT INTO itemTags VALUES (1, 1, 0), (2, 2, 0), (2, 3, 0), (3, 2, 0);
        CREATE TABLE collections (collectionID INTEGER PRIMARY KEY, collectionName TEXT, parentCollectionID INTEGER, clientDateModified TEXT, libraryID INTEGER, key TEXT, version INTEGER, synced INTEGER);
        INSERT INTO collections VALUES (1, 'Test Collection', NULL, '2026-01-01', 1, 'COLLE001', 1, 1);
        INSERT INTO collections VALUES (2, 'Collection C2', NULL, '2026-01-01', 1, 'COLLC002', 1, 1);
        INSERT INTO collections VALUES (3, 'Collection C3', NULL, '2026-01-01', 1, 'COLLC003', 1, 1);
        CREATE TABLE collectionItems (collectionID INTEGER, itemID INTEGER, orderIndex INTEGER);
        -- keep in Test Collection; A and B share Collection C2; B is also in Collection C3.
        INSERT INTO collectionItems VALUES (1, 1, 0), (2, 2, 0), (2, 3, 0), (3, 3, 0);
        CREATE TABLE itemNotes (itemID INTEGER PRIMARY KEY, parentItemID INTEGER, note TEXT, title TEXT);
        INSERT INTO itemNotes VALUES (102, 2, '<p>Note A</p>', 'Note A');
        CREATE TABLE itemAttachments (itemID INTEGER PRIMARY KEY, parentItemID INTEGER, linkMode INTEGER, contentType TEXT, charsetID INTEGER, path TEXT, syncState INTEGER, storageModTime INTEGER, storageHash TEXT, lastProcessedModificationTime INTEGER);
        INSERT INTO itemAttachments (itemID, parentItemID, linkMode, contentType, path) VALUES (101, 2, 0, 'application/pdf', 'storage:a.pdf');
        CREATE TABLE itemAnnotations (itemID INTEGER PRIMARY KEY, parentItemID INTEGER, type INTEGER, authorName TEXT, text TEXT, comment TEXT, color TEXT, pageLabel TEXT, sortIndex TEXT, position TEXT, isExternal INTEGER);
        CREATE TABLE savedSearches (savedSearchID INTEGER PRIMARY KEY, savedSearchName TEXT, clientDateModified TEXT, libraryID INTEGER, key TEXT, version INTEGER, synced INTEGER);
        CREATE TABLE savedSearchConditions (savedSearchID INTEGER, searchConditionID INTEGER, condition TEXT, operator TEXT, value TEXT, required INTEGER);
        "#,
    )
    .unwrap();
    sqlite_path
}

// ── Bare invocation / explicit --dry-run: identical, zero-mutation preview.
//    No bridge_ownership_ok() is scripted, so the mock server's accept loop exits (and its
//    listening socket closes) after exactly 2 connections; the bridge-first preview attempt's
//    ownership probe then fails fast with connection-refused -- this *is* the "bridge
//    unavailable" fallback path (scenario B), proven structurally: zero additional connections
//    are ever accepted, and the preview still succeeds via SQLite. ──

#[test]
fn bare_invocation_previews_by_default_and_makes_zero_mutation_calls() {
    let dir = TestDir::new("merge-preview-bare");
    build_merge_preview_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "merge", "KEEP0001", "OTHR0001"],
    );

    let requests = server.finish();
    assert_eq!(
        requests.len(),
        2,
        "connector-ping + local-api-probe only -- the bridge preview's ownership probe fails \
         fast (connection refused, bridge unavailable) and is never accepted; zero write calls \
         reach the server: {requests:?}"
    );

    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(payload["action"], "item_merge");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["status"], "dry_run");
    assert_eq!(payload["code"], "DRY_RUN");
    assert_eq!(payload["dry_run"], true);
    assert_eq!(payload["preview_source"], "sqlite");
    assert_eq!(
        payload["bridge_error"],
        "JS Bridge endpoint not available. Install the CLI Bridge plugin: \
         zotero-cli app install-plugin, then restart Zotero.",
        "the deterministic 'bridge unavailable' message, carried forward onto the successful \
         sqlite-fallback payload since it's truthy (hygiene.py:386-387)"
    );
    assert_eq!(payload["missing"], json!([]));
    assert_eq!(
        payload["plan"],
        json!({"keep": "KEEP0001", "merge": ["OTHR0001"], "dry_run": true})
    );

    assert_eq!(payload["keep"]["key"], "KEEP0001");
    assert_eq!(payload["keep"]["title"], "Keep Item");
    assert_eq!(payload["keep"]["tags"], json!(["keep-tag"]));
    assert_eq!(payload["keep"]["nCollections"], 1);

    assert_eq!(payload["others"][0]["key"], "OTHR0001");
    assert_eq!(payload["others"][0]["DOI"], "10.1000/a");
    assert_eq!(payload["others"][0]["date"], "2024-01-01");
    assert_eq!(payload["others"][0]["nAttachments"], 1);
    assert_eq!(payload["others"][0]["nNotes"], 1);

    assert_eq!(payload["will"]["move_attachments"], 1);
    assert_eq!(payload["will"]["move_notes"], 1);
    assert_eq!(
        payload["will"]["add_tags"],
        json!(["only-a-tag", "shared-tag"])
    );
    assert_eq!(payload["will"]["trash_items"], json!(["OTHR0001"]));
    assert_eq!(
        payload["message"],
        "Would trash 1 item(s) into keep=KEEP0001: move 1 attachment(s), 1 note(s); \
         add 2 tag(s), 1 collection(s). (preview via sqlite) Re-run with --confirm to apply."
    );
}

#[test]
fn explicit_dry_run_flag_produces_the_same_preview_as_bare_invocation() {
    let dir = TestDir::new("merge-preview-explicit-dry-run");
    build_merge_preview_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "merge", "KEEP0001", "OTHR0001", "--dry-run"],
    );

    assert_eq!(server.finish().len(), 2, "still zero bridge/write calls");
    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(payload["status"], "dry_run");
    assert_eq!(payload["will"]["trash_items"], json!(["OTHR0001"]));
}

// ── Bridge-first preview: success, transport/eval failure, and the "bridge says keep is
//    missing but SQLite still finds it" non-terminal case Python's own fallback ladder defines ──

/// The exact object `T_ITEM_MERGE_PREVIEW`'s JS would return for `KEEP0001`/`OTHR0001` against
/// this file's fixture -- hand-computed to match `summarize_item_for_merge_preview`'s SQLite
/// output field-for-field, since both are supposed to produce the same shape.
fn scripted_bridge_preview_success_body() -> serde_json::Value {
    json!({
        "ok": true,
        "keep": {
            "key": "KEEP0001", "title": "Keep Item", "DOI": "", "date": "", "itemType": "document",
            "tags": ["keep-tag"],
            "collections": [{"id": 1, "key": "COLLE001", "name": "Test Collection"}],
            "attachments": [], "notes": [],
            "nAttachments": 0, "nNotes": 0, "nTags": 1, "nCollections": 1,
        },
        "others": [{
            "key": "OTHR0001", "title": "Other Item A", "DOI": "10.1000/a", "date": "2024-01-01",
            "itemType": "document",
            "tags": ["only-a-tag", "shared-tag"],
            "collections": [{"id": 2, "key": "COLLC002", "name": "Collection C2"}],
            "attachments": [{"key": "ATCH0001", "title": "", "contentType": "application/pdf",
                              "filename": "storage:a.pdf", "linkMode": 0}],
            "notes": [{"key": "NOTE0001", "title": "Note A"}],
            "nAttachments": 1, "nNotes": 1, "nTags": 2, "nCollections": 1,
        }],
        "missing": [],
        "will": {
            "move_attachments": 1, "move_notes": 1,
            "add_tags": ["only-a-tag", "shared-tag"],
            "add_collections": [{"id": 2, "key": "COLLC002", "name": "Collection C2"}],
            "trash_items": ["OTHR0001"],
        },
    })
}

#[test]
fn bridge_available_and_preview_succeeds_reports_bridge_source_and_touches_no_sqlite() {
    // No fixture built at all: if the implementation fell through to SQLite despite the bridge
    // succeeding, `db::resolve_item` would fail loudly against a data dir with no zotero.sqlite --
    // structural proof this path never touches SQLite, not just an unasserted side observation.
    let dir = TestDir::new("merge-preview-bridge-success");
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        ScriptedResponse::json(200, scripted_bridge_preview_success_body()),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "merge", "KEEP0001", "OTHR0001"],
    );

    let requests = server.finish();
    assert_eq!(
        requests.len(),
        4,
        "ping, probe, ownership-ping, preview-eval: {requests:?}"
    );
    let eval_body = String::from_utf8_lossy(&requests[3].body);
    for forbidden in [
        "saveTx", "eraseTx", ".merge(", ".trash", ".deleted", "delete(",
    ] {
        assert!(
            !eval_body.contains(forbidden),
            "preview script sent to the bridge must be read-only; found {forbidden:?} in: {eval_body}"
        );
    }

    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(payload["action"], "item_merge");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["status"], "dry_run");
    assert_eq!(payload["code"], "DRY_RUN");
    assert_eq!(payload["preview_source"], "bridge");
    assert_eq!(payload["dry_run"], true);
    assert_eq!(
        payload.get("bridge_error"),
        None,
        "no error to report on a clean bridge success"
    );
    assert_eq!(payload["keep"]["key"], "KEEP0001");
    assert_eq!(payload["others"][0]["key"], "OTHR0001");
    assert_eq!(payload["missing"], json!([]));
    assert_eq!(payload["will"]["move_attachments"], 1);
    assert_eq!(payload["will"]["move_notes"], 1);
    assert_eq!(
        payload["will"]["add_tags"],
        json!(["only-a-tag", "shared-tag"])
    );
    assert_eq!(payload["will"]["trash_items"], json!(["OTHR0001"]));
    assert_eq!(payload["summary"]["trash_count"], 1);
    assert_eq!(
        payload["summary"]["add_collections"],
        json!(["Collection C2"])
    );
    assert_eq!(
        payload["message"],
        "Would trash 1 item(s) into keep=KEEP0001: move 1 attachment(s), 1 note(s); \
         add 2 tag(s), 1 collection(s). (preview via bridge) Re-run with --confirm to apply."
    );
    assert_eq!(
        payload["plan"],
        json!({"keep": "KEEP0001", "merge": ["OTHR0001"], "dry_run": true})
    );
}

#[test]
fn bridge_returns_missing_merge_away_item_matches_python_non_error_behavior() {
    let dir = TestDir::new("merge-preview-bridge-missing-other");
    let mut body = scripted_bridge_preview_success_body();
    body["others"] = json!([]);
    body["missing"] = json!(["NOPE9999"]);
    body["will"] = json!({
        "move_attachments": 0, "move_notes": 0, "add_tags": [], "add_collections": [], "trash_items": [],
    });
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        ScriptedResponse::json(200, body),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "merge", "KEEP0001", "NOPE9999"],
    );

    assert_eq!(server.finish().len(), 4);
    assert_eq!(
        code, 0,
        "a missing merge-away item is not a preview-time error: {payload}"
    );
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["status"], "dry_run");
    assert_eq!(payload["preview_source"], "bridge");
    assert_eq!(payload["missing"], json!(["NOPE9999"]));
    assert_eq!(payload["others"], json!([]));
}

#[test]
fn bridge_eval_reports_failure_falls_back_to_sqlite_with_bridge_error_recorded() {
    let dir = TestDir::new("merge-preview-bridge-eval-error");
    build_merge_preview_fixture(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        // A transport-level success (HTTP 200, valid JSON) but an app-level eval failure --
        // distinct from "bridge unavailable" (scenario B), matching `hygiene.py:356`'s
        // `data.get("error")` branch, not the `transport.get("error")` one.
        ScriptedResponse::json(200, json!({"ok": false, "error": "sandbox eval exploded"})),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "merge", "KEEP0001", "OTHR0001"],
    );

    assert_eq!(
        server.finish().len(),
        4,
        "ownership succeeded; the eval itself was attempted"
    );
    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["preview_source"], "sqlite");
    assert_eq!(payload["bridge_error"], "sandbox eval exploded");
    assert_eq!(payload["keep"]["key"], "KEEP0001");
    assert_eq!(payload["others"][0]["key"], "OTHR0001");
}

#[test]
fn bridge_cannot_resolve_keep_falls_back_to_sqlite_which_still_finds_it() {
    // Matches `hygiene.py:preview_merge` exactly: the bridge reporting "keep item not found" is
    // *not* terminal -- SQLite gets its own independent attempt, and here it succeeds, so the
    // overall preview succeeds too (with the bridge's error carried forward, not surfaced as a
    // failure).
    let dir = TestDir::new("merge-preview-bridge-keep-not-found");
    build_merge_preview_fixture(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({"ok": false, "error": "keep item not found", "keep": "KEEP0001"}),
        ),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "merge", "KEEP0001", "OTHR0001"],
    );

    assert_eq!(server.finish().len(), 4);
    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["status"], "dry_run");
    assert_eq!(payload["preview_source"], "sqlite");
    assert_eq!(payload["bridge_error"], "keep item not found");
    assert_eq!(payload["keep"]["key"], "KEEP0001");
}

#[test]
fn bridge_and_sqlite_both_fail_to_resolve_keep_is_keep_not_found_with_bridge_error() {
    let dir = TestDir::new("merge-preview-bridge-and-sqlite-keep-not-found");
    build_merge_preview_fixture(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({"ok": false, "error": "keep item not found", "keep": "GHOST999"}),
        ),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "merge", "GHOST999", "OTHR0001"],
    );

    assert_eq!(server.finish().len(), 4);
    assert_eq!(code, 1, "payload: {payload}");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["code"], "KEEP_NOT_FOUND");
    assert_eq!(payload["preview_source"], "sqlite");
    assert_eq!(payload["bridge_error"], "keep item not found");
    assert_eq!(payload["error"], "keep item not found: GHOST999");
}

// ── Multiple merge keys: incremental (not naive-union) dedup across `others` ──

#[test]
fn multiple_merge_keys_accumulate_tags_and_collections_without_duplicating_shared_ones() {
    let dir = TestDir::new("merge-preview-multi");
    build_merge_preview_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "merge", "KEEP0001", "OTHR0001", "OTHR0002"],
    );

    assert_eq!(server.finish().len(), 2);
    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(payload["others"].as_array().unwrap().len(), 2);
    assert_eq!(payload["missing"], json!([]));

    // "shared-tag" appears on both A and B but must be added to the plan only once.
    assert_eq!(
        payload["will"]["add_tags"],
        json!(["only-a-tag", "shared-tag"])
    );
    // "Collection C2" is shared by A and B; only "Collection C3" (B-only) is new besides it.
    assert_eq!(payload["will"]["add_collections"][0]["key"], "COLLC002");
    assert_eq!(payload["will"]["add_collections"][1]["key"], "COLLC003");
    assert_eq!(
        payload["will"]["add_collections"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        payload["summary"]["add_collections"],
        json!(["Collection C2", "Collection C3"])
    );
    assert_eq!(payload["will"]["move_attachments"], 1);
    assert_eq!(payload["will"]["move_notes"], 1);
    assert_eq!(
        payload["will"]["trash_items"],
        json!(["OTHR0001", "OTHR0002"])
    );
}

// ── Missing merge-away item: reported in `missing`, still a successful (exit 0) preview ──

#[test]
fn missing_merge_away_item_is_listed_but_does_not_fail_the_preview() {
    let dir = TestDir::new("merge-preview-missing-other");
    build_merge_preview_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "merge", "KEEP0001", "NOPE9999"],
    );

    assert_eq!(server.finish().len(), 2);
    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["status"], "dry_run");
    assert_eq!(payload["missing"], json!(["NOPE9999"]));
    assert_eq!(payload["others"], json!([]));
    assert_eq!(payload["will"]["trash_items"], json!([]));
}

// ── Missing keep item: the one preview-time error ──

#[test]
fn missing_keep_item_is_a_preview_time_error_exit_one() {
    let dir = TestDir::new("merge-preview-missing-keep");
    build_merge_preview_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "merge", "GHOST999", "OTHR0001"],
    );

    assert_eq!(server.finish().len(), 2, "still zero mutation calls");
    assert_eq!(code, 1, "payload: {payload}");
    assert_eq!(payload["action"], "item_merge");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["status"], "error");
    assert_eq!(payload["code"], "KEEP_NOT_FOUND");
    assert_eq!(payload["error"], "keep item not found: GHOST999");
    assert_eq!(payload["preview_source"], "sqlite");
    assert_eq!(
        payload["plan"],
        json!({"keep": "GHOST999", "merge": ["OTHR0001"], "dry_run": true})
    );
    assert_eq!(payload["dry_run"], true);
}

// ── Self / duplicate refs ──

#[test]
fn self_only_merge_key_is_filtered_to_empty_and_reports_invalid_args() {
    let dir = TestDir::new("merge-preview-self-only");
    build_merge_preview_fixture(dir.path());
    // build_runtime() still probes even though INVALID_ARGS short-circuits before any resolution.
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "merge", "KEEP0001", "KEEP0001"],
    );

    assert_eq!(server.finish().len(), 2);
    assert_eq!(code, 1, "payload: {payload}");
    assert_eq!(
        payload,
        json!({
            "action": "item_merge",
            "ok": false,
            "status": "error",
            "code": "INVALID_ARGS",
            "error": "keep key and at least one other key are required",
        }),
        "no plan/dry_run envelope: Python's INVALID_ARGS check runs before that wrapping"
    );
}

#[test]
fn duplicate_merge_key_is_previewed_and_trashed_twice_but_tags_added_only_once() {
    let dir = TestDir::new("merge-preview-duplicate");
    build_merge_preview_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "merge", "KEEP0001", "OTHR0001", "OTHR0001"],
    );

    assert_eq!(server.finish().len(), 2);
    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(
        payload["others"].as_array().unwrap().len(),
        2,
        "Python never dedupes merge_keys -- each occurrence is resolved independently"
    );
    assert_eq!(
        payload["will"]["trash_items"],
        json!(["OTHR0001", "OTHR0001"])
    );
    assert_eq!(payload["will"]["move_attachments"], 2);
    assert_eq!(payload["will"]["move_notes"], 2);
    // The second OTHR0001 occurrence contributes tags/collections already added by the first.
    assert_eq!(
        payload["will"]["add_tags"],
        json!(["only-a-tag", "shared-tag"])
    );
    assert_eq!(
        payload["will"]["add_collections"].as_array().unwrap().len(),
        1
    );
}

// ── --confirm: existing accepted mutation path is unaffected by this fix ──

#[test]
fn confirm_flag_still_executes_the_existing_bridge_merge_mutation_path() {
    let dir = TestDir::new("merge-preview-confirm");
    build_merge_preview_fixture(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        bridge_resolve_item_ok("KEEP0001", 1),
        bridge_resolve_item_ok("OTHR0001", 2),
        ScriptedResponse::bridge_string(200, "OK: merged 1 items into Keep Item"),
        ScriptedResponse::json(
            200,
            json!({"found": true, "key": "KEEP0001", "libraryID": 1, "data": {"itemType": "document", "title": "Keep Item"}}),
        ),
        ScriptedResponse::json(200, json!({"found": false})),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "merge", "KEEP0001", "OTHR0001", "--confirm"],
    );

    let requests = server.finish();
    assert_eq!(
        requests.len(),
        8,
        "confirm resolves both targets through the live Zotero runtime (never SQLite), then \
         mutates and verifies"
    );
    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(payload["outcome"], "applied");
    assert_eq!(payload["key"], "KEEP0001");
}

// ── Conflicting --dry-run/--confirm: last flag on the command line wins, matching Click ──

#[test]
fn when_both_flags_given_confirm_last_wins_and_mutates() {
    let dir = TestDir::new("merge-preview-conflict-confirm-wins");
    build_merge_preview_fixture(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        bridge_resolve_item_ok("KEEP0001", 1),
        bridge_resolve_item_ok("OTHR0001", 2),
        ScriptedResponse::bridge_string(200, "OK: merged 1 items into Keep Item"),
        ScriptedResponse::json(
            200,
            json!({"found": true, "key": "KEEP0001", "libraryID": 1, "data": {"itemType": "document", "title": "Keep Item"}}),
        ),
        ScriptedResponse::json(200, json!({"found": false})),
    ]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "item",
            "merge",
            "KEEP0001",
            "OTHR0001",
            "--dry-run",
            "--confirm",
        ],
    );

    assert_eq!(
        server.finish().len(),
        8,
        "confirm (given last) must win and mutate"
    );
    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(payload["outcome"], "applied");
}

#[test]
fn when_both_flags_given_dry_run_last_wins_and_makes_zero_mutation_calls() {
    let dir = TestDir::new("merge-preview-conflict-dry-run-wins");
    build_merge_preview_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, payload) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "item",
            "merge",
            "KEEP0001",
            "OTHR0001",
            "--confirm",
            "--dry-run",
        ],
    );

    assert_eq!(
        server.finish().len(),
        2,
        "dry-run (given last) must win -- zero bridge/write calls"
    );
    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(payload["status"], "dry_run");
    assert_eq!(payload["dry_run"], true);
}

// ── Human-mode output: same JSON rendering as --json for an object payload (pre-existing
//    `output::emit` behavior; not new logic, just confirming this new payload flows through it) ──

#[test]
fn human_mode_preview_output_matches_json_mode_shape() {
    let dir = TestDir::new("merge-preview-human");
    build_merge_preview_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let output = Command::new(common::bin_path())
        .arg("--data-dir")
        .arg(dir.path())
        .args(["item", "merge", "KEEP0001", "OTHR0001"])
        .env("ZOTERO_HTTP_PORT", server.port.to_string())
        // Per-test isolation, as documented on `common::run_cli`.
        .env(
            "CLI_ANYTHING_ZOTERO_STATE_DIR",
            dir.path().join("cli-state"),
        )
        .env("ZOTERO_CLI_NO_AUTOLAUNCH", "1")
        .env_remove("ZOTERO_LOCAL_API_KEY")
        .output()
        .expect("failed to run zotero-cli binary");

    assert_eq!(server.finish().len(), 2);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let payload: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|err| {
        panic!("human-mode output must still be JSON for an object payload: {err}: {stdout:?}")
    });
    assert_eq!(payload["status"], "dry_run");
    assert_eq!(payload["action"], "item_merge");
}
