#![allow(dead_code)]

#[path = "../src/csl.rs"]
mod csl;
#[path = "../src/import_attachments.rs"]
mod import_attachments;
#[path = "../src/import_core.rs"]
mod import_core;
#[path = "../src/import_normalization.rs"]
mod import_normalization;

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

#[path = "common/mod.rs"]
mod common;

use std::path::Path;
use std::time::Duration;

use import_attachments::{AttachmentConnector, RemotePdfFetcher};
use import_core::{ConnectorImportClient, ImportOptions};
use serde_json::{json, Value};

#[derive(Default)]
struct MockConnector {
    selected: Value,
    import_results: Vec<anyhow::Result<Vec<Value>>>,
    save_items_result: Option<anyhow::Result<()>>,
    update_result: Option<anyhow::Result<Value>>,
    import_calls: Vec<(String, String)>,
    save_items_calls: Vec<Vec<Value>>,
    update_calls: Vec<(String, String, Vec<String>)>,
    selected_calls: usize,
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
        self.import_calls.push((
            content_type.to_string(),
            String::from_utf8_lossy(content).into_owned(),
        ));
        self.import_results.remove(0)
    }

    fn save_items(
        &mut self,
        _port: u16,
        items: &[Value],
        _session_id: &str,
        _timeout: Duration,
    ) -> anyhow::Result<()> {
        self.save_items_calls.push(items.to_vec());
        self.save_items_result.take().unwrap_or(Ok(()))
    }

    fn update_session(
        &mut self,
        _port: u16,
        session_id: &str,
        target: &str,
        tags: &[String],
        _timeout: Duration,
    ) -> anyhow::Result<Value> {
        self.update_calls
            .push((session_id.to_string(), target.to_string(), tags.to_vec()));
        self.update_result.take().unwrap_or(Ok(json!({})))
    }

    fn get_selected_collection(&mut self, _port: u16, _timeout: Duration) -> anyhow::Result<Value> {
        self.selected_calls += 1;
        Ok(self.selected.clone())
    }
}

#[derive(Default)]
struct MockAttachmentConnector {
    calls: Vec<(String, String)>,
    fail: bool,
}

impl AttachmentConnector for MockAttachmentConnector {
    fn save_attachment(
        &mut self,
        _port: u16,
        _session_id: &str,
        parent_item_id: &str,
        _title: &str,
        url: &str,
        _content: &[u8],
        _timeout: Duration,
    ) -> anyhow::Result<Value> {
        self.calls
            .push((parent_item_id.to_string(), url.to_string()));
        if self.fail {
            anyhow::bail!("attachment write failed");
        }
        Ok(json!({}))
    }
}

struct MockFetcher {
    content: Vec<u8>,
}

impl RemotePdfFetcher for MockFetcher {
    fn fetch_remote_pdf(
        &mut self,
        _url: &str,
        _delay_ms: i64,
        _timeout: i64,
    ) -> anyhow::Result<Vec<u8>> {
        Ok(self.content.clone())
    }
}

