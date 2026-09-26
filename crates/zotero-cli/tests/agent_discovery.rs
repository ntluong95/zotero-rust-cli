//! Agent discovery: cross-library search, live search routing, and library names.
//!
//! The scenario these exist for is the one real use exposed: an agent knows a title but not a
//! `libraryID`, so `item find` searched the wrong library and returned `[]` with no way forward
//! except writing `current_library` and re-running, once per library -- and if Zotero happened
//! to be open, every attempt failed on the WAL lock instead.
//!
//! No test here launches or mutates a real Zotero: searches run against the mock server, and
//! `common::run_cli` sets `ZOTERO_CLI_NO_AUTOLAUNCH=1`.

#[path = "common/mod.rs"]
mod common;

use std::path::Path;

use common::{run_cli, ScriptedResponse, ScriptedServer, TestDir};
use serde_json::{json, Value};

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

/// A Bridge response for a template that returns a JSON *string*.
fn bridge_json_string(value: Value) -> ScriptedResponse {
    ScriptedResponse::json(200, json!(value.to_string()))
}

fn live_library(library_id: i64, kind: &str, name: &str) -> Value {
    json!({
        "libraryID": library_id,
        "type": kind,
        "name": name,
        "editable": 1,
        "filesEditable": 1,
        "version": 1,
        "storageVersion": 1,
        "archived": 0,
    })
}

fn live_item(library_id: i64, key: &str, title: &str, item_id: i64) -> Value {
    json!({
        "itemID": item_id,
        "key": key,
        "libraryID": library_id,
        "itemTypeID": 1,
        "typeName": "document",
        "dateAdded": "2026-01-01",
        "dateModified": "2026-01-01",
        "version": 1,
        "title": title,
        "DOI": "",
        "date": null,
        "hasPdf": false,
    })
}

