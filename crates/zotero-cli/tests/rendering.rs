//! Rendering / Export slice CLI integration tests: exercises the actual `zotero-cli` binary
//! against a scripted mock Local API server for the four commands this slice ports from
//! `core/rendering.py` (`item citation|bibliography|export`) and `zotero_cli.py`'s
//! `export_bib_command` (`export bib`).
//!
//! Every fixture SQLite read is real (via `build_fixture_sqlite`/direct inserts); every Zotero
//! surface is a scripted mock -- no live Zotero, no SQLite writes, no item mutation anywhere in
//! this file.

#[path = "common/mod.rs"]
mod common;

use common::{build_fixture_sqlite, run_cli, ScriptedResponse, ScriptedServer, TestDir};
use serde_json::json;
use std::path::Path;
use std::process::Command;

fn connector_ping_ok() -> ScriptedResponse {
    ScriptedResponse::json(200, json!({}))
}

fn local_api_probe_available() -> ScriptedResponse {
    ScriptedResponse::json(200, json!({}))
}

fn local_api_probe_unavailable() -> ScriptedResponse {
    ScriptedResponse::json(403, json!({"message": "local API disabled"}))
}

fn text_response(status: u16, body: &str) -> ScriptedResponse {
    ScriptedResponse::Http {
        status,
        headers: Vec::new(),
        body: body.as_bytes().to_vec(),
    }
}

/// Runs the CLI without forcing `--json`, for human-mode output assertions -- mirrors
/// `phase7_cli_integration.rs`'s `run_cli_human`.
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
        // Same per-test isolation `common::run_cli` documents: never fall back to the
        // developer's real `~/.config/cli-anything-zotero` session, and never let an automated
        // run reach the lifecycle helper's Zotero-launch path.
        .env("CLI_ANYTHING_ZOTERO_STATE_DIR", data_dir.join("cli-state"))
        .env("ZOTERO_CLI_NO_AUTOLAUNCH", "1")
        .env_remove("ZOTERO_LOCAL_API_KEY");
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let output = command.output().expect("failed to run zotero-cli binary");
    let code = output.status.code().unwrap_or(-1);
    (code, String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Runs the CLI with `--json` but, unlike `common::run_cli`, does not assume stdout is JSON --
/// for asserting `clap`'s own usage-error exit code (2), which (per `error.rs`'s documented
/// accepted divergence) prints plain text to stderr rather than the `{"error": ...}` shape,
/// regardless of `--json`.
fn run_cli_raw(data_dir: &Path, port: u16, args: &[&str]) -> (i32, String, String) {
    let mut command = Command::new(common::bin_path());
    command
        .arg("--json")
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .env("ZOTERO_HTTP_PORT", port.to_string())
        // Same per-test isolation `common::run_cli` documents: never fall back to the
        // developer's real `~/.config/cli-anything-zotero` session, and never let an automated
        // run reach the lifecycle helper's Zotero-launch path.
        .env("CLI_ANYTHING_ZOTERO_STATE_DIR", data_dir.join("cli-state"))
        .env("ZOTERO_CLI_NO_AUTOLAUNCH", "1")
        .env_remove("ZOTERO_LOCAL_API_KEY");
    let output = command.output().expect("failed to run zotero-cli binary");
    let code = output.status.code().unwrap_or(-1);
    (
        code,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Adds a `note`-typed item and a top-level collection membership row (item + note) to the shared
/// fixture, for the `export bib --collection` filtering tests.
fn build_fixture_with_collection_members(dir: &Path) -> std::path::PathBuf {
    let sqlite_path = build_fixture_sqlite(dir);
    let conn = rusqlite::Connection::open(&sqlite_path).unwrap();
    conn.execute_batch(
        r#"
        INSERT INTO itemTypes VALUES (3, 'note', NULL, 1);
        INSERT INTO items VALUES (4, 3, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'NOTEKEY1', 1, 1);
        INSERT INTO itemNotes (itemID, parentItemID, note, title) VALUES (4, NULL, '<div>Standalone note</div>', NULL);
        INSERT INTO collectionItems VALUES (1, 1, 0);
        INSERT INTO collectionItems VALUES (1, 2, 1);
        INSERT INTO collectionItems VALUES (1, 4, 2);
        "#,
    )
    .unwrap();
    sqlite_path
}

// ── item citation ──────────────────────────────────────────────────────────

#[test]
fn item_citation_success_json_mode() {
    let dir = TestDir::new("item-citation-json");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        ScriptedResponse::json(200, json!({"citation": "(Doe, 2020)"})),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "citation", "ITEM0001", "--style", "apa"],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["itemKey"], "ITEM0001");
    assert_eq!(value["libraryID"], 1);
    assert_eq!(value["style"], "apa");
    assert_eq!(value["locale"], serde_json::Value::Null);
    assert_eq!(value["linkwrap"], false);
    assert_eq!(value["citation"], "(Doe, 2020)");
    assert_eq!(
        requests[2].path,
        "/api/users/0/items/ITEM0001?format=json&include=citation&style=apa"
    );
}

#[test]
fn item_citation_human_mode_prints_bare_citation_text() {
    let dir = TestDir::new("item-citation-human");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        ScriptedResponse::json(200, json!({"citation": "(Doe, 2020)"})),
    ]);

    let (code, stdout) = run_cli_human(
        dir.path(),
        server.port,
        &[],
        &["item", "citation", "ITEM0001"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={stdout}");
    assert_eq!(stdout.trim(), "(Doe, 2020)");
}

#[test]
fn item_citation_unicode_style_and_content_round_trip() {
    let dir = TestDir::new("item-citation-unicode");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        ScriptedResponse::json(200, json!({"citation": "(Müller & 张, 2020) — “Über α→β”"})),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "item",
            "citation",
            "ITEM0001",
            "--style",
            "gb-t-7714-2015",
            "--locale",
            "zh-CN",
        ],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["style"], "gb-t-7714-2015");
    assert_eq!(value["locale"], "zh-CN");
    assert_eq!(value["citation"], "(Müller & 张, 2020) — “Über α→β”");
}

