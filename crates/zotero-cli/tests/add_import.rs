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

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use add_import::{AddImportBridge, AddImportFetcher, AddImportOptions};
use bridge::BridgeResponse;
use import_core::ConnectorImportClient;
use serde_json::{json, Value};
use write::WriteOutcome;

static RUNTIME_COUNTER: AtomicU64 = AtomicU64::new(0);

type ImportCall = (u32, String, Option<String>, Option<Vec<String>>);

#[derive(Default)]
struct MockBridge {
    doi_matches: Vec<Value>,
    find_doi_calls: usize,
    doi_imports: Vec<BridgeResponse>,
    pmid_imports: Vec<BridgeResponse>,
    doi_import_calls: Vec<ImportCall>,
    pmid_import_calls: Vec<ImportCall>,
    add_to_collection_calls: usize,
    manage_tags_calls: usize,
    attach_calls: usize,
    attach_response: Option<BridgeResponse>,
    standalone_pdf: Option<BridgeResponse>,
    standalone_calls: usize,
}

impl AddImportBridge for MockBridge {
    fn find_items_by_doi(&mut self, _library_id: u32, _doi: &str, _limit: i64) -> BridgeResponse {
        self.find_doi_calls += 1;
        BridgeResponse::success(Value::Array(self.doi_matches.drain(..).collect()))
    }

    fn import_from_doi(
        &mut self,
        _library_id: u32,
        _doi: &str,
        _collection_key: Option<&str>,
        _tags: Option<&[String]>,
    ) -> BridgeResponse {
        self.doi_import_calls.push((
            _library_id,
            _doi.to_string(),
            _collection_key.map(str::to_string),
            _tags.map(|tags| tags.to_vec()),
        ));
        self.doi_imports.remove(0)
    }