/// A multi-library fixture: a personal library, two groups, and a feed.
///
/// `THOUSAND1` mirrors the real dogfood case -- an item that exists only in a group library, so a
/// search scoped to the personal library finds nothing. `DUPTITLE` exists in two libraries under
/// the same title, which is what makes `libraryID` load-bearing in the output.
fn build_multi_library_fixture(dir: &Path) -> std::path::PathBuf {
    let sqlite_path = dir.join("zotero.sqlite");
    let conn = rusqlite::Connection::open(&sqlite_path).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE libraries (libraryID INTEGER PRIMARY KEY, type TEXT, editable INTEGER, filesEditable INTEGER, version INTEGER, storageVersion INTEGER, lastSync INTEGER, archived INTEGER);
        INSERT INTO libraries VALUES (1, 'user',  1, 1, 1, 1, 0, 0);
        INSERT INTO libraries VALUES (2, 'group', 1, 1, 1, 1, 0, 0);
        INSERT INTO libraries VALUES (7, 'group', 1, 1, 1, 1, 0, 0);
        INSERT INTO libraries VALUES (9, 'feed',  1, 1, 1, 1, 0, 0);

        CREATE TABLE groups (groupID INTEGER PRIMARY KEY, libraryID INT NOT NULL UNIQUE, name TEXT NOT NULL, description TEXT NOT NULL, version INT NOT NULL);
        INSERT INTO groups VALUES (100, 2, 'ASReview public', '', 1);
        INSERT INTO groups VALUES (101, 7, 'INFLUX', '', 1);

        CREATE TABLE feeds (libraryID INTEGER PRIMARY KEY, name TEXT NOT NULL, url TEXT NOT NULL UNIQUE, lastUpdate TIMESTAMP, lastCheck TIMESTAMP, lastCheckError TEXT, cleanupReadAfter INT, cleanupUnreadAfter INT, refreshInterval INT);
        INSERT INTO feeds VALUES (9, 'ASReview_mentions', 'https://example.invalid/feed', NULL, NULL, NULL, NULL, NULL, NULL);

        CREATE TABLE itemTypes (itemTypeID INTEGER PRIMARY KEY, typeName TEXT, templateItemTypeID INTEGER, display INTEGER);
        INSERT INTO itemTypes VALUES (1, 'document', NULL, 1);

        CREATE TABLE items (itemID INTEGER PRIMARY KEY, itemTypeID INTEGER, dateAdded TEXT, dateModified TEXT, clientDateModified TEXT, libraryID INTEGER, key TEXT, version INTEGER, synced INTEGER);
        INSERT INTO items VALUES (1, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'PERSONAL1', 1, 1);
        INSERT INTO items VALUES (2, 1, '2026-01-01', '2026-01-01', '2026-01-01', 7, 'THOUSAND1', 1, 1);
        INSERT INTO items VALUES (3, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'DUPTITLE1', 1, 1);
        INSERT INTO items VALUES (4, 1, '2026-01-01', '2026-01-01', '2026-01-01', 2, 'DUPTITLE2', 1, 1);
        INSERT INTO items VALUES (5, 1, '2026-01-01', '2026-01-01', '2026-01-01', 9, 'FEEDITEM1', 1, 1);

        CREATE TABLE fields (fieldID INTEGER PRIMARY KEY, fieldName TEXT, fieldFormatID INTEGER);
        INSERT INTO fields VALUES (1, 'title', 0), (2, 'DOI', 0);
        CREATE TABLE itemDataValues (valueID INTEGER PRIMARY KEY, value TEXT);
        INSERT INTO itemDataValues VALUES
            (1, 'A personal library paper'),
            (2, 'Thousands turn out to support science'),
            (3, 'Shared Title Across Libraries'),
            (4, 'Thousands of feed mentions');
        CREATE TABLE itemData (itemID INTEGER, fieldID INTEGER, valueID INTEGER);
        INSERT INTO itemData VALUES (1, 1, 1), (2, 1, 2), (3, 1, 3), (4, 1, 3), (5, 1, 4);

        CREATE TABLE creators (creatorID INTEGER PRIMARY KEY, firstName TEXT, lastName TEXT, fieldMode INTEGER);
        CREATE TABLE itemCreators (itemID INTEGER, creatorID INTEGER, creatorTypeID INTEGER, orderIndex INTEGER);
        CREATE TABLE tags (tagID INTEGER PRIMARY KEY, name TEXT);
        CREATE TABLE itemTags (itemID INTEGER, tagID INTEGER, type INTEGER);
        CREATE TABLE collections (collectionID INTEGER PRIMARY KEY, collectionName TEXT, parentCollectionID INTEGER, clientDateModified TEXT, libraryID INTEGER, key TEXT, version INTEGER, synced INTEGER);
        INSERT INTO collections VALUES (1, 'Test Collection', NULL, '2026-01-01', 1, 'COLLE001', 1, 1);
        CREATE TABLE collectionItems (collectionID INTEGER, itemID INTEGER, orderIndex INTEGER);
        CREATE TABLE itemNotes (itemID INTEGER PRIMARY KEY, parentItemID INTEGER, note TEXT, title TEXT);
        CREATE TABLE itemAttachments (itemID INTEGER PRIMARY KEY, parentItemID INTEGER, linkMode INTEGER, contentType TEXT, charsetID INTEGER, path TEXT, syncState INTEGER, storageModTime INTEGER, storageHash TEXT, lastProcessedModificationTime INTEGER);
        CREATE TABLE itemAnnotations (itemID INTEGER PRIMARY KEY, parentItemID INTEGER, type INTEGER, authorName TEXT, text TEXT, comment TEXT, color TEXT, pageLabel TEXT, sortIndex TEXT, position TEXT, isExternal INTEGER);
        CREATE TABLE savedSearches (savedSearchID INTEGER PRIMARY KEY, savedSearchName TEXT, clientDateModified TEXT, libraryID INTEGER, key TEXT, version INTEGER, synced INTEGER);
        CREATE TABLE savedSearchConditions (savedSearchID INTEGER, searchConditionID INTEGER, condition TEXT, operator TEXT, value TEXT, required INTEGER);
        "#,
    )
    .unwrap();
    sqlite_path
}

