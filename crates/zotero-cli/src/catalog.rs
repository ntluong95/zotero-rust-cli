//! Port of `core/catalog.py`'s domain layer needed by the vertical slice:
//! library/collection resolution and the item list/get/find operations.

use std::time::Duration;

use crate::db::{self, Collection, FetchItemsFilter, Item};
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

/// `get_collection()` (`catalog.py:68-80`).
pub fn get_collection(
    runtime: &RuntimeContext,
    collection_ref: &str,
    session: &SessionState,
) -> anyhow::Result<Collection> {
    let library_id = resolve_library_id(runtime, session_library_ref(session).as_deref())?;
    let collection =
        db::resolve_collection(&runtime.environment.sqlite_path, collection_ref, library_id)?;
    collection
        .ok_or_else(|| DomainError::new(format!("Collection not found: {collection_ref}")).into())
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
        Some(cref) if !cref.is_empty() => Some(get_collection(runtime, cref, session)?),
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
        Some(library_id),
        collection_id,
        limit,
        exact_title,
    )
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
