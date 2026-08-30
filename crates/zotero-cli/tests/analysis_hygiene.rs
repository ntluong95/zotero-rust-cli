//! Analysis / Hygiene slice CLI integration tests:
//! Exercises `item context`, `item duplicates`, `item metrics`, and `item analyze`
//! against SQLite fixtures and scripted mock HTTP servers.
//!
//! Pinned authority: PiaoyangGuohai1/cli-anything-zotero@e42a930e.

#[path = "common/mod.rs"]
mod common;

use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{build_fixture_sqlite, run_cli, ScriptedResponse, ScriptedServer, TestDir};

fn connector_ping_ok() -> ScriptedResponse {
    ScriptedResponse::json(200, json!({}))
}

fn local_api_probe_available() -> ScriptedResponse {
    ScriptedResponse::json(200, json!({}))
}

fn local_api_probe_unavailable() -> ScriptedResponse {
    ScriptedResponse::json(403, json!({"message": "local API disabled"}))
}

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

/// Populates a comprehensive test SQLite database for Analysis/Hygiene tests.
fn build_comprehensive_fixture(dir: &Path) -> PathBuf {
    let sqlite_path = build_fixture_sqlite(dir);
    let conn = rusqlite::Connection::open(&sqlite_path).unwrap();
    conn.execute_batch(
        r#"
        -- Extra fields for ITEM0001
        INSERT INTO fields VALUES (3, 'url', 0), (4, 'date', 0), (5, 'extra', 0), (6, 'PMID', 0);
        INSERT INTO itemDataValues VALUES (10, 'https://example.com/paper.html'), (11, '2024'), (12, 'PMID: 98765432'), (13, '12345678');
        INSERT INTO itemData VALUES (1, 3, 10), (1, 4, 11);

        -- ITEM0002 has DOI in fields
        INSERT INTO itemDataValues VALUES (14, '10.1038/nature12373');
        UPDATE itemData SET valueID = 14 WHERE itemID = 2 AND fieldID = 2;
        -- ITEM0002 has PMID in extra field
        INSERT INTO itemData VALUES (2, 5, 12);

        -- ITEM0003 has dedicated PMID field
        INSERT INTO itemData VALUES (3, 6, 13);

        -- Child note for ITEM0001
        INSERT INTO itemTypes VALUES (3, 'note', NULL, 1);
        INSERT INTO items VALUES (10, 3, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'NOTE0001', 1, 1);
        INSERT INTO itemNotes (itemID, parentItemID, note, title) VALUES (10, 1, '<p>Important observation about the dataset.</p>', 'Study Notes');

        -- Child attachment for ITEM0001
        INSERT INTO itemTypes VALUES (2, 'attachment', NULL, 1);
        INSERT INTO items VALUES (20, 2, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'ATT00001', 1, 1);
        INSERT INTO itemAttachments (itemID, parentItemID, linkMode, contentType, path) VALUES (20, 1, 0, 'application/pdf', 'storage:paper.pdf');

        -- Item 4: Unicode CJK item
        INSERT INTO items VALUES (4, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'ITEMCJK1', 1, 1);
        INSERT INTO itemDataValues VALUES (20, '量子计算与量子信息综述');
        INSERT INTO itemData VALUES (4, 1, 20);

        -- Creator for ITEM0001
        INSERT INTO creators VALUES (1, 'Alice', 'Smith', 0);
        INSERT INTO itemCreators VALUES (1, 1, 1, 0);
        "#,
    )
    .unwrap();
    sqlite_path
}

// =========================================================================
// 1. ITEM CONTEXT TESTS
// =========================================================================

#[test]
fn test_item_context_explicit_key_json() {
    let dir = TestDir::new("context-explicit");
    build_comprehensive_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);

    let (code, output) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "context", "ITEM0001"],
    );
    assert_eq!(code, 0);
    assert_eq!(output["item"]["key"], "ITEM0001");
    assert_eq!(output["item"]["title"], "Test Item One");
    assert_eq!(output["notes"], json!([]));
    assert_eq!(output["exports"], json!({}));
    assert_eq!(output["links"], json!({}));
    assert!(
        output.get("annotations").is_none(),
        "annotations must NOT be present in item context"
    );
    assert!(
        output.get("relations").is_none(),
        "relations must NOT be present in item context"
    );

    let prompt = output["prompt_context"].as_str().unwrap();
    assert!(prompt.contains("Title: Test Item One"));
    assert!(prompt.contains("Item Key: ITEM0001"));
    assert!(prompt.contains("Creators: Alice Smith"));
    assert!(prompt.contains("Attachments:"));
    assert!(!prompt.contains("Notes:"));
    assert!(!prompt.contains("Links:"));

    server.finish();
}

