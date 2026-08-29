#[path = "../src/bridge/mod.rs"]
mod bridge;

use bridge::*;
use serde_json::json;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

#[test]
fn test_format_bridge_error_shapes() {
    assert_eq!(format_bridge_error(&json!(null)), "unknown bridge error");
    assert_eq!(format_bridge_error(&json!("")), "unknown bridge error");
    assert_eq!(format_bridge_error(&json!("   ")), "unknown bridge error");
    assert_eq!(
        format_bridge_error(&json!("Custom error message")),
        "Custom error message"
    );

    assert_eq!(
        format_bridge_error(&json!({ "error": "item not found" })),
        "item not found"
    );
    assert_eq!(
        format_bridge_error(&json!({ "message": "Zotero internal error" })),
        "Zotero internal error"
    );
    assert_eq!(
        format_bridge_error(&json!({ "raw": "TypeError: item.saveTx is not a function" })),
        "TypeError: item.saveTx is not a function"
    );
    assert_eq!(
        format_bridge_error(&json!({ "name": "NotFoundError" })),
        "NotFoundError"
    );
    assert_eq!(format_bridge_error(&json!({})), "unknown bridge error");
}

#[test]
fn test_bridge_response_require_data() {
    let ok_resp = BridgeResponse::success(json!({ "ok": true, "key": "ITEM1" }));
    assert!(ok_resp.is_ok());
    assert_eq!(ok_resp.require_data().unwrap()["key"], "ITEM1");

    let err_resp = BridgeResponse::failure("Network error".to_string());
    assert!(!err_resp.is_ok());
    assert!(err_resp.require_data().is_err());
    assert_eq!(err_resp.error_message(), Some("Network error"));

    // Nested application-level error
    let nested_err =
        BridgeResponse::success(json!({ "ok": false, "error": "Item not found in library" }));
    assert!(nested_err.require_data().is_err());
    assert_eq!(
        nested_err.require_data().unwrap_err().to_string(),
        "Item not found in library"
    );
}

#[test]
fn test_probe_caching_and_ownership_invariants() {
    clear_probe_cache();

    // 1. Inactive port should return false and not be cached
    let client_inactive = JSBridgeClient::new(59998);
    assert!(!client_inactive.bridge_endpoint_active());

    // 2. Mock server returning VALID fork ownership
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let valid_port = listener.local_addr().unwrap().port();

    let server_handle = thread::spawn(move || {
        // Only 1 request served because second probe will be served from POSITIVE_PROBES cache
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let body = r#"{"pong":true,"fork":"zotero-rust-cli","id":"cli-bridge@cli-anything-rust.dev","version":"1.2.1"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });

    let client = JSBridgeClient::new(valid_port);
    // First active check should hit mock server, verify ownership, succeed and cache
    assert!(client.bridge_endpoint_active());
    // Second active check should be served from positive cache
    assert!(client.bridge_endpoint_active());

    let _ = server_handle.join();
}

#[test]
fn test_ownership_rejections_and_eval_bypass_prevention() {
    clear_probe_cache();

    // 1. WRONG FORK: HTTP 200 with wrong fork must be rejected and NOT cached
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            for _ in 0..2 {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf);
                    let body = r#"{"pong":true,"fork":"other-upstream-fork"}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                }
            }
        });

        let client = JSBridgeClient::new(port);
        assert!(
            !client.bridge_endpoint_active(),
            "Wrong fork must be rejected"
        );
        // Privileged eval must refuse to run
        let eval_resp = client.execute_js("return 123;", 5);
        assert!(!eval_resp.ok);
        assert!(eval_resp
            .error_message()
            .unwrap()
            .contains("JS Bridge endpoint not available"));
        assert!(client.execute_raw_js("return 123;", 5).is_err());

        let _ = handle.join();
    }

    // 2. MISSING OWNERSHIP: HTTP 200 with raw string or missing fork must be rejected
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = r#"{"pong":true}"#; // Missing fork
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        let client = JSBridgeClient::new(port);
        assert!(
            !client.bridge_endpoint_active(),
            "Missing fork must be rejected"
        );
        let _ = handle.join();
    }

    // 3. MALFORMED OWNERSHIP: HTTP 200 with non-JSON garbage must be rejected
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = "Not JSON At All";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        let client = JSBridgeClient::new(port);
        assert!(
            !client.bridge_endpoint_active(),
            "Malformed body must be rejected"
        );
        let _ = handle.join();
    }

    // 4. WRONG ID: HTTP 200 with correct fork but wrong id must be rejected
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = r#"{"pong":true,"fork":"zotero-rust-cli","id":"wrong-addon-id"}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        let client = JSBridgeClient::new(port);
        assert!(
            !client.bridge_endpoint_active(),
            "Wrong id must be rejected even if fork matches"
        );
        let _ = handle.join();
    }
}

#[test]
fn test_execute_js_success_and_error_handling() {
    clear_probe_cache();

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();

    let server_handle = thread::spawn(move || {
        // Request 1: probe with verified fork ownership
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let body = r#"{"pong":true,"fork":"zotero-rust-cli","id":"cli-bridge@cli-anything-rust.dev","version":"1.2.1"}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }

        // Request 2: successful eval returning OK string
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let body = "\"OK: updated My Title\"";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }

        // Request 3: error eval returning 500 with structured error
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let body = r#"{"error":"item KEY999 not found","name":"NotFoundError","stack":"...","raw":"Error: item KEY999 not found"}"#;
            let resp = format!(
                "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });

    let client = JSBridgeClient::new(port);

    // Call 1: item_update -> OK
    let mut fields = HashMap::new();
    fields.insert("title".to_string(), "My Title".to_string());
    let outcome = client
        .item_update(1, "KEY1", &fields)
        .expect("update succeeds");
    assert_eq!(
        outcome,
        WriteOutcome::Applied {
            affected_key: "KEY1".to_string()
        }
    );

    // Call 2: execute_js -> 500 error
    let err_resp = client.execute_js("some bad code", 5);
    assert!(!err_resp.ok);
    assert_eq!(err_resp.error_message(), Some("item KEY999 not found"));
    assert_eq!(err_resp.error_name.as_deref(), Some("NotFoundError"));

    let _ = server_handle.join();
}
