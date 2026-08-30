//! Phase 7 Slice 4: `core/notes.py::get_note`/`add_note`, ported to `src/notes.rs`.
//! `#[path]`-included (not yet a registered crate module -- no CLI-dispatch slice exists for
//! `note get`/`note add` yet), exactly like Phase 7 Slice 3's own `pdf_cascade.rs`/`pdf_fetch.rs`
//! test files. Deliberately does NOT invoke the `zotero-cli note get`/`note add` subcommands --
//! that CLI-dispatch layer is reserved for a later, serialized Phase 7 integration slice.

#![allow(dead_code)]

#[path = "../src/notes.rs"]
mod notes;

pub mod bridge {
    pub use zotero_cli::bridge::*;
}
pub mod catalog {
    pub use zotero_cli::catalog::*;
}
pub mod db {
    pub use zotero_cli::db::*;
}
pub mod error {
    pub use zotero_cli::error::*;
}
pub mod paths {
    pub use zotero_cli::paths::*;
}
pub mod runtime {
    pub use zotero_cli::runtime::*;
}
pub mod session {
    pub use zotero_cli::session::*;
}

#[path = "common/mod.rs"]
mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use bridge::JSBridgeClient;
use common::{ScriptedResponse, ScriptedServer, TestDir};
use runtime::RuntimeContext;
use serde_json::json;
use session::SessionState;

fn test_runtime(sqlite_path: PathBuf) -> RuntimeContext {
    RuntimeContext {
        environment: paths::ZoteroEnvironment {
            executable: None,
            executable_exists: false,
            install_dir: None,
            version: "10.0.1".to_string(),
            profile_root: PathBuf::from("/tmp/does-not-exist-profile-root"),
            profile_dir: None,
            data_dir: PathBuf::from("/tmp/does-not-exist-data-dir"),
            data_dir_exists: false,
            sqlite_path,
            sqlite_exists: true,
            styles_dir: PathBuf::from("/tmp/does-not-exist-styles"),
            styles_exists: false,
            storage_dir: PathBuf::from("/tmp/does-not-exist-storage"),
            storage_exists: false,
            translators_dir: PathBuf::from("/tmp/does-not-exist-translators"),
            translators_exists: false,
            port: 0,
            local_api_enabled_configured: false,
        },
        backend: "sqlite".to_string(),
        connector_available: false,
        connector_message: String::new(),
        local_api_available: false,
        local_api_message: String::new(),
        server_id: None,
        local_api_writes_available: false,
    }
}

