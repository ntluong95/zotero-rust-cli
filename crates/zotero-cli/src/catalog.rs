//! Port of `core/catalog.py`'s domain layer needed by the vertical slice:
//! library/collection resolution and the item list/get/find operations.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use crate::db::{self, Collection, CollectionNode, FetchItemsFilter, Item, Library, SavedSearch};
use crate::error::DomainError;
use crate::http;
use crate::runtime::RuntimeContext;
use crate::session::SessionState;

pub const SEARCH_SCOPES: [&str; 3] = ["titleCreatorYear", "fields", "everything"];

/// Truncates `items` to match Python's `results[:limit]` (`catalog.py:134`)
/// exactly, including negative-`limit` slice semantics: a negative limit
/// drops the last `|limit|` elements rather than clamping to empty like a
/// naive `.max(0)` truncate would.
fn python_slice_to_limit<T>(items: &mut Vec<T>, limit: i64) {
    let len = items.len() as i64;
    let end = if limit < 0 {
        (len + limit).max(0)
    } else {
        limit.min(len)
    };
    items.truncate(end as usize);
}

/// `resolve_library_id()` (`catalog.py:20-27`).
pub fn resolve_library_id(
    runtime: &RuntimeContext,
    library_ref: Option<&str>,
) -> anyhow::Result<Option<i64>> {
    let Some(library_ref) = library_ref else {
        return Ok(None);
    };
    let library = db::resolve_library(&runtime.environment.sqlite_path, library_ref)?;
    match library {
        Some(lib) => Ok(Some(lib.library_id)),
        None => Err(DomainError::new(format!("Library not found: {library_ref}")).into()),
    }
}

/// Session's `current_library` as an optional string ref, matching how
/// Python passes `session.get("current_library")` (int|str|None) straight
/// into `resolve_library_id`.
fn session_library_ref(session: &SessionState) -> Option<String> {
    match &session.current_library {
        None => None,
        Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) if s.is_empty() => None,
        Some(v) => Some(match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            other => other.to_string(),
        }),
    }
}

/// `_default_library()` (`catalog.py:30-38`).
pub fn default_library(runtime: &RuntimeContext, session: &SessionState) -> anyhow::Result<i64> {
    let session_ref = session_library_ref(session);
    if let Some(current) = resolve_library_id(runtime, session_ref.as_deref())? {
        return Ok(current);
    }
    match db::default_library_id(&runtime.environment.sqlite_path)? {
        Some(id) => Ok(id),
        None => Err(DomainError::new("No Zotero libraries found in the local database").into()),
    }
}

/// `local_api_scope()` (`catalog.py:41-49`).
pub fn local_api_scope(runtime: &RuntimeContext, library_id: i64) -> anyhow::Result<String> {
    let library = db::resolve_library(&runtime.environment.sqlite_path, &library_id.to_string())?;
    let Some(library) = library else {
        return Err(DomainError::new(format!("Library not found: {library_id}")).into());
    };
    match library.kind.as_str() {
        "user" => Ok("/api/users/0".to_string()),
        "group" => Ok(format!("/api/groups/{}", library.library_id)),
        other => Err(DomainError::new(format!(
            "Unsupported library type for Zotero Local API: {other}"
        ))
        .into()),
    }
}

/// `list_collections()` (`catalog.py:56-57`).
pub fn list_collections(
    runtime: &RuntimeContext,
    session: &SessionState,
) -> anyhow::Result<Vec<Collection>> {
    let library_id = default_library(runtime, session)?;
    db::fetch_collections(&runtime.environment.sqlite_path, Some(library_id))
}