#[test]
fn item_citation_linkwrap_sets_query_param() {
    let dir = TestDir::new("item-citation-linkwrap");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        ScriptedResponse::json(200, json!({"citation": "(Doe, 2020)"})),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "citation", "ITEM0001", "--linkwrap"],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["linkwrap"], true);
    assert!(requests[2].path.contains("linkwrap=1"));
}

// ── item bibliography ────────────────────────────────────────────────────

#[test]
fn item_bibliography_success_json_mode() {
    let dir = TestDir::new("item-bib-json");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        ScriptedResponse::json(200, json!({"bib": "Doe, J. (2020). Test Item One."})),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "bibliography", "ITEM0001"],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["bibliography"], "Doe, J. (2020). Test Item One.");
    assert!(requests[2].path.contains("include=bib"));
}

#[test]
fn item_bibliography_human_mode_prints_bare_bib_text() {
    let dir = TestDir::new("item-bib-human");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        ScriptedResponse::json(200, json!([{"bib": "Doe, J. (2020). Test Item One."}])),
    ]);

    let (code, stdout) = run_cli_human(
        dir.path(),
        server.port,
        &[],
        &["item", "bibliography", "ITEM0001"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={stdout}");
    assert_eq!(stdout.trim(), "Doe, J. (2020). Test Item One.");
}

#[test]
fn item_bibliography_empty_array_payload_yields_null_bibliography() {
    let dir = TestDir::new("item-bib-empty-array");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        ScriptedResponse::json(200, json!([])),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "bibliography", "ITEM0001"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["bibliography"], serde_json::Value::Null);
}

// ── item export ─────────────────────────────────────────────────────────

#[test]
fn item_export_supported_formats_round_trip_content() {
    for fmt in [
        "ris", "bibtex", "biblatex", "csljson", "csv", "mods", "refer",
    ] {
        let dir = TestDir::new(&format!("item-export-{fmt}"));
        build_fixture_sqlite(dir.path());

        let body = format!("FIXTURE-CONTENT-{fmt}");
        let server = ScriptedServer::start(vec![
            connector_ping_ok(),
            local_api_probe_available(),
            text_response(200, &body),
        ]);

        let (code, value) = run_cli(
            dir.path(),
            server.port,
            &[],
            &["item", "export", "ITEM0001", "--format", fmt],
        );
        let requests = server.finish();

        assert_eq!(code, 0, "fmt={fmt} stdout={value}");
        assert_eq!(value["format"], fmt);
        assert_eq!(value["content"], body);
        assert!(
            requests[2]
                .path
                .ends_with(&format!("/items/ITEM0001?format={fmt}")),
            "fmt={fmt} path={}",
            requests[2].path
        );
    }
}

