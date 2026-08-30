#![allow(dead_code)]

#[path = "../src/csl.rs"]
mod csl;
#[path = "../src/import_attachments.rs"]
mod import_attachments;
#[path = "../src/import_normalization.rs"]
mod import_normalization;

pub mod http {
    pub use zotero_cli::http::*;
}
pub mod paths {
    pub use zotero_cli::paths::*;
}
pub mod runtime {
    pub use zotero_cli::runtime::*;
}

#[path = "common/mod.rs"]
mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use import_attachments::{
    perform_attachment_upload, AttachmentConnector, RemotePdfFetcher, UreqRemotePdfFetcher,
};
use serde_json::{json, Value};

#[derive(Default)]
struct SavingConnector {
    calls: Vec<(String, String, Vec<u8>)>,
    fail: bool,
}

impl AttachmentConnector for SavingConnector {
    fn save_attachment(
        &mut self,
        _port: u16,
        _session_id: &str,
        parent_item_id: &str,
        _title: &str,
        url: &str,
        content: &[u8],
        _timeout: Duration,
    ) -> anyhow::Result<Value> {
        self.calls.push((
            parent_item_id.to_string(),
            url.to_string(),
            content.to_vec(),
        ));
        if self.fail {
            anyhow::bail!("save failed");
        }
        Ok(json!({}))
    }
}

struct StaticFetcher {
    content: Vec<u8>,
    calls: Vec<String>,
}

impl RemotePdfFetcher for StaticFetcher {
    fn fetch_remote_pdf(
        &mut self,
        url: &str,
        _delay_ms: i64,
        _timeout: i64,
    ) -> anyhow::Result<Vec<u8>> {
        self.calls.push(url.to_string());
        Ok(self.content.clone())
    }
}

fn runtime(port: u16, sqlite_path: &Path) -> zotero_cli::runtime::RuntimeContext {
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
        connector_available: true,
        connector_message: "ok".to_string(),
        local_api_available: false,
        local_api_message: "off".to_string(),
        server_id: None,
        local_api_writes_available: false,
    }
}

fn pdf(path: &Path) {
    std::fs::write(path, b"%PDF-1.7\nbody").unwrap();
}

#[test]
fn local_pdf_upload_and_path_duplicate_are_python_shaped() {
    let dir = common::TestDir::new("attach-local");
    let sqlite = common::build_fixture_sqlite(dir.path());
    let first = dir.path().join("a.pdf");
    pdf(&first);
    let rt = runtime(9, &sqlite);
    let plans = vec![json!({"index": 0, "attachments": [
        {"source_type":"file","source":first,"title":"PDF","delay_ms":0,"timeout":30},
        {"source_type":"file","source":first,"title":"PDF","delay_ms":0,"timeout":30}
    ]})];
    let items = vec![json!({"id":"P1","title":"Article"})];
    let mut connector = SavingConnector::default();
    let mut fetcher = StaticFetcher {
        content: b"%PDF-x".to_vec(),
        calls: Vec::new(),
    };

    let (summary, results) =
        perform_attachment_upload(&rt, "s", &items, &plans, &mut connector, &mut fetcher);

    assert_eq!(summary["planned_count"], 2);
    assert_eq!(summary["created_count"], 1);
    assert_eq!(summary["skipped_count"], 1);
    assert_eq!(results[0]["status"], "created");
    assert_eq!(results[1]["status"], "skipped_duplicate");
    assert_eq!(connector.calls.len(), 1);
    assert!(connector.calls[0].1.starts_with("file://"));
}

#[test]
fn url_and_hash_duplicates_skip_without_second_mutation() {
    let dir = common::TestDir::new("attach-dupe");
    let sqlite = common::build_fixture_sqlite(dir.path());
    let rt = runtime(9, &sqlite);
    let plans = vec![json!({"index": 0, "attachments": [
        {"source_type":"url","source":"HTTPS://Example.TEST/paper.pdf#frag","title":"One","delay_ms":0,"timeout":30},
        {"source_type":"url","source":"https://example.test/paper.pdf","title":"Two","delay_ms":0,"timeout":30},
        {"source_type":"url","source":"https://example.test/other.pdf","title":"Three","delay_ms":0,"timeout":30}
    ]})];
    let items = vec![json!({"id":"P1","title":"Article"})];
    let mut connector = SavingConnector::default();
    let mut fetcher = StaticFetcher {
        content: b"%PDF-same".to_vec(),
        calls: Vec::new(),
    };

    let (summary, results) =
        perform_attachment_upload(&rt, "s", &items, &plans, &mut connector, &mut fetcher);

    assert_eq!(summary["created_count"], 1);
    assert_eq!(summary["skipped_count"], 2);
    assert_eq!(results[1]["status"], "skipped_duplicate");
    assert_eq!(results[2]["status"], "skipped_duplicate");
    assert_eq!(connector.calls.len(), 1);
    assert_eq!(fetcher.calls.len(), 2);
}