fn keys(value: &Value) -> Vec<(i64, String)> {
    value
        .as_array()
        .expect("item find returns an array")
        .iter()
        .map(|item| {
            (
                item["libraryID"].as_i64().unwrap(),
                item["key"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

// ── Cross-library scope, offline ───────────────────────────────────────────

#[test]
fn without_the_flag_search_stays_scoped_to_the_current_library() {
    let dir = TestDir::new("scope-default");
    build_multi_library_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, value) = run_cli(dir.path(), server.port, &[], &["item", "find", "Thousands"]);
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    // The whole reported problem: the paper is in library 7, the default scope is library 1,
    // so canonical semantics correctly return nothing. Preserved exactly.
    assert_eq!(keys(&value), Vec::<(i64, String)>::new());
}

#[test]
fn all_libraries_finds_an_item_in_a_group_library() {
    let dir = TestDir::new("scope-group");
    build_multi_library_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "find", "Thousands", "--all-libraries"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(
        keys(&value),
        vec![(7, "THOUSAND1".to_string())],
        "the item an agent was looking for, found without knowing its library"
    );
}

#[test]
fn all_libraries_finds_an_item_in_the_personal_library() {
    let dir = TestDir::new("scope-personal");
    build_multi_library_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "find", "personal library paper", "--all-libraries"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(keys(&value), vec![(1, "PERSONAL1".to_string())]);
}

#[test]
fn identical_titles_in_two_libraries_stay_distinguishable() {
    let dir = TestDir::new("scope-duplicate-title");
    build_multi_library_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "find", "Shared Title", "--all-libraries"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    let found = keys(&value);
    assert_eq!(found.len(), 2, "both copies are returned: {found:?}");
    // Same title, different libraries: `libraryID` + `key` is what tells them apart, and an
    // agent needs both to target the right one for a follow-up write.
    assert!(found.contains(&(1, "DUPTITLE1".to_string())), "{found:?}");
    assert!(found.contains(&(2, "DUPTITLE2".to_string())), "{found:?}");
}

#[test]
fn no_match_returns_an_empty_array_not_an_error() {
    let dir = TestDir::new("scope-no-match");
    build_multi_library_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "find", "nothing matches this", "--all-libraries"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value, json!([]));
}

#[test]
fn feeds_are_excluded_by_default_and_included_only_on_request() {
    let dir = TestDir::new("scope-feeds");
    build_multi_library_fixture(dir.path());

    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);
    let (code, without) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "find", "Thousands", "--all-libraries"],
    );
    server.finish();
    assert_eq!(code, 0, "stdout={without}");
    let found = keys(&without);
    assert!(
        !found.iter().any(|(library, _)| *library == 9),
        "a feed item is an unsaved RSS entry and must not appear by default: {found:?}"
    );

    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);
    let (code, with) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "item",
            "find",
            "Thousands",
            "--all-libraries",
            "--include-feeds",
        ],
    );
    server.finish();
    assert_eq!(code, 0, "stdout={with}");
    let found = keys(&with);
    assert!(
        found.contains(&(9, "FEEDITEM1".to_string())),
        "--include-feeds must actually include them: {found:?}"
    );
}

#[test]
fn include_feeds_requires_all_libraries() {
    let dir = TestDir::new("scope-feeds-requires");
    build_multi_library_fixture(dir.path());
    let output = std::process::Command::new(common::bin_path())
        .arg("--json")
        .arg("--data-dir")
        .arg(dir.path())
        .args(["item", "find", "x", "--include-feeds"])
        .env("ZOTERO_HTTP_PORT", "1")
        .env(
            "CLI_ANYTHING_ZOTERO_STATE_DIR",
            dir.path().join("cli-state"),
        )
        .env("ZOTERO_CLI_NO_AUTOLAUNCH", "1")
        .output()
        .expect("failed to run zotero-cli");
    // clap usage error: the flag is meaningless on its own, so it is rejected rather than
    // silently ignored.
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn all_libraries_conflicts_with_collection_rather_than_silently_ignoring_one() {
    let dir = TestDir::new("scope-collection-conflict");
    build_multi_library_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "item",
            "find",
            "Thousands",
            "--all-libraries",
            "--collection",
            "COLLE001",
        ],
    );
    server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert!(
        value["error"]
            .as_str()
            .unwrap_or_default()
            .contains("cannot be combined"),
        "a collection pins one library, so the combination is contradictory: {value}"
    );
}

#[test]
fn cross_library_search_never_writes_session_state() {
    let dir = TestDir::new("scope-session-untouched");
    build_multi_library_fixture(dir.path());
    let state_dir = dir.path().join("cli-state");
    std::fs::create_dir_all(&state_dir).unwrap();
    let session_path = state_dir.join("session.json");
    let original = serde_json::json!({
        "current_library": serde_json::Value::Null,
        "current_collection": serde_json::Value::Null,
        "current_item": serde_json::Value::Null,
        "command_history": [],
    });
    std::fs::write(&session_path, serde_json::to_vec_pretty(&original).unwrap()).unwrap();
    let before = std::fs::read(&session_path).unwrap();

    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);
    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "find", "Thousands", "--all-libraries"],
    );
    server.finish();
    assert_eq!(code, 0, "stdout={value}");

    // The manual workaround this feature replaces was `session use-library N` in a loop, which
    // permanently repointed the user's session. Searching must never do that.
    assert_eq!(
        std::fs::read(&session_path).unwrap(),
        before,
        "searching must not mutate persisted session state"
    );
}

