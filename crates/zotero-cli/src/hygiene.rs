//! Library hygiene: duplicate detection by DOI, title, or native Zotero bridge.
//!
//! Ported from `cli_anything/zotero/core/hygiene.py` and `zotero_cli.py:1458-1499`
//! pinned at `PiaoyangGuohai1/cli-anything-zotero@e42a930e`.

use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

use crate::bridge;
use crate::cli::DuplicatesBy;
use crate::db;
use crate::runtime::RuntimeContext;
use crate::session::{self, SessionState};

/// Single duplicate member within a duplicate group.
#[derive(Debug, Clone, Serialize)]
pub struct DuplicateItemMember {
    pub key: String,
    pub title: String,
    #[serde(rename = "DOI")]
    pub doi: String,
    pub date: String,
    #[serde(rename = "hasPdf")]
    pub has_pdf: bool,
}

/// A group of duplicate items matching on normalized DOI or title.
#[derive(Debug, Clone, Serialize)]
pub struct DuplicateGroup {
    pub r#match: String,
    pub count: usize,
    pub keep_suggestion: String,
    pub items: Vec<DuplicateItemMember>,
}

/// Standard duplicate result envelope matching Python `hygiene.py:98-106`.
#[derive(Debug, Clone, Serialize)]
pub struct DuplicatePayload {
    pub action: String,
    pub ok: bool,
    pub status: String,
    pub code: String,
    pub by: String,
    pub group_count: usize,
    pub groups: Vec<DuplicateGroup>,
}

static WS_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"\s+").unwrap());
static NON_WORD_WS_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"[^\w\s]").unwrap());

/// Normalizes a DOI string (`hygiene.py:13-17`):
/// - lowercase
/// - strip URL prefix (`https?://(dx\.)?doi\.org/`)
/// - strip leading `doi:`
/// - strip trailing whitespace, `.`, `)`, `,`, `;`
pub fn norm_doi(value: &str) -> String {
    let mut text = value.trim().to_lowercase();
    if let Some(rest) = text.strip_prefix("https://doi.org/") {
        text = rest.to_string();
    } else if let Some(rest) = text.strip_prefix("http://doi.org/") {
        text = rest.to_string();
    } else if let Some(rest) = text.strip_prefix("https://dx.doi.org/") {
        text = rest.to_string();
    } else if let Some(rest) = text.strip_prefix("http://dx.doi.org/") {
        text = rest.to_string();
    }

    if let Some(rest) = text.strip_prefix("doi:") {
        text = rest.trim_start().to_string();
    }

    text.trim_end_matches([' ', '.', ')', ',', ';']).to_string()
}

/// Normalizes a title string (`hygiene.py:20-24`):
/// - lowercase
/// - collapse consecutive whitespace to a single space
/// - remove characters matching `[^\w\s]`
pub fn norm_title(value: &str) -> String {
    let text = value.trim().to_lowercase();
    let text = WS_RE.replace_all(&text, " ");
    let text = NON_WORD_WS_RE.replace_all(&text, "");
    text.to_string()
}

