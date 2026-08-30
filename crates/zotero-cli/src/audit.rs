//! Port of `core/audit.py` append-only audit log for write/privileged operations.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::paths;

/// `audit_dir()` (`audit.py:12-19`).
pub fn audit_dir() -> PathBuf {
    let path = if let Ok(override_dir) = std::env::var("ZOTERO_CLI_AUDIT_DIR") {
        let trimmed = override_dir.trim();
        if trimmed.is_empty() {
            paths::expand_user_path("~/.local/share/cli-anything-zotero")
        } else {
            paths::expand_user_path(trimmed)
        }
    } else {
        paths::expand_user_path("~/.local/share/cli-anything-zotero")
    };
    let _ = std::fs::create_dir_all(&path);
    path
}

/// `audit_path()` (`audit.py:22-23`).
pub fn audit_path() -> PathBuf {
    audit_dir().join("audit.jsonl")
}

/// Format UTC ISO-8601 timestamp (%Y-%m-%dT%H:%M:%SZ) without external crate dependency.
pub fn now_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// `log_event()` (`audit.py:26-67`).
pub fn log_event(action: &str, fields: &[(&str, Value)]) -> Option<Value> {
    let mut entry = Map::new();
    entry.insert("ts".to_string(), Value::String(now_iso8601()));
    entry.insert("action".to_string(), Value::String(action.to_string()));

    for (key, val) in fields {
        if val.is_null() {
            continue;
        }
        if matches!(
            *key,
            "import_result" | "convert" | "doctor" | "attempts" | "items" | "details"
        ) {
            continue;
        }
        match val {
            Value::String(_) | Value::Number(_) | Value::Bool(_) => {
                entry.insert((*key).to_string(), val.clone());
            }
            Value::Array(_) | Value::Object(_) => {
                let text = serde_json::to_string(val).unwrap_or_default();
                if text.len() <= 2000 {
                    entry.insert((*key).to_string(), val.clone());
                } else {
                    let mut trunc = Map::new();
                    trunc.insert("_truncated".to_string(), Value::Bool(true));
                    trunc.insert(
                        "type".to_string(),
                        Value::String(if val.is_array() {
                            "list".to_string()
                        } else {
                            "dict".to_string()
                        }),
                    );
                    trunc.insert(
                        "size".to_string(),
                        Value::Number(serde_json::Number::from(text.len())),
                    );
                    entry.insert((*key).to_string(), Value::Object(trunc));
                }
            }
            _ => {
                let s = val.to_string();
                let truncated = if s.len() > 500 { &s[..500] } else { &s };
                entry.insert((*key).to_string(), Value::String(truncated.to_string()));
            }
        }
    }

    let val = Value::Object(entry);
    let line = serde_json::to_string(&val).ok()?;
    let path = audit_path();
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()?;
    writeln!(file, "{line}").ok()?;
    Some(val)
}

/// `log_payload()` (`audit.py:69-107`).
pub fn log_payload(payload: &Value) -> Option<Value> {
    let obj = payload.as_object()?;
    let action = obj.get("action")?.as_str()?;
    if action.is_empty() {
        return None;
    }

    let readish = matches!(
        action,
        "app_doctor" | "item_duplicates" | "item_merge_preview" | "collection_stats"
    );
    let is_dry_run = obj.get("status").and_then(Value::as_str) == Some("dry_run");
    if readish && !is_dry_run {
        return None;
    }

    let keep_val = match obj.get("keep") {
        Some(Value::Object(map)) => map.get("key").cloned().unwrap_or(Value::Null),
        Some(other) => other.clone(),
        None => Value::Null,
    };

    let path_or_output = obj
        .get("path")
        .cloned()
        .or_else(|| obj.get("output").cloned())
        .unwrap_or(Value::Null);

    let fields = [
        ("ok", obj.get("ok").cloned().unwrap_or(Value::Null)),
        ("status", obj.get("status").cloned().unwrap_or(Value::Null)),
        ("code", obj.get("code").cloned().unwrap_or(Value::Null)),
        ("key", obj.get("key").cloned().unwrap_or(Value::Null)),
        ("keep", keep_val),
        ("DOI", obj.get("DOI").cloned().unwrap_or(Value::Null)),
        ("path", path_or_output),
        ("source", obj.get("source").cloned().unwrap_or(Value::Null)),
        (
            "mode_used",
            obj.get("mode_used").cloned().unwrap_or(Value::Null),
        ),
        (
            "dry_run",
            obj.get("dry_run").cloned().unwrap_or(Value::Null),
        ),
        ("error", obj.get("error").cloned().unwrap_or(Value::Null)),
        (
            "collection",
            obj.get("collection").cloned().unwrap_or(Value::Null),
        ),
        ("url", obj.get("url").cloned().unwrap_or(Value::Null)),
        (
            "arxiv_id",
            obj.get("arxiv_id").cloned().unwrap_or(Value::Null),
        ),
        (
            "succeeded",
            obj.get("succeeded").cloned().unwrap_or(Value::Null),
        ),
        ("failed", obj.get("failed").cloned().unwrap_or(Value::Null)),
        ("found", obj.get("found").cloned().unwrap_or(Value::Null)),
        (
            "checked",
            obj.get("checked").cloned().unwrap_or(Value::Null),
        ),
    ];

    log_event(action, &fields)
}

/// `_maybe_audit()` (`zotero_cli.py:263-290`).
pub fn maybe_audit(payload: &Value) {
    let Some(obj) = payload.as_object() else {
        return;
    };
    let Some(action) = obj.get("action").and_then(Value::as_str) else {
        return;
    };
    if action.is_empty() {
        return;
    }
    let writeish = action.starts_with("add_")
        || action.starts_with("import_")
        || action.starts_with("item_attach")
        || action.starts_with("item_find_pdf")
        || action.starts_with("item_fetch_pdf")
        || action.starts_with("item_merge")
        || action.starts_with("collection_fetch")
        || action.starts_with("docx_cite")
        || action.starts_with("docx_")
        || matches!(
            action,
            "item_merge"
                | "item_attach"
                | "item_fetch_pdf"
                | "item_find_pdf"
                | "collection_fetch_pdfs"
                | "docx_cite"
        );
    if !writeish {
        return;
    }
    let _ = std::panic::catch_unwind(|| {
        log_payload(payload);
    });
}

/// `tail()` (`audit.py:109-124`).
pub fn tail(limit: usize) -> Vec<Value> {
    let path = audit_path();
    if !path.is_file() {
        return Vec::new();
    }
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    let take_count = limit.max(1);
    let start_idx = if lines.len() > take_count {
        lines.len() - take_count
    } else {
        0
    };
    let mut out = Vec::new();
    for line in &lines[start_idx..] {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(trimmed) {
            Ok(v) => out.push(v),
            Err(_) => {
                let mut err_entry = Map::new();
                err_entry.insert("raw".to_string(), Value::String(trimmed.to_string()));
                err_entry.insert("ok".to_string(), Value::Bool(false));
                err_entry.insert(
                    "error".to_string(),
                    Value::String("invalid json line".to_string()),
                );
                out.push(Value::Object(err_entry));
            }
        }
    }
    out
}
