use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::csl::{is_truthy, value_to_python_string};
use crate::import_normalization::AttachmentDescriptor;
use crate::paths::expand_user_path;
use crate::runtime::RuntimeContext;

const REMOTE_PDF_ACCEPT: &str = "application/pdf,application/octet-stream;q=0.9,*/*;q=0.1";

pub trait AttachmentConnector {
    #[allow(clippy::too_many_arguments)]
    fn save_attachment(
        &mut self,
        port: u16,
        session_id: &str,
        parent_item_id: &str,
        title: &str,
        url: &str,
        content: &[u8],
        timeout: Duration,
    ) -> anyhow::Result<Value>;
}

pub trait RemotePdfFetcher {
    fn fetch_remote_pdf(
        &mut self,
        url: &str,
        delay_ms: i64,
        timeout: i64,
    ) -> anyhow::Result<Vec<u8>>;
}

pub struct HttpAttachmentConnector;
pub struct UreqRemotePdfFetcher;

impl AttachmentConnector for HttpAttachmentConnector {
    fn save_attachment(
        &mut self,
        port: u16,
        session_id: &str,
        parent_item_id: &str,
        title: &str,
        url: &str,
        content: &[u8],
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        crate::http::connector_save_attachment(
            port,
            session_id,
            parent_item_id,
            title,
            url,
            content,
            timeout,
        )
    }
}

impl RemotePdfFetcher for UreqRemotePdfFetcher {
    fn fetch_remote_pdf(
        &mut self,
        url: &str,
        delay_ms: i64,
        timeout: i64,
    ) -> anyhow::Result<Vec<u8>> {
        if delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(delay_ms as u64));
        }
        let mut response = ureq::get(url)
            .header("Accept", REMOTE_PDF_ACCEPT)
            .config()
            .timeout_global(Some(Duration::from_secs(timeout.max(1) as u64)))
            .http_status_as_error(false)
            .build()
            .call()
            .map_err(|err| anyhow::anyhow!("Attachment download failed for {url}: {err}"))?;
        let status = response.status().as_u16();
        if status != 200 {
            anyhow::bail!("Attachment download returned HTTP {status}: {url}");
        }
        let content = response.body_mut().read_to_vec()?;
        ensure_pdf_bytes(&content, url)?;
        Ok(content)
    }
}

pub fn ensure_pdf_bytes(content: &[u8], source: &str) -> anyhow::Result<()> {
    if content.starts_with(b"%PDF-") {
        Ok(())
    } else {
        anyhow::bail!("Attachment source is not a PDF: {source}")
    }
}

pub fn read_local_pdf(path: &str) -> anyhow::Result<(Vec<u8>, String, PathBuf)> {
    let expanded = expand_user_path(path);
    if !expanded.is_file() {
        anyhow::bail!("Attachment file not found: {path}");
    }
    let resolved = expanded.canonicalize()?;
    let content = std::fs::read(&resolved)?;
    ensure_pdf_bytes(&content, path)?;
    let uri = file_uri(&resolved);
    Ok((content, uri, resolved))
}

pub fn normalize_url_for_dedupe(url: &str) -> String {
    let trimmed = url.trim();
    let no_fragment = trimmed.split_once('#').map_or(trimmed, |(head, _)| head);
    let Some((scheme, rest)) = no_fragment.split_once("://") else {
        return no_fragment.to_string();
    };
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = rest[..authority_end].to_lowercase();
    let tail = &rest[authority_end..];
    let tail = if tail.is_empty() || tail.starts_with('?') {
        format!("/{tail}")
    } else {
        tail.to_string()
    };
    format!("{}://{}{}", scheme.to_lowercase(), authority, tail)
}

pub fn attachment_summary(results: &[Value]) -> Value {
    let count = |status: &str| {
        results
            .iter()
            .filter(|result| result.get("status").and_then(Value::as_str) == Some(status))
            .count()
    };
    serde_json::json!({
        "planned_count": results.len(),
        "created_count": count("created"),
        "failed_count": count("failed"),
        "skipped_count": count("skipped_duplicate"),
    })
}

#[allow(clippy::too_many_arguments)]
pub fn perform_attachment_upload<C, F>(
    runtime: &RuntimeContext,
    session_id: &str,
    connector_items: &[Value],
    plans: &[Value],
    connector: &mut C,
    fetcher: &mut F,
) -> (Value, Vec<Value>)
where
    C: AttachmentConnector,
    F: RemotePdfFetcher,
{
    let mut results = Vec::new();
    let mut dedupe: HashMap<String, ParentDedupe> = HashMap::new();
    for plan in plans {
        upload_plan(
            runtime,
            session_id,
            connector_items,
            plan,
            connector,
            fetcher,
            &mut dedupe,
            &mut results,
        );
    }
    (attachment_summary(&results), results)
}

#[derive(Default)]
struct ParentDedupe {
    paths: HashSet<String>,
    urls: HashSet<String>,
    hashes: HashSet<String>,
}