/// Find duplicates by DOI or title from SQLite (`hygiene.py:27-106`).
///
/// Preserves Python oracle's exact pre-sort limit break:
/// 1. Iterates groups in first-seen SQLite fetch order.
/// 2. Appends qualifying groups (`count >= 2`).
/// 3. Breaks immediately once `groups.len() >= limit`.
/// 4. Only then stable-sorts the truncated set by `count` descending.
///
/// Member sort order within each group: `(0 if hasPdf else 1, date ascending)`.
pub fn find_duplicates(
    sqlite_path: &Path,
    by: DuplicatesBy,
    library_id: i64,
    limit: usize,
) -> anyhow::Result<DuplicatePayload> {
    let items = db::fetch_items(
        sqlite_path,
        db::FetchItemsFilter {
            library_id: Some(library_id),
            collection_id: None,
            parent_item_id: None,
            tag: None,
            limit: None,
        },
    )?;

    let mut group_keys: Vec<String> = Vec::new();
    let mut groups_map: std::collections::HashMap<String, Vec<DuplicateItemMember>> =
        std::collections::HashMap::new();

    for item in items {
        if item.is_attachment || item.is_note || item.is_annotation {
            continue;
        }
        let match_key = match by {
            DuplicatesBy::Doi => {
                let k = norm_doi(&item.doi);
                if k.is_empty() {
                    continue;
                }
                k
            }
            DuplicatesBy::Title => {
                let k = norm_title(&item.title);
                if k.is_empty() || k.chars().count() < 8 {
                    continue;
                }
                k
            }
            DuplicatesBy::Zotero => unreachable!("zotero duplicate mode uses bridge"),
        };

        let date = item.date.clone().unwrap_or_default();

        let member = DuplicateItemMember {
            key: item.key.clone(),
            title: item.title.clone(),
            doi: item.doi.clone(),
            date,
            has_pdf: item.has_pdf,
        };

        if let Some(members) = groups_map.get_mut(&match_key) {
            members.push(member);
        } else {
            group_keys.push(match_key.clone());
            groups_map.insert(match_key, vec![member]);
        }
    }

    let mut dup_groups: Vec<DuplicateGroup> = Vec::new();
    for key in group_keys {
        if let Some(mut members) = groups_map.remove(&key) {
            if members.len() < 2 {
                continue;
            }
            // Member sort: (0 if hasPdf else 1, date ascending)
            members.sort_by(|a, b| {
                let a_pdf_key = (!a.has_pdf) as u8;
                let b_pdf_key = (!b.has_pdf) as u8;
                match a_pdf_key.cmp(&b_pdf_key) {
                    std::cmp::Ordering::Equal => a.date.cmp(&b.date),
                    other => other,
                }
            });

            let keep_suggestion = members[0].key.clone();
            dup_groups.push(DuplicateGroup {
                r#match: key,
                count: members.len(),
                keep_suggestion,
                items: members,
            });

            // Critical oracle quirk: break when truncated count reaches limit BEFORE sorting by group count
            if dup_groups.len() >= limit {
                break;
            }
        }
    }

    // Stable sort the truncated set by count descending
    dup_groups.sort_by(|a, b| b.count.cmp(&a.count));

    let by_str = match by {
        DuplicatesBy::Doi => "doi",
        DuplicatesBy::Title => "title",
        DuplicatesBy::Zotero => "zotero",
    };

    Ok(DuplicatePayload {
        action: "item_duplicates".to_string(),
        ok: true,
        status: "success".to_string(),
        code: "OK".to_string(),
        by: by_str.to_string(),
        group_count: dup_groups.len(),
        groups: dup_groups,
    })
}

/// Execute Zotero native duplicate detection via the JS Bridge (`zotero_cli.py:1474-1490`).
///
/// Uses hardcoded `library_id = 1` and returns Zotero native success schema or converts
/// caught errors to `ZOTERO_DUP_FAILED`.
pub fn find_duplicates_zotero(bridge: &bridge::JSBridgeClient, limit: usize) -> (Value, i32) {
    let code = match bridge::templates::render_find_duplicates(1, limit) {
        Ok(c) => c,
        Err(err) => {
            return (
                serde_json::json!({
                    "action": "item_duplicates",
                    "ok": false,
                    "status": "error",
                    "code": "ZOTERO_DUP_FAILED",
                    "by": "zotero",
                    "error": err.to_string(),
                }),
                1,
            );
        }
    };

    let resp = bridge.execute_js(&code, 15);
    if resp.ok {
        if let Some(data) = &resp.data {
            if let Some(err_val) = data.get("error") {
                let count = data.get("count").and_then(|c| c.as_i64()).unwrap_or(0);
                if count == 0 {
                    let err_str = err_val.as_str().unwrap_or("Zotero duplicate search failed");
                    return (
                        serde_json::json!({
                            "action": "item_duplicates",
                            "ok": false,
                            "status": "error",
                            "code": "ZOTERO_DUP_FAILED",
                            "by": "zotero",
                            "error": err_str,
                        }),
                        1,
                    );
                }
            }
            (data.clone(), 0)
        } else {
            (serde_json::to_value(&resp).unwrap_or(Value::Null), 0)
        }
    } else {
        (serde_json::to_value(&resp).unwrap_or(Value::Null), 1)
    }
}