#[test]
fn attachment_failures_do_not_rollback_imported_parent() {
    let dir = common::TestDir::new("attach-fail");
    let sqlite = common::build_fixture_sqlite(dir.path());
    let bad = dir.path().join("bad.pdf");
    std::fs::write(&bad, b"not a pdf").unwrap();
    let rt = runtime(9, &sqlite);
    let plans = vec![json!({"index": 0, "attachments": [
        {"source_type":"file","source":bad,"title":"PDF","delay_ms":0,"timeout":30}
    ]})];
    let items = vec![json!({"id":"P1","title":"Article"})];
    let mut connector = SavingConnector::default();
    let mut fetcher = StaticFetcher {
        content: b"%PDF-x".to_vec(),
        calls: Vec::new(),
    };

    let (summary, results) =
        perform_attachment_upload(&rt, "s", &items, &plans, &mut connector, &mut fetcher);

    assert_eq!(summary["failed_count"], 1);
    assert_eq!(results[0]["status"], "failed");
    assert!(results[0]["error"].as_str().unwrap().contains("not a PDF"));
    assert!(connector.calls.is_empty());
}

#[test]
fn parent_preflight_errors_are_reported_per_attachment() {
    let dir = common::TestDir::new("attach-preflight");
    let sqlite = common::build_fixture_sqlite(dir.path());
    let rt = runtime(9, &sqlite);
    let plans = vec![
        json!({"index": 2, "attachments": [{"source_type":"url","source":"https://e.test/a.pdf","title":"PDF","delay_ms":0,"timeout":30}]}),
        json!({"index": 0, "expected_title":"Expected", "attachments": [{"source_type":"url","source":"https://e.test/b.pdf","title":"PDF","delay_ms":0,"timeout":30}]}),
        json!({"index": 1, "attachments": [{"source_type":"url","source":"https://e.test/c.pdf","title":"PDF","delay_ms":0,"timeout":30}]}),
    ];
    let items = vec![
        json!({"id":"P1","title":"Actual"}),
        json!({"title":"No ID"}),
    ];
    let mut connector = SavingConnector::default();
    let mut fetcher = StaticFetcher {
        content: b"%PDF-x".to_vec(),
        calls: Vec::new(),
    };

    let (summary, results) =
        perform_attachment_upload(&rt, "s", &items, &plans, &mut connector, &mut fetcher);

    assert_eq!(summary["failed_count"], 3);
    assert!(results[0]["error"]
        .as_str()
        .unwrap()
        .contains("no item at index 2"));
    assert!(results[1]["error"]
        .as_str()
        .unwrap()
        .contains("title mismatch"));
    assert!(results[2]["error"]
        .as_str()
        .unwrap()
        .contains("did not include a connector id"));
}

#[test]
fn remote_pdf_get_uses_python_accept_header() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let request_head = Arc::new(Mutex::new(String::new()));
    let request_head_clone = Arc::clone(&request_head);
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0; 4096];
        let n = stream.read(&mut buf).unwrap();
        *request_head_clone.lock().unwrap() = String::from_utf8_lossy(&buf[..n]).into_owned();
        let body = b"%PDF-1.7\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
    });

    let mut fetcher = UreqRemotePdfFetcher;
    let content = fetcher
        .fetch_remote_pdf(&format!("http://127.0.0.1:{port}/paper.pdf"), 0, 5)
        .unwrap();
    handle.join().unwrap();

    assert!(content.starts_with(b"%PDF-"));
    let head = request_head.lock().unwrap().to_lowercase();
    assert!(head.contains("accept: application/pdf,application/octet-stream;q=0.9,*/*;q=0.1"));
}
