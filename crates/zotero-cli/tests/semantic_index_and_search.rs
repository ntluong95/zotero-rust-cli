use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use rusqlite::Connection;
use zotero_cli::semantic::embed::{get_embedding, SemanticConfig};
use zotero_cli::semantic::vectors::decode_f32_vector;
use zotero_cli::semantic::{
    build_index, find_similar, load_f32_vectors, semantic_search, BuildIndexOutput,
};

struct TestDir(PathBuf);
impl TestDir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "zotero-test-{}-{}",
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

/// A lightweight mock HTTP server for OpenAI-compatible embedding API.
struct MockEmbeddingServer {
    port: u16,
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl MockEmbeddingServer {
    fn start(status_code: u16) -> Self {
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

                if status_code == 200 {
                    let req_lower = req_str.to_lowercase();
                    // Deterministic mock embedding based on text content
                    let embedding = if req_lower.contains("biology") || req_lower.contains("cell") {
                        vec![0.9f32, 0.1, 0.0]
                    } else if req_lower.contains("physics") || req_lower.contains("quantum") {
                        vec![0.1f32, 0.9, 0.0]
                    } else if req_lower.contains("computer") || req_lower.contains("rust") {
                        vec![0.0f32, 0.1, 0.9]
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
                } else {
                    let err_body = r#"{"error":{"message":"Internal server error"}}"#;
                    let response = format!(
                        "HTTP/1.1 {} Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        status_code,
                        err_body.len(),
                        err_body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                }
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

fn create_mock_zotero_sqlite(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE items (itemID INTEGER PRIMARY KEY, itemTypeID INTEGER, key TEXT);
        CREATE TABLE fields (fieldID INTEGER PRIMARY KEY, fieldName TEXT);
        CREATE TABLE itemData (itemID INTEGER, fieldID INTEGER, valueID INTEGER, PRIMARY KEY (itemID, fieldID));
        CREATE TABLE itemDataValues (valueID INTEGER PRIMARY KEY, value TEXT);",
    )
    .unwrap();

    // Fields: title = 1, abstractNote = 2
    conn.execute("INSERT INTO fields VALUES (1, 'title')", [])
        .unwrap();
    conn.execute("INSERT INTO fields VALUES (2, 'abstractNote')", [])
        .unwrap();

    // Item 1: biology paper (itemTypeID 2 = journalArticle)
    conn.execute("INSERT INTO items VALUES (1, 2, 'BIO0001')", [])
        .unwrap();
    conn.execute(
        "INSERT INTO itemDataValues VALUES (1, 'Advances in Cell Biology')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO itemDataValues VALUES (2, 'Study of living cells and organisms')",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO itemData VALUES (1, 1, 1)", [])
        .unwrap();
    conn.execute("INSERT INTO itemData VALUES (1, 2, 2)", [])
        .unwrap();

    // Item 2: physics paper (itemTypeID 2 = journalArticle)
    conn.execute("INSERT INTO items VALUES (2, 2, 'PHYS002')", [])
        .unwrap();
    conn.execute(
        "INSERT INTO itemDataValues VALUES (3, 'Quantum Physics Principles')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO itemDataValues VALUES (4, 'Theoretical study of quantum states')",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO itemData VALUES (2, 1, 3)", [])
        .unwrap();
    conn.execute("INSERT INTO itemData VALUES (2, 2, 4)", [])
        .unwrap();

    // Item 3: computer science paper (itemTypeID 2)
    conn.execute("INSERT INTO items VALUES (3, 2, 'CS00003')", [])
        .unwrap();
    conn.execute(
        "INSERT INTO itemDataValues VALUES (5, 'Rust Computer Systems')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO itemDataValues VALUES (6, 'Building robust systems with rust compiler')",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO itemData VALUES (3, 1, 5)", [])
        .unwrap();
    conn.execute("INSERT INTO itemData VALUES (3, 2, 6)", [])
        .unwrap();

    // Item 4: note (itemTypeID 1 -> should be skipped)
    conn.execute("INSERT INTO items VALUES (4, 1, 'NOTE004')", [])
        .unwrap();
    conn.execute("INSERT INTO itemDataValues VALUES (7, 'Note Title')", [])
        .unwrap();
    conn.execute("INSERT INTO itemData VALUES (4, 1, 7)", [])
        .unwrap();

    // Item 5: attachment (itemTypeID 14 -> should be skipped)
    conn.execute("INSERT INTO items VALUES (5, 14, 'ATT0005')", [])
        .unwrap();
    conn.execute(
        "INSERT INTO itemDataValues VALUES (8, 'Attachment Title')",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO itemData VALUES (5, 1, 8)", [])
        .unwrap();

    // Item 6: empty text item (has title field but value is empty)
    conn.execute("INSERT INTO items VALUES (6, 2, 'EMPTY06')", [])
        .unwrap();
    conn.execute("INSERT INTO itemDataValues VALUES (9, '   ')", [])
        .unwrap();
    conn.execute("INSERT INTO itemData VALUES (6, 1, 9)", [])
        .unwrap();
}

#[test]
fn test_mock_embed_client() {
    let server = MockEmbeddingServer::start(200);
    let config = SemanticConfig {
        embed_api: server.url(),
        embed_model: "nomic-embed-text".to_string(),
        embed_key: "".to_string(),
        vector_db: PathBuf::from("mock.sqlite"),
    };

    let emb = get_embedding("biology cells", &config).unwrap();
    assert_eq!(emb.len(), 3);
    assert_eq!(emb, vec![0.9, 0.1, 0.0]);
}

#[test]
fn test_build_index_and_search_lifecycle() {
    let server = MockEmbeddingServer::start(200);
    let tmp = TestDir::new("index-search");
    let zotero_sqlite = tmp.path().join("zotero.sqlite");
    let vector_db = tmp.path().join("vectors.sqlite");

    create_mock_zotero_sqlite(&zotero_sqlite);

    let config = SemanticConfig {
        embed_api: server.url(),
        embed_model: "nomic-embed-text".to_string(),
        embed_key: "".to_string(),
        vector_db: vector_db.clone(),
    };

    // 1. Build index
    let out = build_index(&zotero_sqlite, &config, 20);
    match out {
        BuildIndexOutput::Success(s) => {
            assert!(s.ok);
            // 3 valid items (BIO0001, PHYS002, CS00003), 1 empty text item skipped, items 4 & 5 excluded by SQL itemTypeID
            assert_eq!(s.indexed, 3);
            assert_eq!(s.skipped, 1);
            assert_eq!(s.total, 4);
        }
        BuildIndexOutput::Failure(f) => panic!("build_index failed: {}", f.error),
    }

    // 2. Re-run build index: should skip already indexed keys
    let out_rerun = build_index(&zotero_sqlite, &config, 20);
    match out_rerun {
        BuildIndexOutput::Success(s) => {
            assert!(s.ok);
            assert_eq!(s.indexed, 0);
            assert_eq!(s.skipped, 4);
        }
        BuildIndexOutput::Failure(f) => panic!("re-run build_index failed: {}", f.error),
    }

    // 3. Semantic search for biology query
    let results = semantic_search("biology cells research", &config, 10, 0.3, "all").unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].item_key, "BIO0001");
    assert!(results[0].score > 0.8);
    assert_eq!(results[0].language, "en");

    // 4. Semantic search with language filter
    let results_zh = semantic_search("biology cells", &config, 10, 0.3, "zh").unwrap();
    assert_eq!(results_zh.len(), 0); // No zh items in fixture

    let results_en = semantic_search("biology cells", &config, 10, 0.0, "en").unwrap();
    assert_eq!(results_en.len(), 3);

    // 5. Semantic search with top_k limit
    let results_top1 = semantic_search("biology", &config, 1, 0.0, "all").unwrap();
    assert_eq!(results_top1.len(), 1);

    // 6. Find similar items
    let similar = find_similar("BIO0001", &config, 5, 0.0).unwrap();
    // BIO0001 excluded from its own similar list
    assert_eq!(similar.len(), 2);
    for item in &similar {
        assert_ne!(item.item_key, "BIO0001");
    }

    // 7. Find similar on non-existent item
    let err_similar = find_similar("NONEXISTENT", &config, 5, 0.5).unwrap_err();
    assert!(err_similar["error"]
        .as_str()
        .unwrap()
        .contains("No embedding for item NONEXISTENT"));
}

#[test]
fn test_missing_dbs_and_api_error_handling() {
    let tmp = TestDir::new("errors");
    let missing_zotero = tmp.path().join("nonexistent_zotero.sqlite");
    let missing_vector_db = tmp.path().join("nonexistent_vector.sqlite");

    let config = SemanticConfig {
        embed_api: "http://127.0.0.1:9999/v1/embeddings".to_string(),
        embed_model: "nomic-embed-text".to_string(),
        embed_key: "".to_string(),
        vector_db: missing_vector_db.clone(),
    };

    // Missing zotero DB
    let out = build_index(&missing_zotero, &config, 20);
    match out {
        BuildIndexOutput::Failure(f) => {
            assert!(!f.ok);
            assert!(f.error.contains("Zotero DB not found"));
        }
        BuildIndexOutput::Success(_) => panic!("Expected failure for missing Zotero DB"),
    }

    // Missing vector DB in search
    let search_err = semantic_search("test query", &config, 10, 0.3, "all").unwrap_err();
    assert!(search_err["error"]
        .as_str()
        .unwrap()
        .contains("Vector DB not found"));

    // Missing vector DB in similar
    let similar_err = find_similar("KEY1", &config, 5, 0.5).unwrap_err();
    assert!(similar_err["error"]
        .as_str()
        .unwrap()
        .contains("Vector DB not found"));

    // Server returning 500
    let server500 = MockEmbeddingServer::start(500);
    let config500 = SemanticConfig {
        embed_api: server500.url(),
        embed_model: "nomic-embed-text".to_string(),
        embed_key: "".to_string(),
        vector_db: tmp.path().join("vecs500.sqlite"),
    };
    let embed_err = get_embedding("test", &config500).unwrap_err();
    assert!(embed_err.to_string().contains("HTTP 500"));
}

#[test]
fn test_python_vector_db_interoperability() {
    // Test that Rust can read a vector DB created by Python struct.pack f32
    let tmp = TestDir::new("python-interop");
    let vector_db_path = tmp.path().join("python_vectors.sqlite");

    let conn = Connection::open(&vector_db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE embeddings (
            item_key TEXT, chunk_id INTEGER, chunk_text TEXT, language TEXT,
            PRIMARY KEY (item_key, chunk_id));
        CREATE TABLE vectors_f32 (
            item_key TEXT, chunk_id INTEGER, vector BLOB,
            PRIMARY KEY (item_key, chunk_id));",
    )
    .unwrap();

    // Python-encoded little-endian f32 vectors
    // Vec 1: [1.0, 0.0, 0.0]
    let vec1_bytes: Vec<u8> = vec![
        0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    // Vec 2: [0.0, 1.0, 0.0]
    let vec2_bytes: Vec<u8> = vec![
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x00,
    ];

    conn.execute(
        "INSERT INTO embeddings VALUES ('PYITEM1', 0, 'Python item 1 text chunk', 'en')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO vectors_f32 VALUES ('PYITEM1', 0, ?1)",
        rusqlite::params![vec1_bytes],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO embeddings VALUES ('PYITEM2', 0, 'Python item 2 text chunk', 'zh')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO vectors_f32 VALUES ('PYITEM2', 0, ?1)",
        rusqlite::params![vec2_bytes],
    )
    .unwrap();

    drop(conn);

    // Read through Rust load_f32_vectors
    let ro_conn = zotero_cli::semantic::connect_vector_db_ro(&vector_db_path).unwrap();
    let rows = load_f32_vectors(&ro_conn, "all", None).unwrap();
    assert_eq!(rows.len(), 2);

    let v1 = decode_f32_vector(&rows[0].vector_blob).unwrap();
    assert_eq!(v1, vec![1.0, 0.0, 0.0]);

    let v2 = decode_f32_vector(&rows[1].vector_blob).unwrap();
    assert_eq!(v2, vec![0.0, 1.0, 0.0]);
}
