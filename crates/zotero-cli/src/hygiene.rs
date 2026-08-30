//! Library hygiene: duplicate detection by DOI, title, or native Zotero bridge.
//!
//! Ported from `cli_anything/zotero/core/hygiene.py` and `zotero_cli.py:1458-1499`
//! pinned at `PiaoyangGuohai1/cli-anything-zotero@e42a930e`.

use serde::Serialize;
use serde_json::Value;
use std::path::Path;

use crate::bridge;
use crate::cli::DuplicatesBy;
use crate::db;

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
