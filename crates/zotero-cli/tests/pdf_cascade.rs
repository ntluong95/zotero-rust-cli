//! Collection-level PDF batch tests (Phase 7 Slice 3): `core/pdf_fetch.py::fetch_pdfs_for_collection`
//! and `core/jsbridge.py::find_pdfs_in_collection`, ported to `src/pdf_cascade.rs`. `#[path]`-included
//! (not yet a registered crate module -- no CLI-dispatch slice exists) exactly like Phase 7 Slice
//! 1/2's own test files.

#![allow(dead_code)]

#[path = "../src/csl.rs"]
mod csl;
#[path = "../src/import_normalization.rs"]
mod import_normalization;
#[path = "../src/pdf_cascade.rs"]
mod pdf_cascade;
#[path = "../src/pdf_fetch.rs"]
mod pdf_fetch;

pub mod bridge {
    pub use zotero_cli::bridge::*;
}
pub mod db {
    pub use zotero_cli::db::*;
}
pub mod paths {
    pub use zotero_cli::paths::*;
}
pub mod runtime {
    pub use zotero_cli::runtime::*;
}
pub mod write {
    pub use zotero_cli::write::*;
}

#[path = "common/mod.rs"]
mod common;

use std::path::PathBuf;
use std::time::Duration;

use bridge::JSBridgeClient;
use common::{build_fixture_sqlite, ScriptedResponse, ScriptedServer, TestDir};
use pdf_fetch::{PdfDownloadClient, PdfMetadataClient};
use runtime::RuntimeContext;
use serde_json::{json, Value};

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

struct UnreachableMetadataClient;
impl PdfMetadataClient for UnreachableMetadataClient {
    fn fetch_json(&self, url: &str, _timeout: Duration) -> anyhow::Result<Value> {
        panic!("OA metadata client must not be called in this test (url={url})");
    }
}
struct UnreachableDownloadClient;
impl PdfDownloadClient for UnreachableDownloadClient {
    fn fetch_bytes(&self, url: &str, _timeout: Duration) -> anyhow::Result<Vec<u8>> {
        panic!("PDF download client must not be called in this test (url={url})");
    }
}

fn bridge_ownership_ok() -> ScriptedResponse {
    ScriptedResponse::json(
        200,
        json!({"fork": "zotero-rust-cli", "id": "cli-bridge@cli-anything-rust.dev"}),
    )
}

fn list_missing_response(missing: Vec<(&str, &str)>) -> ScriptedResponse {
    let missing: Vec<Value> = missing
        .into_iter()
        .map(|(key, title)| json!({"key": key, "title": title, "DOI": ""}))
        .collect();
    ScriptedResponse::json(
        200,
        json!({"ok": true, "total": missing.len(), "missing": missing, "missing_count": missing.len()}),
    )
}

/// Sets `HOME` to an isolated temp directory for the duration of `body`, guarded against races
/// with any other test in this binary that also redirects `resume_state_path`'s base directory.
fn with_isolated_resume_home<R>(label: &str, body: impl FnOnce(&std::path::Path) -> R) -> R {
    let _guard = pdf_cascade::RESUME_HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let home = std::env::temp_dir().join(format!(
        "zotero-cli-pdf-cascade-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&home).unwrap();
    let original = std::env::var_os("HOME");
    // SAFETY: serialized by RESUME_HOME_ENV_LOCK against every other HOME-mutating test.
    unsafe {
        std::env::set_var("HOME", &home);
    }
    let result = body(&home);
    unsafe {
        match &original {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }
    std::fs::remove_dir_all(&home).ok();
    result
}

#[allow(clippy::too_many_arguments)]
fn run_fetch_pdfs(
    runtime: &RuntimeContext,
    bridge: &JSBridgeClient,
    collection_key: &str,
    limit: Option<usize>,
    resume: bool,
    reset_resume: bool,
) -> Value {
    pdf_cascade::fetch_pdfs_for_collection(
        runtime,
        bridge,
        &UnreachableMetadataClient,
        &UnreachableDownloadClient,
        collection_key,
        &["zotero".to_string()],
        1,
        limit,
        5,
        5,
        None,
        resume,
        reset_resume,
    )
}

// ── collection all-success ──

#[test]
fn collection_all_success() {
    let dir = TestDir::new("cascade-all-success");
    let sqlite = build_fixture_sqlite(dir.path());
    let runtime = test_runtime(sqlite);
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        list_missing_response(vec![
            ("ITEM0001", "Test Item One"),
            ("ITEM0002", "Test Item Two"),
        ]),
        ScriptedResponse::bridge_string(200, "FOUND: ATT0001"),
        ScriptedResponse::bridge_string(200, "FOUND: ATT0002"),
    ]);
    let bridge = JSBridgeClient::new(server.port);

    let result = run_fetch_pdfs(&runtime, &bridge, "COLLE001", None, false, false);

    server.finish();
    assert_eq!(result["ok"], true);
    assert_eq!(result["status"], "success");
    assert_eq!(result["found"], 2);
    assert_eq!(result["checked"], 2);
    assert_eq!(result["skipped_resume"], 0);
}

// ── collection partial-success ──

#[test]
fn collection_partial_success() {
    let dir = TestDir::new("cascade-partial");
    let sqlite = build_fixture_sqlite(dir.path());
    let runtime = test_runtime(sqlite);
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        list_missing_response(vec![
            ("ITEM0001", "Test Item One"),
            ("ITEM0002", "Test Item Two"),
        ]),
        ScriptedResponse::bridge_string(200, "FOUND: ATT0001"),
        ScriptedResponse::bridge_string(200, "NOT_FOUND: no PDF available for Test Item Two"),
    ]);
    let bridge = JSBridgeClient::new(server.port);

    let result = run_fetch_pdfs(&runtime, &bridge, "COLLE001", None, false, false);

    server.finish();
    // partial_success maps ok=true but is still an exit-1-shaped status downstream (§ spec).
    assert_eq!(result["ok"], true);
    assert_eq!(result["status"], "partial_success");
    assert_eq!(result["found"], 1);
    assert_eq!(result["checked"], 2);
}

