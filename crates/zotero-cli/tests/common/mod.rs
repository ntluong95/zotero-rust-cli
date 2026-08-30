//! Shared CLI-level (subprocess + mock-server) test harness for Phase 6 Slice 6's routing
//! tests. Not a test binary itself (`tests/common/` is not a direct child of `tests/`, so cargo
//! never discovers it as one) -- included via `#[path = "common/mod.rs"] mod common;`.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;

/// A caller-scripted response for one accepted connection.
pub enum ScriptedResponse {
    Http {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    /// Accepts the connection then drops it without writing anything -- a transport-level
    /// failure (connection reset / unexpected EOF), never a valid HTTP response.
    Drop,
}

impl ScriptedResponse {
    pub fn json(status: u16, body: serde_json::Value) -> Self {
        ScriptedResponse::Http {
            status,
            headers: Vec::new(),
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    pub fn json_with_headers(
        status: u16,
        headers: Vec<(String, String)>,
        body: serde_json::Value,
    ) -> Self {
        ScriptedResponse::Http {
            status,
            headers,
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    /// A bridge-shaped success/error string response (`"OK: ..."`, `"ERROR: ..."`) -- the raw
    /// HTTP body must itself be valid JSON (a quoted string), matching what
    /// `bridge::JSBridgeClient::execute_http` expects.
    pub fn bridge_string(status: u16, text: &str) -> Self {
        ScriptedResponse::json(status, serde_json::Value::String(text.to_string()))
    }
}

pub struct CapturedRequest {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
}

impl CapturedRequest {
    pub fn body_json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("captured request body must be valid JSON")
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> (String, Vec<u8>) {
    let mut buffer = Vec::new();
    let mut temp = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut temp).unwrap_or(0);
        if n == 0 {
            break buffer.len();
        }
        buffer.extend_from_slice(&temp[..n]);
        if let Some(pos) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end.min(buffer.len())]).into_owned();
    let content_length: usize = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    let mut body_len = buffer.len().saturating_sub(header_end);
    while body_len < content_length {
        let n = stream.read(&mut temp).unwrap_or(0);
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..n]);
        body_len += n;
    }
    let body = buffer.get(header_end..).unwrap_or_default().to_vec();
    (head, body)
}

/// A single-port mock server standing in for Zotero's HTTP surface (Local API, Connector API,
/// and JS Bridge all share `127.0.0.1:<port>` on a real Zotero instance). Serves each response in
/// `responses`'s order, one per accepted connection, then the accept loop naturally ends -- any
/// call beyond the scripted count gets connection-refused, proving "no extra/retry request" by
/// construction rather than by a separate negative assertion.
pub struct ScriptedServer {
    pub port: u16,
    handle: Option<thread::JoinHandle<()>>,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
}

impl ScriptedServer {
    pub fn start(responses: Vec<ScriptedResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_clone = Arc::clone(&requests);
        let expected = responses.len();
        let handle = thread::spawn(move || {
            let mut queue: std::collections::VecDeque<ScriptedResponse> = responses.into();
            for _ in 0..expected {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let (head, body) = read_request(&mut stream);
                let first_line = head.lines().next().unwrap_or("").to_string();
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("").to_string();
                requests_clone
                    .lock()
                    .unwrap()
                    .push(CapturedRequest { method, path, body });

                match queue.pop_front().unwrap() {
                    ScriptedResponse::Drop => {
                        drop(stream);
                    }
                    ScriptedResponse::Http {
                        status,
                        headers,
                        body,
                    } => {
                        let mut header_text = format!(
                            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                            body.len()
                        );
                        for (k, v) in &headers {
                            header_text.push_str(&format!("{k}: {v}\r\n"));
                        }
                        header_text.push_str("\r\n");
                        let _ = stream.write_all(header_text.as_bytes());
                        let _ = stream.write_all(&body);
                        let _ = stream.flush();
                    }
                }
            }
        });
        ScriptedServer {
            port,
            handle: Some(handle),
            requests,
        }
    }

