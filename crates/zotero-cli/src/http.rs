//! Port of `utils/zotero_http.py`'s connector/Local API client.

use std::time::Duration;

use serde_json::Value;

pub mod connector;
pub mod local_write;

pub use connector::{
    connector_import_text, connector_save_attachment, connector_save_items,
    connector_update_session, get_selected_collection,
};
pub use local_write::{
    local_api_authorize, local_api_delete, local_api_patch, local_api_post, LocalWriteResponse,
};

pub const LOCAL_API_VERSION: &str = "3";

pub(crate) fn base_url(port: u16, path: &str) -> String {
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    format!("http://127.0.0.1:{port}{path}")
}

pub(crate) fn read_response_body(
    response: &mut ureq::http::Response<ureq::Body>,
) -> anyhow::Result<String> {
    let raw = response
        .body_mut()
        .read_to_vec()
        .map_err(|err| anyhow::anyhow!("Failed to read response body: {err}"))?;
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

/// `connector_is_available()` (`zotero_http.py:87-94`).
pub fn connector_is_available(port: u16, timeout: Duration) -> (bool, String) {
    match ureq::get(&base_url(port, "/connector/ping"))
        .header("Accept", "*/*")
        .config()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .call()
    {
        Ok(response) => {
            let status = response.status().as_u16();
            if status == 200 {
                (true, "connector available".to_string())
            } else {
                (false, format!("connector returned HTTP {status}"))
            }
        }
        Err(err) => (
            false,
            format!("HTTP request failed for /connector/ping: {err}"),
        ),
    }
}

/// `wait_for_endpoint()` (`zotero_http.py:208-226`): polls `path` until a response's status is
/// in `ready_statuses` or `timeout` elapses, matching Python's loop shape exactly -- deadline
/// checked *before* each attempt, a per-attempt 3s request timeout independent of the overall
/// poll `timeout`, and `poll_interval` slept after every attempt (including the last one before
/// the deadline trips). A transport-level failure (connection refused, DNS, etc.) is swallowed
/// the same way Python's `except RuntimeError: pass` swallows `request()`'s `RuntimeError` --
/// only an HTTP response with a non-ready status, or no response at all, keeps polling.
pub fn wait_for_endpoint(
    port: u16,
    path: &str,
    timeout: Duration,
    poll_interval: Duration,
    headers: &[(&str, &str)],
    ready_statuses: &[u16],
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        let mut request = ureq::get(&base_url(port, path));
        for (key, value) in headers {
            request = request.header(*key, *value);
        }
        let response = request
            .config()
            .timeout_global(Some(Duration::from_secs(3)))
            .http_status_as_error(false)
            .build()
            .call();
        if let Ok(response) = response {
            if ready_statuses.contains(&response.status().as_u16()) {
                return true;
            }
        }
        std::thread::sleep(poll_interval);
    }
    false
}

/// Result of probing `GET /api/`: availability (Python-parity shape) plus
/// the Zotero 10+ capability discriminator that has no Python equivalent.
pub struct LocalApiProbe {
    pub available: bool,
    pub message: String,
    /// The `Zotero-Server-ID` response header, when present. Live-confirmed
    /// (2026-08-29, real Zotero 10.0.1) to appear on **every** response from
    /// this endpoint, including a `403` when the Local API itself is
    /// disabled in preferences -- so its presence is a reliable Zotero 10+
    /// discriminator independent of whether the Local API is currently
    /// usable. Preferred over parsing `environment.version` (the installed
    /// binary found on disk, which can disagree with the *running*
    /// instance the HTTP port actually belongs to) per
    /// `phase-14-zotero-10-compatibility-gate.md` §4.
    pub server_id: Option<String>,
}

/// `local_api_is_available()` (`zotero_http.py:196-205`): a 403 is
/// special-cased to the exact string `"local API disabled"`. Single-probe
/// helper -- see `probe_local_api` for the Zotero 10+ capability signal
/// this shares the same HTTP round trip with.
pub fn probe_local_api(port: u16, timeout: Duration) -> LocalApiProbe {
    match ureq::get(&base_url(port, "/api/"))
        .header("Zotero-API-Version", LOCAL_API_VERSION)
        .config()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .call()
    {
        Ok(response) => {
            let server_id = response
                .headers()
                .get("Zotero-Server-ID")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let status = response.status().as_u16();
            let (available, message) = if status == 200 {
                (true, "local API available".to_string())
            } else if status == 403 {
                (false, "local API disabled".to_string())
            } else {
                (false, format!("local API returned HTTP {status}"))
            };
            LocalApiProbe {
                available,
                message,
                server_id,
            }
        }
        Err(err) => LocalApiProbe {
            available: false,
            message: format!("HTTP request failed for /api/: {err}"),
            server_id: None,
        },
    }
}

