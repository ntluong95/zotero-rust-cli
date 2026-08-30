//! Item-level PDF cascade tests (Phase 7 Slice 3): `core/pdf_fetch.py::fetch_pdf_for_item` /
//! `item_find_pdf_command`'s core, ported to `src/pdf_fetch.rs`. Not yet a real crate module
//! (no CLI-dispatch slice has registered it in `lib.rs`), so it's `#[path]`-included here exactly
//! like Phase 7 Slice 1/2's own test files already do for `csl.rs`/`import_core.rs`.

#![allow(dead_code)]

#[path = "../src/csl.rs"]
mod csl;
#[path = "../src/import_normalization.rs"]
mod import_normalization;
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

/// A stub metadata client that always fails (network unavailable) -- for tests exercising only
/// the Zotero-source or SQLite-gate paths, where the OA cascade must never be reached anyway.
struct UnreachableMetadataClient;
impl PdfMetadataClient for UnreachableMetadataClient {
    fn fetch_json(&self, url: &str, _timeout: Duration) -> anyhow::Result<Value> {
        panic!("OA metadata client must not be called in this test (url={url})");
    }
}

struct FailingMetadataClient;
impl PdfMetadataClient for FailingMetadataClient {
    fn fetch_json(&self, _url: &str, _timeout: Duration) -> anyhow::Result<Value> {
        anyhow::bail!("network unavailable")
    }
}

struct UnreachableDownloadClient;
impl PdfDownloadClient for UnreachableDownloadClient {
    fn fetch_bytes(&self, url: &str, _timeout: Duration) -> anyhow::Result<Vec<u8>> {
        panic!("PDF download client must not be called in this test (url={url})");
    }
}

struct StubDownloadClient(Option<Vec<u8>>);
impl PdfDownloadClient for StubDownloadClient {
    fn fetch_bytes(&self, _url: &str, _timeout: Duration) -> anyhow::Result<Vec<u8>> {
        self.0.clone().ok_or_else(|| anyhow::anyhow!("not found"))
    }
}

fn valid_pdf_bytes() -> Vec<u8> {
    let mut data = b"%PDF-1.4".to_vec();
    data.resize(9000, 0);
    data
}

fn bridge_ownership_ok() -> ScriptedResponse {
    ScriptedResponse::json(
        200,
        json!({"fork": "zotero-rust-cli", "id": "cli-bridge@cli-anything-rust.dev"}),
    )
}

// ── existing PDF -> skip ──

#[test]
fn existing_pdf_is_skipped_without_any_network_call() {
    let dir = TestDir::new("fetch-existing-pdf");
    let sqlite = build_fixture_sqlite(dir.path());
    let runtime = test_runtime(sqlite);
    // Deliberately no server bound on this port: any network attempt fails loudly rather than
    // silently succeeding, proving the has_pdf gate short-circuits before any HTTP/Bridge call.
    let unused_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    };
    let bridge = JSBridgeClient::new(unused_port);

    let result = pdf_fetch::fetch_pdf_for_item(
        &runtime,
        &bridge,
        &UnreachableMetadataClient,
        &UnreachableDownloadClient,
        "ITEM0003",
        &["zotero".to_string(), "unpaywall".to_string()],
        1,
        5,
        5,
        false,
    );

    assert_eq!(result["ok"], true);
    assert_eq!(result["status"], "already_has_pdf");
    assert_eq!(result["code"], "ALREADY_HAS_PDF");
    assert_eq!(result["source"], "existing");
}

#[test]
fn force_bypasses_the_existing_pdf_skip() {
    let dir = TestDir::new("fetch-force");
    let sqlite = build_fixture_sqlite(dir.path());
    let runtime = test_runtime(sqlite);
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::bridge_string(200, "FOUND: NEWATT01"),
    ]);
    let bridge = JSBridgeClient::new(server.port);

    let result = pdf_fetch::fetch_pdf_for_item(
        &runtime,
        &bridge,
        &UnreachableMetadataClient,
        &UnreachableDownloadClient,
        "ITEM0003",
        &["zotero".to_string()],
        1,
        5,
        5,
        true,
    );

    let requests = server.finish();
    assert_eq!(requests.len(), 2, "ownership ping + find_pdf eval");
    assert_eq!(result["status"], "success");
    assert_eq!(result["code"], "FOUND");
    assert_eq!(result["source"], "zotero");
}

// ── successful discovery (Zotero source) ──

#[test]
fn successful_discovery_via_zotero_source_never_reaches_oa_cascade() {
    let dir = TestDir::new("fetch-zotero-found");
    let sqlite = build_fixture_sqlite(dir.path());
    let runtime = test_runtime(sqlite);
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::bridge_string(200, "FOUND: ZATT0001"),
    ]);
    let bridge = JSBridgeClient::new(server.port);

    let result = pdf_fetch::fetch_pdf_for_item(
        &runtime,
        &bridge,
        &UnreachableMetadataClient,
        &UnreachableDownloadClient,
        "ITEM0001",
        &[
            "zotero".to_string(),
            "unpaywall".to_string(),
            "arxiv".to_string(),
        ],
        1,
        5,
        5,
        false,
    );

    server.finish();
    assert_eq!(result["ok"], true);
    assert_eq!(result["status"], "success");
    assert_eq!(result["code"], "FOUND");
    assert_eq!(result["attachment_key"], "ZATT0001");
}

// ── no PDF found ──

