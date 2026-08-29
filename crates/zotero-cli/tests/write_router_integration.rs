//! `write_router.rs` integration tests (Phase 6 Slices 3-5). Runs against a real TCP mock
//! server, never a live Zotero instance -- proves the credential-preflight/transport-safety/
//! response-classification/revocation behavior end to end, not just the pure-logic unit tests
//! already in `write_router.rs`'s own `#[cfg(test)]` module.
//!
//! This file is its own OS process (a separate integration-test binary), so its
//! `ZOTERO_LOCAL_API_KEY`/`CLI_ANYTHING_ZOTERO_STATE_DIR` env-var mutations never race
//! `zotero_cli`'s lib unit tests (a different process) -- only tests *within this file* can race
//! each other, guarded by `ENV_LOCK` below.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::json;
use zotero_cli::paths::ZoteroEnvironment;
use zotero_cli::runtime::RuntimeContext;
use zotero_cli::write::{AuthorizationReason, CredentialSource, WriteOutcome};

const STATE_DIR_ENV: &str = "CLI_ANYTHING_ZOTERO_STATE_DIR";
const API_KEY_ENV: &str = "ZOTERO_LOCAL_API_KEY";

/// Guards both env vars this file mutates -- every test that touches either must hold this for
/// its entire duration (same reasoning as `session.rs`'s `STATE_DIR_ENV_LOCK`, scoped to this
/// process/test binary only).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn temp_state_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "zotero-cli-test-write-router-{}-{n}-{label}",
        std::process::id()
    ))
}

fn test_runtime(port: u16, server_id: &str) -> RuntimeContext {
    RuntimeContext {
        environment: ZoteroEnvironment {
            executable: None,
            executable_exists: false,
            install_dir: None,
            version: "10.0.1".to_string(),
            profile_root: PathBuf::from("/tmp/does-not-exist-profile-root"),
            profile_dir: None,
            data_dir: PathBuf::from("/tmp/does-not-exist-data-dir"),
            data_dir_exists: false,
            sqlite_path: PathBuf::from("/tmp/does-not-exist.sqlite"),
            sqlite_exists: false,
            styles_dir: PathBuf::from("/tmp/does-not-exist-styles"),
            styles_exists: false,
            storage_dir: PathBuf::from("/tmp/does-not-exist-storage"),
            storage_exists: false,
            translators_dir: PathBuf::from("/tmp/does-not-exist-translators"),
            translators_exists: false,
            port,
            local_api_enabled_configured: true,
        },
        backend: "sqlite".to_string(),
        connector_available: false,
        connector_message: String::new(),
        local_api_available: true,
        local_api_message: "local API available".to_string(),
        server_id: Some(server_id.to_string()),
        local_api_writes_available: true,
    }
}

struct CapturedRequest {
    head: String,
}

fn serve_status(status: u16, body: &'static [u8]) -> (u16, mpsc::Receiver<CapturedRequest>) {
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
        // Must drain the request body before responding: if the client (ureq) is still writing
        // its request when this thread closes the socket, the unread bytes cause a TCP RST
        // instead of a clean close, which can surface as a spurious read error on the client
        // side for the *response* it's trying to read -- a real bug this test file had, not
        // scheduler flakiness, matching the other mock servers' (`connector_http.rs`,
        // `local_write_http.rs`) already-correct Content-Length-draining pattern.
        let content_length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        let mut request_body_len = raw.len() - header_end;
        while request_body_len < content_length {
            let n = stream.read(&mut temp).unwrap();
            if n == 0 {
                break;
            }
            request_body_len += n;
        }
        tx.send(CapturedRequest { head }).unwrap();
        let response = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        let _ = stream.flush();
    });
    (port, rx)
}

/// A listener that accepts exactly one connection then drops it without responding at all --
/// the client sees a transport-level failure (connection reset / unexpected EOF), never a valid
/// HTTP response. Used to prove Blocker 1's TransportError handling and the "exactly one
/// request, no automatic retry" requirement.
fn serve_and_drop(counter_tx: mpsc::Sender<()>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            counter_tx.send(()).unwrap();
            drop(stream);
        }
    });
    port
}

