//! Local API write transport (`phase-06-js-bridge-and-injection-hardening.md` §3.2/§3.13 Slice
//! 3, live-verified against a real Zotero 10.0.1 instance --
//! `plans/research/zotero-10-impact-on-rust-port.md` §8). Deliberately thin: returns the raw
//! HTTP status/body/`Last-Modified-Version` for `write_router.rs` to interpret into a
//! `WriteOutcome`. No credential storage, no outcome mapping, no retry logic here.

use std::time::Duration;

use serde_json::Value;

use super::{base_url, read_response_body};

/// Raw response from a Local API write-adjacent call, before any `WriteOutcome` interpretation.
pub struct LocalWriteResponse {
    pub status: u16,
    pub body: String,
    /// `Last-Modified-Version`, when the response carried one (present on a successful
    /// PATCH/POST/DELETE; absent on error responses such as `401`/`428`).
    pub last_modified_version: Option<i64>,
}

fn last_modified_version_header(response: &ureq::http::Response<ureq::Body>) -> Option<i64> {
    response
        .headers()
        .get("Last-Modified-Version")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
}

/// `POST /api/local/authorize` -- LIVE VERIFIED as the actual trigger for Zotero's human
/// consent dialog (not a bare write, as `phase-06`'s original §3.2 assumed;
/// `zotero-10-impact-on-rust-port.md` §8.1 finding 4). This call blocks on a human GUI decision
/// and must never be invoked automatically inside a write command's own happy path (§3.4a) --
/// callers outside this crate's current scope (Slice 6) are responsible for only calling it from
/// an explicit, deliberate "authorize" action.
pub fn local_api_authorize(
    port: u16,
    server_id: &str,
    app_name: &str,
    timeout: Duration,
) -> anyhow::Result<LocalWriteResponse> {
    let payload = serde_json::json!({ "appName": app_name });
    let mut response = ureq::post(&base_url(port, "/api/local/authorize"))
        .header("Content-Type", "application/json")
        .header("Zotero-Server-ID", server_id)
        .config()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .send_json(&payload)
        .map_err(|err| anyhow::anyhow!("HTTP request failed for /api/local/authorize: {err}"))?;
    let status = response.status().as_u16();
    let last_modified_version = last_modified_version_header(&response);
    let body = read_response_body(&mut response).map_err(|err| {
        anyhow::anyhow!("Failed to read response body for /api/local/authorize: {err}")
    })?;
    Ok(LocalWriteResponse {
        status,
        body,
        last_modified_version,
    })
}

fn finish_response(
    path: &str,
    result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> anyhow::Result<LocalWriteResponse> {
    let mut response =
        result.map_err(|err| anyhow::anyhow!("HTTP request failed for {path}: {err}"))?;
    let status = response.status().as_u16();
    let last_modified_version = last_modified_version_header(&response);
    let body = read_response_body(&mut response)
        .map_err(|err| anyhow::anyhow!("Failed to read response body for {path}: {err}"))?;
    Ok(LocalWriteResponse {
        status,
        body,
        last_modified_version,
    })
}

/// Shared JSON-body request (PATCH/POST): both carry a JSON payload, so `ureq`'s typed builder
/// gives them the same `RequestBuilder<WithBody>` type -- unlike DELETE, which has none (see
/// [`local_write_no_body`]) and therefore cannot share this function's return type.
fn local_write_with_body(
    builder: ureq::RequestBuilder<ureq::typestate::WithBody>,
    path: &str,
    server_id: &str,
    api_key: &str,
    if_unmodified_since_version: Option<i64>,
    body: &Value,
    timeout: Duration,
) -> anyhow::Result<LocalWriteResponse> {
    let mut builder = builder
        .header("Zotero-API-Version", super::LOCAL_API_VERSION)
        .header("Zotero-Server-ID", server_id)
        .header("Zotero-API-Key", api_key)
        .header("Content-Type", "application/json");
    if let Some(version) = if_unmodified_since_version {
        builder = builder.header("If-Unmodified-Since-Version", version.to_string());
    }
    let request = builder
        .config()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build();
    finish_response(path, request.send_json(body))
}

fn local_write_no_body(
    builder: ureq::RequestBuilder<ureq::typestate::WithoutBody>,
    path: &str,
    server_id: &str,
    api_key: &str,
    if_unmodified_since_version: i64,
    timeout: Duration,
) -> anyhow::Result<LocalWriteResponse> {
    let request = builder
        .header("Zotero-API-Version", super::LOCAL_API_VERSION)
        .header("Zotero-Server-ID", server_id)
        .header("Zotero-API-Key", api_key)
        .header(
            "If-Unmodified-Since-Version",
            if_unmodified_since_version.to_string(),
        )
        .config()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build();
    finish_response(path, request.call())
}

/// `PATCH <path>` -- e.g. `/api/users/0/items/<key>`. LIVE VERIFIED: `204 No Content` on success
/// with a bumped `Last-Modified-Version`; `428` (missing `Zotero-Server-ID`, not sent here since
/// this function always sends it), `401` (missing/invalid/expired key) on failure.
pub fn local_api_patch(
    port: u16,
    path: &str,
    server_id: &str,
    api_key: &str,
    if_unmodified_since_version: i64,
    body: &Value,
    timeout: Duration,
) -> anyhow::Result<LocalWriteResponse> {
    local_write_with_body(
        ureq::patch(&base_url(port, path)),
        path,
        server_id,
        api_key,
        Some(if_unmodified_since_version),
        body,
        timeout,
    )
}

/// `POST <path>` -- creation (e.g. `/api/users/0/collections`). Not live-verified in Slice 0
/// (only PATCH was exercised); response-shape handling in `write_router.rs` must not assume a
/// specific success status beyond what Zotero's public Web API v3 docs document for creates.
pub fn local_api_post(
    port: u16,
    path: &str,
    server_id: &str,
    api_key: &str,
    body: &Value,
    timeout: Duration,
) -> anyhow::Result<LocalWriteResponse> {
    local_write_with_body(
        ureq::post(&base_url(port, path)),
        path,
        server_id,
        api_key,
        None,
        body,
        timeout,
    )
}

/// `DELETE <path>`. Not live-verified in Slice 0. `if_unmodified_since_version` is required by
/// Zotero's documented Write Requests contract for delete-class operations.
pub fn local_api_delete(
    port: u16,
    path: &str,
    server_id: &str,
    api_key: &str,
    if_unmodified_since_version: i64,
    timeout: Duration,
) -> anyhow::Result<LocalWriteResponse> {
    local_write_no_body(
        ureq::delete(&base_url(port, path)),
        path,
        server_id,
        api_key,
        if_unmodified_since_version,
        timeout,
    )
}