#[test]
fn cross_library_ordering_is_deterministic() {
    let dir = TestDir::new("scope-ordering");
    build_multi_library_fixture(dir.path());
    let mut runs = Vec::new();
    for _ in 0..3 {
        let server =
            ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);
        let (code, value) = run_cli(
            dir.path(),
            server.port,
            &[],
            &["item", "find", "Shared Title", "--all-libraries"],
        );
        server.finish();
        assert_eq!(code, 0, "stdout={value}");
        runs.push(keys(&value));
    }
    assert_eq!(runs[0], runs[1]);
    assert_eq!(runs[1], runs[2]);
}

// ── Live routing ───────────────────────────────────────────────────────────

/// Holds an exclusive lock on a WAL-mode database, reproducing a running Zotero.
struct LockedWalDb {
    _conn: rusqlite::Connection,
}

impl LockedWalDb {
    fn hold(sqlite_path: &Path) -> Self {
        let conn = rusqlite::Connection::open(sqlite_path).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.pragma_update(None, "locking_mode", "EXCLUSIVE")
            .unwrap();
        conn.execute_batch("BEGIN EXCLUSIVE; CREATE TABLE IF NOT EXISTS _lock (x); COMMIT;")
            .unwrap();
        conn.query_row("SELECT 1", [], |_| Ok(())).unwrap();
        LockedWalDb { _conn: conn }
    }
}

#[test]
fn zotero_closed_uses_the_sqlite_path_and_never_probes_the_bridge() {
    let dir = TestDir::new("live-closed");
    build_multi_library_fixture(dir.path());
    // Only the two runtime probes are scripted. If the search issued a Bridge probe as well it
    // would appear here -- proving the offline path costs exactly what it always did, which is
    // what keeps `item find`'s byte-identical parity (and its recorded http_calls) intact.
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "find", "Thousands", "--all-libraries"],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(keys(&value), vec![(7, "THOUSAND1".to_string())]);
    assert!(
        requests.iter().all(|r| r.path != "/cli-bridge/eval"),
        "an offline search must not speculatively probe the Bridge: {:?}",
        requests.iter().map(|r| &r.path).collect::<Vec<_>>()
    );
}

#[test]
fn zotero_running_with_a_healthy_bridge_searches_live_while_sqlite_is_locked() {
    let dir = TestDir::new("live-bridge");
    let sqlite_path = build_multi_library_fixture(dir.path());
    let _lock = LockedWalDb::hold(&sqlite_path);

    // Sanity: SQLite really is unusable, so a pass below cannot be SQLite answering quietly.
    let (read_code, read_value) = run_cli(dir.path(), 1, &[], &["item", "get", "PERSONAL1"]);
    assert_eq!(read_code, 1, "SQLite must be refused while locked");
    assert!(read_value["error"]
        .as_str()
        .unwrap_or_default()
        .contains("exclusive lock"));

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        bridge_json_string(json!([
            live_library(1, "user", "My Library"),
            live_library(7, "group", "INFLUX"),
            live_library(9, "feed", "ASReview_mentions"),
        ])),
        bridge_json_string(json!([live_item(
            7,
            "THOUSAND1",
            "Thousands turn out to support science",
            2
        )])),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "find", "Thousands", "--all-libraries"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(
        keys(&value),
        vec![(7, "THOUSAND1".to_string())],
        "search must succeed through the live runtime while the database is locked"
    );
}

#[test]
fn zotero_running_without_a_bridge_preserves_the_wal_refusal_verbatim() {
    let dir = TestDir::new("live-no-bridge");
    let sqlite_path = build_multi_library_fixture(dir.path());
    let _lock = LockedWalDb::hold(&sqlite_path);

    // Connector and Local API answer (Zotero is up) but nothing serves the Bridge endpoint.
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "find", "Thousands", "--all-libraries"],
    );
    server.finish();

    assert_eq!(code, 1, "stdout={value}");
    let error = value["error"].as_str().unwrap_or_default();
    // The safety refusal is the fallback, not something routed around -- and its wording is
    // unchanged, so the user still learns the real cause and the documented remedy.
    assert!(
        error.contains("exclusive lock") && error.contains("immutable=1"),
        "the original WAL refusal must be reported unchanged: {error}"
    );
}

