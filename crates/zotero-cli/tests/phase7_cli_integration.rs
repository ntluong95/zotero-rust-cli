//! Phase 7 CLI integration tests: exercises the actual `zotero-cli` binary against a scripted
//! mock server for all 18 commands wired by this slice (`import file|json|doi|pmid`,
//! `item find-pdf|fetch-pdf|search-fulltext|search-annotations|annotations`,
//! `collection find-pdfs|fetch-pdfs`, `note get|add`, `add doi|arxiv|file|bibtex|url`).
//!
//! Backend correctness (partial-success accounting, cascade ordering, resume-state format, etc.)
//! is already covered by each module's own dedicated test file (`tests/add_import.rs`,
//! `tests/import_core.rs`, `tests/pdf_cascade.rs`, `tests/pdf_fetch.rs`, `tests/notes.rs`,
//! `tests/fulltext.rs`, `tests/annotations.rs`). These tests instead prove the CLI-dispatch layer
//! this slice adds: argument parsing, the correct backend function gets called with the correct
//! arguments, and the exit-code/JSON-shape contract at the process boundary.

#[path = "common/mod.rs"]
mod common;

use common::{build_fixture_sqlite, run_cli, ScriptedResponse, ScriptedServer, TestDir};
use serde_json::json;
use std::path::Path;
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

/// Runs the CLI without forcing `--json` (unlike `common::run_cli`), for human-mode output
/// assertions (e.g. `note get`'s bare-text quirk).
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

/// Adds a `note`-typed item (key `NOTEKEY`) to the shared fixture schema, matching the Python
/// golden fixture's `note get` shape.
fn build_fixture_with_note(dir: &Path) -> std::path::PathBuf {
    let sqlite_path = build_fixture_sqlite(dir);
    let conn = rusqlite::Connection::open(&sqlite_path).unwrap();
    conn.execute_batch(
        r#"
        INSERT INTO itemTypes VALUES (3, 'note', NULL, 1);
        INSERT INTO items VALUES (4, 3, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'NOTEKEY', 1, 1);
        INSERT INTO itemNotes (itemID, parentItemID, note, title) VALUES (4, 1, '<div>Example note</div>', NULL);
        "#,
    )
    .unwrap();
    sqlite_path
}

// ── import file / import json ──────────────────────────────────────────────

#[test]
fn import_file_wires_connector_and_reports_success() {
    let dir = TestDir::new("import-file");
    build_fixture_sqlite(dir.path());
    let ris_path = dir.path().join("sample.ris");
    std::fs::write(&ris_path, "TY  - JOUR\nTI  - Fixture RIS\nER  - \n").unwrap();

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        ScriptedResponse::Http {
            status: 201,
            headers: Vec::new(),
            body: serde_json::to_vec(
                &json!([{"id": "imported-1", "itemType": "journalArticle", "title": "Fixture RIS"}]),
            )
            .unwrap(),
        },
        ScriptedResponse::json(200, json!({})),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "import",
            "file",
            ris_path.to_str().unwrap(),
            "--collection",
            "L1",
        ],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["action"], "import_file");
    assert_eq!(value["status"], "success");
    assert_eq!(value["imported_count"], 1);
    assert!(requests[2].path.starts_with("/connector/import"));
    assert_eq!(requests[3].path, "/connector/updateSession");
}

#[test]
fn import_json_wires_save_items_and_reports_success() {
    let dir = TestDir::new("import-json");
    build_fixture_sqlite(dir.path());
    let json_path = dir.path().join("sample.json");
    std::fs::write(
        &json_path,
        json!({"items": [{"itemType": "journalArticle", "title": "Fixture JSON"}]}).to_string(),
    )
    .unwrap();

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        ScriptedResponse::Http {
            status: 201,
            headers: Vec::new(),
            body: Vec::new(),
        },
        ScriptedResponse::json(200, json!({})),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "import",
            "json",
            json_path.to_str().unwrap(),
            "--collection",
            "L1",
        ],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["action"], "import_json");
    assert_eq!(value["status"], "success");
    assert_eq!(requests[2].path, "/connector/saveItems");
}

// ── import doi ──────────────────────────────────────────────────────────────

#[test]
fn import_doi_dedupes_then_translator_imports() {
    let dir = TestDir::new("import-doi");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        ScriptedResponse::json(200, json!([])), // find_items_by_doi: no dedupe hit
        ScriptedResponse::json(
            200,
            json!({"ok": true, "code": "IMPORTED", "key": "NEWKEY01", "title": "New Item", "DOI": "10.1000/newdoi", "source": "zotero-translator"}),
        ),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["import", "doi", "10.1000/newdoi"],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["action"], "import_doi");
    assert_eq!(value["status"], "success");
    assert_eq!(value["key"], "NEWKEY01");
    assert!(requests
        .iter()
        .all(|r| r.path == "/connector/ping" || r.path == "/api/" || r.path == "/cli-bridge/eval"));
}

// ── import pmid (frozen quirk: library_id MUST be 1, session override ignored) ─