/// A dedicated fixture (not `common::build_fixture_sqlite`, which has no note/attachment/
/// annotation item-type rows): two libraries, a top-level document in each, a note/attachment/
/// annotation child of library 1's document (for `add_note`'s parent-type rejection tests), and
/// a note key (`DUPNOTE1`) duplicated across both libraries (for `current_library` scoping).
fn build_notes_fixture(dir: &Path) -> PathBuf {
    let sqlite_path = dir.join("zotero.sqlite");
    let conn = rusqlite::Connection::open(&sqlite_path).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE libraries (libraryID INTEGER PRIMARY KEY, type TEXT, editable INTEGER, filesEditable INTEGER, version INTEGER, storageVersion INTEGER, lastSync INTEGER, archived INTEGER);
        INSERT INTO libraries VALUES (1, 'user', 1, 1, 1, 1, 0, 0);
        INSERT INTO libraries VALUES (2, 'user', 1, 1, 1, 1, 0, 0);

        CREATE TABLE itemTypes (itemTypeID INTEGER PRIMARY KEY, typeName TEXT, templateItemTypeID INTEGER, display INTEGER);
        INSERT INTO itemTypes VALUES (1, 'document', NULL, 1);
        INSERT INTO itemTypes VALUES (2, 'note', NULL, 1);
        INSERT INTO itemTypes VALUES (3, 'attachment', NULL, 1);
        INSERT INTO itemTypes VALUES (4, 'annotation', NULL, 1);

        CREATE TABLE items (itemID INTEGER PRIMARY KEY, itemTypeID INTEGER, dateAdded TEXT, dateModified TEXT, clientDateModified TEXT, libraryID INTEGER, key TEXT, version INTEGER, synced INTEGER);
        INSERT INTO items VALUES (1, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'DOC00001', 1, 1);
        INSERT INTO items VALUES (2, 2, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'NOTE0001', 1, 1);
        INSERT INTO items VALUES (3, 3, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'ATT00001', 1, 1);
        INSERT INTO items VALUES (4, 4, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'ANN00001', 1, 1);
        INSERT INTO items VALUES (7, 2, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'DUPNOTE1', 1, 1);
        INSERT INTO items VALUES (5, 1, '2026-01-01', '2026-01-01', '2026-01-01', 2, 'DOC00002', 1, 1);
        INSERT INTO items VALUES (6, 2, '2026-01-01', '2026-01-01', '2026-01-01', 2, 'NOTE0002', 1, 1);
        INSERT INTO items VALUES (8, 2, '2026-01-01', '2026-01-01', '2026-01-01', 2, 'DUPNOTE1', 1, 1);

        CREATE TABLE fields (fieldID INTEGER PRIMARY KEY, fieldName TEXT, fieldFormatID INTEGER);
        INSERT INTO fields VALUES (1, 'title', 0);
        CREATE TABLE itemDataValues (valueID INTEGER PRIMARY KEY, value TEXT);
        INSERT INTO itemDataValues VALUES (1, 'Doc One'), (2, 'Doc Two');
        CREATE TABLE itemData (itemID INTEGER, fieldID INTEGER, valueID INTEGER);
        INSERT INTO itemData VALUES (1, 1, 1), (5, 1, 2);

        CREATE TABLE creators (creatorID INTEGER PRIMARY KEY, firstName TEXT, lastName TEXT, fieldMode INTEGER);
        CREATE TABLE itemCreators (itemID INTEGER, creatorID INTEGER, creatorTypeID INTEGER, orderIndex INTEGER);
        CREATE TABLE tags (tagID INTEGER PRIMARY KEY, name TEXT);
        CREATE TABLE itemTags (itemID INTEGER, tagID INTEGER, type INTEGER);

        CREATE TABLE collections (collectionID INTEGER PRIMARY KEY, collectionName TEXT, parentCollectionID INTEGER, clientDateModified TEXT, libraryID INTEGER, key TEXT, version INTEGER, synced INTEGER);
        CREATE TABLE collectionItems (collectionID INTEGER, itemID INTEGER, orderIndex INTEGER);

        CREATE TABLE itemNotes (itemID INTEGER PRIMARY KEY, parentItemID INTEGER, note TEXT, title TEXT);
        INSERT INTO itemNotes VALUES (2, 1, '<p>Hello note</p>', NULL);
        INSERT INTO itemNotes VALUES (6, 5, '<p>Lib2 note</p>', NULL);
        INSERT INTO itemNotes VALUES (7, 1, '<p>dup in lib1</p>', NULL);
        INSERT INTO itemNotes VALUES (8, 5, '<p>dup in lib2</p>', NULL);

        CREATE TABLE itemAttachments (itemID INTEGER PRIMARY KEY, parentItemID INTEGER, linkMode INTEGER, contentType TEXT, charsetID INTEGER, path TEXT, syncState INTEGER, storageModTime INTEGER, storageHash TEXT, lastProcessedModificationTime INTEGER);
        INSERT INTO itemAttachments (itemID, parentItemID, linkMode, contentType, path) VALUES (3, 1, 0, 'application/pdf', NULL);

        CREATE TABLE itemAnnotations (itemID INTEGER PRIMARY KEY, parentItemID INTEGER, type INTEGER, authorName TEXT, text TEXT, comment TEXT, color TEXT, pageLabel TEXT, sortIndex TEXT, position TEXT, isExternal INTEGER);
        INSERT INTO itemAnnotations (itemID, parentItemID, type) VALUES (4, 3, 1);

        CREATE TABLE savedSearches (savedSearchID INTEGER PRIMARY KEY, savedSearchName TEXT, clientDateModified TEXT, libraryID INTEGER, key TEXT, version INTEGER, synced INTEGER);
        CREATE TABLE savedSearchConditions (savedSearchID INTEGER, searchConditionID INTEGER, condition TEXT, operator TEXT, value TEXT, required INTEGER);
        "#,
    )
    .unwrap();
    sqlite_path
}