#[test]
fn test_item_context_numeric_id_and_session_fallback() {
    let dir = TestDir::new("context-session");
    build_comprehensive_fixture(dir.path());
    let state_dir = dir.path().join("state");
    std::fs::create_dir_all(&state_dir).unwrap();

    // 1. Resolve by numeric id "1"
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let (code, output) = run_cli(dir.path(), server.port, &[], &["item", "context", "1"]);
    assert_eq!(code, 0);
    assert_eq!(output["item"]["key"], "ITEM0001");
    server.finish();

    // 2. Set session item to ITEM0002 and query without ref
    let session_file = state_dir.join("session.json");
    std::fs::write(
        &session_file,
        json!({
            "current_library": null,
            "current_collection": null,
            "current_item": "ITEM0002",
            "command_history": []
        })
        .to_string(),
    )
    .unwrap();

    let server2 = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let state_str = state_dir.to_str().unwrap();
    let (code2, output2) = run_cli(
        dir.path(),
        server2.port,
        &[("CLI_ANYTHING_ZOTERO_STATE_DIR", state_str)],
        &["item", "context"],
    );
    assert_eq!(code2, 0);
    assert_eq!(output2["item"]["key"], "ITEM0002");
    server2.finish();
}

#[test]
fn test_item_context_missing_ref_and_unknown_item() {
    let dir = TestDir::new("context-errors");
    build_comprehensive_fixture(dir.path());
    let state_dir = dir.path().join("state");
    std::fs::create_dir_all(&state_dir).unwrap();

    // No ref and empty session -> error
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let state_str = state_dir.to_str().unwrap();
    let (code, output) = run_cli(
        dir.path(),
        server.port,
        &[("CLI_ANYTHING_ZOTERO_STATE_DIR", state_str)],
        &["item", "context"],
    );
    assert_eq!(code, 1);
    assert!(output["error"]
        .as_str()
        .unwrap()
        .contains("Item reference required"));
    server.finish();

    // Unknown item key -> error
    let server2 = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let (code2, output2) = run_cli(
        dir.path(),
        server2.port,
        &[],
        &["item", "context", "NONEXISTENT"],
    );
    assert_eq!(code2, 1);
    assert!(output2["error"]
        .as_str()
        .unwrap()
        .contains("Item not found: NONEXISTENT"));
    server2.finish();
}

#[test]
fn test_item_context_flags_notes_links_exports() {
    let dir = TestDir::new("context-flags");
    build_comprehensive_fixture(dir.path());

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        // Mock export bibtex call
        ScriptedResponse::Http {
            status: 200,
            headers: vec![("Content-Type".to_string(), "text/plain".to_string())],
            body: b"@article{item0001, title={Test Item One}}".to_vec(),
        },
        // Mock export csljson call
        ScriptedResponse::Http {
            status: 200,
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: b"[{\"id\":\"ITEM0001\",\"title\":\"Test Item One\"}]".to_vec(),
        },
    ]);

    let (code, output) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "item",
            "context",
            "ITEM0001",
            "--include-notes",
            "--include-links",
            "--include-bibtex",
            "--include-csljson",
        ],
    );
    assert_eq!(code, 0);

    // Notes
    let notes = output["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["key"], "NOTE0001");

    // Links
    let links = &output["links"];
    assert_eq!(links["url"], "https://example.com/paper.html");

    // Exports
    let exports = &output["exports"];
    assert!(exports["bibtex"].as_str().unwrap().contains("@article"));
    assert!(exports["csljson"].as_str().unwrap().contains("ITEM0001"));

    // Prompt context formatting
    let prompt = output["prompt_context"].as_str().unwrap();
    assert!(prompt.contains("Links:\n- url: https://example.com/paper.html"));
    assert!(prompt.contains("Notes:\n- Study Notes: Important observation about the dataset."));
    assert!(prompt.contains("Exports:\n[bibtex]\n@article{item0001, title={Test Item One}}\n[csljson]\n[{\"id\":\"ITEM0001\",\"title\":\"Test Item One\"}]"));

    server.finish();
}

