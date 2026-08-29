use std::time::Duration;

use anyhow::Context;
use serde_json::{Map, Value};

use super::{base_url, read_response_body};

fn python_json_text(value: &Value) -> anyhow::Result<String> {
    match value {
        Value::Array(values) => {
            let rendered = values
                .iter()
                .map(python_json_text)
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok(format!("[{}]", rendered.join(", ")))
        }
        Value::Object(map) => {
            let rendered = map
                .iter()
                .map(|(key, value)| {
                    Ok(format!(
                        "{}: {}",
                        serde_json::to_string(key)?,
                        python_json_text(value)?
                    ))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok(format!("{{{}}}", rendered.join(", ")))
        }
        _ => Ok(serde_json::to_string(value)?),
    }
}

fn connector_json_post(
    port: u16,
    path: &str,
    payload: &Value,
    timeout: Duration,
) -> anyhow::Result<(u16, String)> {
    let body = python_json_text(payload)?;
    connector_post_bytes(
        port,
        path,
        None,
        "application/json",
        body.as_bytes(),
        timeout,
    )
}

fn connector_post_bytes(
    port: u16,
    path: &str,
    query: Option<(&str, &str)>,
    content_type: &str,
    body: &[u8],
    timeout: Duration,
) -> anyhow::Result<(u16, String)> {
    let mut request = ureq::post(&base_url(port, path))
        .header("Accept", "*/*")
        .header("Content-Type", content_type);
    if let Some((key, value)) = query {
        request = request.query(key, value);
    }
    let mut response = request
        .config()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .send(body)
        .map_err(|err| anyhow::anyhow!("HTTP request failed for {path}: {err}"))?;
    let status = response.status().as_u16();
    let response_body = read_response_body(&mut response)
        .map_err(|err| anyhow::anyhow!("Failed to read response body for {path}: {err}"))?;
    Ok((status, response_body))
}

fn ensure_status(operation: &str, status: u16, body: &str, expected: &[u16]) -> anyhow::Result<()> {
    if expected.contains(&status) {
        Ok(())
    } else {
        anyhow::bail!("connector/{operation} returned HTTP {status}: {body}")
    }
}

pub fn get_selected_collection(port: u16, timeout: Duration) -> anyhow::Result<Value> {
    let (status, body) = connector_json_post(
        port,
        "/connector/getSelectedCollection",
        &Value::Object(Map::new()),
        timeout,
    )?;
    ensure_status("getSelectedCollection", status, &body, &[200])?;
    serde_json::from_str(&body).context("connector/getSelectedCollection returned invalid JSON")
}

pub fn connector_import_text(
    port: u16,
    content: &[u8],
    session_id: Option<&str>,
    content_type: &str,
    timeout: Duration,
) -> anyhow::Result<Vec<Value>> {
    let query = session_id.map(|session| ("session", session));
    let (status, body) = connector_post_bytes(
        port,
        "/connector/import",
        query,
        content_type,
        content,
        timeout,
    )?;
    ensure_status("import", status, &body, &[201])?;
    let parsed: Value =
        serde_json::from_str(&body).context("connector/import returned invalid JSON")?;
    match parsed {
        Value::Array(items) => Ok(items),
        value => Ok(vec![value]),
    }
}

pub fn connector_save_items(
    port: u16,
    items: &[Value],
    session_id: &str,
    timeout: Duration,
) -> anyhow::Result<()> {
    let payload = serde_json::json!({
        "sessionID": session_id,
        "items": items,
    });
    let (status, body) = connector_json_post(port, "/connector/saveItems", &payload, timeout)?;
    ensure_status("saveItems", status, &body, &[201])
}

pub fn connector_save_attachment(
    port: u16,
    session_id: &str,
    parent_item_id: &str,
    title: &str,
    url: &str,
    content: &[u8],
    timeout: Duration,
) -> anyhow::Result<Value> {
    let metadata = serde_json::json!({
        "sessionID": session_id,
        "parentItemID": parent_item_id,
        "title": title,
        "url": url,
    });
    let mut response = ureq::post(&base_url(port, "/connector/saveAttachment"))
        .header("Accept", "*/*")
        .header("Content-Type", "application/pdf")
        .header("X-Metadata", python_json_text(&metadata)?)
        .config()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .send(content)
        .map_err(|err| {
            anyhow::anyhow!("HTTP request failed for /connector/saveAttachment: {err}")
        })?;
    let status = response.status().as_u16();
    let body = read_response_body(&mut response).map_err(|err| {
        anyhow::anyhow!("Failed to read response body for /connector/saveAttachment: {err}")
    })?;
    ensure_status("saveAttachment", status, &body, &[200, 201])?;
    if body.is_empty() {
        Ok(Value::Object(Map::new()))
    } else {
        serde_json::from_str(&body).context("connector/saveAttachment returned invalid JSON")
    }
}

pub fn connector_update_session(
    port: u16,
    session_id: &str,
    target: &str,
    tags: &[String],
    timeout: Duration,
) -> anyhow::Result<Value> {
    let payload = serde_json::json!({
        "sessionID": session_id,
        "target": target,
        "tags": tags
            .iter()
            .filter(|tag| !tag.trim().is_empty())
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", "),
    });
    let (status, body) = connector_json_post(port, "/connector/updateSession", &payload, timeout)?;
    ensure_status("updateSession", status, &body, &[200])?;
    if body.is_empty() {
        Ok(Value::Object(Map::new()))
    } else {
        serde_json::from_str(&body).context("connector/updateSession returned invalid JSON")
    }
}
