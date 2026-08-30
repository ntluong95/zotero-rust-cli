//! Collection-level PDF batch driver (`core/pdf_fetch.py::fetch_pdfs_for_collection` and
//! `core/jsbridge.py::find_pdfs_in_collection`, Phase 7 Slice 3). Backend-only, no CLI dispatch.
//!
//! Resume state is load-bearing: file path/name, JSON shape, and fail-open-on-malformed-file
//! behavior are ported byte-for-byte so a Python-started `--resume` run can be picked up by this
//! binary (a stated Phase 7 success criterion). Do not "improve" this format.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

use crate::bridge::JSBridgeClient;
use crate::pdf_fetch::{self, PdfDownloadClient, PdfMetadataClient};
use crate::runtime::RuntimeContext;

static UNSAFE_KEY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^A-Za-z0-9_-]+").unwrap());

/// Guards `HOME`/`USERPROFILE` mutation for any test (in this module or in an including
/// integration test binary) that needs to redirect `resume_state_path`'s `~/.cache/...` base to
/// a temp directory. All such tests must hold this for their entire duration.
pub static RESUME_HOME_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `_resume_state_path()`: `~/.cache/cli-anything-zotero/fetch-pdfs-{safe_key}.json`, where
/// `safe_key` collapses every **run** of non-`[A-Za-z0-9_-]` characters into a single `_`
/// (`re.sub(r"[^A-Za-z0-9_-]+", "_", key)` -- a run, not one underscore per invalid character).
fn resume_state_path(collection_key: &str) -> PathBuf {
    let base = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cache")
        .join("cli-anything-zotero");
    let _ = std::fs::create_dir_all(&base);
    let safe = UNSAFE_KEY_RE.replace_all(collection_key, "_");
    base.join(format!("fetch-pdfs-{safe}.json"))
}

/// `load_resume_keys()`: **fail-open** by design, matching Python exactly -- a missing file or
/// malformed JSON is treated as "nothing completed yet," never surfaced as an error. (This is a
/// deliberate divergence from this project's own Phase 6 fail-closed convention for the Local
/// API credential store; ported as-is per the Slice 3 spec's explicit instruction not to invent
/// an improved format. See the PR description's open-questions note.)
pub fn load_resume_keys(collection_key: &str) -> HashSet<String> {
    let path = resume_state_path(collection_key);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return HashSet::new();
    };
    let Ok(data) = serde_json::from_str::<Value>(&text) else {
        return HashSet::new();
    };
    data.get("completed_keys")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `save_resume_key()`: read-modify-write, sorted ascending, `indent=2` -- and, matching Python
/// exactly, **no file lock**. This is precisely why the collection batch driver below must stay
/// serial: a concurrent writer would race this read-modify-write.
pub fn save_resume_key(collection_key: &str, item_key: &str) -> anyhow::Result<()> {
    let path = resume_state_path(collection_key);
    let mut keys = load_resume_keys(collection_key);
    keys.insert(item_key.to_string());
    let mut sorted: Vec<&String> = keys.iter().collect();
    sorted.sort();
    let payload = serde_json::json!({
        "collection": collection_key,
        "completed_keys": sorted,
    });
    std::fs::write(&path, serde_json::to_string_pretty(&payload)?)?;
    Ok(())
}

/// `clear_resume_state()`: best-effort delete, swallows any error (matches Python's
/// `unlink(missing_ok=True)` inside a bare `except Exception: pass`).
pub fn clear_resume_state(collection_key: &str) {
    let path = resume_state_path(collection_key);
    let _ = std::fs::remove_file(&path);
}

pub fn resume_state_file_path(collection_key: &str) -> PathBuf {
    resume_state_path(collection_key)
}

fn missing_entry_key(entry: &Value) -> Option<String> {
    entry.get("key").and_then(Value::as_str).map(str::to_string)
}