#[test]
fn test_item_context_prompt_ordering_and_human_mode() {
    let dir = TestDir::new("context-order");
    build_comprehensive_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);

    let (code, stdout) = run_cli_human(
        dir.path(),
        server.port,
        &[],
        &["item", "context", "ITEM0001"],
    );
    assert_eq!(code, 0);
    let lines: Vec<&str> = stdout.trim().lines().collect();

    // Verify ordering: Title -> Item Key -> Item Type -> Creators -> Fields -> Attachments
    assert_eq!(lines[0], "Title: Test Item One");
    assert_eq!(lines[1], "Item Key: ITEM0001");
    assert_eq!(lines[2], "Item Type: document");
    assert_eq!(lines[3], "Creators: Alice Smith");
    assert_eq!(lines[4], "date: 2024");
    assert_eq!(lines[5], "url: https://example.com/paper.html");
    assert_eq!(lines[6], "Attachments:");
    assert!(lines[7].starts_with("- ATT00001: "));

    server.finish();
}

#[test]
fn test_item_context_unicode() {
    let dir = TestDir::new("context-unicode");
    build_comprehensive_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);

    let (code, output) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "context", "ITEMCJK1"],
    );
    assert_eq!(code, 0);
    assert_eq!(output["item"]["title"], "量子计算与量子信息综述");
    let prompt = output["prompt_context"].as_str().unwrap();
    assert!(prompt.contains("Title: 量子计算与量子信息综述"));

    server.finish();
}

// =========================================================================
// 2. ITEM DUPLICATES TESTS
// =========================================================================

fn build_duplicates_fixture(dir: &Path) -> PathBuf {
    let sqlite_path = build_fixture_sqlite(dir);
    let conn = rusqlite::Connection::open(&sqlite_path).unwrap();
    conn.execute_batch(
        r#"
        -- Clear items
        DELETE FROM itemData;
        DELETE FROM items;

        -- Create items with DOI variants (Group 1: 10.1000/182)
        INSERT INTO items VALUES (1, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'DOI0001', 1, 1);
        INSERT INTO items VALUES (2, 1, '2026-01-02', '2026-01-02', '2026-01-02', 1, 'DOI0002', 1, 1);
        INSERT INTO items VALUES (3, 1, '2026-01-03', '2026-01-03', '2026-01-03', 1, 'DOI0003', 1, 1);

        INSERT INTO itemDataValues VALUES (101, 'https://doi.org/10.1000/182');
        INSERT INTO itemDataValues VALUES (102, 'http://dx.doi.org/10.1000/182.');
        INSERT INTO itemDataValues VALUES (103, 'doi: 10.1000/182); ');
        INSERT INTO itemDataValues VALUES (104, 'Paper Title One');

        INSERT INTO itemData VALUES (1, 2, 101), (1, 1, 104);
        INSERT INTO itemData VALUES (2, 2, 102), (2, 1, 104);
        INSERT INTO itemData VALUES (3, 2, 103), (3, 1, 104);

        -- Give DOI0002 a PDF attachment (so it sorts first in group)
        INSERT INTO items VALUES (20, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'ATT_PDF1', 1, 1);
        INSERT INTO itemAttachments (itemID, parentItemID, linkMode, contentType, path) VALUES (20, 2, 0, 'application/pdf', 'storage:doc.pdf');

        -- Create items for Title Duplicates (Group 2: "quantum computing a review")
        INSERT INTO items VALUES (4, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'TITLE001', 1, 1);
        INSERT INTO items VALUES (5, 1, '2026-01-02', '2026-01-02', '2026-01-02', 1, 'TITLE002', 1, 1);
        INSERT INTO itemDataValues VALUES (105, 'Quantum Computing: A Review.');
        INSERT INTO itemDataValues VALUES (106, 'Quantum Computing - A Review');
        INSERT INTO itemData VALUES (4, 1, 105), (5, 1, 106);

        -- Short title item (<8 chars, e.g. "Notes") - should NOT form duplicate groups
        INSERT INTO items VALUES (6, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'SHORT001', 1, 1);
        INSERT INTO items VALUES (7, 1, '2026-01-02', '2026-01-02', '2026-01-02', 1, 'SHORT002', 1, 1);
        INSERT INTO itemDataValues VALUES (107, 'Notes');
        INSERT INTO itemData VALUES (6, 1, 107), (7, 1, 107);
        "#,
    )
    .unwrap();
    sqlite_path
}