fn session_with_library(library: &str) -> SessionState {
    SessionState {
        current_library: Some(serde_json::Value::String(library.to_string())),
        ..Default::default()
    }
}

// ─────────────────────────────── NOTE GET ───────────────────────────────

#[test]
fn test_get_note_valid_returns_full_item_representation() {
    let dir = TestDir::new("get-note-valid");
    let runtime = test_runtime(build_notes_fixture(dir.path()));
    let session = SessionState::default();

    let note = notes::get_note(&runtime, Some("NOTE0001"), &session).expect("note found");
    assert_eq!(note.type_name, "note");
    assert_eq!(note.key, "NOTE0001");
    assert_eq!(note.note_text, "Hello note");
    assert!(note.is_note);
    assert!(!note.is_attachment);
    assert!(!note.is_annotation);
}

#[test]
fn test_get_note_missing_ref_errors() {
    let dir = TestDir::new("get-note-missing-ref");
    let runtime = test_runtime(build_notes_fixture(dir.path()));
    let session = SessionState::default();

    let err = notes::get_note(&runtime, None, &session).unwrap_err();
    assert_eq!(err.to_string(), "Note reference required");
}

#[test]
fn test_get_note_not_found_errors() {
    let dir = TestDir::new("get-note-not-found");
    let runtime = test_runtime(build_notes_fixture(dir.path()));
    let session = SessionState::default();

    let err = notes::get_note(&runtime, Some("NOPE0000"), &session).unwrap_err();
    assert_eq!(err.to_string(), "Note not found: NOPE0000");
}

#[test]
fn test_get_note_non_note_item_errors() {
    let dir = TestDir::new("get-note-non-note");
    let runtime = test_runtime(build_notes_fixture(dir.path()));
    let session = SessionState::default();

    let err = notes::get_note(&runtime, Some("DOC00001"), &session).unwrap_err();
    assert_eq!(err.to_string(), "Item is not a note: DOC00001");

    let err = notes::get_note(&runtime, Some("ATT00001"), &session).unwrap_err();
    assert_eq!(err.to_string(), "Item is not a note: ATT00001");
}

#[test]
fn test_get_note_respects_current_library_scoping() {
    let dir = TestDir::new("get-note-scoping");
    let runtime = test_runtime(build_notes_fixture(dir.path()));

    // Without a current_library, the duplicated key is genuinely ambiguous.
    let no_library = SessionState::default();
    let err = notes::get_note(&runtime, Some("DUPNOTE1"), &no_library).unwrap_err();
    assert!(
        err.to_string().contains("Ambiguous item reference"),
        "expected ambiguity error, got {err}"
    );

    // session.current_library disambiguates it, exactly like every other item lookup.
    let lib2 = session_with_library("2");
    let note = notes::get_note(&runtime, Some("DUPNOTE1"), &lib2).expect("resolved via session");
    assert_eq!(note.library_id, 2);
    assert_eq!(note.note_text, "dup in lib2");

    let lib1 = session_with_library("1");
    let note = notes::get_note(&runtime, Some("DUPNOTE1"), &lib1).expect("resolved via session");
    assert_eq!(note.library_id, 1);
    assert_eq!(note.note_text, "dup in lib1");
}

