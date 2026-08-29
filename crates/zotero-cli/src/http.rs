//! Port of `utils/zotero_http.py`'s connector/Local API client (only the
//! read paths needed by the vertical slice: availability probes and
//! `local_api_get_json`, used by `item find`'s Local-API-first branch).

use std::time::Duration;

use serde_json::Value;

pub const LOCAL_API_VERSION: &str = "3";

fn base_url(port: u16, path: &str) -> String {
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    format!("http://127.0.0.1:{port}{path}")
}

/// `connector_is_available()` (`zotero_http.py:87-94`).
pub fn connector_is_available(port: u16, timeout: Duration) -> (bool, String) {
    match ureq::get(&base_url(port, "/connector/ping"))
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

/// `local_api_is_available()` (`zotero_http.py:196-205`): a 403 is
/// special-cased to the exact string `"local API disabled"`.
pub fn local_api_is_available(port: u16, timeout: Duration) -> (bool, String) {
    match ureq::get(&base_url(port, "/api/"))
        .header("Zotero-API-Version", LOCAL_API_VERSION)
        .config()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .call()
    {
        Ok(response) => {
            let status = response.status().as_u16();
            if status == 200 {
                (true, "local API available".to_string())
            } else if status == 403 {
                (false, "local API disabled".to_string())
            } else {
                (false, format!("local API returned HTTP {status}"))
            }
        }
        Err(err) => (false, format!("HTTP request failed for /api/: {err}")),
    }
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
    let raw = response
        .body_mut()
        .read_to_vec()
        .map_err(|err| anyhow::anyhow!("Failed to read response body for {path}: {err}"))?;
    let body = String::from_utf8_lossy(&raw).into_owned();
    if status != 200 {
        anyhow::bail!("Local API returned HTTP {status} for {path}: {body}");
    }
    Ok(serde_json::from_str(&body)?)
}