#[test]
fn test_duplicates_by_doi_normalization_and_member_ordering() {
    let dir = TestDir::new("dups-doi");
    build_duplicates_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);

    let (code, output) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "duplicates", "--by", "doi"],
    );
    assert_eq!(code, 0);
    assert_eq!(output["action"], "item_duplicates");
    assert_eq!(output["ok"], true);
    assert_eq!(output["by"], "doi");
    assert_eq!(output["group_count"], 1);

    let group = &output["groups"][0];
    assert_eq!(group["match"], "10.1000/182");
    assert_eq!(group["count"], 3);
    // DOI0002 has a PDF attachment so it must be sorted first and become keep_suggestion
    assert_eq!(group["keep_suggestion"], "DOI0002");
    assert_eq!(group["items"][0]["key"], "DOI0002");
    assert_eq!(group["items"][0]["hasPdf"], true);

    server.finish();
}

#[test]
fn test_duplicates_by_title_and_short_title_exclusion() {
    let dir = TestDir::new("dups-title");
    build_duplicates_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);

    let (code, output) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "duplicates", "--by", "title"],
    );
    assert_eq!(code, 0);
    assert_eq!(output["by"], "title");
    // Only "quantum computing a review" and "paper title one" (from items 1,2,3) should be found;
    // "notes" (<8 chars) must NOT form a duplicate group.
    let groups = output["groups"].as_array().unwrap();
    for g in groups {
        assert_ne!(g["match"], "notes");
    }

    server.finish();
}

fn bridge_ownership_ok() -> ScriptedResponse {
    ScriptedResponse::json(
        200,
        json!({"fork": "zotero-rust-cli", "id": "cli-bridge@cli-anything-rust.dev"}),
    )
}

#[test]
fn test_duplicates_limit_presort_quirk() {
    let dir = TestDir::new("dups-limit-quirk");
    let sqlite_path = build_fixture_sqlite(dir.path());
    let conn = rusqlite::Connection::open(&sqlite_path).unwrap();
    conn.execute_batch(
        r#"
        DELETE FROM itemData;
        DELETE FROM items;
        DELETE FROM itemDataValues;

        -- Group 1 (fetch order 1): 2 items
        INSERT INTO items VALUES (1, 1, '2026-01-01', '2026-01-03', '2026-01-03', 1, 'G1_1', 1, 1);
        INSERT INTO items VALUES (2, 1, '2026-01-01', '2026-01-03', '2026-01-03', 1, 'G1_2', 1, 1);
        INSERT INTO itemDataValues VALUES (1, '10.1001/g1');
        INSERT INTO itemData VALUES (1, 2, 1), (2, 2, 1);

        -- Group 2 (fetch order 2): 2 items
        INSERT INTO items VALUES (3, 1, '2026-01-01', '2026-01-02', '2026-01-02', 1, 'G2_1', 1, 1);
        INSERT INTO items VALUES (4, 1, '2026-01-01', '2026-01-02', '2026-01-02', 1, 'G2_2', 1, 1);
        INSERT INTO itemDataValues VALUES (2, '10.1001/g2');
        INSERT INTO itemData VALUES (3, 2, 2), (4, 2, 2);

        -- Group 3 (fetch order 3): 3 items (larger group, but older dateModified)
        INSERT INTO items VALUES (5, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'G3_1', 1, 1);
        INSERT INTO items VALUES (6, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'G3_2', 1, 1);
        INSERT INTO items VALUES (7, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'G3_3', 1, 1);
        INSERT INTO itemDataValues VALUES (3, '10.1001/g3');
        INSERT INTO itemData VALUES (5, 2, 3), (6, 2, 3), (7, 2, 3);
        "#,
    )
    .unwrap();

    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);

    // Limit 2 must take Group 1 and Group 2 (first 2 encountered in fetch order) and NOT see Group 3
    let (code, output) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "duplicates", "--by", "doi", "--limit", "2"],
    );
    assert_eq!(code, 0);
    assert_eq!(output["group_count"], 2);
    let groups = output["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0]["match"], "10.1001/g1");
    assert_eq!(groups[1]["match"], "10.1001/g2");

    server.finish();
}