#[test]
fn test_get_note_normalized_representation_matches_full_item_shape() {
    let dir = TestDir::new("get-note-shape");
    let runtime = test_runtime(build_notes_fixture(dir.path()));
    let session = SessionState::default();

    let via_get_note = notes::get_note(&runtime, Some("NOTE0001"), &session).unwrap();
    let via_get_item = catalog::get_item(&runtime, Some("NOTE0001"), &session).unwrap();

    // `get_note` must return the same full item representation `item get` would -- never an
    // invented, smaller note-specific shape.
    assert_eq!(
        serde_json::to_value(&via_get_note).unwrap(),
        serde_json::to_value(&via_get_item).unwrap()
    );

    let json = serde_json::to_value(&via_get_note).unwrap();
    for key in [
        "itemID",
        "key",
        "libraryID",
        "typeName",
        "fields",
        "creators",
        "tags",
        "isNote",
        "noteText",
        "notePreview",
    ] {
        assert!(json.get(key).is_some(), "expected key {key} in {json}");
    }
}

// ────────────────────────── ADD TARGETING ───────────────────────────────

fn bridge_ownership_ok() -> ScriptedResponse {
    ScriptedResponse::json(
        200,
        json!({"fork": "zotero-rust-cli", "id": "cli-bridge@cli-anything-rust.dev"}),
    )
}

fn note_add_success_response() -> ScriptedResponse {
    ScriptedResponse::json(
        200,
        json!({"key": "NEWNOTE1", "itemID": 999, "title": "Doc One"}),
    )
}

#[test]
fn test_add_note_valid_regular_parent_succeeds() {
    let dir = TestDir::new("add-note-valid");
    let runtime = test_runtime(build_notes_fixture(dir.path()));
    let session = SessionState::default();

    let server = ScriptedServer::start(vec![bridge_ownership_ok(), note_add_success_response()]);
    let bridge = JSBridgeClient::new(server.port);

    let result = notes::add_note(
        &runtime,
        &bridge,
        "DOC00001",
        notes::NoteInput::Text("Hello world"),
        None,
        &session,
    )
    .expect("add_note succeeds");

    assert_eq!(result.action, "note_add");
    assert_eq!(result.key.as_deref(), Some("NEWNOTE1"));
    assert_eq!(result.item_id, Some(999));
    assert_eq!(result.parent_item_key, "DOC00001");
    assert_eq!(result.parent_item_id, 1);
    assert_eq!(result.format, "text");
    assert_eq!(result.note_preview, "Hello world");

    let requests = server.finish();
    assert_eq!(
        requests.len(),
        2,
        "exactly one ownership probe + one write attempt"
    );
    let code = String::from_utf8_lossy(&requests[1].body);
    assert!(code.contains("new Zotero.Item('note')"));
    assert!(code.contains("Zotero.Items.getByLibraryAndKey"));
}

#[test]
fn test_add_note_rejects_note_parent() {
    let dir = TestDir::new("add-note-reject-note");
    let runtime = test_runtime(build_notes_fixture(dir.path()));
    let session = SessionState::default();
    let server = ScriptedServer::start(vec![]);
    let bridge = JSBridgeClient::new(server.port);

    let err = notes::add_note(
        &runtime,
        &bridge,
        "NOTE0001",
        notes::NoteInput::Text("x"),
        None,
        &session,
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Child notes can only be attached to top-level bibliographic items"
    );
    assert!(
        server.finish().is_empty(),
        "bridge must never be contacted when the parent type is rejected"
    );
}

#[test]
fn test_add_note_rejects_attachment_parent() {
    let dir = TestDir::new("add-note-reject-attachment");
    let runtime = test_runtime(build_notes_fixture(dir.path()));
    let session = SessionState::default();
    let server = ScriptedServer::start(vec![]);
    let bridge = JSBridgeClient::new(server.port);

    let err = notes::add_note(
        &runtime,
        &bridge,
        "ATT00001",
        notes::NoteInput::Text("x"),
        None,
        &session,
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Child notes can only be attached to top-level bibliographic items"
    );
    assert!(server.finish().is_empty());
}

