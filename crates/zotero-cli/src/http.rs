//! Port of `utils/zotero_http.py`'s connector/Local API client.

use std::time::Duration;

use serde_json::Value;

pub mod connector;

pub use connector::{
    connector_import_text, connector_save_attachment, connector_save_items,
    connector_update_session, get_selected_collection,
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
