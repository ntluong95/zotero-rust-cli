//! Live-first target resolution for write commands.
//!
//! Every write command needs the same handful of facts about the object it is about to mutate --
//! its canonical `key`, its `libraryID`, whether that library is a `user` or a `group` (which
//! decides the Local API scope), and for items the `itemType`/`itemID`. Before this module, all
//! of them came from `catalog::get_item`/`get_collection`/`local_api_scope`, i.e. from SQLite,
//! *before* any backend was selected.
//!
//! That ordering is unsafe by construction for a live write: a running Zotero holds an exclusive
//! lock on a WAL-mode `zotero.sqlite`, so `db::connect_readonly` correctly refuses (it must never
//! fall back to `immutable=1` on a WAL database and silently read a stale snapshot). The result
//! was that `note add`, `item update`, `item tag`, `item delete`, and friends failed during
//! *target lookup* while the very Zotero process that could answer the question sat there
//! healthy on the other side of an owned Bridge.
//!
//! Resolution tries every live source before SQLite, in the order the *caller's own* write
//! routing already decided ([`Prefer`]) -- so a command about to write through the Local API
//! does not first pay for a Bridge handshake it will never use, and a Bridge-routed command
//! resolves through the very Bridge it is about to write with (whose ownership probe is then
//! already cached for the write itself, costing nothing extra).
//!
//! Whichever is preferred, the order is: preferred live source, then the other live source,
//! then SQLite (`catalog::*`) unchanged -- the correct source when Zotero is closed, and the
//! last resort otherwise. This mirrors the bridge-first / SQLite-fallback shape
//! `hygiene::merge_preview` already uses.
//!
//! Read-only by construction: the Bridge templates below resolve and describe, never `saveTx`,
//! `eraseTx`, `merge`, or `trash`. The SQLite safety guard is untouched -- live writes simply
//! stop *needing* it.

use serde_json::Value;

use crate::bridge::JSBridgeClient;
use crate::catalog;
use crate::error::DomainError;
use crate::http;
use crate::runtime::RuntimeContext;
use crate::session::SessionState;

/// Which live source to try first, taken from the caller's own backend routing decision.
///
/// This is not a preference in the "nice to have" sense: asking the wrong source first costs a
/// wasted round trip on every write, and for [`Prefer::Bridge`] callers the Local API cannot
/// supply Zotero's internal numeric `itemID` at all (see [`local_api_resolve_item`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prefer {
    /// The caller writes through the owned Bridge (`note add`, `item attach`, `item merge
    /// --confirm`, and every Local-API-unavailable fallback).
    Bridge,
    /// The caller writes through the Local API.
    LocalApi,
}

impl Prefer {
    /// The routing decision every dual-backend CRUD command already makes.
    pub fn for_runtime(runtime: &RuntimeContext) -> Self {
        if runtime.local_api_writes_available {
            Prefer::LocalApi
        } else {
            Prefer::Bridge
        }
    }
}

/// Everything a write command needs to know about its target item, from whichever source
/// answered. Deliberately backend-neutral: no caller can tell which of the three paths produced
/// it, and no field is Local-API- or Bridge-specific.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetItem {
    pub key: String,
    pub library_id: i64,
    /// `"user"` or `"group"` -- what `local_api_scope` needs, without a SQLite round trip.
    pub library_type: String,
    pub item_type: String,
    /// Zotero's internal numeric item id. `note add` reports it as `parentItemID`.
    pub item_id: i64,
}

/// Collection counterpart to [`TargetItem`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetCollection {
    pub key: String,
    pub library_id: i64,
    pub library_type: String,
    pub name: String,
    pub collection_id: i64,
}

impl TargetItem {
    /// `catalog::local_api_scope`'s answer, derived from an already-resolved target instead of a
    /// second SQLite `resolve_library` call.
    pub fn local_api_scope(&self) -> anyhow::Result<String> {
        library_scope(&self.library_type, self.library_id)
    }
}

impl TargetCollection {
    pub fn local_api_scope(&self) -> anyhow::Result<String> {
        library_scope(&self.library_type, self.library_id)
    }
}

/// `catalog::local_api_scope`'s mapping (`catalog.py:41-49`), split out so both the live and the
/// SQLite paths produce byte-identical scopes.
pub fn library_scope(library_type: &str, library_id: i64) -> anyhow::Result<String> {
    match library_type {
        "user" => Ok("/api/users/0".to_string()),
        "group" => Ok(format!("/api/groups/{library_id}")),
        other => Err(DomainError::new(format!(
            "Unsupported library type for Zotero Local API: {other}"
        ))
        .into()),
    }
}

