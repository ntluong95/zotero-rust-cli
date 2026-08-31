//! Item search: library scoping and read-path selection, shared by every caller.
//!
//! Two additive capabilities live here, and they compose:
//!
//! **Cross-library scope.** Canonical `find_items` searches exactly one library -- the
//! collection's, or `session.current_library`, or the default. An agent that knows a title but
//! not a `libraryID` therefore gets `[]` from a library it never meant to search, with no signal
//! that the paper exists elsewhere. The only way forward was to write `current_library` and
//! re-run, once per library. [`SearchScopeRequest::AllLibraries`] resolves the eligible
//! libraries up front and searches them together, without ever touching persisted session state.
//!
//! **Live read path.** Canonical requires SQLite on every path -- even its Local-API branch is
//! only a key finder whose every hit is re-read through `zotero_sqlite.resolve_item`. Because a
//! running Zotero holds an exclusive lock on its WAL-mode database (which this crate refuses to
//! read rather than falling back to a stale `immutable=1` snapshot), `item find` failed outright
//! whenever Zotero was open -- the state a user is most likely to be in. When an owned Bridge is
//! available the search now runs inside Zotero's own search engine instead, touching no SQLite
//! at all.
//!
//! Routing, in full:
//!
//! ```text
//! Zotero closed                          -> SQLite          (unchanged, canonical)
//! Zotero running + owned Bridge healthy  -> Bridge search    (additive)
//! Zotero running + no owned Bridge       -> SQLite refusal   (unchanged, reported verbatim)
//! --exact-title (any state)              -> SQLite           (Zotero quicksearch has no exact mode)
//! ```
//!
//! **The order is SQLite first, live second, and that is deliberate.** The live path is only
//! attempted after SQLite has actually refused with [`db::DatabaseLocked`] -- never speculatively.
//! Two things follow. First, an offline invocation issues exactly the requests canonical issues:
//! no Bridge probe is added to the common case, so `item find`'s byte-identical parity
//! (including its recorded `http_calls` sequence) is untouched. Second, the WAL refusal is the
//! *trigger* for the live path rather than something routed around, so it can never be
//! accidentally bypassed -- if the Bridge cannot answer either, the original refusal is returned
//! unchanged, with its original wording.
//!
//! `immutable=1` is never used, and nothing here writes.
//!
//! **The Local API is deliberately not a search backend.** It can serve one known library, but
//! it exposes no endpoint that enumerates the groups a Zotero has -- discovering group ids means
//! reading the `libraries` table, i.e. the SQLite that is locked. It therefore cannot satisfy
//! cross-library search in the exact situation that motivates it, so using it would mean two
//! backends, two result shapes, and a capability gap. The Bridge enumerates live and answers
//! both scopes.

use serde_json::Value;

use crate::bridge::JSBridgeClient;
use crate::catalog;
use crate::db::{self, Item, SearchLibraries};
use crate::error::DomainError;
use crate::runtime::RuntimeContext;
use crate::session::SessionState;

/// Library types that are searched by default under `--all-libraries`.
///
/// Feeds are excluded unless asked for: a feed item is an unsaved RSS entry, not a library item,
/// so surfacing one to an agent that will then try `note add` on it produces a confusing failure
/// far from its cause. `--include-feeds` makes the choice explicit rather than silent.
const DEFAULT_SEARCH_LIBRARY_TYPES: [&str; 2] = ["user", "group"];
const FEED_LIBRARY_TYPE: &str = "feed";

/// What the caller asked for, before any library resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchScopeRequest {
    /// Canonical: the collection's library, else the session/default library.
    CurrentLibrary,
    /// Additive `--all-libraries`.
    AllLibraries { include_feeds: bool },
}

/// Where a result set came from. Reported so a caller can tell an empty result on a live search
/// apart from one produced offline, and so tests can assert the routing directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchSource {
    Sqlite,
    Bridge,
}

pub struct SearchRequest<'a> {
    pub query: &'a str,
    pub collection_ref: Option<&'a str>,
    pub limit: i64,
    pub exact_title: bool,
    pub scope: &'a str,
    pub libraries: SearchScopeRequest,
}

/// Maps this CLI's search scope (canonical `titleCreatorYear` / `fields` / `everything`, passed
/// to the Local API as `qmode`) onto Zotero's own quicksearch search-condition names.
fn quicksearch_condition(scope: &str) -> String {
    format!("quicksearch-{scope}")
}

