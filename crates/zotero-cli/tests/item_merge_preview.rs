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
//! This port's preview always resolves via SQLite (`preview_source: "sqlite"`) rather than
//! attempting a live JS Bridge preview first like Python does -- see `hygiene::merge_preview`'s
//! doc comment for why that's a faithful (not invented) subset of Python's own contract.

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

// ── Bare invocation / explicit --dry-run: identical, zero-mutation preview ──

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
        "only build_runtime()'s connector-ping + local-api-probe -- zero bridge/write calls: {requests:?}"
    );

    assert_eq!(code, 0, "payload: {payload}");
    assert_eq!(payload["action"], "item_merge");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["status"], "dry_run");
    assert_eq!(payload["code"], "DRY_RUN");
    assert_eq!(payload["dry_run"], true);
    assert_eq!(payload["preview_source"], "sqlite");
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
        6,
        "the pre-existing confirm path is untouched by this fix"
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
        6,
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
        .env_remove("ZOTERO_LOCAL_API_KEY")
        .env_remove("CLI_ANYTHING_ZOTERO_STATE_DIR")
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