/// Resolves an item reference (a key, or Zotero's numeric `itemID`) across every library the
/// running Zotero has open, honoring an explicit session library scope when one is set. Returns
/// `{found:false}` rather than throwing when nothing matches, so a genuine "not found" stays
/// distinguishable from a transport failure.
const T_RESOLVE_ITEM: &str = r#"
function describe(item) {
  var lib = Zotero.Libraries.get(item.libraryID);
  return JSON.stringify({
    found: true,
    key: item.key,
    libraryID: item.libraryID,
    libraryType: lib ? lib.libraryType : 'user',
    itemType: Zotero.ItemTypes.getName(item.itemTypeID),
    itemID: item.id
  });
}
if (P.numeric) {
  var byId = Zotero.Items.get(P.ref);
  if (byId && (P.libraryID === null || byId.libraryID === P.libraryID)) { return describe(byId); }
  return JSON.stringify({found: false});
}
var libraryIDs = P.libraryID === null
  ? Zotero.Libraries.getAll().map(function (l) { return l.libraryID; })
  : [P.libraryID];
for (var i = 0; i < libraryIDs.length; i++) {
  var item = Zotero.Items.getByLibraryAndKey(libraryIDs[i], P.ref);
  if (item) { return describe(item); }
}
return JSON.stringify({found: false});
"#;

/// Collection counterpart to [`T_RESOLVE_ITEM`].
const T_RESOLVE_COLLECTION: &str = r#"
function describe(col) {
  var lib = Zotero.Libraries.get(col.libraryID);
  return JSON.stringify({
    found: true,
    key: col.key,
    libraryID: col.libraryID,
    libraryType: lib ? lib.libraryType : 'user',
    name: col.name,
    collectionID: col.id
  });
}
if (P.numeric) {
  var byId = Zotero.Collections.get(P.ref);
  if (byId && (P.libraryID === null || byId.libraryID === P.libraryID)) { return describe(byId); }
  return JSON.stringify({found: false});
}
var libraryIDs = P.libraryID === null
  ? Zotero.Libraries.getAll().map(function (l) { return l.libraryID; })
  : [P.libraryID];
for (var i = 0; i < libraryIDs.length; i++) {
  var col = Zotero.Collections.getByLibraryAndKey(libraryIDs[i], P.ref);
  if (col) { return describe(col); }
}
return JSON.stringify({found: false});
"#;

/// `db::is_numeric_ref`'s rule (`zotero_sqlite.py:48-53`): Python's `int(str(value))` trims
/// surrounding whitespace first, so a padded `" 5 "` is still a numeric ref.
fn is_numeric_ref(value: &str) -> bool {
    value.trim().parse::<i64>().is_ok()
}

/// The session's `current_library` as a numeric id, when it is set to one. A non-numeric session
/// library (a name or key) is left for the SQLite path to resolve -- the live resolver simply
/// searches every open library in that case rather than guessing.
fn session_library_id(session: &SessionState) -> Option<i64> {
    match &session.current_library {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// One live-resolution attempt through the owned Bridge. `Ok(None)` means "the Bridge answered
/// and the object genuinely does not exist"; `Err` means the Bridge could not answer at all and
/// the caller should fall through to the next source.
fn bridge_resolve(
    bridge: &JSBridgeClient,
    template: &str,
    item_ref: &str,
    library_id: Option<i64>,
) -> anyhow::Result<Option<Value>> {
    let params = serde_json::json!({
        "ref": if is_numeric_ref(item_ref) {
            Value::from(item_ref.trim().parse::<i64>().unwrap_or_default())
        } else {
            Value::from(item_ref)
        },
        "numeric": is_numeric_ref(item_ref),
        "libraryID": library_id,
    });
    let code = crate::bridge::templates::render(template, &params)?;
    let resp = bridge.execute_js(&code, 10);
    if !resp.ok {
        anyhow::bail!(
            "{}",
            resp.error
                .unwrap_or_else(|| "bridge unavailable".to_string())
        );
    }
    let data = resp
        .data
        .ok_or_else(|| anyhow::anyhow!("bridge returned an empty resolution response"))?;
    // The template returns a JSON *string*; `execute_http` parses a JSON body, so a quoted
    // string arrives as `Value::String` and must be parsed once more.
    let parsed: Value = match &data {
        Value::String(text) => serde_json::from_str(text)?,
        other => other.clone(),
    };
    if parsed.get("found").and_then(Value::as_bool) == Some(true) {
        Ok(Some(parsed))
    } else {
        Ok(None)
    }
}

fn field_str(value: &Value, field: &str) -> anyhow::Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("live resolution response missing `{field}`"))
}