#[test]
fn item_export_human_mode_prints_bare_content() {
    let dir = TestDir::new("item-export-human");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        text_response(200, "TY  - JOUR\nTI  - Test Item One\nER  - \n"),
    ]);

    let (code, stdout) = run_cli_human(
        dir.path(),
        server.port,
        &[],
        &["item", "export", "ITEM0001", "--format", "ris"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={stdout}");
    assert_eq!(stdout, "TY  - JOUR\nTI  - Test Item One\nER  - \n\n");
}

#[test]
fn item_export_unsupported_format_is_rejected_before_any_http_call() {
    let dir = TestDir::new("item-export-unsupported-format");
    build_fixture_sqlite(dir.path());

    // Zero scripted responses -- `--format docx` must be rejected by `clap`'s choice
    // validation before the runtime (and its 2-call connector/Local-API prelude) is ever built.
    let server = ScriptedServer::start(vec![]);

    let (code, stdout, stderr) = run_cli_raw(
        dir.path(),
        server.port,
        &["item", "export", "ITEM0001", "--format", "docx"],
    );
    let requests = server.finish();

    assert_eq!(code, 2, "stdout={stdout} stderr={stderr}");
    assert!(stdout.is_empty());
    assert!(
        requests.is_empty(),
        "expected zero HTTP calls, got {requests:?}"
    );
}

// ── malformed reference / Local API unavailable / malformed response ──────

#[test]
fn item_citation_malformed_item_ref_is_domain_error() {
    let dir = TestDir::new("item-citation-malformed-ref");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "citation", "NOSUCHKEY"],
    );
    let requests = server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert_eq!(value["error"], "Item not found: NOSUCHKEY");
    assert_eq!(
        requests.len(),
        2,
        "no rendering HTTP call should fire for an unresolved item"
    );
}

#[test]
fn item_bibliography_local_api_unavailable_is_domain_error() {
    let dir = TestDir::new("item-bib-local-api-down");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "bibliography", "ITEM0001"],
    );
    let requests = server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert_eq!(
        value["error"],
        "Zotero Local API is not available. Start Zotero and enable `extensions.zotero.httpServer.localAPI.enabled` first."
    );
    assert_eq!(requests.len(), 2);
}

#[test]
fn item_citation_local_api_malformed_response_is_domain_error() {
    let dir = TestDir::new("item-citation-malformed-response");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        text_response(200, "not valid json"),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "citation", "ITEM0001"],
    );
    server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert!(
        !value["error"].as_str().unwrap_or_default().is_empty(),
        "expected a non-empty clean error message, got {value}"
    );
}

// ── export bib ──────────────────────────────────────────────────────────

#[test]
fn export_bib_items_target_writes_output_file() {
    let dir = TestDir::new("export-bib-items");
    build_fixture_sqlite(dir.path());
    let out_path = dir.path().join("refs.bib");

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        text_response(200, "@article{doe2020,\n  title={Test Item One}\n}"),
        text_response(200, "@article{doe2021,\n  title={Test Item Two}\n}"),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "export",
            "bib",
            "--items",
            "ITEM0001,ITEM0002",
            "--format",
            "bibtex",
            "--output",
            out_path.to_str().unwrap(),
        ],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["action"], "export-bib");
    assert_eq!(value["format"], "bibtex");
    assert_eq!(value["item_count"], 2);
    assert_eq!(value["source"]["type"], "items");
    assert_eq!(value["source"]["refs"], json!(["ITEM0001", "ITEM0002"]));
    assert_eq!(requests.len(), 4);

    let written = std::fs::read_to_string(&out_path).unwrap();
    assert_eq!(
        written,
        "@article{doe2020,\n  title={Test Item One}\n}\n\n@article{doe2021,\n  title={Test Item Two}\n}\n"
    );
}

#[test]
fn export_bib_collection_target_filters_non_bibliographic_items() {
    let dir = TestDir::new("export-bib-collection");
    build_fixture_with_collection_members(dir.path());
    let out_path = dir.path().join("collection-refs.bib");

    // Only the 2 real document items (ITEM0001, ITEM0002) are exported; NOTEKEY1 is filtered out.
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        text_response(200, "@article{one,\n  title={Test Item One}\n}"),
        text_response(200, "@article{two,\n  title={Test Item Two}\n}"),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "export",
            "bib",
            "--collection",
            "COLLE001",
            "--format",
            "biblatex",
            "--output",
            out_path.to_str().unwrap(),
        ],
    );
    let requests = server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["item_count"], 2);
    assert_eq!(value["source"]["type"], "collection");
    assert_eq!(value["source"]["collection"]["key"], "COLLE001");
    assert_eq!(requests.len(), 4);
    assert!(std::fs::read_to_string(&out_path)
        .unwrap()
        .contains("Test Item One"));
}