// ── collection all-failed ──

#[test]
fn collection_all_failed() {
    let dir = TestDir::new("cascade-all-failed");
    let sqlite = build_fixture_sqlite(dir.path());
    let runtime = test_runtime(sqlite);
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        list_missing_response(vec![
            ("ITEM0001", "Test Item One"),
            ("ITEM0002", "Test Item Two"),
        ]),
        ScriptedResponse::bridge_string(200, "NOT_FOUND: no PDF available for Test Item One"),
        ScriptedResponse::bridge_string(200, "NOT_FOUND: no PDF available for Test Item Two"),
    ]);
    let bridge = JSBridgeClient::new(server.port);

    let result = run_fetch_pdfs(&runtime, &bridge, "COLLE001", None, false, false);

    server.finish();
    assert_eq!(result["ok"], false);
    assert_eq!(result["status"], "not_found");
    assert_eq!(result["found"], 0);
}

// ── deterministic ordering ──

#[test]
fn collection_processing_preserves_bridge_returned_order() {
    let dir = TestDir::new("cascade-order");
    let sqlite = build_fixture_sqlite(dir.path());
    let runtime = test_runtime(sqlite);
    // Deliberately reversed vs. numeric order, to prove no incidental re-sorting.
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        list_missing_response(vec![
            ("ITEM0002", "Test Item Two"),
            ("ITEM0001", "Test Item One"),
        ]),
        ScriptedResponse::bridge_string(200, "FOUND: ATT0002"),
        ScriptedResponse::bridge_string(200, "FOUND: ATT0001"),
    ]);
    let bridge = JSBridgeClient::new(server.port);

    let result = run_fetch_pdfs(&runtime, &bridge, "COLLE001", None, false, false);

    server.finish();
    let details = result["details"].as_array().unwrap();
    assert_eq!(details[0]["key"], "ITEM0002");
    assert_eq!(details[0]["index"], 1);
    assert_eq!(details[1]["key"], "ITEM0001");
    assert_eq!(details[1]["index"], 2);
}

// ── resume after interruption, completed items not repeated, failed-item retry ──

