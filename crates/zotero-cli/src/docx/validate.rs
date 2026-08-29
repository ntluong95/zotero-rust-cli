//! Placeholder validation against the local Zotero database.
//!
//! Ports `docx validate-placeholders`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use super::inspect::inspect_placeholders;
use crate::catalog;
use crate::db::Item;
use crate::runtime::RuntimeContext;
use crate::session::SessionState;

static YEAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d{4}").expect("YEAR_RE must compile"));

/// Validate DOCX Zotero placeholders against the local Zotero database.
pub fn validate_placeholders(
    runtime: &RuntimeContext,
    path: &Path,
    sample_limit: usize,
    session: &SessionState,
) -> anyhow::Result<Value> {
    let base_report = inspect_placeholders(path, sample_limit)?;
    let mut report_map = match base_report {
        Value::Object(m) => m,
        _ => Map::new(),
    };

    let unique_keys: Vec<String> = report_map
        .get("unique_keys")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let invalid_count = report_map
        .get("invalid_placeholders")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);

    let mut items = Vec::new();
    let mut missing_keys = Vec::new();
    let mut errors: BTreeMap<String, String> = BTreeMap::new();

    for key in &unique_keys {
        match catalog::get_item(runtime, Some(key), session) {
            Ok(item) => {
                items.push(item_summary(&item));
            }
            Err(err) => {
                missing_keys.push(key.clone());
                errors.insert(key.clone(), err.to_string());
            }
        }
    }

    let ok = invalid_count == 0 && missing_keys.is_empty();

    let mut notes: Vec<String> = report_map
        .get("notes")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    if !missing_keys.is_empty() {
        notes
            .push("Some Zotero placeholder keys do not resolve to local Zotero items.".to_string());
    }
    if ok {
        notes.push("All Zotero placeholders resolve to real local Zotero items.".to_string());
    }

    report_map.insert("notes".to_string(), json!(notes));
    report_map.insert("ok".to_string(), json!(ok));
    report_map.insert("valid_count".to_string(), json!(items.len()));
    report_map.insert("missing_count".to_string(), json!(missing_keys.len()));
    report_map.insert("items".to_string(), json!(items));
    report_map.insert("missing_keys".to_string(), json!(missing_keys));
    report_map.insert("errors".to_string(), json!(errors));

    Ok(Value::Object(report_map))
}

pub fn item_summary(item: &Item) -> Value {
    let date_str = item
        .fields
        .get("date")
        .and_then(Value::as_str)
        .or(item.date.as_deref())
        .unwrap_or("");
    let year = YEAR_RE.find(date_str).map(|m| m.as_str().to_string());

    let title = if !item.title.is_empty() {
        item.title.clone()
    } else {
        item.fields
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };

    let doi = item
        .fields
        .get("DOI")
        .or_else(|| item.fields.get("doi"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            if !item.doi.is_empty() {
                Some(item.doi.clone())
            } else {
                None
            }
        });

    let pmid = item
        .fields
        .get("PMID")
        .or_else(|| item.fields.get("pmid"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut map = Map::new();
    map.insert("itemID".to_string(), json!(item.item_id));
    map.insert("key".to_string(), json!(item.key));
    map.insert("libraryID".to_string(), json!(item.library_id));
    map.insert("typeName".to_string(), json!(item.type_name));
    map.insert("title".to_string(), json!(title));
    map.insert("year".to_string(), json!(year));
    map.insert("doi".to_string(), json!(doi));
    map.insert("pmid".to_string(), json!(pmid));

    Value::Object(map)
}
