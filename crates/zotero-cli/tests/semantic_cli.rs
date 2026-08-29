use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use rusqlite::Connection;
use serde_json::Value;

struct TestDir(PathBuf);
impl TestDir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "zotero-test-cli-{}-{}",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&p);
        Self(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct MockEmbeddingServer {
    port: u16,
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl MockEmbeddingServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let handle = thread::spawn(move || {
            for stream_res in listener.incoming() {
                if !running_clone.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(mut stream) = stream_res else {
                    continue;
                };
                let mut buffer = Vec::new();
                let mut temp = [0u8; 1024];
                loop {
                    let n = match stream.read(&mut temp) {
                        Ok(0) => break,
                        Ok(bytes) => bytes,
                        Err(_) => break,
                    };
                    buffer.extend_from_slice(&temp[..n]);
                    if let Some(pos) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&buffer[..pos]);
                        let mut content_length = 0usize;
                        for line in headers.lines() {
                            if let Some((k, v)) = line.split_once(':') {
                                if k.trim().eq_ignore_ascii_case("content-length") {
                                    content_length = v.trim().parse().unwrap_or(0);
                                }
                            }
                        }
                        let body_read = buffer.len() - (pos + 4);
                        if body_read >= content_length {
                            break;
                        }
                    }
                }

                let req_str = String::from_utf8_lossy(&buffer);
                let req_lower = req_str.to_lowercase();

                let embedding = if req_lower.contains("biology") || req_lower.contains("cell") {
                    vec![0.9f32, 0.1, 0.0]
                } else if req_lower.contains("physics") || req_lower.contains("quantum") {
                    vec![0.1f32, 0.9, 0.0]
                } else {
                    vec![0.5f32, 0.5, 0.5]
                };

                let resp_body = serde_json::json!({
                    "data": [
                        {
                            "embedding": embedding,
                            "index": 0,
                            "object": "embedding"
                        }
                    ],
                    "model": "nomic-embed-text",
                    "object": "list"
                });
                let body_bytes = serde_json::to_vec(&resp_body).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body_bytes.len(),
                    String::from_utf8_lossy(&body_bytes)
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });

        Self {
            port,
            running,
            handle: Some(handle),
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/v1/embeddings", self.port)
    }
}

impl Drop for MockEmbeddingServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        let _ = TcpStream::connect(format!("127.0.0.1:{}", self.port));
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn bin_path() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("zotero-cli")
}