fn runtime(
    port: u16,
    sqlite_path: &Path,
    connector_available: bool,
) -> zotero_cli::runtime::RuntimeContext {
    zotero_cli::runtime::RuntimeContext {
        environment: zotero_cli::paths::ZoteroEnvironment {
            executable: None,
            executable_exists: false,
            install_dir: None,
            version: "unknown".to_string(),
            profile_root: sqlite_path.parent().unwrap().to_path_buf(),
            profile_dir: None,
            data_dir: sqlite_path.parent().unwrap().to_path_buf(),
            data_dir_exists: true,
            sqlite_path: sqlite_path.to_path_buf(),
            sqlite_exists: sqlite_path.exists(),
            styles_dir: sqlite_path.parent().unwrap().join("styles"),
            styles_exists: false,
            storage_dir: sqlite_path.parent().unwrap().join("storage"),
            storage_exists: false,
            translators_dir: sqlite_path.parent().unwrap().join("translators"),
            translators_exists: false,
            port,
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

fn options() -> ImportOptions {
    ImportOptions {
        connector_timeout: Duration::from_secs(7),
        ..Default::default()
    }
}

#[test]
fn import_json_saveitems_update_session_and_summary_projection() {
    let dir = common::TestDir::new("import-json");
    let sqlite = common::build_fixture_sqlite(dir.path());
    let source = dir.path().join("items.json");
    std::fs::write(
        &source,
        r#"[{"itemType":"journalArticle","title":"Saved","attachments":[{"url":"https://example.test/a.pdf"}]}]"#,
    ).unwrap();
    let mut client = MockConnector {
        selected: json!({"libraryID":1,"libraryName":"My Library"}),
        ..Default::default()
    };
    let mut attachment = MockAttachmentConnector::default();
    let mut fetcher = MockFetcher {
        content: b"%PDF-inline".to_vec(),
    };

    let out = import_core::import_json_with_clients(
        &runtime(5, &sqlite, true),
        &source,
        options(),
        &mut client,
        &mut attachment,
        &mut fetcher,
    )
    .unwrap();

    assert_eq!(out["action"], "import_json");
    assert_eq!(out["status"], "success");
    assert_eq!(out["format"], "connector");
    assert_eq!(out["submitted_count"], 1);
    assert!(client.save_items_calls[0][0].get("attachments").is_none());
    assert_eq!(
        out["items"],
        json!([{"id":"cli-anything-zotero-1","itemType":"journalArticle","title":"Saved"}])
    );
    assert_eq!(client.update_calls[0].1, "L1");
    assert_eq!(attachment.calls.len(), 1);
}

#[test]
fn import_json_target_resolution_and_trimmed_tags() {
    let dir = common::TestDir::new("target");
    let sqlite = common::build_fixture_sqlite(dir.path());
    let source = dir.path().join("items.json");
    std::fs::write(&source, r#"[{"title":"Fallback"}]"#).unwrap();
    let mut opts = options();
    opts.collection_ref = Some("COLLE001".to_string());
    opts.tags = vec![" one ".to_string(), "".to_string(), "two".to_string()];
    let mut client = MockConnector::default();
    let mut attachment = MockAttachmentConnector::default();
    let mut fetcher = MockFetcher {
        content: b"%PDF-x".to_vec(),
    };

    let out = import_core::import_json_with_clients(
        &runtime(5, &sqlite, true),
        &source,
        opts,
        &mut client,
        &mut attachment,
        &mut fetcher,
    )
    .unwrap();

    assert_eq!(out["target"]["treeViewID"], "C1");
    assert_eq!(out["target"]["source"], "explicit");
    assert_eq!(out["tags"], json!(["one", "two"]));
    assert_eq!(client.selected_calls, 0);
    assert_eq!(client.update_calls[0].1, "C1");
}

#[test]
fn explicit_whitespace_collection_ref_errors_instead_of_falling_back() {
    let dir = common::TestDir::new("target-whitespace");
    let sqlite = common::build_fixture_sqlite(dir.path());
    let mut opts = options();
    opts.collection_ref = Some("   ".to_string());
    let mut client = MockConnector {
        selected: json!({"libraryID":1,"libraryName":"My Library"}),
        ..Default::default()
    };

    let err = import_core::resolve_target(
        &runtime(5, &sqlite, true),
        opts.collection_ref.as_deref(),
        &opts.session,
        &mut client,
    )
    .unwrap_err();

    assert!(err.to_string().contains("Collection not found"));
    assert_eq!(client.selected_calls, 0);
}

#[test]
fn empty_collection_ref_is_absent_and_can_use_selected_target() {
    let dir = common::TestDir::new("target-empty");
    let sqlite = common::build_fixture_sqlite(dir.path());
    let mut opts = options();
    opts.collection_ref = Some(String::new());
    let mut client = MockConnector {
        selected: json!({"id":9,"name":"Chosen","libraryID":1,"libraryName":"My Library"}),
        ..Default::default()
    };

    let target = import_core::resolve_target(
        &runtime(5, &sqlite, true),
        opts.collection_ref.as_deref(),
        &opts.session,
        &mut client,
    )
    .unwrap();

    assert_eq!(target["source"], "selected");
    assert_eq!(target["treeViewID"], "C9");
    assert_eq!(client.selected_calls, 1);
}

#[test]
fn import_file_content_type_mappings_and_errors() {
    let dir = common::TestDir::new("import-file-map");
    let sqlite = common::build_fixture_sqlite(dir.path());
    let rt = runtime(5, &sqlite, true);
    for (name, content_type) in [
        ("one.ris", "application/x-research-info-systems"),
        ("one.bib", "text/x-bibtex"),
        ("one.csv", "text/csv"),
        ("one.txt", "text/plain"),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, "@article{x,title={A}}").unwrap();
        let mut client = MockConnector {
            selected: json!({"libraryID":1,"libraryName":"My Library"}),
            import_results: vec![Ok(vec![json!({"id":"I1","title":"A"})])],
            ..Default::default()
        };
        let mut attachment = MockAttachmentConnector::default();
        let mut fetcher = MockFetcher {
            content: b"%PDF-x".to_vec(),
        };
        import_core::import_file_with_clients(
            &rt,
            &path,
            options(),
            &mut client,
            &mut attachment,
            &mut fetcher,
        )
        .unwrap();
        assert_eq!(client.import_calls[0].0, content_type);
    }
    let missing = import_core::import_file_with_clients(
        &rt,
        &dir.path().join("missing.ris"),
        options(),
        &mut MockConnector::default(),
        &mut MockAttachmentConnector::default(),
        &mut MockFetcher {
            content: b"%PDF-x".to_vec(),
        },
    )
    .unwrap_err();
    assert!(missing.to_string().contains("Import file not found"));
}

#[test]
fn connector_unavailable_and_invalid_json_fail_before_mutation() {
    let dir = common::TestDir::new("preflight");
    let sqlite = common::build_fixture_sqlite(dir.path());
    let path = dir.path().join("bad.json");
    std::fs::write(&path, "{").unwrap();
    let mut client = MockConnector::default();
    let err = import_core::import_json_with_clients(
        &runtime(5, &sqlite, false),
        &path,
        options(),
        &mut client,
        &mut MockAttachmentConnector::default(),
        &mut MockFetcher {
            content: b"%PDF-x".to_vec(),
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("connector is not available"));
    assert!(client.save_items_calls.is_empty());

    let err = import_core::import_json_with_clients(
        &runtime(5, &sqlite, true),
        &path,
        options(),
        &mut client,
        &mut MockAttachmentConnector::default(),
        &mut MockFetcher {
            content: b"%PDF-x".to_vec(),
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("Invalid JSON import file"));
    assert!(client.save_items_calls.is_empty());
}

#[test]
fn attachment_partial_success_does_not_rollback_json_import() {
    let dir = common::TestDir::new("partial");
    let sqlite = common::build_fixture_sqlite(dir.path());
    let source = dir.path().join("items.json");
    std::fs::write(
        &source,
        r#"[{"itemType":"journalArticle","title":"Saved","attachments":[{"url":"https://example.test/a.pdf"}]}]"#,
    ).unwrap();
    let mut client = MockConnector {
        selected: json!({"libraryID":1,"libraryName":"My Library"}),
        ..Default::default()
    };
    let mut attachment = MockAttachmentConnector {
        fail: true,
        ..Default::default()
    };
    let mut fetcher = MockFetcher {
        content: b"%PDF-inline".to_vec(),
    };

    let out = import_core::import_json_with_clients(
        &runtime(5, &sqlite, true),
        &source,
        options(),
        &mut client,
        &mut attachment,
        &mut fetcher,
    )
    .unwrap();

    assert_eq!(out["status"], "partial_success");
    assert_eq!(out["attachment_summary"]["failed_count"], 1);
    assert_eq!(client.save_items_calls.len(), 1);
}

#[test]
fn attachment_manifest_errors_are_python_compatible() {
    let dir = common::TestDir::new("manifest");
    let malformed = dir.path().join("bad.json");
    std::fs::write(&malformed, r#"[{"index":0,"attachments":[1]}]"#).unwrap();
    let err = import_core::read_attachment_manifest(&malformed, 0, 30).unwrap_err();
    assert!(err.to_string().contains("attachment 1 must be an object"));

    let out_of_range = dir.path().join("oor.json");
    std::fs::write(
        &out_of_range,
        r#"[{"index":2,"attachments":[{"url":"https://example.test/a.pdf"}]}]"#,
    )
    .unwrap();
    let sqlite = common::build_fixture_sqlite(dir.path());
    let import_source = dir.path().join("one.ris");
    std::fs::write(&import_source, "TY  - JOUR").unwrap();
    let mut opts = options();
    opts.attachment_manifest = Some(out_of_range);
    let mut client = MockConnector {
        selected: json!({"libraryID":1,"libraryName":"My Library"}),
        import_results: vec![Ok(vec![json!({"id":"I1","title":"A"})])],
        ..Default::default()
    };
    let out = import_core::import_file_with_clients(
        &runtime(5, &sqlite, true),
        &import_source,
        opts,
        &mut client,
        &mut MockAttachmentConnector::default(),
        &mut MockFetcher {
            content: b"%PDF-x".to_vec(),
        },
    )
    .unwrap();
    assert_eq!(out["status"], "partial_success");
    assert!(out["attachment_results"][0]["error"]
        .as_str()
        .unwrap()
        .contains("no item at index 2"));
}

#[test]
fn split_bibtex_reports_success_partial_and_error_without_retries() {
    let dir = common::TestDir::new("split");
    let sqlite = common::build_fixture_sqlite(dir.path());
    let source = dir.path().join("many.bib");
    std::fs::write(&source, "@article{a,title={A}}\n@article{b,title={B}}").unwrap();

    for (results, status, imported, failed) in [
        (
            vec![Ok(vec![json!({"id":"A"})]), Ok(vec![json!({"id":"B"})])],
            "success",
            2,
            0,
        ),
        (
            vec![Ok(vec![json!({"id":"A"})]), Err(anyhow::anyhow!("bad"))],
            "partial_success",
            1,
            1,
        ),
        (
            vec![Err(anyhow::anyhow!("bad a")), Err(anyhow::anyhow!("bad b"))],
            "error",
            0,
            2,
        ),
    ] {
        let mut opts = options();
        opts.split_bib = true;
        let mut client = MockConnector {
            selected: json!({"libraryID":1,"libraryName":"My Library"}),
            import_results: results,
            ..Default::default()
        };
        let out = import_core::import_file_with_clients(
            &runtime(5, &sqlite, true),
            &source,
            opts,
            &mut client,
            &mut MockAttachmentConnector::default(),
            &mut MockFetcher {
                content: b"%PDF-x".to_vec(),
            },
        )
        .unwrap();
        assert_eq!(out["status"], status);
        assert_eq!(out["imported_count"], imported);
        assert_eq!(out["failed_count"], failed);
        assert_eq!(client.import_calls.len(), 2);
    }
}

#[test]
fn update_session_error_propagates_without_second_mutation() {
    let dir = common::TestDir::new("update-error");
    let sqlite = common::build_fixture_sqlite(dir.path());
    let source = dir.path().join("one.ris");
    std::fs::write(&source, "TY  - JOUR").unwrap();
    let mut client = MockConnector {
        selected: json!({"libraryID":1,"libraryName":"My Library"}),
        import_results: vec![Ok(vec![json!({"id":"I1","title":"A"})])],
        update_result: Some(Err(anyhow::anyhow!("ambiguous mutation"))),
        ..Default::default()
    };

    let err = import_core::import_file_with_clients(
        &runtime(5, &sqlite, true),
        &source,
        options(),
        &mut client,
        &mut MockAttachmentConnector::default(),
        &mut MockFetcher {
            content: b"%PDF-x".to_vec(),
        },
    )
    .unwrap_err();

    assert!(err.to_string().contains("ambiguous mutation"));
    assert_eq!(client.import_calls.len(), 1);
    assert_eq!(client.update_calls.len(), 1);
}