#[test]
fn exact_title_keeps_the_refusal_rather_than_changing_what_the_flag_means() {
    let dir = TestDir::new("live-exact-title");
    let sqlite_path = build_multi_library_fixture(dir.path());
    let _lock = LockedWalDb::hold(&sqlite_path);

    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);
    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "item",
            "find",
            "Thousands turn out to support science",
            "--all-libraries",
            "--exact-title",
        ],
    );
    let requests = server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert!(value["error"]
        .as_str()
        .unwrap_or_default()
        .contains("exclusive lock"));
    // Zotero's quicksearch is substring-only. Approximating an exact match live would silently
    // redefine `--exact-title`, so the live path is not attempted at all here.
    assert!(
        requests.iter().all(|r| r.path != "/cli-bridge/eval"),
        "--exact-title must not fall through to a substring live search"
    );
}

// ── Library names ──────────────────────────────────────────────────────────

fn libraries_by_id(value: &Value) -> std::collections::HashMap<i64, Value> {
    value
        .as_array()
        .expect("library list returns an array")
        .iter()
        .map(|l| (l["libraryID"].as_i64().unwrap(), l.clone()))
        .collect()
}

#[test]
fn library_list_names_user_group_and_feed_libraries() {
    let dir = TestDir::new("names-all-types");
    build_multi_library_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, value) = run_cli(dir.path(), server.port, &[], &["library", "list"]);
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    let libraries = libraries_by_id(&value);
    assert_eq!(libraries[&1]["name"], "My Library");
    assert_eq!(libraries[&2]["name"], "ASReview public");
    assert_eq!(libraries[&7]["name"], "INFLUX");
    assert_eq!(libraries[&9]["name"], "ASReview_mentions");

    // Additive: every canonical field is still present, unchanged and in place.
    for field in [
        "libraryID",
        "type",
        "editable",
        "filesEditable",
        "version",
        "storageVersion",
        "lastSync",
        "archived",
    ] {
        assert!(
            libraries[&7].get(field).is_some(),
            "canonical field {field} must be preserved"
        );
    }
}

#[test]
fn an_unresolvable_library_name_is_null_and_never_invented() {
    let dir = TestDir::new("names-unresolvable");
    let sqlite_path = build_multi_library_fixture(dir.path());
    let conn = rusqlite::Connection::open(&sqlite_path).unwrap();
    // A group library whose `groups` row is missing: possible mid-sync, and the shape every
    // fixture without a `groups` table already has.
    conn.execute_batch("INSERT INTO libraries VALUES (12, 'group', 1, 1, 1, 1, 0, 0);")
        .unwrap();
    drop(conn);

    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);
    let (code, value) = run_cli(dir.path(), server.port, &[], &["library", "list"]);
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    let libraries = libraries_by_id(&value);
    assert_eq!(
        libraries[&12]["name"],
        Value::Null,
        "an absent name is reported as null, never guessed or defaulted"
    );
    // The command still succeeds and still lists the library.
    assert_eq!(libraries[&12]["type"], "group");
}

#[test]
fn library_list_works_on_a_fixture_with_no_groups_or_feeds_tables() {
    let dir = TestDir::new("names-minimal-fixture");
    common::build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, value) = run_cli(dir.path(), server.port, &[], &["library", "list"]);
    server.finish();

    // The name join must not require tables a minimal database does not have.
    assert_eq!(code, 0, "stdout={value}");
    let libraries = libraries_by_id(&value);
    assert_eq!(libraries[&1]["name"], "My Library");
}