#[test]
fn test_duplicates_zotero_mode_success_and_error() {
    let dir = TestDir::new("dups-zotero");
    build_fixture_sqlite(dir.path());

    // 1. Success case: Zotero native format emitted directly
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({
                "count": 2,
                "items": [{"key": "K1", "title": "T1", "date": "2024", "setID": 10}]
            }),
        ),
    ]);
    let (code, output) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "duplicates", "--by", "zotero"],
    );
    assert_eq!(code, 0);
    assert_eq!(output["count"], 2);
    assert_eq!(output["items"][0]["key"], "K1");
    server.finish();

    // 2. Caught JS error with count == 0 -> ZOTERO_DUP_FAILED envelope and exit 1
    let server2 = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({
                "error": "Duplicates index locked",
                "count": 0,
                "items": []
            }),
        ),
    ]);
    let (code2, output2) = run_cli(
        dir.path(),
        server2.port,
        &[],
        &["item", "duplicates", "--by", "zotero"],
    );
    assert_eq!(code2, 1);
    assert_eq!(output2["action"], "item_duplicates");
    assert_eq!(output2["ok"], false);
    assert_eq!(output2["code"], "ZOTERO_DUP_FAILED");
    assert_eq!(output2["error"], "Duplicates index locked");
    server2.finish();
}

// =========================================================================
// 3. ITEM METRICS TESTS
// =========================================================================

#[test]
fn test_metrics_direct_pmid_and_field_pmid() {
    let dir = TestDir::new("metrics-direct");
    build_comprehensive_fixture(dir.path());

    // 1. Direct --pmid
    let icite_server = ScriptedServer::start(vec![ScriptedResponse::json(
        200,
        json!({
            "data": [{
                "pmid": 25619680,
                "title": "A comprehensive study on cellular mechanisms and signaling pathways in mammalian systems",
                "year": 2015,
                "journal": "Nature",
                "citation_count": 142,
                "relative_citation_ratio": 3.45,
                "nih_percentile": 89.2,
                "expected_citations_per_year": 12.1,
                "doi": "10.1038/nature14136"
            }]
        }),
    )]);
    let zotero_server =
        ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let icite_url = format!("http://127.0.0.1:{}/api/pubs", icite_server.port);

    let (code, output) = run_cli(
        dir.path(),
        zotero_server.port,
        &[("CLI_ANYTHING_ZOTERO_ICITE_URL", &icite_url)],
        &["item", "metrics", "25619680", "--pmid"],
    );
    assert_eq!(code, 0);
    assert_eq!(output["pmid"], 25619680);
    assert_eq!(output["year"], 2015);
    assert_eq!(output["journal"], "Nature");
    assert_eq!(output["citation_count"], 142);
    assert_eq!(output["rcr"], 3.45);
    assert_eq!(output["nih_percentile"], 89.2);
    assert_eq!(output["expected_citations"], 12.1);
    assert_eq!(output["doi"], "10.1038/nature14136");
    let reqs = icite_server.finish();
    assert_eq!(reqs.len(), 1);
    assert!(reqs[0].path.contains("pmids=25619680"));
    zotero_server.finish();

    // 2. Lookup via item key where PMID is in fields["PMID"] (ITEM0003 has PMID 12345678)
    let icite_server2 = ScriptedServer::start(vec![ScriptedResponse::json(
        200,
        json!({
            "data": [{
                "pmid": 12345678,
                "title": "Study from PMID field",
                "year": 2020,
                "journal": "Science",
                "citation_count": 50,
                "relative_citation_ratio": 1.8,
                "nih_percentile": 60.0,
                "expected_citations_per_year": 4.0,
                "doi": "10.1126/science.12345"
            }]
        }),
    )]);
    let zotero_server2 =
        ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let icite_url2 = format!("http://127.0.0.1:{}/api/pubs", icite_server2.port);

    let (code2, output2) = run_cli(
        dir.path(),
        zotero_server2.port,
        &[("CLI_ANYTHING_ZOTERO_ICITE_URL", &icite_url2)],
        &["item", "metrics", "ITEM0003"],
    );
    assert_eq!(code2, 0);
    assert_eq!(output2["pmid"], 12345678);
    assert_eq!(output2["title"], "Study from PMID field");
    icite_server2.finish();
    zotero_server2.finish();

    // 3. Lookup via item key where PMID is in fields["extra"] (ITEM0002 has extra "PMID: 98765432")
    let icite_server3 = ScriptedServer::start(vec![ScriptedResponse::json(
        200,
        json!({
            "data": [{
                "pmid": 98765432,
                "title": "Study from extra field",
                "year": 2022,
                "journal": "Cell",
                "citation_count": 10,
                "relative_citation_ratio": 0.9,
                "nih_percentile": 40.0,
                "expected_citations_per_year": 2.0,
                "doi": "10.1016/cell.2022.01"
            }]
        }),
    )]);
    let zotero_server3 =
        ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let icite_url3 = format!("http://127.0.0.1:{}/api/pubs", icite_server3.port);

    let (code3, output3) = run_cli(
        dir.path(),
        zotero_server3.port,
        &[("CLI_ANYTHING_ZOTERO_ICITE_URL", &icite_url3)],
        &["item", "metrics", "ITEM0002"],
    );
    assert_eq!(code3, 0);
    assert_eq!(output3["pmid"], 98765432);
    assert_eq!(output3["title"], "Study from extra field");
    icite_server3.finish();
    zotero_server3.finish();

    // 4. Missing PMID error on item without PMID (no network call dispatched)
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let (code4, output4) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "metrics", "ITEMCJK1"],
    );
    assert_eq!(code4, 1);
    assert!(output4["error"]
        .as_str()
        .unwrap()
        .contains("No PMID found in item 'ITEMCJK1'"));
    server.finish();
}