/// Zero-mutation dry-run preview for `item merge` (default; `--dry-run`), mirroring
/// `hygiene.py`'s `preview_merge()` (`hygiene.py:297-398`) composed with `merge_items()`'s
/// dry-run envelope wrapping (`action`, `plan`, `dry_run`), which Python applies unconditionally
/// on both the success and error branches (`hygiene.py:421-426`).
///
/// Bridge-first, SQLite-fallback, exactly like upstream: attempts the same read-only preview JS
/// (`T_ITEM_MERGE_PREVIEW`, a verbatim port of `hygiene.py:_preview_js` -- resolves/summarizes
/// items only, never `saveTx`/`eraseTx`/`merge`/`trash`) via the JS Bridge first; on success,
/// `preview_source: "bridge"`. If the bridge is unreachable, its ownership handshake fails, or
/// the eval itself reports `ok:false` (including "keep item not found" -- Python does not treat
/// that as terminal; it still falls through to the SQLite attempt), falls back to the existing
/// SQLite-only preview, `preview_source: "sqlite"`, carrying the captured `bridge_error` forward
/// exactly as Python's `preview_merge` does.
///
/// The confirmed mutation path (`item merge --confirm`) is untouched by this function and keeps
/// using the existing, accepted `Zotero.Items.merge()` JS Bridge call in `item_merge_command`.
pub fn merge_preview(
    runtime: &RuntimeContext,
    session: &SessionState,
    keep_key: &str,
    merge_keys: &[String],
) -> anyhow::Result<Value> {
    let library_id = session::session_library_id(session, 1)?;
    let plan = serde_json::json!({"keep": keep_key, "merge": merge_keys, "dry_run": true});

    let bridge_error = match render_and_execute_bridge_preview(keep_key, merge_keys, library_id) {
        BridgePreviewOutcome::Success(data) => {
            return Ok(build_bridge_preview_payload(keep_key, data, &plan));
        }
        BridgePreviewOutcome::Failed(err) => err,
    };

    merge_preview_sqlite(
        runtime,
        keep_key,
        merge_keys,
        library_id,
        bridge_error,
        plan,
    )
}

enum BridgePreviewOutcome {
    Success(Value),
    /// Mirrors Python's `bridge_error` local: `None` when the bridge reported no error text at
    /// all (e.g. a malformed-but-still-a-dict response with no `error` key), matching
    /// `hygiene.py:356`'s `(data or {}).get("error")` exactly (no "preview failed" fallback
    /// string in that specific sub-case).
    Failed(Option<String>),
}

/// `preview_merge()`'s try/bridge block (`hygiene.py:319-360`). `bridge.execute_js` in this port
/// never throws (`BridgeResponse` mirrors Python's `transport` dict), so there is no `except`
/// to port -- transport-level failure is just `resp.ok == false`, same as Python's `else` arm.
fn render_and_execute_bridge_preview(
    keep_key: &str,
    merge_keys: &[String],
    library_id: i64,
) -> BridgePreviewOutcome {
    let Ok(code) = bridge::templates::render_item_merge_preview(library_id, keep_key, merge_keys)
    else {
        return BridgePreviewOutcome::Failed(Some("preview failed".to_string()));
    };
    let client = bridge::JSBridgeClient::with_default_port();
    let resp = client.execute_js(&code, 20);
    if !resp.ok {
        return BridgePreviewOutcome::Failed(Some(
            resp.error
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "bridge preview failed".to_string()),
        ));
    }
    match resp.data {
        Some(data) if data.is_object() => {
            if data.get("ok").and_then(Value::as_bool) == Some(true) {
                BridgePreviewOutcome::Success(data)
            } else {
                BridgePreviewOutcome::Failed(
                    data.get("error")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                )
            }
        }
        _ => BridgePreviewOutcome::Failed(Some("preview failed".to_string())),
    }
}

/// `preview_merge()`'s bridge-success branch (`hygiene.py:328-355`), already wrapped in
/// `merge_items()`'s dry-run envelope (`action: "item_merge"`, `plan`, `dry_run: true`).
fn build_bridge_preview_payload(keep_key: &str, data: Value, plan: &Value) -> Value {
    let will = data.get("will").cloned().unwrap_or(Value::Null);
    let (summary, message) = summary_and_message_from_will(&will, keep_key, "bridge");
    serde_json::json!({
        "action": "item_merge",
        "ok": true,
        "status": "dry_run",
        "code": "DRY_RUN",
        "keep": data.get("keep").cloned().unwrap_or(Value::Null),
        "others": data.get("others").cloned().unwrap_or(Value::Array(Vec::new())),
        "missing": data.get("missing").cloned().unwrap_or(Value::Array(Vec::new())),
        "will": will,
        "preview_source": "bridge",
        "summary": summary,
        "message": message,
        "plan": plan,
        "dry_run": true,
    })
}