fn field_i64(value: &Value, field: &str) -> anyhow::Result<i64> {
    value
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow::anyhow!("live resolution response missing `{field}`"))
}

/// Resolves the item a write command is about to mutate, live-first.
///
/// `item_ref` accepts the same forms `catalog::get_item` does (a key or a numeric `itemID`) and
/// falls back to `session.current_item` when `None`, so this is a drop-in replacement at every
/// write call site.
pub fn resolve_item(
    runtime: &RuntimeContext,
    bridge: &JSBridgeClient,
    item_ref: Option<&str>,
    session: &SessionState,
    prefer: Prefer,
) -> anyhow::Result<TargetItem> {
    let resolved = item_ref
        .map(str::to_string)
        .or_else(|| session.current_item.clone());
    let Some(resolved) = resolved else {
        return Err(DomainError::new("Item reference required or set it in session first").into());
    };
    let scope_library = session_library_id(session);

    let try_bridge = || match bridge_resolve(bridge, T_RESOLVE_ITEM, &resolved, scope_library) {
        Ok(Some(found)) => Some(Ok(TargetItem {
            key: field_str(&found, "key").ok()?,
            library_id: field_i64(&found, "libraryID").ok()?,
            library_type: field_str(&found, "libraryType").ok()?,
            item_type: field_str(&found, "itemType").ok()?,
            item_id: field_i64(&found, "itemID").ok()?,
        })),
        // The Bridge answered authoritatively that no such item exists in the live library. Do
        // not fall through to SQLite for this: a stale snapshot claiming otherwise would be
        // worse than the truthful answer.
        Ok(None) => Some(Err(anyhow::Error::from(DomainError::new(format!(
            "Item not found: {resolved}"
        ))))),
        Err(_) => None,
    };
    let try_local_api = || local_api_resolve_item(runtime, &resolved);

    match prefer {
        Prefer::Bridge => {
            if let Some(result) = try_bridge() {
                return result;
            }
            if let Some(target) = try_local_api()? {
                return Ok(target);
            }
        }
        Prefer::LocalApi => {
            if let Some(target) = try_local_api()? {
                return Ok(target);
            }
            if let Some(result) = try_bridge() {
                return result;
            }
        }
    }

    // Offline / last resort: the existing SQLite path, unchanged. When Zotero is running and
    // holds the WAL lock this raises the established refusal, which is the correct outcome once
    // both live sources have already declined.
    let item = catalog::get_item(runtime, Some(&resolved), session)?;
    let library = crate::db::resolve_library(
        &runtime.environment.sqlite_path,
        &item.library_id.to_string(),
    )?
    .ok_or_else(|| DomainError::new(format!("Library not found: {}", item.library_id)))?;
    Ok(TargetItem {
        key: item.key,
        library_id: item.library_id,
        library_type: library.kind,
        item_type: item.type_name,
        item_id: item.item_id,
    })
}