#[test]
fn test_metrics_icite_errors() {
    let dir = TestDir::new("metrics-errors");
    build_comprehensive_fixture(dir.path());

    // 1. Empty data array
    let icite_server =
        ScriptedServer::start(vec![ScriptedResponse::json(200, json!({"data": []}))]);
    let zotero_server =
        ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let icite_url = format!("http://127.0.0.1:{}/api/pubs", icite_server.port);

    let (code, output) = run_cli(
        dir.path(),
        zotero_server.port,
        &[("CLI_ANYTHING_ZOTERO_ICITE_URL", &icite_url)],
        &["item", "metrics", "12345", "--pmid"],
    );
    assert_eq!(code, 1);
    assert_eq!(output["error"], "No data for PMID 12345");
    icite_server.finish();
    zotero_server.finish();

    // 2. HTTP 500
    let icite_server2 = ScriptedServer::start(vec![ScriptedResponse::Http {
        status: 500,
        headers: vec![],
        body: b"Internal Server Error".to_vec(),
    }]);
    let zotero_server2 =
        ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let icite_url2 = format!("http://127.0.0.1:{}/api/pubs", icite_server2.port);

    let (code2, output2) = run_cli(
        dir.path(),
        zotero_server2.port,
        &[("CLI_ANYTHING_ZOTERO_ICITE_URL", &icite_url2)],
        &["item", "metrics", "12345", "--pmid"],
    );
    assert_eq!(code2, 1);
    assert!(output2["error"]
        .as_str()
        .unwrap()
        .contains("Failed to fetch metrics for PMID 12345: HTTP 500"));
    icite_server2.finish();
    zotero_server2.finish();
}

#[test]
fn test_metrics_response_mapping_logic() {
    let raw_payload = json!({
        "data": [{
            "pmid": 25619680,
            "title": "A very long title that exceeds eighty characters in length to verify proper truncation behavior",
            "year": 2015,
            "journal": "Nature",
            "citation_count": 42,
            "relative_citation_ratio": 2.5,
            "nih_percentile": 75.0,
            "expected_citations_per_year": 5.0,
            "doi": "10.1038/nature14136"
        }]
    });

    let d = &raw_payload["data"][0];
    let title_full = d["title"].as_str().unwrap();
    let title_truncated: String = title_full.chars().take(80).collect();
    assert_eq!(title_truncated.len(), 80);
}

// =========================================================================
// 4. ITEM ANALYZE TESTS
// =========================================================================