/// `preview_merge()`'s SQLite-offline-fallback branch (`hygiene.py:362-388`), reached either
/// because the bridge attempt failed/was unreachable, or (matching Python exactly) because the
/// bridge itself reported `keep item not found` -- that is not treated as terminal upstream;
/// SQLite gets its own independent attempt to resolve `keep_key` before this ever becomes a
/// `KEEP_NOT_FOUND` error.
fn merge_preview_sqlite(
    runtime: &RuntimeContext,
    keep_key: &str,
    merge_keys: &[String],
    library_id: i64,
    bridge_error: Option<String>,
    plan: Value,
) -> anyhow::Result<Value> {
    let sqlite_path = &runtime.environment.sqlite_path;

    let Some(keep_item) = db::resolve_item(sqlite_path, keep_key, Some(library_id))? else {
        return Ok(serde_json::json!({
            "action": "item_merge",
            "ok": false,
            "status": "error",
            "code": "KEEP_NOT_FOUND",
            "error": format!("keep item not found: {keep_key}"),
            // Always present, even when null -- matches `bridge_error=bridge_error` always being
            // passed as a `result_payload` kwarg on this branch (`hygiene.py:372`), unlike the
            // success branch below where the key is only added when truthy.
            "bridge_error": bridge_error,
            "preview_source": "sqlite",
            "plan": plan,
            "dry_run": true,
        }));
    };
    let keep_sum = summarize_item_for_merge_preview(sqlite_path, &keep_item)?;

    let mut others: Vec<Value> = Vec::new();
    let mut missing: Vec<Value> = Vec::new();
    for key in merge_keys {
        match db::resolve_item(sqlite_path, key, Some(library_id))? {
            Some(item) => others.push(summarize_item_for_merge_preview(sqlite_path, &item)?),
            None => missing.push(Value::String(key.clone())),
        }
    }

    // `_preview_from_summaries` (`hygiene.py:237-294`): incremental de-dup against `keep`'s
    // existing tags/collections *and* against earlier `others` entries already folded in -- an
    // order-sensitive accumulation, not a set union, so duplicate merge keys or overlapping
    // tags/collections across `others` are only added to the plan once, in first-seen order.
    let mut keep_tag_set: HashSet<String> = keep_sum["tags"]
        .as_array()
        .expect("keep_sum.tags is always an array")
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    // Python falls back to `str(id)` when a collection's `key` is falsy; unreachable here since
    // every Zotero collection row always has a non-null `key`.
    let mut keep_col_set: HashSet<String> = keep_sum["collections"]
        .as_array()
        .expect("keep_sum.collections is always an array")
        .iter()
        .map(|c| c["key"].as_str().unwrap_or_default().to_string())
        .collect();

    let mut tags_to_add: Vec<Value> = Vec::new();
    let mut cols_to_add: Vec<Value> = Vec::new();
    let mut attachments_to_move: i64 = 0;
    let mut notes_to_move: i64 = 0;
    for other in &others {
        attachments_to_move += other["nAttachments"].as_i64().unwrap_or(0);
        notes_to_move += other["nNotes"].as_i64().unwrap_or(0);
        for tag in other["tags"]
            .as_array()
            .expect("others[].tags is always an array")
        {
            let t = tag.as_str().unwrap_or_default().to_string();
            if !keep_tag_set.contains(&t) {
                tags_to_add.push(Value::String(t.clone()));
                keep_tag_set.insert(t);
            }
        }
        for col in other["collections"]
            .as_array()
            .expect("others[].collections is always an array")
        {
            let ck = col["key"].as_str().unwrap_or_default().to_string();
            if !keep_col_set.contains(&ck) {
                cols_to_add.push(col.clone());
                keep_col_set.insert(ck);
            }
        }
    }
    let trash_items: Vec<Value> = others.iter().map(|o| o["key"].clone()).collect();

    let will = serde_json::json!({
        "move_attachments": attachments_to_move,
        "move_notes": notes_to_move,
        "add_tags": tags_to_add,
        "add_collections": cols_to_add,
        "trash_items": trash_items,
    });
    let (summary, message) = summary_and_message_from_will(&will, keep_key, "sqlite");

    let mut payload = serde_json::json!({
        "action": "item_merge",
        "ok": true,
        "status": "dry_run",
        "code": "DRY_RUN",
        "keep": keep_sum,
        "others": others,
        "missing": missing,
        "will": will,
        "preview_source": "sqlite",
        "summary": summary,
        "message": message,
        "plan": plan,
        "dry_run": true,
    });
    // `if bridge_error: payload["bridge_error"] = bridge_error` (`hygiene.py:386-387`) -- only
    // attached when truthy (non-`None`, non-empty), unlike the `KEEP_NOT_FOUND` branch above.
    if let Some(err) = bridge_error.filter(|s| !s.is_empty()) {
        payload["bridge_error"] = Value::String(err);
    }
    Ok(payload)
}

