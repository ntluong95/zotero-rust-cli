#![allow(dead_code)]

#[path = "../src/add_import.rs"]
mod add_import;
#[path = "../src/csl.rs"]
mod csl;
#[path = "../src/import_attachments.rs"]
mod import_attachments;
#[path = "../src/import_core.rs"]
mod import_core;
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
pub mod http {
    pub use zotero_cli::http::*;
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
pub mod write {
    pub use zotero_cli::write::*;
}

#[path = "common/mod.rs"]
mod common;

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use add_import::{AddImportBridge, AddImportFetcher, AddImportOptions};
use bridge::BridgeResponse;
use import_core::ConnectorImportClient;
use serde_json::{json, Value};
use write::WriteOutcome;

static RUNTIME_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct MockBridge {
    doi_matches: Vec<Value>,
    doi_imports: Vec<BridgeResponse>,
    pmid_imports: Vec<BridgeResponse>,
    add_to_collection_calls: usize,
    manage_tags_calls: usize,
    attach_calls: usize,
    standalone_pdf: Option<BridgeResponse>,
}

impl AddImportBridge for MockBridge {
    fn find_items_by_doi(&mut self, _library_id: u32, _doi: &str, _limit: i64) -> BridgeResponse {
        BridgeResponse::success(Value::Array(self.doi_matches.drain(..).collect()))
    }

    fn import_from_doi(
        &mut self,
        _library_id: u32,
        _doi: &str,
        _collection_key: Option<&str>,
        _tags: Option<&[String]>,
    ) -> BridgeResponse {
        self.doi_imports.remove(0)
    }

    fn import_from_pmid(
        &mut self,
        _library_id: u32,
        _pmid: &str,
        _collection_key: Option<&str>,
        _tags: Option<&[String]>,
    ) -> BridgeResponse {
        self.pmid_imports.remove(0)
    }

    fn add_to_collection(
        &mut self,
        _library_id: u32,
        _item_key: &str,
        _collection_key: &str,
    ) -> anyhow::Result<WriteOutcome> {
        self.add_to_collection_calls += 1;
        Ok(WriteOutcome::Applied {
            affected_key: "ITEM0001".to_string(),
        })
    }

    fn manage_tags(
        &mut self,
        _library_id: u32,
        _item_key: &str,
        _add_tags: &[String],
    ) -> anyhow::Result<WriteOutcome> {
        self.manage_tags_calls += 1;
        Ok(WriteOutcome::Applied {
            affected_key: "ITEM0001".to_string(),
        })
    }

    fn attach_pdf(
        &mut self,
        _library_id: u32,
        _item_key: &str,
        _path: &Path,
    ) -> anyhow::Result<WriteOutcome> {
        self.attach_calls += 1;
        Ok(WriteOutcome::Applied {
            affected_key: "ITEM0001".to_string(),
        })
    }

    fn standalone_pdf_import(
        &mut self,
        _library_id: u32,
        _file_path: &str,
        _title: &str,
        _collection_key: Option<&str>,
        _tags: &[String],
    ) -> BridgeResponse {
        self.standalone_pdf.take().unwrap_or_else(|| {
            BridgeResponse::success(json!({"ok": true, "key": "PDF0001", "title": "paper"}))
        })
    }
}

#[derive(Default)]
struct MockConnector {
    selected: Value,
    imported: Vec<Value>,
    saved_items: Vec<Vec<Value>>,
    import_calls: usize,
    update_calls: Vec<(String, Vec<String>)>,
}

impl ConnectorImportClient for MockConnector {
    fn import_text(
        &mut self,
        _port: u16,
        content: &[u8],
        _session_id: &str,
        content_type: &str,
        _timeout: Duration,
    ) -> anyhow::Result<Vec<Value>> {
        self.import_calls += 1;
        assert_eq!(content_type, "text/x-bibtex");
        assert!(String::from_utf8_lossy(content).contains('@'));
        Ok(self.imported.clone())
    }

    fn save_items(
        &mut self,
        _port: u16,
        items: &[Value],
        _session_id: &str,
        _timeout: Duration,
    ) -> anyhow::Result<()> {
        self.saved_items.push(items.to_vec());
        Ok(())
    }

    fn update_session(
        &mut self,
        _port: u16,
        _session_id: &str,
        target: &str,
        tags: &[String],
        _timeout: Duration,
    ) -> anyhow::Result<Value> {
        self.update_calls.push((target.to_string(), tags.to_vec()));
        Ok(json!({}))
    }