#[test]
fn missing_credential_returns_authorization_required_with_zero_write_requests() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        std::env::remove_var(API_KEY_ENV);
        std::env::set_var(STATE_DIR_ENV, temp_state_dir("missing-cred"));
    }

    // Deliberately never bind anything on this port: if patch_item made a network attempt, it
    // would get connection-refused (mapped to TransportError, not AuthorizationRequired) --
    // getting AuthorizationRequired back is itself the proof zero requests were attempted.
    let unused_port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    };
    let runtime = test_runtime(unused_port, "SRV-MISSING");

    let outcome = zotero_cli::write_router::patch_item(
        &runtime,
        "/api/users/0/items/ABCD1234",
        "ABCD1234",
        &json!({"title": "x"}),
        1,
    )
    .unwrap();

    assert!(matches!(
        outcome,
        WriteOutcome::AuthorizationFailed {
            reason: AuthorizationReason::Required,
            source: CredentialSource::None,
            ..
        }
    ));

    unsafe {
        std::env::remove_var(STATE_DIR_ENV);
    }
}

#[test]
fn transport_failure_maps_to_transport_error_with_exactly_one_request() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        std::env::set_var(API_KEY_ENV, "env-key-for-transport-test");
        std::env::remove_var(STATE_DIR_ENV);
    }

    let (tx, rx) = mpsc::channel();
    let port = serve_and_drop(tx);
    let runtime = test_runtime(port, "SRV-TRANSPORT");

    let outcome = zotero_cli::write_router::patch_item(
        &runtime,
        "/api/users/0/items/ABCD1234",
        "ABCD1234",
        &json!({"title": "x"}),
        1,
    )
    .unwrap();

    assert!(
        matches!(outcome, WriteOutcome::TransportError { .. }),
        "expected TransportError, got {outcome:?}"
    );
    assert!(
        rx.recv_timeout(Duration::from_secs(2)).is_ok(),
        "server must have received exactly one connection"
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "no second connection (i.e. no automatic retry) must arrive"
    );

    unsafe {
        std::env::remove_var(API_KEY_ENV);
    }
}

#[test]
fn status_codes_map_to_the_documented_write_outcome_variants() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        std::env::set_var(API_KEY_ENV, "env-key-for-status-test");
        std::env::remove_var(STATE_DIR_ENV);
    }

    type OutcomePredicate = fn(&WriteOutcome) -> bool;
    let cases: &[(u16, &[u8], OutcomePredicate)] = &[
        (
            401,
            br#"API key required -- POST /api/local/authorize to obtain one"#,
            |o| {
                matches!(
                    o,
                    WriteOutcome::AuthorizationFailed {
                        reason: AuthorizationReason::Required,
                        ..
                    }
                )
            },
        ),
        (401, br#"Invalid or expired API key"#, |o| {
            matches!(
                o,
                WriteOutcome::AuthorizationFailed {
                    reason: AuthorizationReason::Revoked,
                    ..
                }
            )
        }),
        (403, br#"denied"#, |o| {
            matches!(
                o,
                WriteOutcome::AuthorizationFailed {
                    reason: AuthorizationReason::Denied,
                    ..
                }
            )
        }),
        (428, br#"Zotero-Server-ID not provided"#, |o| {
            matches!(o, WriteOutcome::PreconditionFailed { .. })
        }),
        (429, br#"too many requests"#, |o| {
            matches!(
                o,
                WriteOutcome::AuthorizationFailed {
                    reason: AuthorizationReason::RateLimited,
                    ..
                }
            )
        }),
        (412, br#"version mismatch"#, |o| {
            matches!(o, WriteOutcome::Conflict { .. })
        }),
    ];

    for (status, body, predicate) in cases {
        let (port, _rx) = serve_status(*status, body);
        let runtime = test_runtime(port, "SRV-STATUS");
        let outcome = zotero_cli::write_router::patch_item(
            &runtime,
            "/api/users/0/items/ABCD1234",
            "ABCD1234",
            &json!({"title": "x"}),
            1,
        )
        .unwrap();
        assert!(
            predicate(&outcome),
            "status {status} with body {body:?} produced unexpected outcome: {outcome:?}"
        );
    }

    unsafe {
        std::env::remove_var(API_KEY_ENV);
    }
}