#[allow(clippy::too_many_arguments)]
fn upload_plan<C, F>(
    runtime: &RuntimeContext,
    session_id: &str,
    connector_items: &[Value],
    plan: &Value,
    connector: &mut C,
    fetcher: &mut F,
    dedupe: &mut HashMap<String, ParentDedupe>,
    results: &mut Vec<Value>,
) where
    C: AttachmentConnector,
    F: RemotePdfFetcher,
{
    let item_index = plan.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
    let attachments = plan
        .get("attachments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let item = connector_items.get(item_index);
    let expected_title = plan.get("expected_title").and_then(Value::as_str);
    let parent_id = item
        .and_then(|item| item.get("id"))
        .filter(|id| is_truthy(Some(id)));
    let parent_id_text = parent_id.map(value_to_python_string);
    let parent_id_ref = parent_id_text.as_deref();

    let preflight_error = match item {
        None => Some(format!("Import returned no item at index {item_index}")),
        Some(item) if expected_title.is_some() => {
            let expected = expected_title.unwrap();
            let actual = item_title(item);
            (actual != expected).then(|| {
                format!(
                    "Imported item title mismatch at index {item_index}: expected {expected:?}, got {actual:?}"
                )
            })
        }
        Some(_) if parent_id_ref.is_none() => Some(format!(
            "Imported item at index {item_index} did not include a connector id"
        )),
        Some(_) => None,
    };

    if let Some(error) = preflight_error {
        for raw in attachments {
            results.push(attachment_result(
                item_index,
                parent_id_ref,
                &descriptor_from_value(&raw),
                "failed",
                Some(&error),
            ));
        }
        return;
    }

    let parent_id = parent_id_ref.unwrap();
    for raw in attachments {
        let descriptor = descriptor_from_value(&raw);
        let result = match descriptor {
            Some(ref descriptor) => upload_descriptor(
                runtime,
                session_id,
                parent_id,
                item_index,
                descriptor,
                connector,
                fetcher,
                dedupe.entry(parent_id.to_string()).or_default(),
            ),
            None => Err(anyhow::anyhow!("Attachment descriptor is malformed")),
        };
        results.push(match result {
            Ok(Some(status)) => {
                attachment_result(item_index, Some(parent_id), &descriptor, status, None)
            }
            Ok(None) => continue,
            Err(error) => attachment_result(
                item_index,
                Some(parent_id),
                &descriptor,
                "failed",
                Some(&error.to_string()),
            ),
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn upload_descriptor<C, F>(
    runtime: &RuntimeContext,
    session_id: &str,
    parent_id: &str,
    _item_index: usize,
    descriptor: &AttachmentDescriptor,
    connector: &mut C,
    fetcher: &mut F,
    state: &mut ParentDedupe,
) -> anyhow::Result<Option<&'static str>>
where
    C: AttachmentConnector,
    F: RemotePdfFetcher,
{
    let (content, metadata_url, path_key, url_key) = if descriptor.source_type == "file" {
        let path_key = expand_user_path(&descriptor.source)
            .canonicalize()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|_| descriptor.source.clone());
        if state.paths.contains(&path_key) {
            return Ok(Some("skipped_duplicate"));
        }
        let (content, metadata_url, resolved) = read_local_pdf(&descriptor.source)?;
        (
            content,
            metadata_url,
            Some(resolved.to_string_lossy().into_owned()),
            None,
        )
    } else {
        let url_key = normalize_url_for_dedupe(&descriptor.source);
        if state.urls.contains(&url_key) {
            return Ok(Some("skipped_duplicate"));
        }
        (
            fetcher.fetch_remote_pdf(
                &descriptor.source,
                descriptor.delay_ms,
                descriptor.timeout,
            )?,
            descriptor.source.clone(),
            None,
            Some(url_key),
        )
    };

    let hash = format!("{:x}", Sha256::digest(&content));
    if state.hashes.contains(&hash) {
        return Ok(Some("skipped_duplicate"));
    }
    connector.save_attachment(
        runtime.environment.port,
        session_id,
        parent_id,
        &descriptor.title,
        &metadata_url,
        &content,
        Duration::from_secs(descriptor.timeout.max(1) as u64),
    )?;
    if let Some(path_key) = path_key {
        state.paths.insert(path_key);
    }
    if let Some(url_key) = url_key {
        state.urls.insert(url_key);
    }
    state.hashes.insert(hash);
    Ok(Some("created"))
}

fn descriptor_from_value(raw: &Value) -> Option<AttachmentDescriptor> {
    let obj = raw.as_object()?;
    Some(AttachmentDescriptor {
        source_type: obj.get("source_type")?.as_str()?.to_string(),
        source: obj.get("source")?.as_str()?.to_string(),
        title: obj.get("title")?.as_str()?.to_string(),
        delay_ms: obj.get("delay_ms").and_then(Value::as_i64).unwrap_or(0),
        timeout: obj.get("timeout").and_then(Value::as_i64).unwrap_or(30),
    })
}

fn attachment_result(
    item_index: usize,
    parent_id: Option<&str>,
    descriptor: &Option<AttachmentDescriptor>,
    status: &str,
    error: Option<&str>,
) -> Value {
    let mut out = Map::new();
    out.insert("item_index".to_string(), Value::from(item_index));
    out.insert(
        "parent_connector_id".to_string(),
        parent_id.map(Value::from).unwrap_or(Value::Null),
    );
    out.insert(
        "source_type".to_string(),
        descriptor
            .as_ref()
            .map(|d| Value::from(d.source_type.clone()))
            .unwrap_or(Value::Null),
    );
    out.insert(
        "source".to_string(),
        descriptor
            .as_ref()
            .map(|d| Value::from(d.source.clone()))
            .unwrap_or(Value::Null),
    );
    out.insert(
        "title".to_string(),
        descriptor
            .as_ref()
            .map(|d| Value::from(d.title.clone()))
            .unwrap_or(Value::Null),
    );
    out.insert("status".to_string(), Value::from(status));
    if let Some(error) = error {
        out.insert("error".to_string(), Value::from(error));
    }
    Value::Object(out)
}

fn item_title(item: &Value) -> String {
    ["title", "bookTitle", "publicationTitle"]
        .iter()
        .filter_map(|key| item.get(*key))
        .find(|value| is_truthy(Some(value)))
        .map(value_to_python_string)
        .unwrap_or_default()
}

fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut encoded = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}