#[test]
fn import_pmid_always_uses_library_id_one_regardless_of_session() {
    let dir = TestDir::new("import-pmid");
    build_fixture_sqlite(dir.path());
    let state_dir = dir.path().join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(
        state_dir.join("session.json"),
        json!({"current_library": 42}).to_string(),
    )
    .unwrap();

    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({"ok": true, "code": "IMPORTED", "key": "PMIDKEY1", "title": "PMID Item", "DOI": "", "source": "zotero-translator"}),
        ),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[("CLI_ANYTHING_ZOTERO_STATE_DIR", state_dir.to_str().unwrap())],
        &["import", "pmid", "12345678"],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["key"], "PMIDKEY1");
    let eval_body = String::from_utf8_lossy(&requests[1].body);
    assert!(
        eval_body.contains("\\\"libraryID\\\":1"),
        "expected hardcoded library id 1 in eval body, got: {eval_body}"
    );
}

// ── item find-pdf ───────────────────────────────────────────────────────────

#[test]
fn item_find_pdf_found_reports_success_exit_zero() {
    let dir = TestDir::new("item-find-pdf-found");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::bridge_string(200, "FOUND: ATT0001"),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "find-pdf", "ITEM0001", "--timeout", "1"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["action"], "item_find_pdf");
    assert_eq!(value["ok"], true);
    assert_eq!(value["code"], "FOUND");
    assert_eq!(value["attachment_key"], "ATT0001");
}

#[test]
fn item_find_pdf_not_found_reports_exit_one() {
    let dir = TestDir::new("item-find-pdf-not-found");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::bridge_string(200, "NOT_FOUND: no PDF available for Test Item One"),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "find-pdf", "ITEM0001"],
    );
    server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "NOT_FOUND");
}

// ── item fetch-pdf ──────────────────────────────────────────────────────────

#[test]
fn item_fetch_pdf_already_has_pdf_short_circuits_without_bridge_calls() {
    let dir = TestDir::new("item-fetch-pdf");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "fetch-pdf", "ITEM0003"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["status"], "already_has_pdf");
    assert_eq!(value["code"], "ALREADY_HAS_PDF");
    assert_eq!(value["source"], "existing");
}

// ── item search-fulltext / search-annotations / annotations ────────────────

#[test]
fn item_search_fulltext_passes_query_and_limit_through() {
    let dir = TestDir::new("search-fulltext");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(200, json!([{"key": "ATT0001", "title": "PDF", "date": ""}])),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "search-fulltext", "Sample", "--limit", "5"],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value[0]["key"], "ATT0001");
    let eval_body = String::from_utf8_lossy(&requests[1].body);
    assert!(eval_body.contains("Sample"));
    assert!(eval_body.contains("limit\\\":5") || eval_body.contains("\"limit\":5"));
}

#[test]
fn item_search_annotations_passes_colors_through() {
    let dir = TestDir::new("search-annotations");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(200, json!([])),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "item",
            "search-annotations",
            "--color",
            "yellow",
            "--limit",
            "5",
        ],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value, json!([]));
    let eval_body = String::from_utf8_lossy(&requests[1].body);
    assert!(eval_body.contains("yellow"));
}

/// Frozen quirk: `item annotations` may return a bare `"ERROR: ..."` string with exit code 0
/// (a transport-level success carrying an application-level error string, not a transport
/// failure or a `{"ok": false, ...}` structured failure).
#[test]
fn item_annotations_bare_error_string_is_exit_zero() {
    let dir = TestDir::new("annotations-bare-error");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::bridge_string(200, "ERROR: item NOPE0001 not found"),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "annotations", "NOPE0001"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value, "ERROR: item NOPE0001 not found");
}

// ── collection find-pdfs / fetch-pdfs ───────────────────────────────────────

#[test]
fn collection_find_pdfs_unwraps_transport_envelope_on_success() {
    let dir = TestDir::new("collection-find-pdfs");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({"ok": true, "total": 0, "missing": [], "missing_count": 0}),
        ),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "collection",
            "find-pdfs",
            "COLLE001",
            "--timeout-per-item",
            "1",
        ],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    // Unwrapped to the inner summary (not the {ok,data,error} transport wrapper).
    assert_eq!(value["collection"], "COLLE001");
    assert_eq!(value["checked"], 0);
    assert!(value.get("data").is_none());
}

#[test]
fn collection_find_pdfs_reports_transport_failure_as_exit_one() {
    let dir = TestDir::new("collection-find-pdfs-fail");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({"ok": false, "error": "collection COLLE001 not found"}),
        ),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["collection", "find-pdfs", "COLLE001"],
    );
    server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert_eq!(value["ok"], false);
    assert_eq!(value["data"], serde_json::Value::Null);
}

#[test]
fn collection_fetch_pdfs_reports_list_failure_as_exit_one() {
    let dir = TestDir::new("collection-fetch-pdfs");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({"ok": false, "error": "collection COLLE001 not found"}),
        ),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "collection",
            "fetch-pdfs",
            "COLLE001",
            "--sources",
            "zotero",
        ],
    );
    server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert_eq!(value["action"], "collection_fetch_pdfs");
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "LIST_FAILED");
}

// ── note get / note add ─────────────────────────────────────────────────────