#[test]
fn revoked_env_credential_is_reported_but_the_environment_is_never_mutated() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        std::env::set_var(API_KEY_ENV, "an-env-provided-key");
        std::env::remove_var(STATE_DIR_ENV);
    }

    let (port, _rx) = serve_status(401, br#"Invalid or expired API key"#);
    let runtime = test_runtime(port, "SRV-ENV-REVOKE");
    let outcome = zotero_cli::write_router::patch_item(
        &runtime,
        "/api/users/0/items/ABCD1234",
        "ABCD1234",
        &json!({"title": "x"}),
        1,
    )
    .unwrap();

    assert!(matches!(
        outcome,
        WriteOutcome::AuthorizationFailed {
            reason: AuthorizationReason::Revoked,
            source: CredentialSource::Environment,
            ..
        }
    ));
    assert_eq!(
        std::env::var(API_KEY_ENV).as_deref(),
        Ok("an-env-provided-key"),
        "the environment variable must never be mutated by this module"
    );

    unsafe {
        std::env::remove_var(API_KEY_ENV);
    }
}

#[test]
fn revoked_file_credential_removes_only_the_matching_server_id() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = temp_state_dir("revoke-scoped");
    unsafe {
        std::env::remove_var(API_KEY_ENV);
        std::env::set_var(STATE_DIR_ENV, &dir);
    }

    zotero_cli::credentials::store_credential(
        "SRV-A",
        &zotero_cli::credentials::LocalApiCredential {
            app_name: "zotero-rust-cli".to_string(),
            key: "key-a".to_string(),
            remember: true,
            issued_at: String::new(),
        },
    )
    .unwrap();
    zotero_cli::credentials::store_credential(
        "SRV-B",
        &zotero_cli::credentials::LocalApiCredential {
            app_name: "zotero-rust-cli".to_string(),
            key: "key-b".to_string(),
            remember: true,
            issued_at: String::new(),
        },
    )
    .unwrap();

    let (port, _rx) = serve_status(401, br#"Invalid or expired API key"#);
    let runtime = test_runtime(port, "SRV-A");
    let outcome = zotero_cli::write_router::patch_item(
        &runtime,
        "/api/users/0/items/ABCD1234",
        "ABCD1234",
        &json!({"title": "x"}),
        1,
    )
    .unwrap();
    assert!(matches!(
        outcome,
        WriteOutcome::AuthorizationFailed {
            reason: AuthorizationReason::Revoked,
            source: CredentialSource::Store,
            ..
        }
    ));

    let (credential_a, source_a) = zotero_cli::credentials::resolve_credential("SRV-A");
    assert!(credential_a.is_none(), "SRV-A's credential must be removed");
    assert_eq!(source_a, zotero_cli::credentials::CredentialSource::None);

    let (credential_b, source_b) = zotero_cli::credentials::resolve_credential("SRV-B");
    assert_eq!(source_b, zotero_cli::credentials::CredentialSource::Store);
    assert_eq!(
        credential_b.unwrap().key,
        "key-b",
        "SRV-B's credential must survive SRV-A's revocation"
    );

    std::fs::remove_dir_all(&dir).ok();
    unsafe {
        std::env::remove_var(STATE_DIR_ENV);
    }
}

#[test]
fn verify_absent_reads_a_404_as_absent_via_the_local_api_only() {
    let (port, rx) = serve_status(404, br#"{"message":"not found"}"#);
    let runtime = test_runtime(port, "SRV-VERIFY");

    let result = zotero_cli::write_router::verify_absent(&runtime, "/api/users/0/items/GONE0000");
    assert!(matches!(
        result,
        zotero_cli::write_router::PresenceCheck::Absent
    ));
    let request = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        request.head.starts_with("GET /api/users/0/items/GONE0000"),
        "verification must use a Local API GET: {}",
        request.head
    );
}

#[test]
fn verify_present_reads_a_200_item_as_present() {
    let (port, _rx) = serve_status(
        200,
        br#"{"key":"ABCD1234","version":5,"library":{"id":0},"data":{"itemType":"document","title":"X"}}"#,
    );
    let runtime = test_runtime(port, "SRV-VERIFY");

    let result = zotero_cli::write_router::verify_present(&runtime, "/api/users/0/items/ABCD1234");
    match result {
        zotero_cli::write_router::PresenceCheck::Present(summary) => {
            assert_eq!(summary.key, "ABCD1234");
            assert_eq!(summary.version, 5);
        }
        other => panic!("expected Present, got {other:?}"),
    }
}
