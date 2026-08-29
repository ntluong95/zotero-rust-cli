use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

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
        self.head.lines().find(|line| {
            line.to_ascii_lowercase()
                .starts_with(&prefix.to_ascii_lowercase())
        })
    }

    fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

fn serve_one(
    status: u16,
    content_type: &str,
    body: &'static [u8],
) -> (u16, mpsc::Receiver<CapturedRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let content_type = content_type.to_string();

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
            .map(|index| index + 4)
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

        let reason = if status == 201 { "Created" } else { "OK" };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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

fn assert_zotero10_hardening_headers(request: &CapturedRequest) {
    assert!(
        request.header("origin").is_none(),
        "Connector request must not send Origin:\n{}",
        request.head
    );
    if let Some(user_agent) = request.header("user-agent") {
        assert!(
            !user_agent.to_ascii_lowercase().contains("mozilla/"),
            "Connector request must not send Mozilla-style User-Agent: {user_agent}"
        );
    }
}

#[test]
fn get_selected_collection_sends_post_empty_json() {
    let (port, rx) = serve_one(
        200,
        "application/json",
        br#"{"libraryID":1,"name":"Sample Collection"}"#,
    );

    let value = zotero_cli::http::get_selected_collection(port, Duration::from_secs(3)).unwrap();
    let request = captured(rx);

    assert_eq!(
        request.request_line(),
        "POST /connector/getSelectedCollection HTTP/1.1"
    );
    assert_eq!(request.body_text(), "{}");
    assert!(request
        .header("content-type")
        .unwrap()
        .contains("application/json"));
    assert_zotero10_hardening_headers(&request);
    assert_eq!(value["libraryID"], 1);
}

#[test]
fn connector_import_accepts_201_and_sends_raw_content_with_session_query() {
    let (port, rx) = serve_one(
        201,
        "application/json",
        br#"[{"id":"imported-1","title":"Imported"}]"#,
    );

    let items = zotero_cli::http::connector_import_text(
        port,
        b"TY  - JOUR\nER  - \n",
        Some("import-file-abc"),
        "application/x-research-info-systems",
        Duration::from_secs(3),
    )
    .unwrap();
    let request = captured(rx);

    assert_eq!(
        request.request_line(),
        "POST /connector/import?session=import-file-abc HTTP/1.1"
    );
    assert_eq!(request.body, b"TY  - JOUR\nER  - \n");
    assert!(request
        .header("content-type")
        .unwrap()
        .contains("application/x-research-info-systems"));
    assert_zotero10_hardening_headers(&request);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "imported-1");
}