    /// Waits for the scripted accept loop to finish, then returns every captured request in
    /// order. Call this only after the subprocess under test has exited.
    pub fn finish(mut self) -> Vec<CapturedRequest> {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        Arc::try_unwrap(self.requests)
            .unwrap_or_else(|arc| Mutex::new(arc.lock().unwrap().drain(..).collect()))
            .into_inner()
            .unwrap()
    }
}

pub struct TestDir(pub PathBuf);

impl TestDir {
    pub fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "zotero-cli-write-routing-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Builds a minimal but schema-complete `zotero.sqlite` fixture (same table set already proven
/// sufficient for item/collection resolution by `tests/docx_inspect.rs`'s
/// `test_validate_placeholders_with_mock_db`): one user library, two items, one top-level
/// collection.
pub fn build_fixture_sqlite(dir: &Path) -> PathBuf {
    let sqlite_path = dir.join("zotero.sqlite");
    let conn = rusqlite::Connection::open(&sqlite_path).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE libraries (libraryID INTEGER PRIMARY KEY, type TEXT, editable INTEGER, filesEditable INTEGER, version INTEGER, storageVersion INTEGER, lastSync INTEGER, archived INTEGER);
        INSERT INTO libraries VALUES (1, 'user', 1, 1, 1, 1, 0, 0);
        CREATE TABLE itemTypes (itemTypeID INTEGER PRIMARY KEY, typeName TEXT, templateItemTypeID INTEGER, display INTEGER);
        INSERT INTO itemTypes VALUES (1, 'document', NULL, 1);
        CREATE TABLE items (itemID INTEGER PRIMARY KEY, itemTypeID INTEGER, dateAdded TEXT, dateModified TEXT, clientDateModified TEXT, libraryID INTEGER, key TEXT, version INTEGER, synced INTEGER);
        INSERT INTO items VALUES (1, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'ITEM0001', 1, 1);
        INSERT INTO items VALUES (2, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'ITEM0002', 1, 1);
        CREATE TABLE fields (fieldID INTEGER PRIMARY KEY, fieldName TEXT, fieldFormatID INTEGER);
        INSERT INTO fields VALUES (1, 'title', 0);
        CREATE TABLE itemDataValues (valueID INTEGER PRIMARY KEY, value TEXT);
        INSERT INTO itemDataValues VALUES (1, 'Test Item One'), (2, 'Test Item Two');
        CREATE TABLE itemData (itemID INTEGER, fieldID INTEGER, valueID INTEGER);
        INSERT INTO itemData VALUES (1, 1, 1), (2, 1, 2);
        CREATE TABLE creators (creatorID INTEGER PRIMARY KEY, firstName TEXT, lastName TEXT, fieldMode INTEGER);
        CREATE TABLE itemCreators (itemID INTEGER, creatorID INTEGER, creatorTypeID INTEGER, orderIndex INTEGER);
        CREATE TABLE tags (tagID INTEGER PRIMARY KEY, name TEXT);
        CREATE TABLE itemTags (itemID INTEGER, tagID INTEGER, type INTEGER);
        CREATE TABLE collections (collectionID INTEGER PRIMARY KEY, collectionName TEXT, parentCollectionID INTEGER, clientDateModified TEXT, libraryID INTEGER, key TEXT, version INTEGER, synced INTEGER);
        INSERT INTO collections VALUES (1, 'Test Collection', NULL, '2026-01-01', 1, 'COLLE001', 1, 1);
        INSERT INTO collections VALUES (2, 'Existing Collection One', NULL, '2026-01-01', 1, 'EXISTC1', 1, 1);
        INSERT INTO collections VALUES (3, 'Existing Collection Two', NULL, '2026-01-01', 1, 'EXISTC2', 1, 1);
        CREATE TABLE collectionItems (collectionID INTEGER, itemID INTEGER, orderIndex INTEGER);
        CREATE TABLE itemNotes (itemID INTEGER PRIMARY KEY, parentItemID INTEGER, note TEXT, title TEXT);
        CREATE TABLE itemAttachments (itemID INTEGER PRIMARY KEY, parentItemID INTEGER, linkMode INTEGER, contentType TEXT, charsetID INTEGER, path TEXT, syncState INTEGER, storageModTime INTEGER, storageHash TEXT, lastProcessedModificationTime INTEGER);
        CREATE TABLE itemAnnotations (itemID INTEGER PRIMARY KEY, parentItemID INTEGER, type INTEGER, authorName TEXT, text TEXT, comment TEXT, color TEXT, pageLabel TEXT, sortIndex TEXT, position TEXT, isExternal INTEGER);
        CREATE TABLE savedSearches (savedSearchID INTEGER PRIMARY KEY, savedSearchName TEXT, clientDateModified TEXT, libraryID INTEGER, key TEXT, version INTEGER, synced INTEGER);
        CREATE TABLE savedSearchConditions (savedSearchID INTEGER, searchConditionID INTEGER, condition TEXT, operator TEXT, value TEXT, required INTEGER);
        "#,
    )
    .unwrap();
    sqlite_path
}

/// Hand-writes a Local API write-credential store file directly (bypassing
/// `zotero_cli::credentials::store_credential`, which would require mutating this test
/// process's own `CLI_ANYTHING_ZOTERO_STATE_DIR`/environment and racing other tests in this
/// binary running in parallel). The child subprocess reads this file when given the same
/// directory via `CLI_ANYTHING_ZOTERO_STATE_DIR`.
pub fn write_stored_credential(state_dir: &Path, server_id: &str, key: &str) {
    std::fs::create_dir_all(state_dir).unwrap();
    let contents = serde_json::json!({
        "version": 1,
        "credentials": {
            server_id: {
                "app_name": "zotero-rust-cli",
                "key": key,
                "remember": true,
                "issued_at": "2026-01-01T00:00:00Z",
            }
        }
    });
    std::fs::write(
        state_dir.join("local_api_credentials.json"),
        serde_json::to_vec_pretty(&contents).unwrap(),
    )
    .unwrap();
}

/// Reads back the same store file `write_stored_credential` writes, for asserting a revoked
/// entry was actually removed after a subprocess run.
pub fn read_stored_credentials(state_dir: &Path) -> serde_json::Value {
    let raw = std::fs::read_to_string(state_dir.join("local_api_credentials.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

pub fn bin_path() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove this test binary's own name
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("zotero-cli")
}

/// Runs the `zotero-cli` binary pointed at `data_dir` (an explicit `--data-dir` containing a
/// fixture `zotero.sqlite`) and `port` (via `ZOTERO_HTTP_PORT`, the single mock-server port
/// standing in for Local API / Connector / JS Bridge), with `--json` always set. `extra_env`
/// lets a test inject `ZOTERO_LOCAL_API_KEY` / `CLI_ANYTHING_ZOTERO_STATE_DIR` without ever
/// mutating this test process's own environment (so parallel tests in this binary never race).
pub fn run_cli(
    data_dir: &Path,
    port: u16,
    extra_env: &[(&str, &str)],
    args: &[&str],
) -> (i32, serde_json::Value) {
    let mut command = Command::new(bin_path());
    command
        .arg("--json")
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
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value = serde_json::from_str(stdout.trim()).unwrap_or_else(|err| {
        panic!(
            "expected JSON stdout, got parse error {err}: stdout={stdout:?} stderr={}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (code, value)
}

/// Recursively asserts none of `forbidden_keys` appears as an object key anywhere in `value` --
/// the standing backend-identity denylist check (§3.5/Testing Strategy).
pub fn assert_no_forbidden_keys(value: &serde_json::Value, forbidden_keys: &[&str], path: &str) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, nested) in map {
                assert!(
                    !forbidden_keys.contains(&key.as_str()),
                    "forbidden backend-identity key {key:?} found at {path}.{key} in {value}"
                );
                assert_no_forbidden_keys(nested, forbidden_keys, &format!("{path}.{key}"));
            }
        }
        serde_json::Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                assert_no_forbidden_keys(item, forbidden_keys, &format!("{path}[{i}]"));
            }
        }
        _ => {}
    }
}