#[test]
fn test_add_note_rejects_annotation_parent() {
    let dir = TestDir::new("add-note-reject-annotation");
    let runtime = test_runtime(build_notes_fixture(dir.path()));
    let session = SessionState::default();
    let server = ScriptedServer::start(vec![]);
    let bridge = JSBridgeClient::new(server.port);

    let err = notes::add_note(
        &runtime,
        &bridge,
        "ANN00001",
        notes::NoteInput::Text("x"),
        None,
        &session,
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Child notes can only be attached to top-level bibliographic items"
    );
    assert!(server.finish().is_empty());
}

#[test]
fn test_add_note_parent_not_found() {
    let dir = TestDir::new("add-note-parent-missing");
    let runtime = test_runtime(build_notes_fixture(dir.path()));
    let session = SessionState::default();
    let server = ScriptedServer::start(vec![]);
    let bridge = JSBridgeClient::new(server.port);

    let err = notes::add_note(
        &runtime,
        &bridge,
        "NOPE0000",
        notes::NoteInput::Text("x"),
        None,
        &session,
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "Item not found: NOPE0000");
    assert!(server.finish().is_empty());
}

// ──────────────────────────────── FILE ──────────────────────────────────

#[test]
fn test_add_note_from_utf8_file() {
    let dir = TestDir::new("add-note-file");
    let runtime = test_runtime(build_notes_fixture(dir.path()));
    let session = SessionState::default();

    let note_file = dir.path().join("note.txt");
    std::fs::write(&note_file, "Héllo wörld 你好 🎉").unwrap();
    let file_path_str = note_file.to_string_lossy().into_owned();

    let server = ScriptedServer::start(vec![bridge_ownership_ok(), note_add_success_response()]);
    let bridge = JSBridgeClient::new(server.port);

    let result = notes::add_note(
        &runtime,
        &bridge,
        "DOC00001",
        notes::NoteInput::File(&file_path_str),
        None,
        &session,
    )
    .expect("file-sourced note succeeds");

    assert_eq!(result.note_preview, "Héllo wörld 你好 🎉");
    server.finish();
}

#[test]
fn test_add_note_file_read_error_propagates() {
    let dir = TestDir::new("add-note-file-missing");
    let runtime = test_runtime(build_notes_fixture(dir.path()));
    let session = SessionState::default();

    let missing_path = dir.path().join("does-not-exist.txt");
    let missing_str = missing_path.to_string_lossy().into_owned();

    // The bridge must never be reached -- the file read fails before any Bridge call.
    let server = ScriptedServer::start(vec![]);
    let bridge = JSBridgeClient::new(server.port);

    let err = notes::add_note(
        &runtime,
        &bridge,
        "DOC00001",
        notes::NoteInput::File(&missing_str),
        None,
        &session,
    )
    .unwrap_err();
    // Matches Python's own exposure: an uncaught FileNotFoundError, not a friendly DomainError
    // message this crate invents on top of it.
    assert!(
        err.to_string().to_lowercase().contains("no such file")
            || err.to_string().to_lowercase().contains("cannot find"),
        "expected a raw io error, got {err}"
    );
    assert!(server.finish().is_empty());
}

// ─────────────────────────────── BRIDGE ─────────────────────────────────

#[test]
fn test_note_add_template_contains_expected_js_calls() {
    let js = bridge::templates::render_note_add(1, "PARENT1", "<p>hi</p>").unwrap();
    assert!(js.starts_with("const P = JSON.parse("));
    assert!(js.contains("Zotero.Items.getByLibraryAndKey"));
    assert!(js.contains("new Zotero.Item('note')"));
    assert!(js.contains("setNote"));
    assert!(js.contains("saveTx"));
}

