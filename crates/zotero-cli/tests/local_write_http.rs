//! Local API write-transport tests (`http/local_write.rs`, Phase 6 Slice 3). Exercises exact
//! method/path/headers/body against a real TCP mock server -- same pattern as
//! `tests/connector_http.rs` -- not a live Zotero instance.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[derive(Debug)]
struct CapturedRequest {
    head: String,
    body: Vec<u8>,
}

impl CapturedRequest {
    fn request_line(&self) -> &str {
        self.head.lines().next().unwrap_or("")
    }

    fn header(&self, name: &str) -> Option<&str> {
        let prefix = format!("{name}:");
        self.head
            .lines()
            .find(|line| {
                line.to_ascii_lowercase()
                    .starts_with(&prefix.to_ascii_lowercase())
            })
            .map(|line| line.split_once(':').map(|(_, v)| v.trim()).unwrap_or(""))
    }

    fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

fn serve_one(status: u16, body: &'static [u8]) -> (u16, mpsc::Receiver<CapturedRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut raw = Vec::new();
        let mut temp = [0u8; 4096];
        loop {
            let n = stream.read(&mut temp).unwrap();
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&temp[..n]);
            if raw.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let header_end = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|i| i + 4)
            .unwrap();
        let head = String::from_utf8_lossy(&raw[..header_end]).into_owned();
        let content_length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        let mut request_body = raw[header_end..].to_vec();
        while request_body.len() < content_length {
            let n = stream.read(&mut temp).unwrap();
            if n == 0 {
                break;
            }
            request_body.extend_from_slice(&temp[..n]);
        }
        request_body.truncate(content_length);
        tx.send(CapturedRequest {
            head,
            body: request_body,
        })
        .unwrap();

        let reason = if status == 204 { "No Content" } else { "OK" };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nLast-Modified-Version: 4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
    });

    (port, rx)
}

fn captured(rx: mpsc::Receiver<CapturedRequest>) -> CapturedRequest {
    rx.recv_timeout(Duration::from_secs(2))
        .expect("fake server did not capture request")
}

fn assert_no_hardening_workarounds(request: &CapturedRequest) {
    assert!(
        request.header("origin").is_none(),
        "Local API write request must not send Origin:\n{}",
        request.head
    );
    if let Some(user_agent) = request.header("user-agent") {
        assert!(
            !user_agent.to_ascii_lowercase().contains("mozilla/"),
            "Local API write request must not send Mozilla-style User-Agent: {user_agent}"
        );
    }
}

#[test]
fn authorize_sends_exact_method_path_headers_and_body() {
    let (port, rx) = serve_one(
        200,
        br#"{"key":"THEKEY123456789012345678901234","remember":true}"#,
    );

    let response = zotero_cli::http::local_api_authorize(
        port,
        "SRV123",
        "zotero-rust-cli",
        Duration::from_secs(3),
    )
    .unwrap();
    let request = captured(rx);

    assert_eq!(request.request_line(), "POST /api/local/authorize HTTP/1.1");
    let body: serde_json::Value = serde_json::from_str(&request.body_text()).unwrap();
    assert_eq!(body, serde_json::json!({"appName": "zotero-rust-cli"}));
    assert_eq!(request.header("zotero-server-id"), Some("SRV123"));
    assert!(request
        .header("content-type")
        .unwrap()
        .contains("application/json"));
    assert_no_hardening_workarounds(&request);
    assert_eq!(response.status, 200);
}

#[test]
fn patch_sends_exact_method_path_headers_and_body() {
    let (port, rx) = serve_one(204, b"");

    let body = serde_json::json!({"title": "New Title"});
    let response = zotero_cli::http::local_api_patch(
        port,
        "/api/users/0/items/ABCD1234",
        "SRV123",
        "the-api-key",
        7,
        &body,
        Duration::from_secs(3),
    )
    .unwrap();
    let request = captured(rx);

    assert_eq!(
        request.request_line(),
        "PATCH /api/users/0/items/ABCD1234 HTTP/1.1"
    );
    let sent_body: serde_json::Value = serde_json::from_str(&request.body_text()).unwrap();
    assert_eq!(sent_body, body);
    assert_eq!(request.header("zotero-server-id"), Some("SRV123"));
    assert_eq!(request.header("zotero-api-key"), Some("the-api-key"));
    assert_eq!(request.header("zotero-api-version"), Some("3"));
    assert_eq!(request.header("if-unmodified-since-version"), Some("7"));
    assert_no_hardening_workarounds(&request);
    assert_eq!(response.status, 204);
    assert_eq!(response.last_modified_version, Some(4));
}

#[test]
fn post_sends_exact_method_path_headers_and_body_with_no_version_header() {
    let (port, rx) = serve_one(200, br#"{"successful":{"0":{"key":"NEWKEY01"}}}"#);

    let body = serde_json::json!([{"name": "New Collection"}]);
    zotero_cli::http::local_api_post(
        port,
        "/api/users/0/collections",
        "SRV123",
        "the-api-key",
        &body,
        Duration::from_secs(3),
    )
    .unwrap();
    let request = captured(rx);

    assert_eq!(
        request.request_line(),
        "POST /api/users/0/collections HTTP/1.1"
    );
    let sent_body: serde_json::Value = serde_json::from_str(&request.body_text()).unwrap();
    assert_eq!(sent_body, body);
    assert_eq!(request.header("zotero-server-id"), Some("SRV123"));
    assert_eq!(request.header("zotero-api-key"), Some("the-api-key"));
    assert!(
        request.header("if-unmodified-since-version").is_none(),
        "POST (create) must not send If-Unmodified-Since-Version"
    );
    assert_no_hardening_workarounds(&request);
}

#[test]
fn delete_sends_exact_method_path_headers_and_no_body() {
    let (port, rx) = serve_one(204, b"");

    zotero_cli::http::local_api_delete(
        port,
        "/api/users/0/items/ABCD1234",
        "SRV123",
        "the-api-key",
        9,
        Duration::from_secs(3),
    )
    .unwrap();
    let request = captured(rx);

    assert_eq!(
        request.request_line(),
        "DELETE /api/users/0/items/ABCD1234 HTTP/1.1"
    );
    assert_eq!(request.body, Vec::<u8>::new());
    assert_eq!(request.header("zotero-server-id"), Some("SRV123"));
    assert_eq!(request.header("zotero-api-key"), Some("the-api-key"));
    assert_eq!(request.header("if-unmodified-since-version"), Some("9"));
    assert_no_hardening_workarounds(&request);
}

#[test]
fn get_raw_never_bails_on_404_and_preserves_status_and_body() {
    let (port, rx) = serve_one(404, br#"{"message":"not found"}"#);

    let response = zotero_cli::http::local_api_get_raw(
        port,
        "/api/users/0/items/GONE0000",
        Duration::from_secs(3),
    )
    .unwrap();
    let request = captured(rx);

    assert_eq!(
        request.request_line(),
        "GET /api/users/0/items/GONE0000 HTTP/1.1"
    );
    assert_eq!(response.status, 404);
    assert_eq!(response.body, r#"{"message":"not found"}"#);
    assert_no_hardening_workarounds(&request);
}