/// `summary`/`message` construction shared by the bridge-success and SQLite-success branches
/// (`hygiene.py:338-353` and `hygiene.py:281-293` respectively -- same shape, same formula).
fn summary_and_message_from_will(will: &Value, keep_key: &str, source: &str) -> (Value, String) {
    let trash_items = will
        .get("trash_items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let move_attachments = will
        .get("move_attachments")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let move_notes = will.get("move_notes").and_then(Value::as_i64).unwrap_or(0);
    let add_tags = will
        .get("add_tags")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let add_collections = will
        .get("add_collections")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let add_collections_summary: Vec<Value> = add_collections
        .iter()
        .map(|c| {
            let name = c
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            let key = c.get("key").and_then(Value::as_str).unwrap_or_default();
            Value::String(name.unwrap_or(key).to_string())
        })
        .collect();

    let summary = serde_json::json!({
        "trash_count": trash_items.len(),
        "move_attachments": move_attachments,
        "move_notes": move_notes,
        "add_tags": add_tags,
        "add_collections": add_collections_summary,
    });

    let message = format!(
        "Would trash {} item(s) into keep={keep_key}: move {} attachment(s), {} note(s); \
         add {} tag(s), {} collection(s). (preview via {source}) Re-run with --confirm to apply.",
        trash_items.len(),
        move_attachments,
        move_notes,
        add_tags.len(),
        add_collections.len(),
    );
    (summary, message)
}

/// Per-item summary shape shared by `keep` and each `others[]` entry
/// (`hygiene.py:_sqlite_summarize_item`, `hygiene.py:170-234`).
fn summarize_item_for_merge_preview(sqlite_path: &Path, item: &db::Item) -> anyhow::Result<Value> {
    let tags: Vec<Value> = item
        .tags
        .iter()
        .filter(|t| !t.name.is_empty())
        .map(|t| Value::String(t.name.clone()))
        .collect();

    let item_id_str = item.item_id.to_string();
    let attachments: Vec<Value> = db::fetch_item_attachments(sqlite_path, &item_id_str)?
        .iter()
        .map(|a| {
            serde_json::json!({
                "key": a.key,
                "title": a.title,
                "contentType": a.content_type.clone().unwrap_or_default(),
                // Matches Python's exact field, which reuses the raw attachment path as
                // "filename" rather than a basename (`hygiene.py:187`) -- not "fixed" here.
                "filename": a.attachment_path.clone().unwrap_or_default(),
            })
        })
        .collect();

    let notes: Vec<Value> = db::fetch_item_notes(sqlite_path, &item_id_str)?
        .iter()
        .map(|n| {
            let title_source = if !n.title.is_empty() {
                n.title.as_str()
            } else {
                n.note_preview.as_str()
            };
            let truncated: String = title_source.chars().take(80).collect();
            serde_json::json!({"key": n.key, "title": truncated})
        })
        .collect();

    let collections: Vec<Value> = db::fetch_item_collections(sqlite_path, item.item_id)?
        .iter()
        .map(|c| serde_json::json!({"id": c.id, "key": c.key, "name": c.name}))
        .collect();

    Ok(serde_json::json!({
        "key": item.key,
        "title": item.title,
        "DOI": item.doi,
        "date": item.date.clone().unwrap_or_default(),
        "itemType": item.type_name,
        "tags": tags,
        "collections": collections,
        "attachments": attachments,
        "notes": notes,
        "nAttachments": attachments.len(),
        "nNotes": notes.len(),
        "nTags": tags.len(),
        "nCollections": collections.len(),
    }))
}
