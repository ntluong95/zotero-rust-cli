use regex::Regex;
use serde_json::{Map, Value};
use std::sync::LazyLock;

use crate::csl::{csl_item_to_connector, is_truthy, looks_like_csl_item, value_to_python_string};

static DOI_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^https?://(dx\.)?doi\.org/").unwrap());
static DOI_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^doi:\s*").unwrap());
static BIBTEX_ENTRY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*@\w+\s*\{").unwrap());

pub fn normalize_doi(doi: Option<&str>) -> String {
    let text = doi.unwrap_or_default().trim();
    let text = DOI_URL_RE.replace(text, "");
    let text = DOI_PREFIX_RE.replace(&text, "");
    text.trim()
        .trim_end_matches([' ', '.', ')', ',', ';'])
        .to_string()
}

pub fn count_bibtex_entries(content: &str) -> usize {
    BIBTEX_ENTRY_RE.find_iter(content).count()
}

pub fn split_bibtex_entries(content: &str) -> Vec<String> {
    let matches = BIBTEX_ENTRY_RE.find_iter(content).collect::<Vec<_>>();
    if matches.is_empty() {
        let stripped = content.trim();
        return if stripped.is_empty() {
            Vec::new()
        } else {
            vec![stripped.to_string()]
        };
    }
    matches
        .iter()
        .enumerate()
        .filter_map(|(index, mat)| {
            let end = matches
                .get(index + 1)
                .map(|next| next.start())
                .unwrap_or(content.len());
            let entry = content[mat.start()..end].trim();
            (!entry.is_empty()).then(|| entry.to_string())
        })
        .collect()
}

fn str_or_empty(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn first_or_python_index(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(Value::Array(values)) => values.first().cloned(),
        Some(Value::String(text)) => text.chars().next().map(|ch| Value::String(ch.to_string())),
        Some(other) => Some(other.clone()),
        None => None,
    }
}

fn crossref_title(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(Value::Array(values)) => values.first().cloned(),
        Some(other) => Some(other.clone()),
        None => None,
    }
}

fn crossref_issued(msg: &Map<String, Value>) -> Value {
    if let Some(issued) = msg.get("issued").filter(|value| is_truthy(Some(value))) {
        return issued.clone();
    }
    let date_parts = msg
        .get("published-print")
        .and_then(Value::as_object)
        .and_then(|published| published.get("date-parts"))
        .and_then(Value::as_array)
        .and_then(|parts| parts.first())
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    serde_json::json!({ "date-parts": [date_parts] })
}

fn crossref_work_to_item(msg: &Map<String, Value>) -> Map<String, Value> {
    let mut csl = Map::new();
    csl.insert(
        "type".to_string(),
        Value::String("article-journal".to_string()),
    );
    csl.insert(
        "title".to_string(),
        crossref_title(msg.get("title")).unwrap_or(Value::String(String::new())),
    );
    csl.insert(
        "DOI".to_string(),
        msg.get("DOI").cloned().unwrap_or(Value::Null),
    );
    csl.insert(
        "URL".to_string(),
        msg.get("URL").cloned().unwrap_or(Value::Null),
    );
    csl.insert(
        "container-title".to_string(),
        first_or_python_index(msg.get("container-title")).unwrap_or(Value::Null),
    );
    for key in ["volume", "issue", "page"] {
        csl.insert(
            key.to_string(),
            msg.get(key).cloned().unwrap_or(Value::Null),
        );
    }
    let authors = msg
        .get("author")
        .and_then(Value::as_array)
        .map(|authors| {
            authors
                .iter()
                .filter_map(|author| {
                    let author = author.as_object()?;
                    let mut out = Map::new();
                    out.insert(
                        "family".to_string(),
                        Value::String(str_or_empty(author.get("family"))),
                    );
                    out.insert(
                        "given".to_string(),
                        Value::String(str_or_empty(author.get("given"))),
                    );
                    Some(Value::Object(out))
                })
                .collect()
        })
        .unwrap_or_default();
    csl.insert("author".to_string(), Value::Array(authors));
    csl.insert("issued".to_string(), crossref_issued(msg));
    csl_item_to_connector(&csl, 1)
}