#[test]
fn export_bib_empty_collection_is_domain_error() {
    let dir = TestDir::new("export-bib-empty-collection");
    build_fixture_sqlite(dir.path());
    let out_path = dir.path().join("empty.bib");

    // Second collection fixture ("EXISTC1") has zero member items -- no rendering calls needed
    // for an empty refs list, so only the connector/Local-API prelude is scripted.
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "export",
            "bib",
            "--collection",
            "EXISTC1",
            "--output",
            out_path.to_str().unwrap(),
        ],
    );
    let requests = server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert_eq!(value["error"], "No exportable Zotero items found.");
    assert!(!out_path.exists(), "no file should be written on failure");
    assert_eq!(requests.len(), 2);
}

#[test]
fn export_bib_requires_exactly_one_of_items_or_collection() {
    let dir = TestDir::new("export-bib-both-selectors");
    build_fixture_sqlite(dir.path());
    let out_path = dir.path().join("out.bib");

    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "export",
            "bib",
            "--items",
            "ITEM0001",
            "--collection",
            "COLLE001",
            "--output",
            out_path.to_str().unwrap(),
        ],
    );
    server.finish();
    assert_eq!(code, 1, "stdout={value}");
    assert_eq!(
        value["error"],
        "Pass exactly one of --items or --collection."
    );

    let server2 = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let (code2, value2) = run_cli(
        dir.path(),
        server2.port,
        &[],
        &["export", "bib", "--output", out_path.to_str().unwrap()],
    );
    server2.finish();
    assert_eq!(code2, 1, "stdout={value2}");
    assert_eq!(
        value2["error"],
        "Pass exactly one of --items or --collection."
    );
}

#[test]
fn export_bib_missing_output_flag_is_usage_error() {
    let dir = TestDir::new("export-bib-missing-output");
    build_fixture_sqlite(dir.path());

    // Zero scripted responses -- a missing required `--output` must be rejected by `clap`
    // before any runtime/HTTP work happens.
    let server = ScriptedServer::start(vec![]);
    let (code, stdout, stderr) = run_cli_raw(
        dir.path(),
        server.port,
        &["export", "bib", "--items", "ITEM0001"],
    );
    let requests = server.finish();

    assert_eq!(code, 2, "stdout={stdout} stderr={stderr}");
    assert!(stdout.is_empty());
    assert!(requests.is_empty());
}

#[test]
fn export_bib_local_api_unavailable_is_domain_error() {
    let dir = TestDir::new("export-bib-local-api-down");
    build_fixture_sqlite(dir.path());
    let out_path = dir.path().join("out.bib");

    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);
    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "export",
            "bib",
            "--items",
            "ITEM0001",
            "--output",
            out_path.to_str().unwrap(),
        ],
    );
    server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert!(value["error"]
        .as_str()
        .unwrap_or_default()
        .contains("Zotero Local API is not available"));
    assert!(!out_path.exists());
}

/// Output paths with spaces and a nested directory that doesn't exist yet -- proves
/// `--output`'s parent-directory creation and that the path is used byte-for-byte, unquoted
/// spaces and all. Directory/file names deliberately avoid characters illegal on Windows
/// (`: * ? " < > |`) so this test stays portable across all 4 target platforms.
#[test]
fn export_bib_output_path_with_spaces_and_new_parent_dir() {
    let dir = TestDir::new("export-bib-spaces");
    build_fixture_sqlite(dir.path());
    let out_dir = dir.path().join("dir with spaces");
    let out_path = out_dir.join("my refs file.bib");

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        text_response(200, "@article{one,\n  title={Test Item One}\n}"),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "export",
            "bib",
            "--items",
            "ITEM0001",
            "--output",
            out_path.to_str().unwrap(),
        ],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert!(out_path.exists());
    assert_eq!(
        std::fs::read_to_string(&out_path).unwrap(),
        "@article{one,\n  title={Test Item One}\n}\n"
    );
}