/// `local_api_is_available()` (`zotero_http.py:196-205`), landed in Phase 5.
/// Thin wrapper over `probe_local_api` preserving the original Python
/// function's `(bool, str)` shape. `runtime::build_runtime_context` calls
/// `probe_local_api` directly as of Phase 14 (it needs `server_id` too, from
/// the same round trip) -- this has no callers in-crate as a result, kept
/// for the documented Python-parity surface (`phase-05` §Success Criteria
/// names it explicitly) and any future caller that only needs the plain
/// availability tuple.
pub fn local_api_is_available(port: u16, timeout: Duration) -> (bool, String) {
    let probe = probe_local_api(port, timeout);
    (probe.available, probe.message)
}

/// `local_api_get_json()` (`zotero_http.py:229-233`).
pub fn local_api_get_json(
    port: u16,
    path: &str,
    params: &[(&str, String)],
    timeout: Duration,
) -> anyhow::Result<Value> {
    let mut request = ureq::get(&base_url(port, path))
        .header("Zotero-API-Version", LOCAL_API_VERSION)
        .header("Accept", "application/json");
    for (key, value) in params {
        request = request.query(*key, value);
    }
    // Disable status-as-error so a non-200 response is inspectable here
    // (status + body), matching Python's HTTPError-caught-into-HttpResponse
    // handling rather than losing the body to a generic transport error.
    let mut response = request
        .config()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|err| anyhow::anyhow!("HTTP request failed for {path}: {err}"))?;
    let status = response.status().as_u16();
    // Matches Python's `response.read().decode("utf-8", errors="replace")`:
    // never fails on invalid UTF-8 (substitutes replacement characters),
    // but a genuine I/O failure still propagates instead of silently
    // becoming an empty body.
    let body = read_response_body(&mut response)
        .map_err(|err| anyhow::anyhow!("Failed to read response body for {path}: {err}"))?;
    if status != 200 {
        anyhow::bail!("Local API returned HTTP {status} for {path}: {body}");
    }
    Ok(serde_json::from_str(&body)?)
}

/// `local_api_get_text()` (`zotero_http.py:236-240`): unlike
/// [`local_api_get_json`], the `Accept` header is left at the request-layer
/// default (`*/*`) rather than overridden to `application/json` -- matches
/// Python's `local_api_get_text`, which only ever passes the
/// `Zotero-API-Version` header through and never touches `Accept`.
pub fn local_api_get_text(
    port: u16,
    path: &str,
    params: &[(&str, String)],
    timeout: Duration,
) -> anyhow::Result<String> {
    let mut request = ureq::get(&base_url(port, path))
        .header("Zotero-API-Version", LOCAL_API_VERSION)
        .header("Accept", "*/*");
    for (key, value) in params {
        request = request.query(*key, value);
    }
    let mut response = request
        .config()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|err| anyhow::anyhow!("HTTP request failed for {path}: {err}"))?;
    let status = response.status().as_u16();
    let body = read_response_body(&mut response)
        .map_err(|err| anyhow::anyhow!("Failed to read response body for {path}: {err}"))?;
    if status != 200 {
        anyhow::bail!("Local API returned HTTP {status} for {path}: {body}");
    }
    Ok(body)
}

/// Status/body-preserving counterpart to [`local_api_get_json`], for `write_router`'s
/// post-write verification primitives (§Blocker 3). Unlike `local_api_get_json`, this never
/// `bail!`s on a non-200 status -- a `404` after a `DELETE` is the *expected* success case for
/// an absence check, not an error to be swallowed into a generic transport failure.
pub fn local_api_get_raw(
    port: u16,
    path: &str,
    timeout: Duration,
) -> anyhow::Result<LocalWriteResponse> {
    let mut response = ureq::get(&base_url(port, path))
        .header("Zotero-API-Version", LOCAL_API_VERSION)
        .header("Accept", "application/json")
        .config()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|err| anyhow::anyhow!("HTTP request failed for {path}: {err}"))?;
    let status = response.status().as_u16();
    let body = read_response_body(&mut response)
        .map_err(|err| anyhow::anyhow!("Failed to read response body for {path}: {err}"))?;
    Ok(LocalWriteResponse {
        status,
        body,
        last_modified_version: None,
    })
}