pub fn normalize_import_json_payload(payload: &Value) -> anyhow::Result<(Vec<Value>, String)> {
    if let Some(msg) = payload
        .as_object()
        .and_then(|obj| obj.get("message"))
        .and_then(Value::as_object)
    {
        if is_truthy(msg.get("DOI")) || is_truthy(msg.get("title")) {
            return Ok((
                vec![Value::Object(crossref_work_to_item(msg))],
                "crossref".to_string(),
            ));
        }
    }

    let items_raw = if let Some(items) = payload.as_array() {
        items
    } else if let Some(obj) = payload.as_object() {
        if let Some(items) = obj.get("items").and_then(Value::as_array) {
            items
        } else if looks_like_csl_item(payload) || is_truthy(obj.get("itemType")) {
            return normalize_import_json_payload(&Value::Array(vec![payload.clone()]));
        } else {
            anyhow::bail!(
                "JSON import expects an array, {{items:[...]}}, CSL object, or Crossref work"
            );
        }
    } else {
        anyhow::bail!("JSON import expects an array of objects");
    };

    if items_raw.is_empty() {
        return Ok((Vec::new(), "empty".to_string()));
    }

    let first = items_raw
        .first()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if is_truthy(first.get("itemType")) {
        let mut out = Vec::with_capacity(items_raw.len());
        for (index, item) in items_raw.iter().enumerate() {
            let Some(item) = item.as_object() else {
                anyhow::bail!("JSON import item {} is not an object", index + 1);
            };
            let mut copied = item.clone();
            copied
                .entry("id".to_string())
                .or_insert_with(|| Value::String(format!("cli-anything-zotero-{}", index + 1)));
            out.push(Value::Object(copied));
        }
        return Ok((out, "connector".to_string()));
    }

    if looks_like_csl_item(items_raw.first().unwrap()) || first.contains_key("type") {
        let mut out = Vec::with_capacity(items_raw.len());
        for (index, item) in items_raw.iter().enumerate() {
            let Some(item) = item.as_object() else {
                anyhow::bail!("JSON import item {} is not an object", index + 1);
            };
            out.push(Value::Object(csl_item_to_connector(item, index + 1)));
        }
        return Ok((out, "csl-json".to_string()));
    }

    let mut out = Vec::with_capacity(items_raw.len());
    for (index, item) in items_raw.iter().enumerate() {
        let Some(item) = item.as_object() else {
            anyhow::bail!("JSON import item {} is not an object", index + 1);
        };
        let mut copied = item.clone();
        copied
            .entry("itemType".to_string())
            .or_insert_with(|| Value::String("journalArticle".to_string()));
        copied
            .entry("id".to_string())
            .or_insert_with(|| Value::String(format!("cli-anything-zotero-{}", index + 1)));
        out.push(Value::Object(copied));
    }
    Ok((out, "connector-fallback".to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentDescriptor {
    pub source_type: String,
    pub source: String,
    pub title: String,
    pub delay_ms: i64,
    pub timeout: i64,
}

fn normalize_attachment_int(
    value: Option<&Value>,
    name: &str,
    default: i64,
    minimum: i64,
) -> anyhow::Result<i64> {
    let value = value.cloned().unwrap_or(Value::from(default));
    let parsed = match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_u64().and_then(|n| i64::try_from(n).ok()))
            .or_else(|| number.as_f64().map(|n| n as i64)),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        Value::Bool(value) => Some(i64::from(value)),
        _ => None,
    }
    .ok_or_else(|| anyhow::anyhow!("Attachment `{name}` must be an integer"))?;
    if parsed < minimum {
        let comparator = if minimum == 0 {
            "greater than or equal to".to_string()
        } else {
            format!("at least {minimum}")
        };
        anyhow::bail!("Attachment `{name}` must be {comparator}");
    }
    Ok(parsed)
}

pub fn normalize_attachment_descriptor(
    raw: &Value,
    index_label: &str,
    attachment_label: &str,
    default_delay_ms: i64,
    default_timeout: i64,
) -> anyhow::Result<AttachmentDescriptor> {
    let Some(raw) = raw.as_object() else {
        anyhow::bail!("{index_label} {attachment_label} must be an object");
    };
    let has_path = raw
        .get("path")
        .is_some_and(|v| !matches!(v, Value::Null) && v.as_str() != Some(""));
    let has_url = raw
        .get("url")
        .is_some_and(|v| !matches!(v, Value::Null) && v.as_str() != Some(""));
    if has_path == has_url {
        anyhow::bail!(
            "{index_label} {attachment_label} must include exactly one of `path` or `url`"
        );
    }
    let title = raw
        .get("title")
        .filter(|value| is_truthy(Some(value)))
        .map(value_to_python_string)
        .unwrap_or_else(|| "PDF".to_string());
    let title = title.trim();
    let title = if title.is_empty() { "PDF" } else { title };
    let delay_ms = normalize_attachment_int(raw.get("delay_ms"), "delay_ms", default_delay_ms, 0)?;
    let timeout = normalize_attachment_int(raw.get("timeout"), "timeout", default_timeout, 1)?;
    let key = if has_path { "path" } else { "url" };
    let source = raw
        .get(key)
        .map(value_to_python_string)
        .as_deref()
        .map(str::trim)
        .map(str::to_string)
        .unwrap_or_default();
    if source.is_empty() {
        anyhow::bail!("{index_label} {attachment_label} {key} must not be empty");
    }
    Ok(AttachmentDescriptor {
        source_type: if has_path { "file" } else { "url" }.to_string(),
        source,
        title: title.to_string(),
        delay_ms,
        timeout,
    })
}

pub fn extract_inline_attachment_plans(
    items: &[Value],
    default_delay_ms: i64,
    default_timeout: i64,
) -> anyhow::Result<(Vec<Value>, Vec<Value>)> {
    let mut stripped_items = Vec::with_capacity(items.len());
    let mut plans = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let Some(item) = item.as_object() else {
            anyhow::bail!("JSON import item {} is not an object", index + 1);
        };
        let mut copied = item.clone();
        let raw_attachments = copied
            .remove("attachments")
            .unwrap_or(Value::Array(Vec::new()));
        if raw_attachments.is_null() || raw_attachments == Value::Array(Vec::new()) {
            stripped_items.push(Value::Object(copied));
            continue;
        }
        let Some(raw_attachments) = raw_attachments.as_array() else {
            anyhow::bail!(
                "JSON import item {} attachments must be an array",
                index + 1
            );
        };
        let attachments = raw_attachments
            .iter()
            .enumerate()
            .map(|(attachment_index, descriptor)| {
                let descriptor = normalize_attachment_descriptor(
                    descriptor,
                    &format!("JSON import item {}", index + 1),
                    &format!("attachment {}", attachment_index + 1),
                    default_delay_ms,
                    default_timeout,
                )?;
                Ok(serde_json::json!({
                    "source_type": descriptor.source_type,
                    "source": descriptor.source,
                    "title": descriptor.title,
                    "delay_ms": descriptor.delay_ms,
                    "timeout": descriptor.timeout,
                }))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        plans.push(serde_json::json!({ "index": index, "attachments": attachments }));
        stripped_items.push(Value::Object(copied));
    }
    Ok((stripped_items, plans))
}