/// The libraries `--all-libraries` covers, from either source.
///
/// Applied identically to the SQLite and Bridge enumerations so `--include-feeds` means exactly
/// the same thing whichever answered, and so the two paths can never drift into searching
/// different sets.
fn eligible_library_ids<'a>(
    libraries: impl Iterator<Item = (i64, &'a str)>,
    include_feeds: bool,
) -> Vec<i64> {
    let mut ids: Vec<i64> = libraries
        .filter(|(_, kind)| {
            DEFAULT_SEARCH_LIBRARY_TYPES.contains(kind)
                || (include_feeds && *kind == FEED_LIBRARY_TYPE)
        })
        .map(|(id, _)| id)
        .collect();
    ids.sort_unstable();
    ids
}

/// Live library enumeration, projecting exactly the columns `db::fetch_libraries` reads from the
/// `libraries` table plus the additive `name`, so both sources produce the same records.
///
/// `editable`/`filesEditable`/`archived` are booleans on Zotero's `Library` object and integers
/// in the database; they are normalized to integers here so the JSON does not change shape
/// depending on which source answered. `lastSync` is a JS `Date` (or `false`); the database
/// stores an integer, so a live value is reported as `null` rather than reformatted into
/// something that would not round-trip.
const T_LIST_LIBRARIES: &str = r#"
return JSON.stringify(Zotero.Libraries.getAll().map(function (l) {
  return {
    libraryID: l.libraryID,
    type: l.libraryType,
    name: l.name === undefined ? null : l.name,
    editable: l.editable ? 1 : 0,
    filesEditable: l.filesEditable ? 1 : 0,
    version: l.libraryVersion || 0,
    storageVersion: l.storageVersion || 0,
    archived: l.archived ? 1 : 0
  };
}));
"#;