#[test]
fn test_note_add_template_safely_encodes_special_characters() {
    let tricky = "line one\nline two\\ with \"quotes\" and 'apostrophes' and `backticks`";
    let js = bridge::templates::render_note_add(1, "PARENT1", tricky).unwrap();

    let line = js.lines().next().unwrap();
    assert!(line.starts_with("const P = JSON.parse("));
    let json_literal = &line["const P = JSON.parse(".len()..line.len() - 2];
    let parsed_json_str: String = serde_json::from_str(json_literal).expect("outer parses");
    let payload: serde_json::Value = serde_json::from_str(&parsed_json_str).expect("inner parses");

    assert_eq!(payload["noteHtml"].as_str().unwrap(), tricky);
}

#[test]
fn test_note_add_success_returns_raw_key_item_id_title() {
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({"key": "N1", "itemID": 42, "title": "Some Title"}),
        ),
    ]);
    let bridge = JSBridgeClient::new(server.port);

    let data = bridge.note_add(1, "PARENT1", "<p>hi</p>").expect("success");
    assert_eq!(data["key"], "N1");
    assert_eq!(data["itemID"], 42);
    assert_eq!(data["title"], "Some Title");
    server.finish();
}

#[test]
fn test_note_add_bridge_error_message_matches_python() {
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(500, json!({"error": "boom"})),
    ]);
    let bridge = JSBridgeClient::new(server.port);

    let err = bridge.note_add(1, "PARENT1", "<p>hi</p>").unwrap_err();
    assert_eq!(err.to_string(), "Failed to create note via JS bridge: boom");
    server.finish();
}

#[test]
fn test_note_add_non_object_response_matches_python_message() {
    // The JS template's own `'ERROR: parent item not found'` string return (parent vanished
    // between resolution and the write attempt) is a well-formed `ok: true` response whose
    // `data` is a string, not a dict -- Python's own accepted, slightly odd failure shape.
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::bridge_string(200, "ERROR: parent item not found"),
    ]);
    let bridge = JSBridgeClient::new(server.port);

    let err = bridge.note_add(1, "PARENT1", "<p>hi</p>").unwrap_err();
    assert_eq!(
        err.to_string(),
        "Unexpected JS Bridge response (expected dict, got str): ERROR: parent item not found"
    );
    server.finish();
}

#[test]
fn test_add_note_result_handles_missing_item_id_gracefully() {
    let dir = TestDir::new("add-note-missing-item-id");
    let runtime = test_runtime(build_notes_fixture(dir.path()));
    let session = SessionState::default();

    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(200, json!({"key": "N1", "title": "x"})),
    ]);
    let bridge = JSBridgeClient::new(server.port);

    let err = notes::add_note(
        &runtime,
        &bridge,
        "DOC00001",
        notes::NoteInput::Text("hi"),
        None,
        &session,
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Invalid note creation response: missing or invalid key/itemID"
    );
    server.finish();
}

#[test]
fn test_note_add_rejects_malformed_identity_responses() {
    let malformed_payloads = [
        json!({}),
        json!({"key": "ABC"}),
        json!({"itemID": 123}),
        json!({"key": "", "itemID": 123}),
        json!({"key": "ABC", "itemID": null}),
    ];

    for payload in malformed_payloads {
        let server = ScriptedServer::start(vec![
            bridge_ownership_ok(),
            ScriptedResponse::json(200, payload.clone()),
        ]);
        let bridge = JSBridgeClient::new(server.port);

        let err = bridge.note_add(1, "PARENT1", "<p>hi</p>").unwrap_err();
        assert_eq!(
            err.to_string(),
            "Invalid note creation response: missing or invalid key/itemID",
            "failed to reject payload: {payload}"
        );
        server.finish();
    }
}