/// Collection counterpart to [`resolve_item`].
pub fn resolve_collection(
    runtime: &RuntimeContext,
    bridge: &JSBridgeClient,
    collection_ref: Option<&str>,
    session: &SessionState,
    prefer: Prefer,
) -> anyhow::Result<TargetCollection> {
    let resolved = collection_ref
        .map(str::to_string)
        .or_else(|| crate::session::session_collection_ref(session));
    let Some(resolved) = resolved else {
        return Err(
            DomainError::new("Collection reference required or set it in session first").into(),
        );
    };
    let scope_library = session_library_id(session);

    let try_bridge = || match bridge_resolve(bridge, T_RESOLVE_COLLECTION, &resolved, scope_library)
    {
        Ok(Some(found)) => Some(Ok(TargetCollection {
            key: field_str(&found, "key").ok()?,
            library_id: field_i64(&found, "libraryID").ok()?,
            library_type: field_str(&found, "libraryType").ok()?,
            name: field_str(&found, "name").ok()?,
            collection_id: field_i64(&found, "collectionID").ok()?,
        })),
        Ok(None) => Some(Err(anyhow::Error::from(DomainError::new(format!(
            "Collection not found: {resolved}"
        ))))),
        Err(_) => None,
    };
    let try_local_api = || local_api_resolve_collection(runtime, &resolved);

    match prefer {
        Prefer::Bridge => {
            if let Some(result) = try_bridge() {
                return result;
            }
            if let Some(target) = try_local_api()? {
                return Ok(target);
            }
        }
        Prefer::LocalApi => {
            if let Some(target) = try_local_api()? {
                return Ok(target);
            }
            if let Some(result) = try_bridge() {
                return result;
            }
        }
    }

    let collection = catalog::get_collection(runtime, Some(&resolved), session)?;
    let library = crate::db::resolve_library(
        &runtime.environment.sqlite_path,
        &collection.library_id.to_string(),
    )?
    .ok_or_else(|| DomainError::new(format!("Library not found: {}", collection.library_id)))?;
    Ok(TargetCollection {
        key: collection.key,
        library_id: collection.library_id,
        library_type: library.kind,
        name: collection.collection_name,
        collection_id: collection.collection_id,
    })
}

/// The library a create-style write lands in, when there is no existing object to resolve it
/// from. `collection create` is the one command in this shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetLibrary {
    pub library_id: i64,
    pub library_type: String,
}

impl TargetLibrary {
    pub fn local_api_scope(&self) -> anyhow::Result<String> {
        library_scope(&self.library_type, self.library_id)
    }
}

const T_RESOLVE_LIBRARY: &str = r#"
var libraryID = P.libraryID === null ? Zotero.Libraries.userLibraryID : P.libraryID;
var lib = Zotero.Libraries.get(libraryID);
if (!lib) { return JSON.stringify({found: false}); }
return JSON.stringify({
  found: true,
  libraryID: lib.libraryID,
  libraryType: lib.libraryType
});
"#;

/// `catalog::default_library`'s answer, live-first: the session's `current_library` when it names
/// one, otherwise the running Zotero's own personal library. Same three-source order as
/// [`resolve_item`].
pub fn resolve_default_library(
    runtime: &RuntimeContext,
    bridge: &JSBridgeClient,
    session: &SessionState,
    prefer: Prefer,
) -> anyhow::Result<TargetLibrary> {
    let scope_library = session_library_id(session);

    // A Local-API-routed create needs no lookup at all: the personal-library scope is fixed
    // (`/api/users/0`), and `library.id` is reported as `0` by every Local API response, so
    // reporting it the same way here keeps a resolved library comparable with a resolved
    // collection. Probing the Bridge for a fact the caller will not use would be a wasted round
    // trip on every `collection create`.
    if prefer == Prefer::LocalApi && runtime.local_api_available {
        return Ok(TargetLibrary {
            library_id: scope_library.unwrap_or(0),
            library_type: "user".to_string(),
        });
    }

    let params = serde_json::json!({ "libraryID": scope_library });
    if let Ok(code) = crate::bridge::templates::render(T_RESOLVE_LIBRARY, &params) {
        let resp = bridge.execute_js(&code, 10);
        if resp.ok {
            if let Some(data) = resp.data {
                let parsed: Value = match &data {
                    Value::String(text) => serde_json::from_str(text).unwrap_or(Value::Null),
                    other => other.clone(),
                };
                if parsed.get("found").and_then(Value::as_bool) == Some(true) {
                    return Ok(TargetLibrary {
                        library_id: field_i64(&parsed, "libraryID")?,
                        library_type: field_str(&parsed, "libraryType")?,
                    });
                }
            }
        }
    }

    // The Local API exposes no library enumeration, so there is no middle source here: a session
    // library that is already a plain id needs no lookup at all to be usable as a user-library
    // scope, and anything else falls through to SQLite.
    if let Some(library_id) = scope_library {
        if runtime.local_api_available {
            return Ok(TargetLibrary {
                library_id,
                library_type: "user".to_string(),
            });
        }
    }

    let library_id = catalog::default_library(runtime, session)?;
    let library =
        crate::db::resolve_library(&runtime.environment.sqlite_path, &library_id.to_string())?
            .ok_or_else(|| DomainError::new(format!("Library not found: {library_id}")))?;
    Ok(TargetLibrary {
        library_id,
        library_type: library.kind,
    })
}