#[test]
fn connector_import_normalizes_object_response_to_single_item_list() {
    let (port, _rx) = serve_one(201, "application/json", br#"{"id":"solo"}"#);

    let items = zotero_cli::http::connector_import_text(
        port,
        b"@article{solo}",
        None,
        "text/x-bibtex",
        Duration::from_secs(3),
    )
    .unwrap();

    assert_eq!(items, vec![json!({"id": "solo"})]);
}

#[test]
fn connector_import_rejects_non_201_status_with_python_style_error() {
    let (port, _rx) = serve_one(200, "application/json", br#"{"ok":true}"#);

    let error = zotero_cli::http::connector_import_text(
        port,
        b"@article{x}",
        Some("s1"),
        "text/x-bibtex",
        Duration::from_secs(3),
    )
    .unwrap_err()
    .to_string();

    assert_eq!(error, r#"connector/import returned HTTP 200: {"ok":true}"#);
}

#[test]
fn save_items_accepts_201_and_sends_python_shaped_json() {
    let (port, rx) = serve_one(201, "application/json", b"");
    let items = vec![json!({"itemType": "journalArticle", "title": "Fixture JSON"})];

    zotero_cli::http::connector_save_items(port, &items, "import-json-abc", Duration::from_secs(3))
        .unwrap();
    let request = captured(rx);

    assert_eq!(request.request_line(), "POST /connector/saveItems HTTP/1.1");
    assert_eq!(
        request.body_text(),
        r#"{"sessionID": "import-json-abc", "items": [{"itemType": "journalArticle", "title": "Fixture JSON"}]}"#
    );
    assert!(request
        .header("content-type")
        .unwrap()
        .contains("application/json"));
    assert_zotero10_hardening_headers(&request);
}

#[test]
fn save_items_rejects_200_with_python_style_error() {
    let (port, _rx) = serve_one(200, "text/plain", b"not created");
    let items = vec![json!({"title": "Wrong status"})];

    let error = zotero_cli::http::connector_save_items(port, &items, "s1", Duration::from_secs(3))
        .unwrap_err()
        .to_string();

    assert_eq!(error, "connector/saveItems returned HTTP 200: not created");
}

#[test]
fn save_attachment_accepts_200_and_sends_pdf_bytes_and_metadata() {
    let (port, rx) = serve_one(200, "application/json", br#"{"key":"ATTACH1"}"#);

    let response = zotero_cli::http::connector_save_attachment(
        port,
        "import-json-abc",
        "imported-1",
        "PDF",
        "file:///tmp/a.pdf",
        b"%PDF-1.4\nbytes",
        Duration::from_secs(3),
    )
    .unwrap();
    let request = captured(rx);

    assert_eq!(
        request.request_line(),
        "POST /connector/saveAttachment HTTP/1.1"
    );
    assert_eq!(request.body, b"%PDF-1.4\nbytes");
    assert!(request
        .header("content-type")
        .unwrap()
        .contains("application/pdf"));
    let metadata = request.header("x-metadata").unwrap();
    assert!(metadata.contains(r#""sessionID": "import-json-abc""#));
    assert!(metadata.contains(r#""parentItemID": "imported-1""#));
    assert!(metadata.contains(r#""url": "file:///tmp/a.pdf""#));
    assert_zotero10_hardening_headers(&request);
    assert_eq!(response["key"], "ATTACH1");
}

#[test]
fn save_attachment_accepts_201_and_empty_body() {
    let (port, _rx) = serve_one(201, "application/json", b"");

    let response = zotero_cli::http::connector_save_attachment(
        port,
        "s1",
        "p1",
        "PDF",
        "file:///tmp/a.pdf",
        b"%PDF-1.4",
        Duration::from_secs(3),
    )
    .unwrap();

    assert_eq!(response, Value::Object(Default::default()));
}

#[test]
fn update_session_joins_tags_exactly_with_comma_space() {
    let (port, rx) = serve_one(200, "application/json", br#"{}"#);
    let tags = vec!["alpha".to_string(), "".to_string(), " beta ".to_string()];

    let response = zotero_cli::http::connector_update_session(
        port,
        "import-json-abc",
        "C1",
        &tags,
        Duration::from_secs(3),
    )
    .unwrap();
    let request = captured(rx);

    assert_eq!(
        request.request_line(),
        "POST /connector/updateSession HTTP/1.1"
    );
    assert_eq!(
        request.body_text(),
        r#"{"sessionID": "import-json-abc", "target": "C1", "tags": "alpha,  beta "}"#
    );
    assert_zotero10_hardening_headers(&request);
    assert_eq!(response, json!({}));
}

#[test]
fn update_session_rejects_non_success_with_python_style_error() {
    let (port, _rx) = serve_one(500, "text/plain", b"boom");

    let error =
        zotero_cli::http::connector_update_session(port, "s1", "C1", &[], Duration::from_secs(3))
            .unwrap_err()
            .to_string();

    assert_eq!(error, "connector/updateSession returned HTTP 500: boom");
}

#[test]
fn transport_errors_preserve_python_style_path_prefix() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let error = zotero_cli::http::get_selected_collection(port, Duration::from_millis(200))
        .unwrap_err()
        .to_string();

    assert!(
        error.starts_with("HTTP request failed for /connector/getSelectedCollection:"),
        "unexpected transport error: {error}"
    );
}