/// `fetch_pdfs_for_collection()`. Collection traversal itself is 100% Bridge-driven
/// (`list_items_missing_pdf`, `Zotero.Collections.getByLibraryAndKey` + `getChildItems(true)`
/// server-side) -- no SQLite/catalog collection resolution happens in this function, matching
/// Python exactly. `getChildItems(true)` recurses into child collections (Zotero's own
/// documented behavior); this function does not re-filter that set.
#[allow(clippy::too_many_arguments)]
pub fn fetch_pdfs_for_collection<M: PdfMetadataClient, D: PdfDownloadClient>(
    runtime: &RuntimeContext,
    bridge: &JSBridgeClient,
    metadata_client: &M,
    download_client: &D,
    collection_key: &str,
    sources: &[String],
    library_id: i64,
    limit: Option<usize>,
    zotero_timeout: u64,
    download_timeout: u64,
    mut progress_callback: Option<&mut dyn FnMut(&Value)>,
    resume: bool,
    reset_resume: bool,
) -> Value {
    if reset_resume {
        clear_resume_state(collection_key);
    }

    let library_id_u32 = library_id.max(0) as u32;
    let listed = bridge.list_items_missing_pdf(library_id_u32, collection_key);
    if !listed.is_ok() {
        return pdf_fetch::result_payload(
            "collection_fetch_pdfs",
            false,
            "error",
            Some("LIST_FAILED"),
            Some(
                listed
                    .error_message()
                    .unwrap_or("failed to list items missing PDFs"),
            ),
            vec![("collection", Value::String(collection_key.to_string()))],
        );
    }
    let payload = listed.data.clone().unwrap_or(Value::Object(Map::new()));
    if payload.get("ok") == Some(&Value::Bool(false)) {
        let error = payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("collection list failed")
            .to_string();
        return pdf_fetch::result_payload(
            "collection_fetch_pdfs",
            false,
            "error",
            Some("LIST_FAILED"),
            Some(&error),
            vec![("collection", Value::String(collection_key.to_string()))],
        );
    }

    let mut missing: Vec<Value> = payload
        .get("missing")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut skipped_resume = 0usize;
    if resume {
        let done = load_resume_keys(collection_key);
        let before = missing.len();
        missing.retain(|entry| {
            missing_entry_key(entry)
                .map(|key| !done.contains(&key))
                .unwrap_or(true)
        });
        skipped_resume = before - missing.len();
    }
    if let Some(limit) = limit {
        missing.truncate(limit);
    }

    let mut details: Vec<Value> = Vec::new();
    let mut found = 0usize;
    for (position, entry) in missing.iter().enumerate() {
        let index = position + 1;
        let key = missing_entry_key(entry);
        let one = match &key {
            Some(key) => pdf_fetch::fetch_pdf_for_item(
                runtime,
                bridge,
                metadata_client,
                download_client,
                key,
                sources,
                library_id,
                zotero_timeout,
                download_timeout,
                false,
            ),
            None => pdf_fetch::result_payload(
                "item_fetch_pdf",
                false,
                "error",
                Some("ITEM_NOT_FOUND"),
                Some("missing item key"),
                vec![],
            ),
        };
        let ok = one.get("ok") == Some(&Value::Bool(true));
        let status = one.get("status").and_then(Value::as_str).unwrap_or("");
        let code = one.get("code").and_then(Value::as_str).unwrap_or("");
        if ok
            && matches!(status, "success" | "already_has_pdf")
            && matches!(code, "FOUND" | "ATTACHED" | "ALREADY_HAS_PDF")
        {
            found += 1;
            if resume {
                if let Some(key) = &key {
                    let _ = save_resume_key(collection_key, key);
                }
            }
        }
        let row = serde_json::json!({
            "index": index,
            "total": missing.len(),
            "key": key,
            "title": entry.get("title"),
            "DOI": entry.get("DOI"),
            "ok": ok,
            "status": one.get("status"),
            "code": one.get("code"),
            "source": one.get("source"),
            "error": one.get("error"),
        });
        if let Some(cb) = progress_callback.as_deref_mut() {
            cb(&row);
        }
        details.push(row);
    }

    let (status, ok) = if missing.is_empty() {
        ("success", true)
    } else if found == 0 {
        ("not_found", false)
    } else if found < missing.len() {
        ("partial_success", true)
    } else {
        if resume {
            // Full success for this run's (already resume/limit-filtered) batch -- clear resume
            // state. Matches Python's exact quirk: this is scoped to *this run's* `missing.len()`,
            // not the collection's true total, so a `--limit`-truncated full success still clears
            // state even though further missing items beyond the limit were never touched.
            clear_resume_state(collection_key);
        }
        ("success", true)
    };

    pdf_fetch::result_payload(
        "collection_fetch_pdfs",
        ok,
        status,
        Some("DONE"),
        None,
        vec![
            ("collection", Value::String(collection_key.to_string())),
            ("checked", Value::from(missing.len())),
            ("found", Value::from(found)),
            ("skipped_resume", Value::from(skipped_resume)),
            ("resume", Value::Bool(resume)),
            (
                "resume_state",
                if resume {
                    Value::String(
                        resume_state_path(collection_key)
                            .to_string_lossy()
                            .into_owned(),
                    )
                } else {
                    Value::Null
                },
            ),
            (
                "missing_total",
                payload.get("missing_count").cloned().unwrap_or(Value::Null),
            ),
            ("details", Value::Array(details)),
            (
                "sources",
                Value::Array(sources.iter().cloned().map(Value::String).collect()),
            ),
        ],
    )
}