/// Every library the running Zotero has open, or `None` when the Bridge cannot answer at all
/// (so the caller falls through to SQLite).
pub fn bridge_libraries(bridge: &JSBridgeClient) -> Option<Vec<db::Library>> {
    let code = crate::bridge::templates::render(T_LIST_LIBRARIES, &serde_json::json!({})).ok()?;
    let parsed = parse_bridge_json(bridge.execute_js(&code, 10))?;
    let entries = parsed.as_array()?;
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        out.push(db::Library {
            library_id: entry.get("libraryID").and_then(Value::as_i64)?,
            kind: entry.get("type").and_then(Value::as_str)?.to_string(),
            editable: entry.get("editable").and_then(Value::as_i64).unwrap_or(0),
            files_editable: entry
                .get("filesEditable")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            version: entry.get("version").and_then(Value::as_i64).unwrap_or(0),
            storage_version: entry
                .get("storageVersion")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            last_sync: None,
            archived: entry.get("archived").and_then(Value::as_i64).unwrap_or(0),
            name: entry
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
    Some(out)
}

/// `library list`, live-first for the same reason `item find` is: a running Zotero holds the
/// database lock, and an agent discovering which library to work in should not have to close
/// Zotero to read a list of names.
pub fn list_libraries(
    runtime: &RuntimeContext,
    bridge: &JSBridgeClient,
) -> anyhow::Result<(Vec<db::Library>, SearchSource)> {
    // SQLite first, for the same reason `find_items` does it in that order: an offline run must
    // issue exactly the requests it always did, with no speculative Bridge probe added to a
    // command that is byte-compared against canonical.
    let refusal = match db::fetch_libraries(&runtime.environment.sqlite_path) {
        Ok(libraries) => return Ok((libraries, SearchSource::Sqlite)),
        Err(err) if db::is_database_locked(&err) => err,
        Err(err) => return Err(err),
    };
    match bridge_libraries(bridge) {
        Some(mut libraries) => {
            libraries.sort_by_key(|library| library.library_id);
            Ok((libraries, SearchSource::Bridge))
        }
        None => Err(refusal),
    }
}

/// Unwraps a Bridge response whose template returns a JSON *string*: `execute_http` parses the
/// HTTP body, so a quoted string arrives as `Value::String` and needs one more parse.
fn parse_bridge_json(resp: crate::bridge::BridgeResponse) -> Option<Value> {
    if !resp.ok {
        return None;
    }
    match resp.data? {
        Value::String(text) => serde_json::from_str(&text).ok(),
        other => Some(other),
    }
}

/// Runs `query` inside the live Zotero runtime, across `library_ids`, using Zotero's own search
/// engine. Read-only by construction: it resolves and describes items and never mutates.
///
/// Returns every field `db::find_items_by_title` populates for a search hit (which calls
/// `normalize_item` with `include_related = false`, so `fields`/`creators`/`tags` are empty on
/// that path too) -- so both read paths produce the same JSON shape.
const T_SEARCH_ITEMS: &str = r#"
var out = [];
for (var i = 0; i < P.libraryIDs.length; i++) {
  var libraryID = P.libraryIDs[i];
  var s = new Zotero.Search();
  s.libraryID = libraryID;
  s.addCondition(P.condition, 'contains', P.query);
  if (P.collectionKey) { s.addCondition('collection', 'is', P.collectionKey); }
  var ids = await s.search();
  if (!ids || !ids.length) { continue; }
  var items = await Zotero.Items.getAsync(ids);
  for (var j = 0; j < items.length; j++) {
    var it = items[j];
    if (it.parentItemID) { continue; }
    var hasPdf = false;
    try {
      var attachmentIDs = it.getAttachments();
      for (var k = 0; k < attachmentIDs.length; k++) {
        var att = Zotero.Items.get(attachmentIDs[k]);
        if (att && att.attachmentContentType === 'application/pdf') { hasPdf = true; break; }
      }
    } catch (e) { hasPdf = false; }
    out.push({
      itemID: it.id,
      key: it.key,
      libraryID: it.libraryID,
      itemTypeID: it.itemTypeID,
      typeName: Zotero.ItemTypes.getName(it.itemTypeID),
      dateAdded: it.dateAdded,
      dateModified: it.dateModified,
      version: it.version,
      title: it.getDisplayTitle ? it.getDisplayTitle() : (it.getField('title') || ''),
      DOI: it.getField('DOI') || '',
      date: it.getField('date') || null,
      hasPdf: hasPdf
    });
  }
}
return JSON.stringify(out);
"#;

/// Builds the same `Item` shape the SQLite search path returns, from one live search record.
///
/// Every field the SQLite path leaves empty for a search hit is left empty here too, so the two
/// paths cannot drift: `fields`/`creators`/`tags` are empty maps/vectors (SQLite passes
/// `include_related = false`), and the note/attachment/annotation columns are `None` because the
/// live search only yields top-level regular items.
fn live_item(record: &Value) -> Option<Item> {
    let type_name = record.get("typeName").and_then(Value::as_str)?.to_string();
    let title = record
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Some(Item {
        item_id: record.get("itemID").and_then(Value::as_i64)?,
        key: record.get("key").and_then(Value::as_str)?.to_string(),
        library_id: record.get("libraryID").and_then(Value::as_i64)?,
        item_type_id: record
            .get("itemTypeID")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        type_name: type_name.clone(),
        date_added: record
            .get("dateAdded")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        date_modified: record
            .get("dateModified")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        version: record.get("version").and_then(Value::as_i64).unwrap_or(0),
        title: title.clone(),
        doi: record
            .get("DOI")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        date: record
            .get("date")
            .and_then(Value::as_str)
            .map(str::to_string),
        has_pdf: record
            .get("hasPdf")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        note_parent_item_id: None,
        note_content: None,
        attachment_parent_item_id: None,
        annotation_parent_item_id: None,
        annotation_text: None,
        annotation_comment: None,
        link_mode: None,
        content_type: None,
        attachment_path: None,
        fields: serde_json::Map::new(),
        creators: Vec::new(),
        tags: Vec::new(),
        is_attachment: type_name == "attachment",
        is_note: type_name == "note",
        is_annotation: type_name == "annotation",
        parent_item_id: None,
        note_text: String::new(),
        note_preview: String::new(),
    })
}

/// Applies the same relevance ordering the SQLite path encodes in SQL, so a cross-library live
/// result set is ordered identically to a cross-library offline one: exact title first, then
/// prefix match, then earliest match position, then most-recently-modified, then highest item id.
/// Deterministic for any fixed input.
fn order_like_sqlite(items: &mut [Item], query: &str) {
    let needle = query.trim().to_lowercase();
    items.sort_by(|a, b| {
        let rank = |item: &Item| {
            let title = item.title.to_lowercase();
            if title == needle {
                0
            } else if title.starts_with(&needle) {
                1
            } else {
                2
            }
        };
        let position = |item: &Item| {
            item.title
                .to_lowercase()
                .find(&needle)
                .map(|i| i as i64 + 1)
                .unwrap_or(0)
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| position(a).cmp(&position(b)))
            .then_with(|| b.date_modified.cmp(&a.date_modified))
            .then_with(|| b.item_id.cmp(&a.item_id))
    });
}

/// Searches through the live Zotero runtime. `Ok(None)` means the Bridge could not answer and
/// the caller should fall back to SQLite; `Err` is reserved for a malformed response.
fn bridge_search(
    bridge: &JSBridgeClient,
    query: &str,
    library_ids: &[i64],
    collection_key: Option<&str>,
    scope: &str,
    limit: i64,
) -> anyhow::Result<Option<Vec<Item>>> {
    let params = serde_json::json!({
        "libraryIDs": library_ids,
        "query": query,
        "condition": quicksearch_condition(scope),
        "collectionKey": collection_key,
    });
    let code = crate::bridge::templates::render(T_SEARCH_ITEMS, &params)?;
    let Some(parsed) = parse_bridge_json(bridge.execute_js(&code, 20)) else {
        return Ok(None);
    };
    let Some(records) = parsed.as_array() else {
        anyhow::bail!("live search returned an unexpected response shape: {parsed}");
    };
    let mut items: Vec<Item> = records.iter().filter_map(live_item).collect();
    order_like_sqlite(&mut items, query);
    items.truncate(limit.max(0) as usize);
    Ok(Some(items))
}

/// `find_items` with explicit scope and read-path selection.
///
/// With [`SearchScopeRequest::CurrentLibrary`] and no live Bridge this is exactly canonical
/// `find_items`, including its Local-API-first branch.
pub fn find_items(
    runtime: &RuntimeContext,
    bridge: &JSBridgeClient,
    session: &SessionState,
    request: SearchRequest<'_>,
) -> anyhow::Result<(Vec<Item>, SearchSource)> {
    if !catalog::SEARCH_SCOPES.contains(&request.scope) {
        return Err(
            DomainError::new(format!("Unsupported item search scope: {}", request.scope)).into(),
        );
    }

    // A collection already pins exactly one library, so combining it with `--all-libraries` is
    // contradictory rather than merely redundant. Rejecting it is clearer than silently letting
    // one option win.
    if matches!(request.libraries, SearchScopeRequest::AllLibraries { .. })
        && request.collection_ref.is_some()
    {
        return Err(DomainError::new(
            "--all-libraries cannot be combined with --collection: a collection already belongs \
             to exactly one library",
        )
        .into());
    }

    // Attempt 1: the offline path. Canonical for `CurrentLibrary`, byte for byte.
    let offline = match &request.libraries {
        SearchScopeRequest::CurrentLibrary => catalog::find_items(
            runtime,
            request.query,
            request.collection_ref,
            request.limit,
            request.exact_title,
            request.scope,
            session,
        ),
        SearchScopeRequest::AllLibraries { include_feeds } => {
            sqlite_all_libraries(runtime, &request, *include_feeds)
        }
    };
    let refusal = match offline {
        Ok(items) => return Ok((items, SearchSource::Sqlite)),
        // Only the "Zotero holds the database" refusal is retryable live. Every other failure
        // (missing database, bad scope, unresolvable collection) is returned as-is: a live
        // retry would neither fix it nor describe it better.
        Err(err) if db::is_database_locked(&err) => err,
        Err(err) => return Err(err),
    };

    // Attempt 2: the live path, now that SQLite has genuinely refused.
    //
    // `--exact-title` has no live equivalent -- Zotero's quicksearch is substring-only, and
    // approximating an exact match would silently change what the flag means -- so it keeps the
    // refusal rather than getting different semantics.
    if request.exact_title {
        return Err(refusal);
    }
    match live_search(runtime, bridge, session, &request) {
        Ok(Some(items)) => Ok((items, SearchSource::Bridge)),
        // The Bridge could not answer either: report the original SQLite refusal verbatim, so
        // the user still learns the real cause and the documented remedy.
        Ok(None) => Err(refusal),
        Err(err) => Err(err),
    }
}

/// The offline `--all-libraries` search: resolve eligible libraries, then one scoped query.
fn sqlite_all_libraries(
    runtime: &RuntimeContext,
    request: &SearchRequest<'_>,
    include_feeds: bool,
) -> anyhow::Result<Vec<Item>> {
    let libraries = db::fetch_libraries(&runtime.environment.sqlite_path)?;
    let library_ids = eligible_library_ids(
        libraries.iter().map(|l| (l.library_id, l.kind.as_str())),
        include_feeds,
    );
    db::find_items_by_title(
        &runtime.environment.sqlite_path,
        request.query,
        &SearchLibraries::Some(library_ids),
        None,
        request.limit,
        request.exact_title,
    )
}

/// The live counterpart of both scopes. `Ok(None)` means the Bridge declined, so the caller
/// restores the SQLite refusal.
fn live_search(
    runtime: &RuntimeContext,
    bridge: &JSBridgeClient,
    session: &SessionState,
    request: &SearchRequest<'_>,
) -> anyhow::Result<Option<Vec<Item>>> {
    match request.libraries {
        SearchScopeRequest::CurrentLibrary => {
            try_live_current_library(runtime, bridge, session, request)
        }
        SearchScopeRequest::AllLibraries { include_feeds } => {
            let Some(live) = bridge_libraries(bridge) else {
                return Ok(None);
            };
            let library_ids = eligible_library_ids(
                live.iter().map(|l| (l.library_id, l.kind.as_str())),
                include_feeds,
            );
            bridge_search(
                bridge,
                request.query,
                &library_ids,
                None,
                request.scope,
                request.limit,
            )
        }
    }
}

/// The live equivalent of canonical's single-library search. Resolving *which* library to search
/// is itself a SQLite read on the offline path, so this only runs when the session names a
/// numeric library or the Bridge can supply the default -- otherwise it declines and the
/// canonical path (and its honest WAL refusal) takes over.
fn try_live_current_library(
    runtime: &RuntimeContext,
    bridge: &JSBridgeClient,
    session: &SessionState,
    request: &SearchRequest<'_>,
) -> anyhow::Result<Option<Vec<Item>>> {
    let Some(library_id) = live_current_library_id(bridge, session) else {
        return Ok(None);
    };
    // A collection ref must be resolved to a key before the live search can scope by it. That
    // resolution is itself live (`target::resolve_collection`), so it never reaches SQLite here.
    let collection_key = match request.collection_ref {
        Some(collection_ref) => {
            match crate::target::resolve_collection(
                runtime,
                bridge,
                Some(collection_ref),
                session,
                crate::target::Prefer::Bridge,
            ) {
                Ok(collection) => Some(collection.key),
                Err(_) => return Ok(None),
            }
        }
        None => None,
    };
    bridge_search(
        bridge,
        request.query,
        &[library_id],
        collection_key.as_deref(),
        request.scope,
        request.limit,
    )
}

const T_DEFAULT_LIBRARY: &str = r#"
return JSON.stringify({libraryID: Zotero.Libraries.userLibraryID});
"#;

/// The library a live single-library search should target: the session's, when it names one
/// numerically, else the running Zotero's own personal library.
fn live_current_library_id(bridge: &JSBridgeClient, session: &SessionState) -> Option<i64> {
    match &session.current_library {
        Some(Value::Number(n)) => return n.as_i64(),
        Some(Value::String(s)) => {
            if let Ok(id) = s.trim().parse::<i64>() {
                return Some(id);
            }
            // A non-numeric session library (a name) needs SQLite to resolve; decline so the
            // canonical path handles it.
            return None;
        }
        _ => {}
    }
    let code = crate::bridge::templates::render(T_DEFAULT_LIBRARY, &serde_json::json!({})).ok()?;
    parse_bridge_json(bridge.execute_js(&code, 10))?
        .get("libraryID")
        .and_then(Value::as_i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quicksearch_conditions_map_every_supported_scope() {
        assert_eq!(
            quicksearch_condition("titleCreatorYear"),
            "quicksearch-titleCreatorYear"
        );
        assert_eq!(quicksearch_condition("fields"), "quicksearch-fields");
        assert_eq!(
            quicksearch_condition("everything"),
            "quicksearch-everything"
        );
        // Every canonical scope must map to a real Zotero condition name.
        for scope in catalog::SEARCH_SCOPES {
            assert!(quicksearch_condition(scope).starts_with("quicksearch-"));
        }
    }

    /// The live search and library-enumeration templates are read-only by construction. This is
    /// the structural guard that keeps a future edit from turning a search into a write path.
    #[test]
    fn live_templates_contain_no_mutation_verbs() {
        for template in [T_SEARCH_ITEMS, T_LIST_LIBRARIES, T_DEFAULT_LIBRARY] {
            for verb in [
                "saveTx",
                "eraseTx",
                "merge(",
                "trash",
                "removeItem",
                "setField",
                "new Zotero.Item",
            ] {
                assert!(
                    !template.contains(verb),
                    "search template must stay read-only but contains {verb:?}"
                );
            }
        }
    }

    /// This module must never open a SQLite connection of its own.
    ///
    /// Stronger and more durable than grepping for `immutable=1`: every SQLite read here goes
    /// through `db::`, which owns the single `connect_readonly` guard, so there is no second
    /// place where the WAL/busy refusal could be forgotten or worked around.
    #[test]
    fn search_never_opens_its_own_sqlite_connection() {
        // Scan only the production half: this test's own needle list would otherwise match
        // itself.
        let source = include_str!("search.rs");
        let source = source
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .unwrap_or(source);
        for forbidden in [
            "Connection::open",
            "OpenFlags",
            "rusqlite::Connection",
            "mode=ro",
        ] {
            assert!(
                !source.contains(forbidden),
                "search must route every SQLite read through db:: but references {forbidden:?}"
            );
        }
    }

    fn item(title: &str, date_modified: &str, item_id: i64) -> Item {
        let mut record = serde_json::json!({
            "itemID": item_id,
            "key": format!("K{item_id:07}"),
            "libraryID": 1,
            "itemTypeID": 1,
            "typeName": "document",
            "dateAdded": "2026-01-01",
            "dateModified": date_modified,
            "version": 1,
            "title": title,
            "DOI": "",
            "date": null,
            "hasPdf": false,
        });
        record["title"] = Value::String(title.to_string());
        live_item(&record).expect("fixture record is well formed")
    }

    #[test]
    fn eligible_libraries_cover_user_and_group_but_not_feeds_by_default() {
        let libraries = [(1, "user"), (7, "group"), (10, "feed"), (2, "group")];
        assert_eq!(
            eligible_library_ids(libraries.iter().copied(), false),
            vec![1, 2, 7],
            "feeds are excluded by default and ids come back sorted"
        );
        assert_eq!(
            eligible_library_ids(libraries.iter().copied(), true),
            vec![1, 2, 7, 10],
            "--include-feeds adds feed libraries and nothing else"
        );
    }

    #[test]
    fn eligible_libraries_ignore_unknown_library_types() {
        // A library type this build does not know about must not be searched silently.
        let libraries = [(1, "user"), (99, "publications")];
        assert_eq!(
            eligible_library_ids(libraries.iter().copied(), true),
            vec![1]
        );
    }

    #[test]
    fn ordering_matches_the_sqlite_relevance_rules() {
        let mut items = vec![
            item("a study of thousands", "2026-01-01", 1),
            item("Thousands", "2026-01-01", 2),
            item("thousands turn out", "2026-01-01", 3),
            item("thousands turn out", "2026-01-02", 4),
        ];
        order_like_sqlite(&mut items, "Thousands");
        let titles: Vec<(&str, i64)> = items
            .iter()
            .map(|i| (i.title.as_str(), i.item_id))
            .collect();
        assert_eq!(
            titles,
            vec![
                // exact match first
                ("Thousands", 2),
                // then prefix matches, most recently modified first
                ("thousands turn out", 4),
                ("thousands turn out", 3),
                // then position of the match within the title
                ("a study of thousands", 1),
            ]
        );
    }

    #[test]
    fn ordering_is_stable_for_a_fixed_input() {
        let build = || {
            vec![
                item("thousands b", "2026-01-01", 10),
                item("thousands a", "2026-01-01", 11),
                item("Thousands", "2026-01-01", 12),
            ]
        };
        let mut first = build();
        let mut second = build();
        order_like_sqlite(&mut first, "thousands");
        order_like_sqlite(&mut second, "thousands");
        let ids = |v: &[Item]| v.iter().map(|i| i.item_id).collect::<Vec<_>>();
        assert_eq!(ids(&first), ids(&second));
    }

    #[test]
    fn live_item_leaves_the_same_fields_empty_as_the_sqlite_search_path() {
        let parsed = item("Some Title", "2026-01-01", 5);
        // `find_items_by_title` calls `normalize_item(.., include_related = false)`, so a search
        // hit never carries these on the offline path either. Both paths must agree.
        assert!(parsed.fields.is_empty());
        assert!(parsed.creators.is_empty());
        assert!(parsed.tags.is_empty());
        assert_eq!(parsed.note_text, "");
        assert_eq!(parsed.parent_item_id, None);
    }
}