    fn get_selected_collection(&mut self, _port: u16, _timeout: Duration) -> anyhow::Result<Value> {
        Ok(self.selected.clone())
    }
}

#[derive(Default)]
struct MockFetcher {
    crossref: Option<anyhow::Result<String>>,
    arxiv: Option<anyhow::Result<String>>,
    html: Option<anyhow::Result<HtmlProbe>>,
}

type HtmlProbe = (Option<String>, Option<String>, String);

impl AddImportFetcher for MockFetcher {
    fn fetch_crossref_bibtex(&mut self, _doi: &str, _timeout: Duration) -> anyhow::Result<String> {
        self.crossref
            .take()
            .unwrap_or_else(|| Ok("@article{x}".to_string()))
    }

    fn fetch_arxiv_bibtex(
        &mut self,
        _arxiv_id: &str,
        _timeout: Duration,
    ) -> anyhow::Result<String> {
        self.arxiv
            .take()
            .unwrap_or_else(|| Ok("@article{a}".to_string()))
    }

    fn fetch_html_title_and_doi(
        &mut self,
        _url: &str,
        _timeout: Duration,
    ) -> anyhow::Result<(Option<String>, Option<String>, String)> {
        self.html.take().unwrap_or_else(|| {
            Ok((
                Some("Example Page".to_string()),
                None,
                "https://example.test".to_string(),
            ))
        })
    }
}

fn runtime(connector_available: bool) -> runtime::RuntimeContext {
    let dir = std::env::temp_dir().join(format!(
        "zotero-cli-add-import-runtime-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        RUNTIME_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let sqlite = common::build_fixture_sqlite(&dir);
    runtime::RuntimeContext {
        environment: paths::ZoteroEnvironment {
            executable: None,
            executable_exists: false,
            install_dir: None,
            version: "unknown".to_string(),
            profile_root: dir.clone(),
            profile_dir: None,
            data_dir: dir.clone(),
            data_dir_exists: true,
            sqlite_path: sqlite,
            sqlite_exists: true,
            styles_dir: dir.join("styles"),
            styles_exists: false,
            storage_dir: dir.join("storage"),
            storage_exists: false,
            translators_dir: dir.join("translators"),
            translators_exists: false,
            port: 9,
            local_api_enabled_configured: false,
        },
        backend: "connector".to_string(),
        connector_available,
        connector_message: if connector_available { "ok" } else { "down" }.to_string(),
        local_api_available: false,
        local_api_message: "off".to_string(),
        server_id: None,
        local_api_writes_available: false,
    }
}

fn options() -> AddImportOptions {
    AddImportOptions {
        collection_key: Some("COLLE001".to_string()),
        tags: vec![" one ".to_string(), "two".to_string()],
        fetch_pdf: false,
        connector_timeout: Duration::from_secs(7),
        ..Default::default()
    }
}

#[test]
fn import_doi_existing_item_uses_dedupe_and_optional_file_updates() {
    let runtime = runtime(true);
    let mut bridge = MockBridge {
        doi_matches: vec![json!({"key":"ITEM0001","title":"Existing","DOI":"10.1000/existing"})],
        ..Default::default()
    };
    let mut connector = MockConnector::default();
    let mut fetcher = MockFetcher::default();

    let out = add_import::import_doi_with_clients(
        &runtime,
        &mut bridge,
        &mut connector,
        &mut fetcher,
        "https://doi.org/10.1000/existing.",
        options(),
    );

    assert_eq!(out["action"], "import_doi");
    assert_eq!(out["status"], "already_exists");
    assert_eq!(out["code"], "ALREADY_EXISTS");
    assert_eq!(out["DOI"], "10.1000/existing");
    assert_eq!(out["modified"], true);
    assert_eq!(bridge.add_to_collection_calls, 1);
    assert_eq!(bridge.manage_tags_calls, 1);
    assert_eq!(connector.import_calls, 0);
}

#[test]
fn import_doi_translator_success_stops_before_crossref() {
    let runtime = runtime(true);
    let mut bridge = MockBridge {
        doi_imports: vec![BridgeResponse::success(json!({
            "ok": true,
            "code": "IMPORTED",
            "key": "NEW0001",
            "title": "Imported",
            "DOI": "10.2000/new",
            "source": "zotero-translator"
        }))],
        ..Default::default()
    };
    let mut connector = MockConnector::default();
    let mut fetcher = MockFetcher::default();
    let mut opts = options();
    opts.dedupe = false;
    opts.if_exists = "duplicate".to_string();

    let out = add_import::import_doi_with_clients(
        &runtime,
        &mut bridge,
        &mut connector,
        &mut fetcher,
        "10.2000/new",
        opts,
    );

    assert_eq!(out["status"], "success");
    assert_eq!(out["key"], "NEW0001");
    assert_eq!(out["source"], "zotero-translator");
    assert_eq!(connector.import_calls, 0);
    assert_eq!(out["attempts"][0]["step"], "zotero-translator");
}

#[test]
fn import_doi_translator_failure_falls_back_to_crossref_connector_once() {
    let runtime = runtime(true);
    let mut bridge = MockBridge {
        doi_imports: vec![BridgeResponse::success(json!({
            "ok": false,
            "code": "NO_TRANSLATOR",
            "error": "No DOI translators available for 10.3000/fallback"
        }))],
        ..Default::default()
    };
    let mut connector = MockConnector {
        selected: json!({"libraryID":1,"libraryName":"My Library"}),
        imported: vec![json!({"id":"item-1","key":"CROSS001","title":"Crossref"})],
        ..Default::default()
    };
    let mut fetcher = MockFetcher {
        crossref: Some(Ok("@article{fallback}".to_string())),
        ..Default::default()
    };
    let mut opts = options();
    opts.dedupe = false;

    let out = add_import::import_doi_with_clients(
        &runtime,
        &mut bridge,
        &mut connector,
        &mut fetcher,
        "10.3000/fallback",
        opts,
    );

    assert_eq!(out["status"], "success");
    assert_eq!(out["source"], "crossref-bibtex");
    assert_eq!(
        out["translator_error"],
        "No DOI translators available for 10.3000/fallback"
    );
    assert_eq!(connector.import_calls, 1);
    assert_eq!(connector.update_calls[0].0, "C1");
    assert_eq!(
        connector.update_calls[0].1,
        vec!["one".to_string(), "two".to_string()]
    );
    assert_eq!(out["attempts"][0]["step"], "zotero-translator");
    assert_eq!(out["attempts"][1]["step"], "crossref-bibtex");
}

#[test]
fn import_doi_rejects_invalid_inputs_without_mutation() {
    let runtime = runtime(true);
    let mut bridge = MockBridge::default();
    let mut connector = MockConnector::default();
    let mut fetcher = MockFetcher::default();
    let mut opts = options();
    opts.if_exists = "replace".to_string();

    let bad_policy = add_import::import_doi_with_clients(
        &runtime,
        &mut bridge,
        &mut connector,
        &mut fetcher,
        "10.1000/x",
        opts,
    );
    assert_eq!(bad_policy["code"], "INVALID_IF_EXISTS");

    let bad_doi = add_import::import_doi_with_clients(
        &runtime,
        &mut bridge,
        &mut connector,
        &mut fetcher,
        "not-a-doi",
        AddImportOptions::default(),
    );
    assert_eq!(bad_doi["code"], "INVALID_DOI");
    assert_eq!(connector.import_calls, 0);
}

#[test]
fn import_pmid_projects_bridge_application_payload() {
    let mut bridge = MockBridge {
        pmid_imports: vec![BridgeResponse::success(json!({
            "ok": true,
            "code": "IMPORTED",
            "key": "PMID001",
            "title": "PubMed",
            "DOI": "",
            "source": "zotero-translator"
        }))],
        ..Default::default()
    };

    let out = add_import::import_pmid(
        &mut bridge,
        "12345",
        Some("COLLE001"),
        &["tag".to_string()],
        1,
    );

    assert_eq!(out["ok"], true);
    assert_eq!(out["code"], "IMPORTED");
    assert_eq!(out["key"], "PMID001");
}

#[test]
fn add_arxiv_uses_doi_path_then_arxiv_bibtex_fallback() {
    let runtime = runtime(true);
    let mut bridge = MockBridge {
        doi_imports: vec![BridgeResponse::success(json!({
            "ok": false,
            "code": "TRANSLATOR_EMPTY",
            "error": "No items returned"
        }))],
        ..Default::default()
    };
    let mut connector = MockConnector {
        selected: json!({"libraryID":1,"libraryName":"My Library"}),
        imported: vec![json!({"id":"arxiv-1","key":"ARXIV01","title":"Arxiv"})],
        ..Default::default()
    };
    let mut fetcher = MockFetcher {
        crossref: Some(Err(anyhow::anyhow!("crossref unavailable"))),
        arxiv: Some(Ok("@article{arxiv}".to_string())),
        ..Default::default()
    };
    let mut opts = AddImportOptions {
        dedupe: false,
        ..options()
    };

    let out = add_import::add_arxiv_with_clients(
        &runtime,
        &mut bridge,
        &mut connector,
        &mut fetcher,
        "https://arxiv.org/abs/2401.01234v2",
        &mut opts,
    );

    assert_eq!(out["action"], "add_arxiv");
    assert_eq!(out["arxiv_id"], "2401.01234");
    assert_eq!(out["source"], "arxiv-bibtex");
    assert_eq!(out["key"], "ARXIV01");
}

#[test]
fn add_url_routes_doi_and_webpage_shapes() {
    let runtime = runtime(true);
    let mut bridge = MockBridge {
        doi_imports: vec![BridgeResponse::success(json!({
            "ok": true,
            "code": "IMPORTED",
            "key": "DOIURL1",
            "title": "DOI URL",
            "DOI": "10.4000/url",
            "source": "zotero-translator"
        }))],
        ..Default::default()
    };
    let mut connector = MockConnector::default();
    let mut fetcher = MockFetcher::default();
    let mut opts = options();
    opts.dedupe = false;

    let doi = add_import::add_url_with_clients(
        &runtime,
        &mut bridge,
        &mut connector,
        &mut fetcher,
        "https://doi.org/10.4000/url",
        opts.clone(),
    );
    assert_eq!(doi["action"], "add_url");
    assert_eq!(doi["url_kind"], "doi");
    assert_eq!(doi["key"], "DOIURL1");

    let mut webpage_bridge = MockBridge::default();
    let mut webpage_connector = MockConnector {
        selected: json!({"libraryID":1,"libraryName":"My Library"}),
        ..Default::default()
    };
    let webpage = add_import::add_url_with_clients(
        &runtime,
        &mut webpage_bridge,
        &mut webpage_connector,
        &mut MockFetcher::default(),
        "https://example.test/article",
        opts,
    );
    assert_eq!(webpage["url_kind"], "webpage");
    assert_eq!(webpage["title"], "Example Page");
    assert_eq!(webpage_connector.saved_items[0][0]["itemType"], "webpage");
}

#[test]
fn add_file_reports_missing_and_unsupported_without_live_mutation() {
    let runtime = runtime(true);
    let mut bridge = zotero_cli::bridge::JSBridgeClient::new(9);
    let dir = common::TestDir::new("add-file");
    let missing = add_import::add_file(
        &runtime,
        &mut bridge,
        &dir.path().join("missing.pdf"),
        AddImportOptions::default(),
    );
    assert_eq!(missing["code"], "FILE_NOT_FOUND");

    let unsupported_path = dir.path().join("paper.txt");
    std::fs::write(&unsupported_path, "plain").unwrap();
    let unsupported = add_import::add_file(
        &runtime,
        &mut bridge,
        &unsupported_path,
        AddImportOptions::default(),
    );
    assert_eq!(unsupported["code"], "UNSUPPORTED_FILE");
}

#[test]
fn add_bibtex_wraps_import_failure_as_import_failed() {
    let runtime = runtime(false);
    let dir = common::TestDir::new("add-bibtex");
    let bib = dir.path().join("refs.bib");
    std::fs::write(&bib, "@article{x}").unwrap();

    let out = add_import::add_bibtex(
        &runtime,
        &bib,
        Some("COLLE001".to_string()),
        vec!["tag".to_string()],
        session::SessionState::default(),
    );

    assert_eq!(out["action"], "add_bibtex");
    assert_eq!(out["ok"], false);
    assert_eq!(out["code"], "IMPORT_FAILED");
}

#[test]
fn bridge_templates_render_add_import_primitives() {
    let js = bridge::templates::render_import_from_doi(
        1,
        "10.1000/test",
        Some("COLLE001"),
        Some(&["tag".to_string()]),
    )
    .unwrap();
    assert!(js.starts_with("const P = JSON.parse("));
    assert!(js.contains("translate.setIdentifier({DOI: P.doi})"));
    assert!(js.contains("item.addToCollection"));

    let pmid = bridge::templates::render_import_from_pmid(1, "12345", None, None).unwrap();
    assert!(pmid.contains("translate.setIdentifier({PMID: P.pmid})"));

    let dedupe = bridge::templates::render_find_items_by_doi(1, "10.1000/test", 20).unwrap();
    assert!(dedupe.contains("s.addCondition('DOI', 'is', P.doi)"));

    let pdf = bridge::templates::render_standalone_pdf_import(
        1,
        "/tmp/paper.pdf",
        "paper",
        Some("COLLE001"),
        &["tag".to_string()],
    )
    .unwrap();
    assert!(pdf.contains("Zotero.Attachments.importFromFile"));
}