#[test]
fn no_pdf_found_anywhere_returns_not_found_status() {
    let dir = TestDir::new("fetch-not-found");
    let sqlite = build_fixture_sqlite(dir.path());
    let runtime = test_runtime(sqlite);
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::bridge_string(200, "NOT_FOUND: no PDF available for Test Item One"),
    ]);
    let bridge = JSBridgeClient::new(server.port);

    let result = pdf_fetch::fetch_pdf_for_item(
        &runtime,
        &bridge,
        &FailingMetadataClient,
        &StubDownloadClient(None),
        "ITEM0001",
        &[
            "zotero".to_string(),
            "unpaywall".to_string(),
            "arxiv".to_string(),
        ],
        1,
        5,
        5,
        false,
    );

    server.finish();
    assert_eq!(result["ok"], false);
    assert_eq!(result["status"], "not_found");
    assert_eq!(result["code"], "PDF_NOT_FOUND");
}

// ── non-PDF body ──

#[test]
fn non_pdf_body_is_rejected_and_cascade_reports_not_found() {
    let dir = TestDir::new("fetch-non-pdf-body");
    let sqlite = build_fixture_sqlite(dir.path());
    let runtime = test_runtime(sqlite);

    let result = pdf_fetch::fetch_pdf_for_item(
        &runtime,
        &JSBridgeClient::new(0), // "zotero" not in sources -- never dialed
        &FailingMetadataClient,
        &StubDownloadClient(Some(b"this is not a pdf, just html error text".to_vec())),
        "ITEM0002",
        &["biorxiv".to_string()],
        1,
        5,
        5,
        false,
    );

    assert_eq!(result["ok"], false);
    assert_eq!(result["status"], "not_found");
    assert_eq!(result["code"], "PDF_NOT_FOUND");
}

// ── successful download + attachment creation + correct parent key ──

#[test]
fn successful_download_attaches_via_the_existing_item_attach_bridge_primitive() {
    let dir = TestDir::new("fetch-download-attach");
    let sqlite = build_fixture_sqlite(dir.path());
    let runtime = test_runtime(sqlite);
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        ScriptedResponse::bridge_string(200, "OK: NEWATT02 attached to Test Item Two"),
    ]);
    let bridge = JSBridgeClient::new(server.port);

    let result = pdf_fetch::fetch_pdf_for_item(
        &runtime,
        &bridge,
        &FailingMetadataClient,
        &StubDownloadClient(Some(valid_pdf_bytes())),
        "ITEM0002",
        &["biorxiv".to_string()],
        1,
        5,
        5,
        false,
    );

    let requests = server.finish();
    assert_eq!(requests.len(), 2, "ownership ping + attach eval");
    let attach_code = String::from_utf8_lossy(&requests[1].body);
    assert!(
        attach_code.contains("ITEM0002"),
        "attach must target the correct parent item key: {attach_code}"
    );

    assert_eq!(result["ok"], true);
    assert_eq!(result["status"], "success");
    assert_eq!(result["code"], "ATTACHED");
    assert_eq!(result["source"], "oa-cascade");
}

// ── ambiguous attachment mutation -> no retry (find_pdf's timeout-then-verify path) ──

#[test]
fn ambiguous_find_pdf_timeout_verifies_instead_of_retrying_add_available_pdf() {
    let server = ScriptedServer::start(vec![
        bridge_ownership_ok(),
        // `execute_http` floors every call's timeout at 10s regardless of the caller's request
        // (pre-existing Phase 6 behavior, unrelated to this slice) -- stall past that floor so
        // ureq raises a genuine client-side timeout, not a fast connection-reset.
        ScriptedResponse::Stall(Duration::from_secs(12)),
        // The verify-only fallback call succeeds immediately.
        ScriptedResponse::bridge_string(200, "FOUND: VERIFIEDATT"),
    ]);
    let bridge = JSBridgeClient::new(server.port);

    let response = bridge.find_pdf(1, "ITEM0001", 1);

    let requests = server.finish();
    assert_eq!(
        requests.len(),
        3,
        "ownership ping, one (stalled) addAvailablePDF attempt, one verify call -- no second \
         addAvailablePDF retry"
    );
    assert!(response.is_ok());
    assert_eq!(
        response.data,
        Some(Value::String("FOUND: VERIFIEDATT".to_string()))
    );
}

#[test]
fn non_timeout_find_pdf_failure_is_returned_without_any_verify_call() {
    let server = ScriptedServer::start(vec![bridge_ownership_ok(), ScriptedResponse::Drop]);
    let bridge = JSBridgeClient::new(server.port);

    let response = bridge.find_pdf(1, "ITEM0001", 5);

    let requests = server.finish();
    assert_eq!(
        requests.len(),
        2,
        "a non-timeout transport failure must not trigger the verify fallback"
    );
    assert!(!response.is_ok());
}

// ── pure-logic regression: the missing-DOI quirk must not silently "fix" itself ──

#[test]
fn item_with_no_doi_still_attempts_arxiv_using_the_raw_item_key() {
    let dir = TestDir::new("fetch-no-doi-quirk");
    let sqlite = build_fixture_sqlite(dir.path());
    let runtime = test_runtime(sqlite);

    let result = pdf_fetch::fetch_pdf_for_item(
        &runtime,
        &JSBridgeClient::new(0),
        &FailingMetadataClient,
        &StubDownloadClient(None),
        "ITEM0001", // fixture item has no DOI field at all
        &["arxiv".to_string()],
        1,
        5,
        5,
        false,
    );

    assert_eq!(result["status"], "not_found");
    let attempts = result["attempts"].as_array().unwrap();
    assert_eq!(attempts[0]["source"], "arxiv");
    assert_eq!(attempts[0]["error"], "no candidate urls");
}