#[test]
fn resume_skips_completed_items_and_retries_failed_ones_then_clears_state_on_full_success() {
    with_isolated_resume_home("resume-flow", |_home| {
        let dir = TestDir::new("cascade-resume");
        let sqlite = build_fixture_sqlite(dir.path());
        let runtime = test_runtime(sqlite);
        let collection_key = "COLLE001";

        // Run 1: both items are attempted; ITEM0001 succeeds, ITEM0002 fails.
        let server1 = ScriptedServer::start(vec![
            bridge_ownership_ok(),
            list_missing_response(vec![
                ("ITEM0001", "Test Item One"),
                ("ITEM0002", "Test Item Two"),
            ]),
            ScriptedResponse::bridge_string(200, "FOUND: ATT0001"),
            ScriptedResponse::bridge_string(200, "NOT_FOUND: no PDF available for Test Item Two"),
        ]);
        let bridge1 = JSBridgeClient::new(server1.port);
        let run1 = run_fetch_pdfs(&runtime, &bridge1, collection_key, None, true, false);
        server1.finish();
        assert_eq!(run1["status"], "partial_success");
        assert_eq!(run1["found"], 1);
        assert_eq!(run1["skipped_resume"], 0);
        let completed = pdf_cascade::load_resume_keys(collection_key);
        assert_eq!(
            completed.len(),
            1,
            "only the succeeded item must be recorded"
        );
        assert!(completed.contains("ITEM0001"));
        assert!(
            !completed.contains("ITEM0002"),
            "a failed item must never be recorded as completed"
        );

        // Run 2 ("after interruption"): Zotero's own missing-list still returns both items
        // (Zotero itself has no notion of resume) -- ITEM0001 must be skipped via resume state
        // (completed items not repeated), and ITEM0002 must be retried (failed-item retry).
        let server2 = ScriptedServer::start(vec![
            bridge_ownership_ok(),
            list_missing_response(vec![
                ("ITEM0001", "Test Item One"),
                ("ITEM0002", "Test Item Two"),
            ]),
            // Only one find_pdf eval expected -- for ITEM0002. If ITEM0001 were incorrectly
            // retried, this server would receive a 3rd request it has no scripted response for
            // and the accept loop would hang waiting for a connection that never fully resolves,
            // failing the test by timeout instead of assertion.
            ScriptedResponse::bridge_string(200, "FOUND: ATT0002RETRY"),
        ]);
        let bridge2 = JSBridgeClient::new(server2.port);
        let run2 = run_fetch_pdfs(&runtime, &bridge2, collection_key, None, true, false);
        let requests2 = server2.finish();
        assert_eq!(
            requests2.len(),
            3,
            "ownership ping, list, and exactly one retry of ITEM0002"
        );

        assert_eq!(
            run2["skipped_resume"], 1,
            "ITEM0001 must be skipped, not reprocessed"
        );
        assert_eq!(run2["checked"], 1);
        assert_eq!(run2["found"], 1);
        assert_eq!(run2["status"], "success");

        // Cleanup after successful completion: the batch is now fully done, so resume state is
        // cleared.
        assert!(
            pdf_cascade::load_resume_keys(collection_key).is_empty(),
            "resume state must be cleared after a fully successful run"
        );
        assert!(!pdf_cascade::resume_state_file_path(collection_key).exists());
    });
}

#[test]
fn reset_resume_clears_state_before_the_run_starts() {
    with_isolated_resume_home("reset-resume", |_home| {
        let collection_key = "COLLE002";
        pdf_cascade::save_resume_key(collection_key, "STALE0001").unwrap();
        assert!(!pdf_cascade::load_resume_keys(collection_key).is_empty());

        let dir = TestDir::new("cascade-reset-resume");
        let sqlite = build_fixture_sqlite(dir.path());
        let runtime = test_runtime(sqlite);
        let server = ScriptedServer::start(vec![
            bridge_ownership_ok(),
            list_missing_response(vec![("ITEM0001", "Test Item One")]),
            ScriptedResponse::bridge_string(200, "FOUND: ATT0001"),
        ]);
        let bridge = JSBridgeClient::new(server.port);

        let result = run_fetch_pdfs(&runtime, &bridge, collection_key, None, true, true);
        server.finish();

        assert_eq!(
            result["skipped_resume"], 0,
            "--reset-resume must clear stale state first"
        );
    });
}

// ── find-pdfs (Zotero-only, per-item, no resume, no OA cascade) ──

#[test]
fn find_pdfs_in_collection_aggregates_found_not_found_and_timeout() {
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        list_missing_response(vec![
            ("ITEM0001", "Test Item One"),
            ("ITEM0002", "Test Item Two"),
        ]),
        ScriptedResponse::bridge_string(200, "FOUND: ATT0001"),
        ScriptedResponse::bridge_string(200, "NOT_FOUND: no PDF available for Test Item Two"),
    ]);
    let bridge = JSBridgeClient::new(server.port);

    let response = pdf_cascade::find_pdfs_in_collection(&bridge, "COLLE001", 1, 30, None);

    server.finish();
    assert_eq!(response["ok"], true);
    let data = &response["data"];
    assert_eq!(data["found"], 1);
    assert_eq!(data["not_found"], 1);
    assert_eq!(data["timeouts"], 0);
    assert_eq!(data["strategy"], "per-item");
}

#[test]
fn find_pdfs_in_collection_never_touches_resume_state() {
    with_isolated_resume_home("find-pdfs-no-resume", |_home| {
        let collection_key = "COLLE003";
        let server = ScriptedServer::start(vec![
            bridge_ownership_ok(),
            list_missing_response(vec![("ITEM0001", "Test Item One")]),
            ScriptedResponse::bridge_string(200, "FOUND: ATT0001"),
        ]);
        let bridge = JSBridgeClient::new(server.port);

        pdf_cascade::find_pdfs_in_collection(&bridge, collection_key, 1, 30, None);
        server.finish();

        assert!(!pdf_cascade::resume_state_file_path(collection_key).exists());
    });
}