/// Local API fallback for [`resolve_item`], used when the Bridge could not answer at all.
///
/// The Local API exposes no cross-library lookup, and its `itemID` is not part of the response
/// at all, so this only covers the personal library (`/api/users/0`) and reports `item_id` as
/// `0`. That is sufficient for every write command's own routing (all of which key off
/// `key`/`library_id`/`library_type`); `note add` is the one caller that surfaces `item_id`, and
/// it prefers the Bridge, which always supplies a real one.
///
/// `Ok(None)` means the Local API is unusable here (unreachable, or a non-`user` reference);
/// a `404` becomes a clean "not found" error rather than a silent fall-through to SQLite.
fn local_api_resolve_item(
    runtime: &RuntimeContext,
    item_ref: &str,
) -> anyhow::Result<Option<TargetItem>> {
    if !runtime.local_api_available || is_numeric_ref(item_ref) {
        return Ok(None);
    }
    let path = format!("/api/users/0/items/{item_ref}");
    let response = match http::local_api_get_raw(
        runtime.environment.port,
        &path,
        std::time::Duration::from_secs(10),
    ) {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    match response.status {
        200 => {}
        404 => return Err(DomainError::new(format!("Item not found: {item_ref}")).into()),
        _ => return Ok(None),
    }
    let json: Value = serde_json::from_str(&response.body)?;
    let library_id = json
        .get("library")
        .and_then(|lib| lib.get("id"))
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow::anyhow!("Local API item response missing `library.id`"))?;
    let library_type = json
        .get("library")
        .and_then(|lib| lib.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("user")
        .to_string();
    let item_type = json
        .get("data")
        .and_then(|d| d.get("itemType"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let key = json
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or(item_ref)
        .to_string();
    Ok(Some(TargetItem {
        key,
        library_id,
        library_type,
        item_type,
        item_id: 0,
    }))
}

/// Collection counterpart to [`local_api_resolve_item`], with the same personal-library-only
/// limitation.
fn local_api_resolve_collection(
    runtime: &RuntimeContext,
    collection_ref: &str,
) -> anyhow::Result<Option<TargetCollection>> {
    if !runtime.local_api_available || is_numeric_ref(collection_ref) {
        return Ok(None);
    }
    let path = format!("/api/users/0/collections/{collection_ref}");
    let response = match http::local_api_get_raw(
        runtime.environment.port,
        &path,
        std::time::Duration::from_secs(10),
    ) {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    match response.status {
        200 => {}
        404 => {
            return Err(DomainError::new(format!("Collection not found: {collection_ref}")).into())
        }
        _ => return Ok(None),
    }
    let json: Value = serde_json::from_str(&response.body)?;
    let library_id = json
        .get("library")
        .and_then(|lib| lib.get("id"))
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow::anyhow!("Local API collection response missing `library.id`"))?;
    let library_type = json
        .get("library")
        .and_then(|lib| lib.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("user")
        .to_string();
    let name = json
        .get("data")
        .and_then(|d| d.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let key = json
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or(collection_ref)
        .to_string();
    Ok(Some(TargetCollection {
        key,
        library_id,
        library_type,
        name,
        collection_id: 0,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_scope_matches_catalog_local_api_scope() {
        assert_eq!(library_scope("user", 1).unwrap(), "/api/users/0");
        assert_eq!(library_scope("group", 4242).unwrap(), "/api/groups/4242");
        assert!(library_scope("feed", 7).is_err());
    }

    #[test]
    fn numeric_refs_trim_surrounding_whitespace_like_python() {
        assert!(is_numeric_ref(" 5 "));
        assert!(is_numeric_ref("12"));
        assert!(!is_numeric_ref("ITEM0001"));
        assert!(!is_numeric_ref(""));
    }

    // The resolution templates are read-only by construction: no mutation verb may appear in
    // either one. This is the structural guard that keeps a future edit from turning a lookup
    // into a write path.
    #[test]
    fn resolution_templates_contain_no_mutation_verbs() {
        for template in [T_RESOLVE_ITEM, T_RESOLVE_COLLECTION] {
            for verb in [
                "saveTx",
                "eraseTx",
                "merge(",
                "trash",
                "removeItem",
                "setField",
            ] {
                assert!(
                    !template.contains(verb),
                    "resolution template must stay read-only but contains {verb:?}"
                );
            }
        }
    }
}