/// `get_collection()` (`catalog.py:68-80`): `ref: None` falls back to
/// `session.current_collection`, matching `get_item`'s established
/// pattern below (the two low-level `resolve_*` helpers this and
/// `get_item` wrap were already consistent; the original vertical slice
/// only narrowed this one to a required `&str` because its sole
/// internal caller, `find_items`, always supplies `Some`. Restored to
/// match Python's public contract now that `collection get`/`items`
/// need the same None-falls-back-to-session semantics `item get`
/// already has).
pub fn get_collection(
    runtime: &RuntimeContext,
    collection_ref: Option<&str>,
    session: &SessionState,
) -> anyhow::Result<Collection> {
    let resolved = collection_ref
        .map(str::to_string)
        .or_else(|| crate::session::session_collection_ref(session));
    let Some(resolved) = resolved else {
        return Err(
            DomainError::new("Collection reference required or set it in session first").into(),
        );
    };
    let library_id = resolve_library_id(runtime, session_library_ref(session).as_deref())?;
    let collection =
        db::resolve_collection(&runtime.environment.sqlite_path, &resolved, library_id)?;
    collection.ok_or_else(|| DomainError::new(format!("Collection not found: {resolved}")).into())
}

/// `list_items()` (`catalog.py:94-95`).
pub fn list_items(
    runtime: &RuntimeContext,
    session: &SessionState,
    limit: Option<i64>,
) -> anyhow::Result<Vec<Item>> {
    let library_id = default_library(runtime, session)?;
    db::fetch_items(
        &runtime.environment.sqlite_path,
        FetchItemsFilter {
            library_id: Some(library_id),
            limit,
            ..Default::default()
        },
    )
}

/// `get_item()` (`catalog.py:147-159`).
pub fn get_item(
    runtime: &RuntimeContext,
    item_ref: Option<&str>,
    session: &SessionState,
) -> anyhow::Result<Item> {
    let resolved = item_ref
        .map(str::to_string)
        .or_else(|| session.current_item.clone());
    let Some(resolved) = resolved else {
        return Err(DomainError::new("Item reference required or set it in session first").into());
    };
    let library_id = resolve_library_id(runtime, session_library_ref(session).as_deref())?;
    let item = db::resolve_item(&runtime.environment.sqlite_path, &resolved, library_id)?;
    item.ok_or_else(|| DomainError::new(format!("Item not found: {resolved}")).into())
}

#[allow(clippy::too_many_arguments)]
/// `find_items()` (`catalog.py:98-144`).
pub fn find_items(
    runtime: &RuntimeContext,
    query: &str,
    collection_ref: Option<&str>,
    limit: i64,
    exact_title: bool,
    search_scope: &str,
    session: &SessionState,
) -> anyhow::Result<Vec<Item>> {
    if !SEARCH_SCOPES.contains(&search_scope) {
        return Err(
            DomainError::new(format!("Unsupported item search scope: {search_scope}")).into(),
        );
    }

    let collection = match collection_ref {
        Some(cref) if !cref.is_empty() => Some(get_collection(runtime, Some(cref), session)?),
        _ => None,
    };
    let library_id = match &collection {
        Some(c) => c.library_id,
        None => default_library(runtime, session)?,
    };

    if !exact_title && runtime.local_api_available {
        let scope = local_api_scope(runtime, library_id)?;
        let path = match &collection {
            Some(c) => format!("{scope}/collections/{}/items/top", c.key),
            None => format!("{scope}/items/top"),
        };
        let params = [
            ("format", "json".to_string()),
            ("q", query.to_string()),
            ("qmode", search_scope.to_string()),
            ("limit", limit.to_string()),
        ];
        let payload = http::local_api_get_json(
            runtime.environment.port,
            &path,
            &params,
            Duration::from_secs(10),
        )?;
        let mut results = Vec::new();
        if let Some(array) = payload.as_array() {
            for record in array {
                let Some(key) = record.get("key").and_then(|k| k.as_str()) else {
                    continue;
                };
                if let Some(item) =
                    db::resolve_item(&runtime.environment.sqlite_path, key, Some(library_id))?
                {
                    results.push(item);
                }
            }
        }
        if !results.is_empty() {
            python_slice_to_limit(&mut results, limit);
            return Ok(results);
        }
    }

    let collection_id = collection.as_ref().map(|c| c.collection_id);
    db::find_items_by_title(
        &runtime.environment.sqlite_path,
        query,
        &db::SearchLibraries::One(library_id),
        collection_id,
        limit,
        exact_title,
    )
}