fn create_mock_zotero_sqlite(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE items (itemID INTEGER PRIMARY KEY, itemTypeID INTEGER, key TEXT);
        CREATE TABLE fields (fieldID INTEGER PRIMARY KEY, fieldName TEXT);
        CREATE TABLE itemData (itemID INTEGER, fieldID INTEGER, valueID INTEGER, PRIMARY KEY (itemID, fieldID));
        CREATE TABLE itemDataValues (valueID INTEGER PRIMARY KEY, value TEXT);",
    )
    .unwrap();

    conn.execute("INSERT INTO fields VALUES (1, 'title')", [])
        .unwrap();
    conn.execute("INSERT INTO fields VALUES (2, 'abstractNote')", [])
        .unwrap();

    conn.execute("INSERT INTO items VALUES (1, 2, 'ITEM_BIO')", [])
        .unwrap();
    conn.execute(
        "INSERT INTO itemDataValues VALUES (1, 'Cell Biology Research')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO itemDataValues VALUES (2, 'Abstract about living cells')",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO itemData VALUES (1, 1, 1)", [])
        .unwrap();
    conn.execute("INSERT INTO itemData VALUES (1, 2, 2)", [])
        .unwrap();

    conn.execute("INSERT INTO items VALUES (2, 2, 'ITEM_PHYS')", [])
        .unwrap();
    conn.execute(
        "INSERT INTO itemDataValues VALUES (3, 'Quantum Physics Dynamics')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO itemDataValues VALUES (4, 'Abstract about quantum physics')",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO itemData VALUES (2, 1, 3)", [])
        .unwrap();
    conn.execute("INSERT INTO itemData VALUES (2, 2, 4)", [])
        .unwrap();
}

#[test]
fn test_cli_help_for_phase8_commands() {
    let bin = bin_path();

    // item build-index --help
    let output = Command::new(&bin)
        .args(["item", "build-index", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("build-index"));

    // item semantic-search --help
    let output = Command::new(&bin)
        .args(["item", "semantic-search", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--top-k"));
    assert!(stdout.contains("--min-score"));
    assert!(stdout.contains("--language"));

    // item similar --help
    let output = Command::new(&bin)
        .args(["item", "similar", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--top-k"));
    assert!(stdout.contains("--min-score"));
}

#[test]
fn test_cli_missing_database_error_payloads() {
    let bin = bin_path();
    let tmp = TestDir::new("cli-errors");

    let vector_db = tmp.path().join("missing_vec.sqlite");

    // item build-index with non-existent zotero data dir
    let output = Command::new(&bin)
        .args([
            "--json",
            "--data-dir",
            tmp.path().to_str().unwrap(),
            "item",
            "build-index",
        ])
        .env("ZOTERO_VECTOR_DB", vector_db.to_str().unwrap())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["indexed"], 0);
    assert_eq!(json["skipped"], 0);
    assert!(json["error"]
        .as_str()
        .unwrap()
        .contains("Zotero DB not found"));

    // item semantic-search with missing vector db
    let output_search = Command::new(&bin)
        .args(["--json", "item", "semantic-search", "biology"])
        .env("ZOTERO_VECTOR_DB", vector_db.to_str().unwrap())
        .output()
        .unwrap();
    assert_eq!(output_search.status.code(), Some(1));
    let stdout_search = String::from_utf8_lossy(&output_search.stdout);
    let json_search: Value = serde_json::from_str(stdout_search.trim()).unwrap();
    assert_eq!(json_search["ok"], false);
    assert_eq!(json_search["data"], Value::Null);
    assert!(json_search["error"]
        .as_str()
        .unwrap()
        .contains("Vector DB not found"));

    // item similar with missing vector db
    let output_sim = Command::new(&bin)
        .args(["--json", "item", "similar", "ITEM_BIO"])
        .env("ZOTERO_VECTOR_DB", vector_db.to_str().unwrap())
        .output()
        .unwrap();
    assert_eq!(output_sim.status.code(), Some(1));
    let stdout_sim = String::from_utf8_lossy(&output_sim.stdout);
    let json_sim: Value = serde_json::from_str(stdout_sim.trim()).unwrap();
    assert_eq!(json_sim["ok"], false);
    assert_eq!(json_sim["data"], Value::Null);
    assert!(json_sim["error"]
        .as_str()
        .unwrap()
        .contains("Vector DB not found"));
}

#[test]
fn test_cli_end_to_end_workflow() {
    let bin = bin_path();
    let server = MockEmbeddingServer::start();
    let tmp = TestDir::new("cli-e2e");
    let data_dir = tmp.path().join("zotero_data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let zotero_sqlite = data_dir.join("zotero.sqlite");
    create_mock_zotero_sqlite(&zotero_sqlite);

    let vector_db = tmp.path().join("vectors.sqlite");

    // 1. Run build-index --json
    let out_build = Command::new(&bin)
        .args([
            "--json",
            "--data-dir",
            data_dir.to_str().unwrap(),
            "item",
            "build-index",
        ])
        .env("ZOTERO_EMBED_API", server.url())
        .env("ZOTERO_VECTOR_DB", vector_db.to_str().unwrap())
        .output()
        .unwrap();
    assert_eq!(out_build.status.code(), Some(0));
    let json_build: Value =
        serde_json::from_str(String::from_utf8_lossy(&out_build.stdout).trim()).unwrap();
    assert_eq!(json_build["ok"], true);
    assert_eq!(json_build["indexed"], 2);
    assert_eq!(json_build["skipped"], 0);
    assert_eq!(json_build["total"], 2);

    // 2. Run semantic-search --json
    let out_search = Command::new(&bin)
        .args([
            "--json",
            "item",
            "semantic-search",
            "cells and biology",
            "--top-k",
            "5",
            "--min-score",
            "0.3",
        ])
        .env("ZOTERO_EMBED_API", server.url())
        .env("ZOTERO_VECTOR_DB", vector_db.to_str().unwrap())
        .output()
        .unwrap();
    assert_eq!(out_search.status.code(), Some(0));
    let json_search: Value =
        serde_json::from_str(String::from_utf8_lossy(&out_search.stdout).trim()).unwrap();
    assert!(json_search.is_array());
    let results = json_search.as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["item_key"], "ITEM_BIO");
    assert_eq!(results[0]["score"], 1.0);

    // 3. Run semantic-search in human mode (non-JSON)
    let out_search_human = Command::new(&bin)
        .args(["item", "semantic-search", "cells and biology"])
        .env("ZOTERO_EMBED_API", server.url())
        .env("ZOTERO_VECTOR_DB", vector_db.to_str().unwrap())
        .output()
        .unwrap();
    assert_eq!(out_search_human.status.code(), Some(0));
    let stdout_human = String::from_utf8_lossy(&out_search_human.stdout);
    assert!(stdout_human.contains("ITEM_BIO"));

    // 4. Run item similar --json
    let out_sim = Command::new(&bin)
        .args([
            "--json",
            "item",
            "similar",
            "ITEM_BIO",
            "--top-k",
            "5",
            "--min-score",
            "0.0",
        ])
        .env("ZOTERO_EMBED_API", server.url())
        .env("ZOTERO_VECTOR_DB", vector_db.to_str().unwrap())
        .output()
        .unwrap();
    assert_eq!(out_sim.status.code(), Some(0));
    let json_sim: Value =
        serde_json::from_str(String::from_utf8_lossy(&out_sim.stdout).trim()).unwrap();
    assert!(json_sim.is_array());
    let sim_results = json_sim.as_array().unwrap();
    assert_eq!(sim_results.len(), 1);
    assert_eq!(sim_results[0]["item_key"], "ITEM_PHYS");
}