    fn import_from_pmid(
        &mut self,
        _library_id: u32,
        _pmid: &str,
        _collection_key: Option<&str>,
        _tags: Option<&[String]>,
    ) -> BridgeResponse {
        self.pmid_import_calls.push((
            _library_id,
            _pmid.to_string(),
            _collection_key.map(str::to_string),
            _tags.map(|tags| tags.to_vec()),
        ));
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

    fn attach_pdf(&mut self, _library_id: u32, _item_key: &str, _path: &Path) -> BridgeResponse {
        self.attach_calls += 1;
        self.attach_response.take().unwrap_or_else(|| {
            BridgeResponse::success(Value::String("OK: PDF0001 attached".to_string()))
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
        self.standalone_calls += 1;
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
    save_error: Option<anyhow::Error>,
    import_error: Option<anyhow::Error>,
    update_calls: Vec<(String, Vec<String>)>,
    update_error: Option<anyhow::Error>,
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
        if let Some(err) = self.import_error.take() {
            return Err(err);
        }
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
        if let Some(err) = self.save_error.take() {
            return Err(err);
        }
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
        if let Some(err) = self.update_error.take() {
            return Err(err);
        }
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
    crossref_timeouts: Vec<Duration>,
    arxiv_timeouts: Vec<Duration>,
    html_timeouts: Vec<Duration>,
}

type HtmlProbe = (Option<String>, Option<String>, String);

impl AddImportFetcher for MockFetcher {
    fn fetch_crossref_bibtex(&mut self, _doi: &str, _timeout: Duration) -> anyhow::Result<String> {
        self.crossref_timeouts.push(_timeout);
        self.crossref
            .take()
            .unwrap_or_else(|| Ok("@article{x}".to_string()))
    }

    fn fetch_arxiv_bibtex(
        &mut self,
        _arxiv_id: &str,
        _timeout: Duration,
    ) -> anyhow::Result<String> {
        self.arxiv_timeouts.push(_timeout);
        self.arxiv
            .take()
            .unwrap_or_else(|| Ok("@article{a}".to_string()))
    }

    fn fetch_html_title_and_doi(
        &mut self,
        _url: &str,
        _timeout: Duration,
    ) -> anyhow::Result<(Option<String>, Option<String>, String)> {
        self.html_timeouts.push(_timeout);
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
fn import_pmid_matches_emit_js_require_data_envelopes() {
    let mut transport_failure = MockBridge {
        pmid_imports: vec![BridgeResponse::failure("bridge down".to_string())],
        ..Default::default()
    };
    let failed = add_import::import_pmid(&mut transport_failure, "12345", None, &[], 1);
    assert_eq!(failed["ok"], false);
    assert_eq!(failed["data"], Value::Null);
    assert_eq!(failed["error"], "bridge down");
    assert!(failed.get("status").is_none());
    assert!(failed.get("action").is_none());
    assert!(failed.get("code").is_none());
    assert_eq!(transport_failure.pmid_import_calls.len(), 1);

    let mut empty = MockBridge {
        pmid_imports: vec![BridgeResponse {
            ok: true,
            data: None,
            error: None,
            error_name: None,
            error_stack: None,
            error_raw: None,
        }],
        ..Default::default()
    };
    let empty_out = add_import::import_pmid(&mut empty, "12345", None, &[], 1);
    assert_eq!(empty_out["ok"], false);
    assert_eq!(empty_out["data"], Value::Null);
    assert_eq!(empty_out["code"], "EMPTY_RESULT");
    assert_eq!(
        empty_out["error"],
        "JS bridge returned empty success (data is null)"
    );
    assert!(empty_out.get("status").is_none());

    let mut null_data = MockBridge {
        pmid_imports: vec![BridgeResponse::success(Value::Null)],
        ..Default::default()
    };
    let null_out = add_import::import_pmid(&mut null_data, "12345", None, &[], 1);
    assert_eq!(null_out["ok"], false);
    assert_eq!(null_out["data"], Value::Null);
    assert_eq!(null_out["code"], "EMPTY_RESULT");

    let mut app_error = MockBridge {
        pmid_imports: vec![BridgeResponse::success(json!({
            "ok": false,
            "code": "NO_TRANSLATOR",
            "error": "No PMID translators available"
        }))],
        ..Default::default()
    };
    let app = add_import::import_pmid(
        &mut app_error,
        "12345",
        Some("COLLE001"),
        &[" tag ".to_string()],
        1,
    );
    assert_eq!(app["ok"], false);
    assert_eq!(app["code"], "NO_TRANSLATOR");
    assert_eq!(app["error"], "No PMID translators available");
    assert_eq!(
        app_error.pmid_import_calls[0],
        (
            1,
            "12345".to_string(),
            Some("COLLE001".to_string()),
            Some(vec![" tag ".to_string()])
        )
    );

    let mut string_payload = MockBridge {
        pmid_imports: vec![BridgeResponse::success(Value::String(
            "OK: imported PMID".to_string(),
        ))],
        ..Default::default()
    };
    let string_out = add_import::import_pmid(&mut string_payload, "12345", None, &[], 1);
    assert_eq!(string_out, Value::String("OK: imported PMID".to_string()));
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
fn add_url_covers_arxiv_embedded_doi_http_failure_and_ambiguous_write() {
    let runtime = runtime(true);
    let mut arxiv_bridge = MockBridge {
        doi_imports: vec![BridgeResponse::success(json!({
            "ok": true,
            "code": "IMPORTED",
            "key": "ARXIVDOI",
            "title": "Arxiv DOI",
            "DOI": "10.48550/arXiv.2401.01234",
            "source": "zotero-translator"
        }))],
        ..Default::default()
    };
    let mut arxiv_connector = MockConnector::default();
    let arxiv = add_import::add_url_with_clients(
        &runtime,
        &mut arxiv_bridge,
        &mut arxiv_connector,
        &mut MockFetcher::default(),
        "https://arxiv.org/pdf/2401.01234.pdf",
        AddImportOptions {
            dedupe: false,
            ..options()
        },
    );
    assert_eq!(arxiv["url_kind"], "arxiv");
    assert_eq!(arxiv["arxiv_id"], "2401.01234");
    assert_eq!(arxiv_bridge.doi_import_calls.len(), 1);
    assert_eq!(arxiv_connector.import_calls, 0);

    let mut embedded_bridge = MockBridge {
        doi_imports: vec![BridgeResponse::success(json!({
            "ok": true,
            "code": "IMPORTED",
            "key": "EMBEDDOI",
            "title": "Embedded DOI",
            "DOI": "10.5555/embedded",
            "source": "zotero-translator"
        }))],
        ..Default::default()
    };
    let mut embedded_connector = MockConnector::default();
    let mut embedded_fetcher = MockFetcher {
        html: Some(Ok((
            Some("Ignored after DOI".to_string()),
            Some("10.5555/embedded".to_string()),
            "https://example.test/final".to_string(),
        ))),
        ..Default::default()
    };
    let embedded = add_import::add_url_with_clients(
        &runtime,
        &mut embedded_bridge,
        &mut embedded_connector,
        &mut embedded_fetcher,
        "https://example.test/page",
        AddImportOptions {
            dedupe: false,
            ..options()
        },
    );
    assert_eq!(embedded["url_kind"], "doi");
    assert_eq!(embedded["key"], "EMBEDDOI");
    assert!(embedded_connector.saved_items.is_empty());
    assert_eq!(
        embedded_fetcher.html_timeouts,
        vec![Duration::from_secs(20)]
    );

    let mut failure_connector = MockConnector {
        selected: json!({"libraryID":1,"libraryName":"My Library"}),
        ..Default::default()
    };
    let webpage = add_import::add_url_with_clients(
        &runtime,
        &mut MockBridge::default(),
        &mut failure_connector,
        &mut MockFetcher {
            html: Some(Err(anyhow::anyhow!("network down"))),
            ..Default::default()
        },
        "https://example.test/offline",
        AddImportOptions::default(),
    );
    assert_eq!(webpage["url_kind"], "webpage");
    assert_eq!(webpage["title"], "https://example.test/offline");
    assert_eq!(failure_connector.saved_items.len(), 1);

    let mut ambiguous_connector = MockConnector {
        save_error: Some(anyhow::anyhow!("ambiguous save")),
        ..Default::default()
    };
    let failed = add_import::add_url_with_clients(
        &runtime,
        &mut MockBridge::default(),
        &mut ambiguous_connector,
        &mut MockFetcher::default(),
        "https://example.test/ambiguous",
        AddImportOptions::default(),
    );
    assert_eq!(failed["code"], "IMPORT_FAILED");
    assert_eq!(ambiguous_connector.saved_items.len(), 0);
    assert_eq!(ambiguous_connector.update_calls.len(), 0);
}

#[test]
fn add_url_doi_extraction_stops_before_query_and_fragment() {
    let runtime = runtime(true);
    let mut bridge = MockBridge {
        doi_imports: vec![
            BridgeResponse::success(json!({
                "ok": true,
                "code": "IMPORTED",
                "key": "QUERY01",
                "title": "Query DOI",
                "DOI": "10.1234/abc",
                "source": "zotero-translator"
            })),
            BridgeResponse::success(json!({
                "ok": true,
                "code": "IMPORTED",
                "key": "FRAG001",
                "title": "Fragment DOI",
                "DOI": "10.1234/abc",
                "source": "zotero-translator"
            })),
        ],
        ..Default::default()
    };
    let mut connector = MockConnector::default();
    let mut fetcher = MockFetcher::default();
    let options = AddImportOptions {
        dedupe: false,
        ..options()
    };

    let query = add_import::add_url_with_clients(
        &runtime,
        &mut bridge,
        &mut connector,
        &mut fetcher,
        "https://doi.org/10.1234/abc?ref=xyz",
        options.clone(),
    );
    assert_eq!(query["url_kind"], "doi");
    assert_eq!(query["DOI"], "10.1234/abc");
    assert_eq!(bridge.doi_import_calls[0].1, "10.1234/abc");

    let fragment = add_import::add_url_with_clients(
        &runtime,
        &mut bridge,
        &mut connector,
        &mut fetcher,
        "https://doi.org/10.1234/abc#frag",
        options,
    );
    assert_eq!(fragment["url_kind"], "doi");
    assert_eq!(fragment["DOI"], "10.1234/abc");
    assert_eq!(bridge.doi_import_calls[1].1, "10.1234/abc");
}

#[test]
fn doi_normalization_and_stem_pattern_preserves_special_characters_without_filesystem() {
    let stem_raw = "10.1234_with?query#fragment".replace('_', "/");
    let normalized = import_normalization::normalize_doi(Some(&stem_raw));
    assert_eq!(normalized, "10.1234/with?query#fragment");
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
fn add_file_covers_pdf_doi_attach_and_standalone_branches() {
    let runtime = runtime(true);
    let dir = common::TestDir::new("add-file-pdf");
    let doi_pdf = dir.path().join("10.1234_sample-doi.pdf");
    std::fs::write(&doi_pdf, b"%PDF- fake").unwrap();
    let mut bridge = MockBridge {
        doi_imports: vec![BridgeResponse::success(json!({
            "ok": true,
            "code": "IMPORTED",
            "key": "PDFITEM1",
            "title": "PDF Item",
            "DOI": "10.1234/sample-doi",
            "source": "zotero-translator"
        }))],
        attach_response: Some(BridgeResponse::success(Value::String(
            "OK: ATTACH01 attached to PDF Item".to_string(),
        ))),
        ..Default::default()
    };
    let out = add_import::add_file_with_bridge(
        &runtime,
        &mut bridge,
        &doi_pdf,
        AddImportOptions {
            dedupe: false,
            ..options()
        },
    );
    assert_eq!(out["status"], "success");
    assert_eq!(out["code"], "IMPORTED_WITH_PDF");
    assert_eq!(out["DOI"], "10.1234/sample-doi");
    assert_eq!(bridge.doi_import_calls[0].1, "10.1234/sample-doi");
    assert_eq!(
        out["attach_result"],
        Value::String("OK: ATTACH01 attached to PDF Item".to_string())
    );
    let rendered = out["attach_result"].to_string();
    assert!(!rendered.contains("Ok("));
    assert!(!rendered.contains("Err("));
    assert!(!rendered.contains("WriteOutcome"));
    assert_eq!(bridge.attach_calls, 1);

    let failed_pdf = dir.path().join("10.1234_failed.pdf");
    std::fs::write(&failed_pdf, b"%PDF- fake").unwrap();
    let mut failed_bridge = MockBridge {
        doi_imports: vec![BridgeResponse::success(json!({
            "ok": true,
            "code": "IMPORTED",
            "key": "PDFITEM2",
            "title": "PDF Item",
            "DOI": "10.1234/failed",
            "source": "zotero-translator"
        }))],
        attach_response: Some(BridgeResponse::failure("attach failed".to_string())),
        ..Default::default()
    };
    let failed = add_import::add_file_with_bridge(
        &runtime,
        &mut failed_bridge,
        &failed_pdf,
        AddImportOptions {
            dedupe: false,
            ..options()
        },
    );
    assert_eq!(failed["status"], "partial_success");
    assert_eq!(failed["code"], "IMPORTED_ATTACH_FAILED");
    assert_eq!(failed["error"], "attach failed");
    assert_eq!(failed["attach_result"], Value::Null);

    let standalone_pdf = dir.path().join("plain-paper.pdf");
    std::fs::write(&standalone_pdf, b"%PDF- fake").unwrap();
    let mut standalone_bridge = MockBridge {
        standalone_pdf: Some(BridgeResponse::success(json!({
            "ok": true,
            "key": "PDFONLY1",
            "title": "plain-paper"
        }))),
        ..Default::default()
    };
    let standalone = add_import::add_file_with_bridge(
        &runtime,
        &mut standalone_bridge,
        &standalone_pdf,
        AddImportOptions::default(),
    );
    assert_eq!(standalone["code"], "ATTACHED_STANDALONE");
    assert_eq!(standalone["source"], "attachment-import");
    assert_eq!(standalone_bridge.standalone_calls, 1);
}

#[test]
fn add_file_bibliographic_and_json_branches_fail_before_live_mutation_when_connector_down() {
    let runtime = runtime(false);
    let dir = common::TestDir::new("add-file-import-branches");
    let bib = dir.path().join("refs.bib");
    std::fs::write(&bib, "@article{x}").unwrap();
    let json = dir.path().join("items.json");
    std::fs::write(&json, "[]").unwrap();
    let mut bridge = MockBridge::default();

    let bib_out =
        add_import::add_file_with_bridge(&runtime, &mut bridge, &bib, AddImportOptions::default());
    assert_eq!(bib_out["kind"], Value::Null);
    assert_eq!(bib_out["code"], "IMPORT_FAILED");

    let json_out =
        add_import::add_file_with_bridge(&runtime, &mut bridge, &json, AddImportOptions::default());
    assert_eq!(json_out["code"], "IMPORT_FAILED");
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
fn import_doi_if_exists_modes_crossref_failures_and_no_retry() {
    let runtime = runtime(true);
    let mut skip_bridge = MockBridge {
        doi_matches: vec![json!({"key":"ITEM0001","title":"Existing","DOI":"10.1000/existing"})],
        ..Default::default()
    };
    let mut connector = MockConnector::default();
    let mut fetcher = MockFetcher::default();
    let skip = add_import::import_doi_with_clients(
        &runtime,
        &mut skip_bridge,
        &mut connector,
        &mut fetcher,
        "10.1000/existing",
        AddImportOptions {
            if_exists: "skip".to_string(),
            ..options()
        },
    );
    assert_eq!(skip["status"], "already_exists");
    assert_eq!(skip["modified"], false);
    assert_eq!(skip_bridge.add_to_collection_calls, 0);
    assert_eq!(skip_bridge.manage_tags_calls, 0);

    let mut duplicate_bridge = MockBridge {
        doi_imports: vec![BridgeResponse::success(json!({
            "ok": false,
            "code": "NO_TRANSLATOR",
            "error": "translator failed"
        }))],
        ..Default::default()
    };
    let mut duplicate_connector = MockConnector {
        selected: json!({"libraryID":1,"libraryName":"My Library"}),
        imported: vec![json!({"id":"item-1","key":"DUP0001","title":"Dup"})],
        ..Default::default()
    };
    let duplicate = add_import::import_doi_with_clients(
        &runtime,
        &mut duplicate_bridge,
        &mut duplicate_connector,
        &mut MockFetcher {
            crossref: Some(Ok("@article{dup}".to_string())),
            ..Default::default()
        },
        "10.1000/duplicate",
        AddImportOptions {
            if_exists: "duplicate".to_string(),
            ..options()
        },
    );
    assert_eq!(duplicate["source"], "crossref-bibtex");
    assert_eq!(duplicate_bridge.find_doi_calls, 0);
    assert_eq!(duplicate_connector.import_calls, 1);
    assert_eq!(duplicate_connector.update_calls.len(), 1);

    let mut fail_bridge = MockBridge {
        doi_imports: vec![BridgeResponse::success(json!({
            "ok": false,
            "code": "NO_TRANSLATOR",
            "error": "translator failed"
        }))],
        ..Default::default()
    };
    let mut fail_connector = MockConnector::default();
    let failed = add_import::import_doi_with_clients(
        &runtime,
        &mut fail_bridge,
        &mut fail_connector,
        &mut MockFetcher {
            crossref: Some(Err(anyhow::anyhow!("crossref down"))),
            ..Default::default()
        },
        "10.1000/failure",
        AddImportOptions {
            dedupe: false,
            ..options()
        },
    );
    assert_eq!(failed["code"], "IMPORT_FAILED");
    assert_eq!(fail_bridge.doi_import_calls.len(), 1);
    assert_eq!(fail_connector.import_calls, 0);
}

#[test]
fn add_import_constants_match_python_pdf_fetch_and_http_acquisition() {
    assert_eq!(add_import::ADD_IMPORT_PDF_FETCH_TIMEOUT_SECONDS, 45);
    assert_eq!(
        add_import::ADD_IMPORT_HTTP_USER_AGENT,
        "cli-anything-zotero/1.2 (mailto:cli-anything@local; research agent)"
    );
}

#[test]
fn composed_pdf_fetch_request_uses_python_45_second_timeouts_at_call_site() {
    let mut opts = options();
    opts.fetch_pdf = true;
    opts.library_id = 7;
    opts.pdf_sources = Some("zotero,arxiv".to_string());

    let request = add_import::compose_pdf_fetch_request(&opts, "zotero,unpaywall").unwrap();

    assert_eq!(
        request.sources,
        vec!["zotero".to_string(), "arxiv".to_string()]
    );
    assert_eq!(request.library_id, 7);
    assert_eq!(request.zotero_timeout, 45);
    assert_eq!(request.download_timeout, 45);
}

#[test]
fn generic_html_fetch_sends_python_user_agent_header() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        let size = stream.read(&mut request).unwrap();
        let text = String::from_utf8_lossy(&request[..size]).to_string();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n<html><title>Header Test</title></html>")
            .unwrap();
        text
    });

    let mut fetcher = add_import::UreqAddImportFetcher;
    let (title, doi, _) = fetcher
        .fetch_html_title_and_doi(&url, Duration::from_secs(5))
        .unwrap();
    let request = handle.join().unwrap();
    assert_eq!(title.as_deref(), Some("Header Test"));
    assert_eq!(doi, None);
    assert!(request.to_ascii_lowercase().contains("user-agent:"));
    assert!(request.contains(add_import::ADD_IMPORT_HTTP_USER_AGENT));
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

#[test]
fn bridge_templates_keep_hostile_values_as_json_data() {
    let hostile = "' \" \\ \n Unicode `backtick` ${globalThis.pwned=true} </script> ); throw new Error(\"x\"); //";
    let js = bridge::templates::render_import_from_pmid(
        1,
        hostile,
        Some(hostile),
        Some(&[hostile.to_string()]),
    )
    .unwrap();
    assert!(js.starts_with("const P = JSON.parse("));
    let body = js.lines().skip(1).collect::<Vec<_>>().join("\n");
    assert!(!body.contains(hostile));
    assert!(body.contains("translate.setIdentifier({PMID: P.pmid})"));
    assert!(body.contains("Zotero.Collections.getByLibraryAndKey(P.libraryID, P.collectionKey)"));
    assert!(body.contains("item.addTag(t)"));
    assert!(!body.contains("${globalThis.pwned=true}"));
    assert!(!body.contains("throw new Error(\"x\")"));

    let pdf = bridge::templates::render_standalone_pdf_import(
        1,
        hostile,
        hostile,
        Some(hostile),
        &[hostile.to_string()],
    )
    .unwrap();
    let pdf_body = pdf.lines().skip(1).collect::<Vec<_>>().join("\n");
    assert!(!pdf_body.contains(hostile));
    assert!(pdf_body.contains("P.filePath"));
    assert!(pdf_body.contains("P.title"));
}