#[test]
fn test_note_add_accepts_valid_identity_responses() {
    let valid_payloads = [
        (json!({"key": "ABC", "itemID": 123}), "ABC", 123i64),
        (json!({"key": "ABC", "itemID": 0}), "ABC", 0i64),
    ];

    for (payload, expected_key, expected_id) in valid_payloads {
        let server = ScriptedServer::start(vec![
            bridge_ownership_ok(),
            ScriptedResponse::json(200, payload.clone()),
        ]);
        let bridge = JSBridgeClient::new(server.port);

        let data = bridge
            .note_add(1, "PARENT1", "<p>hi</p>")
            .expect("should accept valid identity payload");
        assert_eq!(data["key"], expected_key);
        assert_eq!(data["itemID"], expected_id);
        server.finish();
    }
}

fn drain_request(stream: &mut std::net::TcpStream) {
    let mut raw = Vec::new();
    let mut temp = [0u8; 4096];
    loop {
        let n = stream.read(&mut temp).unwrap_or(0);
        if n == 0 {
            return;
        }
        raw.extend_from_slice(&temp[..n]);
        if raw.windows(4).any(|w| w == b"\r\n\r\n") {
            return;
        }
    }
}

#[test]
fn test_note_add_no_retry_after_ambiguous_transport_failure() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();

    let server_handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            drain_request(&mut stream);
            let body = r#"{"fork":"zotero-rust-cli","id":"cli-bridge@cli-anything-rust.dev"}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
        tx.send(()).unwrap();

        // The write attempt itself: accept, then drop without responding -- a genuine
        // transport-level failure, distinct from a well-formed error body.
        if let Ok((stream, _)) = listener.accept() {
            tx.send(()).unwrap();
            drop(stream);
        }
    });

    let bridge = JSBridgeClient::new(port);
    let err = bridge.note_add(1, "PARENT1", "<p>hi</p>").unwrap_err();
    assert!(
        err.to_string()
            .starts_with("Failed to create note via JS bridge:"),
        "unexpected error: {err}"
    );

    assert!(
        rx.recv_timeout(Duration::from_secs(2)).is_ok(),
        "the ownership probe connection must have arrived"
    );
    assert!(
        rx.recv_timeout(Duration::from_secs(2)).is_ok(),
        "exactly one write-attempt connection must have arrived"
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "no second write-attempt connection (i.e. no automatic retry) must arrive"
    );

    let _ = server_handle.join();
}

// ────────────────────────────── PREVIEW ─────────────────────────────────

#[test]
fn test_add_note_preview_strips_markup_and_decodes_entities() {
    let dir = TestDir::new("add-note-preview-text");
    let runtime = test_runtime(build_notes_fixture(dir.path()));
    let session = SessionState::default();

    let server = ScriptedServer::start(vec![bridge_ownership_ok(), note_add_success_response()]);
    let bridge = JSBridgeClient::new(server.port);

    let result = notes::add_note(
        &runtime,
        &bridge,
        "DOC00001",
        notes::NoteInput::Text("Tom & Jerry <3"),
        None,
        &session,
    )
    .expect("succeeds");

    // format=text HTML-escapes `&`/`<` as part of normalization; the preview must come back
    // through HTML-to-text (decoding entities, stripping the wrapping <p>), never the raw
    // escaped/tagged markup.
    assert_eq!(result.note_preview, "Tom & Jerry <3");
    assert!(!result.note_preview.contains("&amp;"));
    assert!(!result.note_preview.contains("<p>"));
    server.finish();
}

#[test]
fn test_add_note_preview_from_markdown_strips_tags() {
    let dir = TestDir::new("add-note-preview-markdown");
    let runtime = test_runtime(build_notes_fixture(dir.path()));
    let session = SessionState::default();

    let server = ScriptedServer::start(vec![bridge_ownership_ok(), note_add_success_response()]);
    let bridge = JSBridgeClient::new(server.port);

    let result = notes::add_note(
        &runtime,
        &bridge,
        "DOC00001",
        notes::NoteInput::Text("Some **bold** text"),
        Some("markdown"),
        &session,
    )
    .expect("succeeds");

    assert_eq!(result.format, "markdown");
    assert_eq!(result.note_preview, "Some bold text");
    assert!(!result.note_preview.contains("<strong>"));
    server.finish();
}