#[test]
fn library_list_succeeds_with_null_names_when_group_and_feed_tables_are_absent() {
    let dir = TestDir::new("names-missing-tables-group-feed");
    let sqlite_path = dir.path().join("zotero.sqlite");
    let conn = rusqlite::Connection::open(&sqlite_path).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE libraries (libraryID INTEGER PRIMARY KEY, type TEXT, editable INTEGER, filesEditable INTEGER, version INTEGER, storageVersion INTEGER, lastSync INTEGER, archived INTEGER);
        INSERT INTO libraries VALUES (1, 'user', 1, 1, 1, 1, 0, 0);
        INSERT INTO libraries VALUES (2, 'group', 1, 1, 1, 1, 0, 0);
        INSERT INTO libraries VALUES (3, 'feed', 1, 1, 1, 1, 0, 0);
        "#,
    )
    .unwrap();
    drop(conn);

    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);
    let (code, value) = run_cli(dir.path(), server.port, &[], &["library", "list"]);
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    let libraries = libraries_by_id(&value);
    assert_eq!(libraries[&1]["name"], "My Library");
    assert_eq!(libraries[&2]["name"], Value::Null);
    assert_eq!(libraries[&3]["name"], Value::Null);

    for id in [1, 2, 3] {
        for field in [
            "libraryID",
            "type",
            "editable",
            "filesEditable",
            "version",
            "storageVersion",
            "lastSync",
            "archived",
        ] {
            assert!(
                libraries[&id].get(field).is_some(),
                "canonical field {field} must remain present"
            );
        }
    }
}

#[test]
fn library_list_reads_live_when_sqlite_is_locked() {
    let dir = TestDir::new("names-live");
    let sqlite_path = build_multi_library_fixture(dir.path());
    let _lock = LockedWalDb::hold(&sqlite_path);

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        bridge_json_string(json!([
            live_library(1, "user", "My Library"),
            live_library(7, "group", "INFLUX"),
        ])),
    ]);

    let (code, value) = run_cli(dir.path(), server.port, &[], &["library", "list"]);
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    let libraries = libraries_by_id(&value);
    assert_eq!(libraries[&7]["name"], "INFLUX");
    // Same key set as the offline path, so a caller cannot tell which source answered.
    let mut fields: Vec<&str> = libraries[&7]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        vec![
            "archived",
            "editable",
            "filesEditable",
            "lastSync",
            "libraryID",
            "name",
            "storageVersion",
            "type",
            "version",
        ]
    );
}

// ── Live snapshot fallback for catalog reads ───────────────────────────────

/// Seeds the CLI's snapshot cache with a copy of `sqlite_path` under `key`, as an earlier
/// invocation's `VACUUM INTO` would have left it, and returns that key.
fn seed_snapshot_cache(data_dir: &Path, sqlite_path: &Path) -> &'static str {
    let key = "fixture-1-0";
    let dir = zotero_cli::live_snapshot::cache_dir(
        &data_dir.join("cli-state").join("live-snapshot"),
        sqlite_path,
    );
    std::fs::create_dir_all(&dir).unwrap();
    rusqlite::Connection::open(sqlite_path)
        .unwrap()
        .execute(
            "VACUUM INTO ?1",
            [dir.join(format!("snapshot-{key}.sqlite")).to_string_lossy()],
        )
        .unwrap();
    key
}

/// The JSON params of every Bridge eval request, decoded from the rendered template prelude.
fn bridge_params(requests: &[common::CapturedRequest]) -> Vec<Value> {
    requests
        .iter()
        .filter(|request| request.path == "/cli-bridge/eval")
        .filter_map(|request| {
            let code = std::str::from_utf8(&request.body).unwrap();
            let encoded = code
                .strip_prefix("const P = JSON.parse(")?
                .split_once(");\n")?
                .0;
            let serialized: String = serde_json::from_str(encoded).unwrap();
            Some(serde_json::from_str(&serialized).unwrap())
        })
        .collect()
}