/// `find_collections()` (`catalog.py:60-61`).
pub fn find_collections(
    runtime: &RuntimeContext,
    query: &str,
    limit: i64,
    session: &SessionState,
) -> anyhow::Result<Vec<Collection>> {
    let library_id = default_library(runtime, session)?;
    db::find_collections(
        &runtime.environment.sqlite_path,
        query,
        Some(library_id),
        limit,
    )
}

/// `collection_tree()` (`catalog.py:64-65`).
pub fn collection_tree(
    runtime: &RuntimeContext,
    session: &SessionState,
) -> anyhow::Result<Vec<CollectionNode>> {
    let collections = list_collections(runtime, session)?;
    Ok(db::build_collection_tree(&collections))
}

/// `collection_items()` (`catalog.py:83-85`).
pub fn collection_items(
    runtime: &RuntimeContext,
    collection_ref: Option<&str>,
    session: &SessionState,
) -> anyhow::Result<Vec<Item>> {
    let collection = get_collection(runtime, collection_ref, session)?;
    db::fetch_items(
        &runtime.environment.sqlite_path,
        FetchItemsFilter {
            library_id: Some(collection.library_id),
            collection_id: Some(collection.collection_id),
            ..Default::default()
        },
    )
}

/// `use_selected_collection()` (`catalog.py:88-91`): read-only Connector query -- never mutates
/// Zotero library data. The caller (`collection use-selected` / `session use-selected`) persists
/// the returned value into CLI-owned session state.
pub fn use_selected_collection(runtime: &RuntimeContext) -> anyhow::Result<serde_json::Value> {
    if !runtime.connector_available {
        return Err(DomainError::new(format!(
            "Zotero connector is not available: {}",
            runtime.connector_message
        ))
        .into());
    }
    http::get_selected_collection(runtime.environment.port, Duration::from_secs(5))
}

/// `item_children()` (`catalog.py:162-164`).
pub fn item_children(
    runtime: &RuntimeContext,
    item_ref: Option<&str>,
    session: &SessionState,
) -> anyhow::Result<Vec<Item>> {
    let item = get_item(runtime, item_ref, session)?;
    db::fetch_item_children(&runtime.environment.sqlite_path, &item.item_id.to_string())
}

/// `item_notes()` (`catalog.py:167-169`).
pub fn item_notes(
    runtime: &RuntimeContext,
    item_ref: Option<&str>,
    session: &SessionState,
) -> anyhow::Result<Vec<Item>> {
    let item = get_item(runtime, item_ref, session)?;
    db::fetch_item_notes(&runtime.environment.sqlite_path, &item.item_id.to_string())
}

/// `item_attachments()` (`catalog.py:172-177`): each attachment gets a
/// `resolvedPath` field the SQLite layer doesn't populate on its own.
#[derive(Debug, Clone, Serialize)]
pub struct ItemWithResolvedPath {
    #[serde(flatten)]
    pub item: Item,
    #[serde(rename = "resolvedPath")]
    pub resolved_path: Option<String>,
}

pub fn item_attachments(
    runtime: &RuntimeContext,
    item_ref: Option<&str>,
    session: &SessionState,
) -> anyhow::Result<Vec<ItemWithResolvedPath>> {
    let item = get_item(runtime, item_ref, session)?;
    let attachments =
        db::fetch_item_attachments(&runtime.environment.sqlite_path, &item.item_id.to_string())?;
    Ok(attachments
        .into_iter()
        .map(|a| {
            let resolved_path = db::resolve_attachment_real_path(
                a.attachment_path.as_deref(),
                &a.key,
                &runtime.environment.data_dir,
            );
            ItemWithResolvedPath {
                item: a,
                resolved_path,
            }
        })
        .collect())
}

/// `item_file()` (`catalog.py:180-197`).
#[derive(Debug, Clone, Serialize)]
pub struct ItemFile {
    #[serde(rename = "itemID")]
    pub item_id: i64,
    pub key: String,
    pub title: String,
    #[serde(rename = "contentType")]
    pub content_type: Option<String>,
    pub path: Option<String>,
    #[serde(rename = "resolvedPath")]
    pub resolved_path: Option<String>,
    pub exists: bool,
}