/// `find_pdfs_in_collection()`: per-item, Zotero-only (no OA cascade, no resume state at all --
/// resume is exclusive to `fetch_pdfs_for_collection`). Returns the same wrapped transport
/// envelope shape (`{ok, data, error}`) Python's `JSBridgeClient.find_pdfs_in_collection` returns
/// -- unwrapping to the bare summary is the future CLI-dispatch slice's job (`emit_js`), not this
/// function's.
pub fn find_pdfs_in_collection(
    bridge: &JSBridgeClient,
    collection_key: &str,
    library_id: i64,
    timeout_per_item: u64,
    limit: Option<usize>,
) -> Value {
    let library_id_u32 = library_id.max(0) as u32;
    let listed = bridge.list_items_missing_pdf(library_id_u32, collection_key);
    if !listed.is_ok() {
        return serde_json::json!({
            "ok": false,
            "data": null,
            "error": listed.error_message(),
        });
    }
    let payload = listed.data.clone().unwrap_or(Value::Null);
    if !payload.is_object() || payload.get("ok") == Some(&Value::Bool(false)) {
        let error = payload
            .get("error")
            .and_then(Value::as_str)
            .or_else(|| listed.error_message())
            .unwrap_or("failed to list items missing PDFs");
        return serde_json::json!({"ok": false, "data": null, "error": error});
    }

    let mut missing: Vec<Value> = payload
        .get("missing")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some(limit) = limit {
        missing.truncate(limit);
    }

    let mut details: Vec<Value> = Vec::new();
    let mut found = 0usize;
    for entry in &missing {
        let key = missing_entry_key(entry);
        let title = entry
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .take(60)
            .collect::<String>();
        let Some(key) = key else {
            details.push(serde_json::json!({
                "key": null, "title": title, "DOI": entry.get("DOI").cloned().unwrap_or(Value::String(String::new())),
                "status": "ERROR", "attachment_key": null, "message": "missing item key",
            }));
            continue;
        };

        let transport = bridge.find_pdf(library_id_u32, &key, timeout_per_item);
        let mut status = "ERROR".to_string();
        let mut message: Option<String> = None;
        let mut attachment_key: Option<String> = None;
        if transport.is_ok() {
            let text = transport
                .data
                .as_ref()
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_default();
            if text.starts_with("FOUND:") {
                status = "FOUND".to_string();
                found += 1;
                attachment_key = Some(text.trim_start_matches("FOUND:").trim().to_string());
            } else if text.starts_with("NOT_FOUND") {
                status = "NOT_FOUND".to_string();
                message = Some(text);
            } else if text.starts_with("TIMEOUT") {
                status = "TIMEOUT".to_string();
                message = Some(text);
            } else {
                status = "UNKNOWN".to_string();
                message = Some(text);
            }
        } else {
            let err = transport
                .error_message()
                .unwrap_or("find_pdf failed")
                .to_string();
            if err.to_lowercase().contains("timed out") {
                status = "TIMEOUT".to_string();
            }
            message = Some(err);
        }

        details.push(serde_json::json!({
            "key": key,
            "title": title,
            "DOI": entry.get("DOI").and_then(Value::as_str).unwrap_or(""),
            "status": status,
            "attachment_key": attachment_key,
            "message": message,
        }));
    }

    let summary = serde_json::json!({
        "ok": true,
        "collection": collection_key,
        "total_in_collection": payload.get("total"),
        "checked": missing.len(),
        "found": found,
        "not_found": details.iter().filter(|d| d["status"] == "NOT_FOUND").count(),
        "timeouts": details.iter().filter(|d| d["status"] == "TIMEOUT").count(),
        "errors": details.iter().filter(|d| d["status"] == "ERROR").count(),
        "details": details,
        "strategy": "per-item",
        "timeout_per_item": timeout_per_item,
    });
    serde_json::json!({"ok": true, "data": summary, "error": null})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "zotero-cli-pdf-cascade-test-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn resume_state_path_collapses_runs_of_unsafe_characters() {
        let safe = UNSAFE_KEY_RE.replace_all("My Collection!!", "_");
        assert_eq!(safe, "My_Collection_");
    }

    #[test]
    fn load_resume_keys_is_fail_open_on_missing_or_malformed_file() {
        // Missing file.
        assert!(load_resume_keys("NEVER-CREATED-COLLECTION-KEY-XYZ").is_empty());
    }

    #[test]
    fn save_load_and_clear_resume_state_round_trip() {
        let _guard = RESUME_HOME_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let home = temp_home("roundtrip");
        std::fs::create_dir_all(&home).unwrap();
        // SAFETY: serialized against every other HOME-mutating test in this binary by the guard
        // above, held for this whole function.
        let original_home = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", &home);
        }

        let collection = "TESTCOL1";
        assert!(load_resume_keys(collection).is_empty());
        save_resume_key(collection, "ITEM0001").unwrap();
        save_resume_key(collection, "ITEM0002").unwrap();
        let keys = load_resume_keys(collection);
        assert_eq!(keys.len(), 2);
        assert!(keys.contains("ITEM0001") && keys.contains("ITEM0002"));

        // Exact on-disk JSON shape.
        let path = resume_state_path(collection);
        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["collection"], "TESTCOL1");
        assert_eq!(
            parsed["completed_keys"],
            serde_json::json!(["ITEM0001", "ITEM0002"])
        );

        clear_resume_state(collection);
        assert!(load_resume_keys(collection).is_empty());
        assert!(!path.exists());

        unsafe {
            match original_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
        std::fs::remove_dir_all(&home).ok();
    }
}
