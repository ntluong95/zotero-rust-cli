//! Zotero 10 compatibility-gate conformance tests
//! (`phase-14-zotero-10-compatibility-gate.md`).
//!
//! ## HTTP hardening (§5)
//!
//! Zotero 10 silently drops any request whose `User-Agent` starts with
//! `Mozilla/`, or that carries **any** `Origin` header at all, unless the
//! request carries `Zotero-Allowed-Request` or the target endpoint opts out
//! with `allowRequestsFromUnsafeWebContent`. Live-confirmed against a real,
//! running Zotero 10.0.1 instance (2026-08-29): both conditions
//! independently produce a closed connection with zero bytes of response
//! (`curl` exit 52, "empty reply from server"), while a plain `ureq`
//! request (no `Origin`, default `ureq/x.y` UA) passes cleanly. This file
//! locks that in as a regression test against *our own client's* actual
//! outbound headers -- it cannot re-verify Zotero's own server behavior
//! without a live instance, but it prevents a future change (e.g. a
//! "unify User-Agent across clients" refactor, explicitly warned against
//! in `phase-14` §5) from silently reintroducing a header Zotero 10 drops.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Accepts exactly one connection, captures its raw request head, and
/// replies with a minimal 200 so the client under test doesn't hang.
fn capture_one_request(listener: TcpListener) -> Arc<Mutex<Option<String>>> {
    let captured = Arc::new(Mutex::new(None));
    let captured_clone = captured.clone();
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buffer = Vec::new();
        let mut temp = [0u8; 4096];
        loop {
            match stream.read(&mut temp) {
                Ok(0) | Err(_) => break,
                Ok(n) => buffer.extend_from_slice(&temp[..n]),
            }
            if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        *captured_clone.lock().unwrap() = Some(String::from_utf8_lossy(&buffer).into_owned());
        let body = b"{}";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(body);
    });
    captured
}

fn header_line<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!("{name}:");
    request.lines().find(|line| {
        line.to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
    })
}

fn assert_zotero10_hardening_would_pass(request: &str) {
    if let Some(ua_line) = header_line(request, "user-agent") {
        assert!(
            !ua_line.to_ascii_lowercase().contains("mozilla/"),
            "outbound request must not carry a Mozilla/-prefixed User-Agent \
             (Zotero 10 drops it silently, no response): {ua_line}"
        );
    }
    assert!(
        header_line(request, "origin").is_none(),
        "outbound request must not carry any Origin header \
         (Zotero 10 drops it silently, no response):\n{request}"
    );
}

fn wait_for_capture(captured: &Arc<Mutex<Option<String>>>) -> String {
    for _ in 0..50 {
        if let Some(request) = captured.lock().unwrap().clone() {
            return request;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("fake server never captured a request");
}

#[test]
fn connector_ping_carries_no_mozilla_ua_and_no_origin() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let captured = capture_one_request(listener);

    let _ = zotero_cli::http::connector_is_available(port, Duration::from_secs(3));

    let request = wait_for_capture(&captured);
    assert!(
        request.starts_with("GET /connector/ping"),
        "sanity: fake server should have seen the ping request: {request}"
    );
    assert_zotero10_hardening_would_pass(&request);
}

#[test]
fn probe_local_api_carries_no_mozilla_ua_and_no_origin() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let captured = capture_one_request(listener);

    let _ = zotero_cli::http::probe_local_api(port, Duration::from_secs(3));

    let request = wait_for_capture(&captured);
    assert!(
        request.starts_with("GET /api/"),
        "sanity: fake server should have seen the Local API probe: {request}"
    );
    assert_zotero10_hardening_would_pass(&request);
}

/// Capability detection (§4): `Zotero-Server-ID` presence is the Zotero
/// 10+ discriminator, live-confirmed to appear even on a `403` response
/// when the Local API itself is disabled -- so `probe_local_api` must
/// read the header regardless of status code, not only on a 200.
#[test]
fn probe_local_api_reads_server_id_header_even_on_non_200_status() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buffer = Vec::new();
        let mut temp = [0u8; 4096];
        loop {
            match stream.read(&mut temp) {
                Ok(0) | Err(_) => break,
                Ok(n) => buffer.extend_from_slice(&temp[..n]),
            }
            if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        // Matches the real Zotero 10.0.1 behavior observed live: a 403
        // when Local API is disabled in preferences still carries
        // Zotero-Server-ID.
        let body = b"Nothing to see here.";
        let response = format!(
            "HTTP/1.1 403 Forbidden\r\nZotero-Server-ID: QR43gFhLblRt\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(body);
    });

    let probe = zotero_cli::http::probe_local_api(port, Duration::from_secs(3));
    assert!(!probe.available, "403 must not be reported as available");
    assert_eq!(
        probe.server_id.as_deref(),
        Some("QR43gFhLblRt"),
        "server_id must be captured even on a non-200 response"
    );
}