#[test]
fn note_get_json_mode_returns_the_full_item_shape() {
    let dir = TestDir::new("note-get-json");
    build_fixture_with_note(dir.path());
    let server = ScriptedServer::start(vec![]);

    let (code, value) = run_cli(dir.path(), server.port, &[], &["note", "get", "NOTEKEY"]);
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["key"], "NOTEKEY");
    assert_eq!(value["typeName"], "note");
    assert_eq!(value["noteText"], "Example note");
}

#[test]
fn note_get_human_mode_prints_bare_note_text() {
    let dir = TestDir::new("note-get-human");
    build_fixture_with_note(dir.path());
    let server = ScriptedServer::start(vec![]);

    let (code, stdout) = run_cli_human(dir.path(), server.port, &[], &["note", "get", "NOTEKEY"]);
    server.finish();

    assert_eq!(code, 0, "stdout={stdout}");
    assert_eq!(stdout.trim(), "Example note");
}

#[test]
fn note_add_wires_text_and_format_through() {
    let dir = TestDir::new("note-add");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({"key": "MOCKNOTE", "itemID": 99999, "title": "Test Item One"}),
        ),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["note", "add", "ITEM0001", "--text", "Fixture note"],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["action"], "note_add");
    assert_eq!(value["key"], "MOCKNOTE");
    assert_eq!(value["parentItemKey"], "ITEM0001");
    assert_eq!(value["format"], "text");
    assert_eq!(value["notePreview"], "Fixture note");
    let eval_body = String::from_utf8_lossy(&requests[3].body);
    assert!(eval_body.contains("Fixture note"));
}

#[test]
fn note_add_requires_exactly_one_of_text_or_file() {
    let dir = TestDir::new("note-add-conflict");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![]);

    let (code, value) = run_cli(dir.path(), server.port, &[], &["note", "add", "ITEM0001"]);
    server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert!(value["error"]
        .as_str()
        .unwrap_or_default()
        .contains("Provide exactly one of"));
}

// ── add doi / add arxiv / add file / add bibtex / add url ──────────────────

#[test]
fn add_doi_dedupes_then_translator_imports() {
    let dir = TestDir::new("add-doi");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        bridge_ownership_ok(),
        ScriptedResponse::json(200, json!([])),
        ScriptedResponse::json(
            200,
            json!({"ok": true, "code": "IMPORTED", "key": "NEWKEY02", "title": "New Item", "DOI": "10.1000/adddoi", "source": "zotero-translator"}),
        ),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["add", "doi", "10.1000/adddoi", "--no-fetch-pdf"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["action"], "add_doi");
    assert_eq!(value["ok"], true);
    assert_eq!(value["key"], "NEWKEY02");
}

#[test]
fn add_arxiv_rejects_an_invalid_id_without_any_bridge_calls() {
    let dir = TestDir::new("add-arxiv-invalid");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["add", "arxiv", "not-an-arxiv-id"],
    );
    server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert_eq!(value["action"], "add_arxiv");
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "INVALID_ARXIV");
}

#[test]
fn add_file_routes_a_ris_file_through_the_connector() {
    let dir = TestDir::new("add-file");
    build_fixture_sqlite(dir.path());
    let ris_path = dir.path().join("sample.ris");
    std::fs::write(&ris_path, "TY  - JOUR\nTI  - Fixture RIS\nER  - \n").unwrap();

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        ScriptedResponse::Http {
            status: 201,
            headers: Vec::new(),
            body: serde_json::to_vec(
                &json!([{"id": "imported-1", "itemType": "journalArticle", "title": "Fixture RIS"}]),
            )
            .unwrap(),
        },
        ScriptedResponse::json(200, json!({})),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "add",
            "file",
            ris_path.to_str().unwrap(),
            "--collection",
            "L1",
        ],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["action"], "add_file");
    assert_eq!(value["ok"], true);
}

#[test]
fn add_bibtex_routes_through_the_connector_import_path() {
    let dir = TestDir::new("add-bibtex");
    build_fixture_sqlite(dir.path());
    let bib_path = dir.path().join("sample.bib");
    std::fs::write(&bib_path, "@article{fixture,title={Fixture BibTeX}}\n").unwrap();

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_unavailable(),
        ScriptedResponse::Http {
            status: 201,
            headers: Vec::new(),
            body: serde_json::to_vec(
                &json!([{"id": "imported-1", "itemType": "journalArticle", "title": "Imported Sample"}]),
            )
            .unwrap(),
        },
        ScriptedResponse::json(200, json!({})),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "add",
            "bibtex",
            bib_path.to_str().unwrap(),
            "--collection",
            "L1",
        ],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["action"], "add_bibtex");
    assert_eq!(value["ok"], true);
    assert_eq!(value["imported_count"], 1);
}

#[test]
fn add_url_rejects_an_empty_url_without_any_bridge_calls() {
    let dir = TestDir::new("add-url-empty");
    build_fixture_sqlite(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, value) = run_cli(dir.path(), server.port, &[], &["add", "url", ""]);
    server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert_eq!(value["action"], "add_url");
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], "INVALID_URL");
}