pub fn item_file(
    runtime: &RuntimeContext,
    item_ref: Option<&str>,
    session: &SessionState,
) -> anyhow::Result<ItemFile> {
    let item = get_item(runtime, item_ref, session)?;
    // `target = item; if item["typeName"] != "attachment": target = attachments[0]`
    // (catalog.py:181-186) — a plain tuple of the 5 fields the payload
    // actually needs, from whichever record ends up being `target`.
    let (item_id, key, title, content_type, attachment_path): (
        i64,
        String,
        String,
        Option<String>,
        Option<String>,
    ) = if item.type_name != "attachment" {
        let attachments = db::fetch_item_attachments(
            &runtime.environment.sqlite_path,
            &item.item_id.to_string(),
        )?;
        let target = attachments.into_iter().next().ok_or_else(|| {
            DomainError::new(format!("No attachment file found for item: {}", item.key))
        })?;
        (
            target.item_id,
            target.key,
            target.title,
            target.content_type,
            target.attachment_path,
        )
    } else {
        (
            item.item_id,
            item.key.clone(),
            item.title.clone(),
            item.content_type.clone(),
            item.attachment_path.clone(),
        )
    };
    let resolved_path = db::resolve_attachment_real_path(
        attachment_path.as_deref(),
        &key,
        &runtime.environment.data_dir,
    );
    let exists = resolved_path
        .as_deref()
        .map(|p| Path::new(p).exists())
        .unwrap_or(false);
    Ok(ItemFile {
        item_id,
        key,
        title,
        content_type,
        path: attachment_path,
        resolved_path,
        exists,
    })
}

/// `list_libraries()` (`catalog.py:52-53`).
pub fn list_libraries(runtime: &RuntimeContext) -> anyhow::Result<Vec<Library>> {
    db::fetch_libraries(&runtime.environment.sqlite_path)
}

/// `list_searches()` (`catalog.py:200-201`).
pub fn list_searches(
    runtime: &RuntimeContext,
    session: &SessionState,
) -> anyhow::Result<Vec<SavedSearch>> {
    let library_id = default_library(runtime, session)?;
    db::fetch_saved_searches(&runtime.environment.sqlite_path, Some(library_id))
}

/// `get_search()` (`catalog.py:204-216`).
pub fn get_search(
    runtime: &RuntimeContext,
    search_ref: Option<&str>,
    session: &SessionState,
) -> anyhow::Result<SavedSearch> {
    let Some(search_ref) = search_ref else {
        return Err(DomainError::new("Search reference required").into());
    };
    let library_id = resolve_library_id(runtime, session_library_ref(session).as_deref())?;
    let search =
        db::resolve_saved_search(&runtime.environment.sqlite_path, search_ref, library_id)?;
    search.ok_or_else(|| DomainError::new(format!("Saved search not found: {search_ref}")).into())
}

/// `search_items()` (`catalog.py:218-227`): raw Local API JSON passthrough.
pub fn search_items(
    runtime: &RuntimeContext,
    search_ref: Option<&str>,
    session: &SessionState,
) -> anyhow::Result<serde_json::Value> {
    if !runtime.local_api_available {
        return Err(DomainError::new(
            "search items requires the Zotero Local API to be running and enabled",
        )
        .into());
    }
    let search = get_search(runtime, search_ref, session)?;
    let scope = local_api_scope(runtime, search.library_id)?;
    http::local_api_get_json(
        runtime.environment.port,
        &format!("{scope}/searches/{}/items", search.key),
        &[("format", "json".to_string())],
        Duration::from_secs(10),
    )
}

/// `list_tags()` (`catalog.py:230-231`).
pub fn list_tags(
    runtime: &RuntimeContext,
    session: &SessionState,
) -> anyhow::Result<Vec<db::TagSummary>> {
    let library_id = default_library(runtime, session)?;
    db::fetch_tags(&runtime.environment.sqlite_path, Some(library_id))
}