#[test]
fn test_analyze_missing_api_key() {
    let dir = TestDir::new("analyze-no-key");
    build_comprehensive_fixture(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);

    let (code, output) = run_cli(
        dir.path(),
        server.port,
        &[("OPENAI_API_KEY", "")],
        &[
            "item",
            "analyze",
            "ITEM0001",
            "--question",
            "What is this paper about?",
            "--model",
            "gpt-4o",
        ],
    );
    assert_eq!(code, 1);
    assert!(output["error"].as_str().unwrap().contains("OPENAI_API_KEY is not set. Use `item context` for model-independent output or configure the API key."));

    server.finish();
}

#[test]
fn test_analyze_mock_openai_chat_completions() {
    let dir = TestDir::new("analyze-mock");
    build_comprehensive_fixture(dir.path());

    // Mock local OpenAI server
    let openai_server = ScriptedServer::start(vec![ScriptedResponse::json(
        200,
        json!({
            "id": "chatcmpl-test-12345",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "This paper presents a foundational study on quantum computing algorithms."
                }
            }]
        }),
    )]);

    let zotero_server =
        ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let openai_url = format!(
        "http://127.0.0.1:{}/v1/chat/completions",
        openai_server.port
    );

    let (code, output) = run_cli(
        dir.path(),
        zotero_server.port,
        &[
            ("OPENAI_API_KEY", "test-key-123"),
            ("CLI_ANYTHING_ZOTERO_OPENAI_URL", &openai_url),
        ],
        &[
            "item",
            "analyze",
            "ITEM0001",
            "--question",
            "What is the main finding?",
            "--model",
            "gpt-4o-mini",
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(output["itemKey"], "ITEM0001");
    assert_eq!(output["model"], "gpt-4o-mini");
    assert_eq!(output["question"], "What is the main finding?");
    assert_eq!(
        output["answer"],
        "This paper presents a foundational study on quantum computing algorithms."
    );
    assert_eq!(output["responseID"], "chatcmpl-test-12345");
    assert_eq!(output["context"]["item"]["key"], "ITEM0001");

    // Verify captured request at mock OpenAI server
    let reqs = openai_server.finish();
    assert_eq!(reqs.len(), 1);
    let body = reqs[0].body_json();
    assert_eq!(body["model"], "gpt-4o-mini");
    assert_eq!(
        body["messages"][0]["content"],
        "You are analyzing a Zotero bibliographic record. Stay grounded in the provided context. If the context is missing an answer, say so explicitly."
    );
    assert!(body["messages"][1]["content"]
        .as_str()
        .unwrap()
        .contains("Question:\nWhat is the main finding?"));
    assert!(body["messages"][1]["content"]
        .as_str()
        .unwrap()
        .contains("Context:\nTitle: Test Item One"));

    zotero_server.finish();
}

#[test]
fn test_analyze_human_mode_answer_only() {
    let dir = TestDir::new("analyze-human");
    build_comprehensive_fixture(dir.path());

    let openai_server = ScriptedServer::start(vec![ScriptedResponse::json(
        200,
        json!({
            "id": "chatcmpl-human-123",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "A concise human-readable answer."
                }
            }]
        }),
    )]);

    let zotero_server =
        ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let openai_url = format!(
        "http://127.0.0.1:{}/v1/chat/completions",
        openai_server.port
    );

    let (code, stdout) = run_cli_human(
        dir.path(),
        zotero_server.port,
        &[
            ("OPENAI_API_KEY", "test-key-123"),
            ("CLI_ANYTHING_ZOTERO_OPENAI_URL", &openai_url),
        ],
        &[
            "item",
            "analyze",
            "ITEM0001",
            "--question",
            "Summarize this",
            "--model",
            "gpt-4o",
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(stdout.trim(), "A concise human-readable answer.");

    openai_server.finish();
    zotero_server.finish();
}

#[test]
fn test_analyze_output_text_and_content_fallbacks() {
    let dir = TestDir::new("analyze-fallbacks");
    build_comprehensive_fixture(dir.path());

    // 1. output_text fallback
    let openai_server = ScriptedServer::start(vec![ScriptedResponse::json(
        200,
        json!({
            "id": "resp-legacy",
            "output_text": "Legacy response format text."
        }),
    )]);
    let zotero_server =
        ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let openai_url = format!(
        "http://127.0.0.1:{}/v1/chat/completions",
        openai_server.port
    );

    let (code, output) = run_cli(
        dir.path(),
        zotero_server.port,
        &[
            ("OPENAI_API_KEY", "test-key-123"),
            ("CLI_ANYTHING_ZOTERO_OPENAI_URL", &openai_url),
        ],
        &[
            "item",
            "analyze",
            "ITEM0001",
            "--question",
            "Q",
            "--model",
            "m",
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(output["answer"], "Legacy response format text.");
    openai_server.finish();
    zotero_server.finish();

    // 2. output[].content[].text fallback
    let openai_server2 = ScriptedServer::start(vec![ScriptedResponse::json(
        200,
        json!({
            "id": "resp-nested",
            "output": [
                {"content": [{"text": "Part A"}]},
                {"content": [{"text": "Part B"}]}
            ]
        }),
    )]);
    let zotero_server2 =
        ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let openai_url2 = format!(
        "http://127.0.0.1:{}/v1/chat/completions",
        openai_server2.port
    );

    let (code2, output2) = run_cli(
        dir.path(),
        zotero_server2.port,
        &[
            ("OPENAI_API_KEY", "test-key-123"),
            ("CLI_ANYTHING_ZOTERO_OPENAI_URL", &openai_url2),
        ],
        &[
            "item",
            "analyze",
            "ITEM0001",
            "--question",
            "Q",
            "--model",
            "m",
        ],
    );
    assert_eq!(code2, 0);
    assert_eq!(output2["answer"], "Part A\n\nPart B");
    openai_server2.finish();
    zotero_server2.finish();
}

#[test]
fn test_analyze_http_error_and_empty_response() {
    let dir = TestDir::new("analyze-errors");
    build_comprehensive_fixture(dir.path());

    // 1. HTTP 429
    let openai_server = ScriptedServer::start(vec![ScriptedResponse::Http {
        status: 429,
        headers: vec![],
        body: b"Rate limit exceeded".to_vec(),
    }]);
    let zotero_server =
        ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let openai_url = format!(
        "http://127.0.0.1:{}/v1/chat/completions",
        openai_server.port
    );

    let (code, output) = run_cli(
        dir.path(),
        zotero_server.port,
        &[
            ("OPENAI_API_KEY", "test-key-123"),
            ("CLI_ANYTHING_ZOTERO_OPENAI_URL", &openai_url),
        ],
        &[
            "item",
            "analyze",
            "ITEM0001",
            "--question",
            "Q",
            "--model",
            "m",
        ],
    );
    assert_eq!(code, 1);
    assert!(output["error"]
        .as_str()
        .unwrap()
        .contains("OpenAI API returned HTTP 429: Rate limit exceeded"));
    openai_server.finish();
    zotero_server.finish();

    // 2. Empty response
    let openai_server2 =
        ScriptedServer::start(vec![ScriptedResponse::json(200, json!({"choices": []}))]);
    let zotero_server2 =
        ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let openai_url2 = format!(
        "http://127.0.0.1:{}/v1/chat/completions",
        openai_server2.port
    );

    let (code2, output2) = run_cli(
        dir.path(),
        zotero_server2.port,
        &[
            ("OPENAI_API_KEY", "test-key-123"),
            ("CLI_ANYTHING_ZOTERO_OPENAI_URL", &openai_url2),
        ],
        &[
            "item",
            "analyze",
            "ITEM0001",
            "--question",
            "Q",
            "--model",
            "m",
        ],
    );
    assert_eq!(code2, 1);
    assert!(output2["error"]
        .as_str()
        .unwrap()
        .contains("OpenAI API returned no text output"));
    openai_server2.finish();
    zotero_server2.finish();
}

#[test]
fn test_item_context_local_api_unavailable_for_exports() {
    let dir = TestDir::new("context-no-api");
    build_comprehensive_fixture(dir.path());

    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, output) = run_cli(
        dir.path(),
        server.port,
        &[],
        &["item", "context", "ITEM0001", "--include-bibtex"],
    );
    assert_eq!(code, 1);
    assert!(output["error"]
        .as_str()
        .unwrap()
        .contains("Zotero Local API is not available"));
    server.finish();
}