#[test]
fn catalog_reads_use_a_live_snapshot_while_sqlite_is_locked() {
    // The commands an agent (ASK) needs while Zotero is open, which previously all refused.
    let cases: &[&[&str]] = &[
        &["collection", "list"],
        &["collection", "get", "COLLE001"],
        &["item", "get", "PERSONAL1"],
        &["session", "use-library", "1"],
    ];
    for args in cases {
        let dir = TestDir::new("snapshot-live");
        let sqlite_path = build_multi_library_fixture(dir.path());

        // Offline answer first, for a byte-for-byte comparison.
        let offline_server =
            ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);
        let (offline_code, offline_value) = run_cli(dir.path(), offline_server.port, &[], args);
        drop(offline_server);
        assert_eq!(offline_code, 0, "{args:?} offline: {offline_value}");

        // A warm cache: Zotero's change key is unchanged since an earlier invocation copied the
        // database, so the whole read costs one Bridge round trip and no new copy.
        let key = seed_snapshot_cache(dir.path(), &sqlite_path);
        let script = vec![
            connector_ping_ok(),
            local_api_probe_unavailable(),
            bridge_ownership_ok(),
            bridge_json_string(json!({"key": key, "reused": true})),
        ];
        let _lock = LockedWalDb::hold(&sqlite_path);
        let server = ScriptedServer::start(script);
        let (code, value) = run_cli(dir.path(), server.port, &[], args);
        let requests = server.finish();

        assert_eq!(code, 0, "{args:?} must succeed while locked: {value}");
        let params = bridge_params(&requests);
        assert_eq!(
            params.len(),
            1,
            "{args:?}: one Bridge request per read: {params:?}"
        );
        assert_eq!(params[0]["op"], "snapshot");
        assert_eq!(params[0]["have"], json!([key]));
        if args[0] != "session" {
            assert_eq!(
                value, offline_value,
                "{args:?}: live snapshot must match offline output"
            );
        }
    }
}

#[test]
fn catalog_reads_still_refuse_while_locked_when_no_bridge_answers() {
    let dir = TestDir::new("snapshot-no-bridge");
    let sqlite_path = build_multi_library_fixture(dir.path());
    let _lock = LockedWalDb::hold(&sqlite_path);
    let (code, value) = run_cli(dir.path(), 1, &[], &["collection", "list"]);
    assert_eq!(code, 1);
    assert!(
        value["error"]
            .as_str()
            .unwrap_or_default()
            .contains("exclusive lock"),
        "the original refusal must be reported verbatim: {value}"
    );
}

#[test]
fn all_libraries_honors_fields_scope_through_the_local_api_per_library() {
    let dir = TestDir::new("all-libs-fields-scope");
    build_multi_library_fixture(dir.path());
    let empty = || ScriptedResponse::json(200, json!([]));
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        ScriptedResponse::json(200, json!({})),
        // Library 1 (personal): no field hit -> that library falls back to its title search.
        empty(),
        // Library 2 is group 100: the Local API must be addressed by groupID.
        ScriptedResponse::json(200, json!([{"key": "DUPTITLE2"}])),
        // Library 7 is group 101.
        empty(),
    ]);
    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "item",
            "find",
            "10.9999/dup",
            "--all-libraries",
            "--scope",
            "fields",
        ],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(keys(&value), vec![(2, "DUPTITLE2".to_string())]);
    let paths: Vec<&str> = requests.iter().map(|r| r.path.as_str()).collect();
    assert!(
        paths
            .iter()
            .any(|p| p.starts_with("/api/users/0/items/top")),
        "{paths:?}"
    );
    assert!(
        paths
            .iter()
            .any(|p| p.starts_with("/api/groups/100/items/top")),
        "{paths:?}"
    );
    assert!(
        paths
            .iter()
            .any(|p| p.starts_with("/api/groups/101/items/top")),
        "{paths:?}"
    );
    assert!(
        paths.iter().all(|p| !p.starts_with("/api/groups/2/")),
        "libraryID must never be used as a groupID: {paths:?}"
    );
}

#[test]
fn include_feeds_keeps_fields_scope_for_libraries_and_title_searches_feeds() {
    let dir = TestDir::new("all-libs-fields-feeds");
    build_multi_library_fixture(dir.path());
    let empty = || ScriptedResponse::json(200, json!([]));
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        ScriptedResponse::json(200, json!({})),
        empty(),
        // A field-only hit: DUPTITLE2's title does not contain the query, so it can only be
        // found if the fields scope survives `--include-feeds`.
        ScriptedResponse::json(200, json!([{"key": "DUPTITLE2"}])),
        empty(),
    ]);
    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "item",
            "find",
            "Thousands",
            "--all-libraries",
            "--include-feeds",
            "--scope",
            "fields",
        ],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "stdout={value}");
    let found = keys(&value);
    assert!(
        found.contains(&(2, "DUPTITLE2".to_string())),
        "the fields scope must still apply to non-feed libraries: {found:?}"
    );
    assert!(
        found.contains(&(9, "FEEDITEM1".to_string())),
        "feeds are searched by title: {found:?}"
    );
    assert!(
        requests.iter().all(|r| !r.path.contains("/9/")),
        "a feed has no Local API endpoint: {:?}",
        requests.iter().map(|r| &r.path).collect::<Vec<_>>()
    );
}