/// `tag_items()` (`catalog.py:234-235`).
pub fn tag_items(
    runtime: &RuntimeContext,
    tag_ref: &str,
    session: &SessionState,
) -> anyhow::Result<Vec<Item>> {
    let library_id = default_library(runtime, session)?;
    db::fetch_tag_items(&runtime.environment.sqlite_path, tag_ref, Some(library_id))
}

/// `list_styles()` (`catalog.py:238-259`): walks `*.csl` files under
/// `styles_dir` and extracts the CSL `id`/`title` elements via a real
/// namespace-aware XML parse (matching `ElementTree`'s `root.iter()`
/// document-order walk), not a text heuristic.
#[derive(Debug, Clone, Serialize)]
pub struct Style {
    pub path: String,
    pub id: Option<String>,
    pub title: String,
    pub valid: bool,
}

fn parse_csl_id_and_title(
    xml_bytes: &[u8],
) -> Result<(Option<String>, Option<String>), quick_xml::Error> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    #[derive(Clone, Copy)]
    enum Capture {
        Id,
        Title,
    }

    let mut reader = Reader::from_reader(xml_bytes);
    reader.config_mut().trim_text(true);
    reader.config_mut().check_end_names = true;
    let mut buf = Vec::new();
    let mut id: Option<String> = None;
    let mut title: Option<String> = None;
    let mut capturing: Option<Capture> = None;

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Eof => break,
            Event::Start(e) => {
                let local = e.local_name();
                capturing = match local.as_ref() {
                    "id" if id.is_none() => Some(Capture::Id),
                    "title" if title.is_none() => Some(Capture::Title),
                    _ => None,
                };
            }
            Event::Empty(_) => {
                // Matches Python's `element.text is None` for a
                // self-closing tag: no text to capture.
                capturing = None;
            }
            Event::Text(t) => {
                if let Some(kind) = capturing.take() {
                    let text = t.xml10_content().into_owned();
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        match kind {
                            Capture::Id if id.is_none() => id = Some(trimmed.to_string()),
                            Capture::Title if title.is_none() => title = Some(trimmed.to_string()),
                            _ => {}
                        }
                    }
                }
            }
            Event::End(_) => {
                capturing = None;
            }
            _ => {}
        }
        buf.clear();
    }
    Ok((id, title))
}

pub fn list_styles(runtime: &RuntimeContext) -> anyhow::Result<Vec<Style>> {
    let styles_dir = &runtime.environment.styles_dir;
    if !styles_dir.exists() {
        return Ok(Vec::new());
    }
    let mut csl_paths: Vec<std::path::PathBuf> = std::fs::read_dir(styles_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("csl"))
        .collect();
    csl_paths.sort();

    let mut styles = Vec::with_capacity(csl_paths.len());
    for path in csl_paths {
        let path_str = path.to_string_lossy().into_owned();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bytes = std::fs::read(&path)?;
        match parse_csl_id_and_title(&bytes) {
            Ok((id, title)) => styles.push(Style {
                path: path_str,
                id,
                title: title.unwrap_or(stem),
                valid: true,
            }),
            Err(_) => styles.push(Style {
                path: path_str,
                id: None,
                title: stem,
                valid: false,
            }),
        }
    }
    Ok(styles)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Matches Python's `list[:limit]` slicing exactly, including negative
    // limits — a review pass found the original `.max(0)`-clamped
    // `Vec::truncate` diverged on that case.
    #[test]
    fn python_slice_to_limit_matches_python_list_slicing() {
        let cases: &[(Vec<i32>, i64, Vec<i32>)] = &[
            (vec![1, 2, 3, 4, 5], 3, vec![1, 2, 3]),
            (vec![1, 2, 3, 4, 5], 10, vec![1, 2, 3, 4, 5]),
            (vec![1, 2, 3, 4, 5], 0, vec![]),
            (vec![1, 2, 3, 4, 5], -2, vec![1, 2, 3]),
            (vec![1, 2, 3, 4, 5], -10, vec![]),
            (vec![], 5, vec![]),
        ];
        for (input, limit, expected) in cases {
            let mut items = input.clone();
            python_slice_to_limit(&mut items, *limit);
            assert_eq!(&items, expected, "limit={limit} input={input:?}");
        }
    }
}
