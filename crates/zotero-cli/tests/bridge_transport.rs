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
fn test_probe_caching_behavior() {
    clear_probe_cache();

    // 1. Inactive port should return false and not be cached as true
    let client_inactive = JSBridgeClient::new(59998);
    assert!(!client_inactive.bridge_endpoint_active());

    // 2. Start a mock server on an ephemeral port
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();

    let server_handle = thread::spawn(move || {
        // Only 1 request expected because second probe will be served from cache!
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let response = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 6\r\nConnection: close\r\n\r\n\"ping\"";
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });

    let client = JSBridgeClient::new(port);
    // First active check should hit mock server, succeed and cache
    assert!(client.bridge_endpoint_active());
    // Second active check should be cached (server has exited / not accepting connections, but returns true)
    assert!(client.bridge_endpoint_active());

    let _ = server_handle.join();
}

#[test]
fn test_execute_js_success_and_error_handling() {
    clear_probe_cache();

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();

    let server_handle = thread::spawn(move || {
        // Request 1: probe
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 6\r\nConnection: close\r\n\r\n\"ping\"";
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
