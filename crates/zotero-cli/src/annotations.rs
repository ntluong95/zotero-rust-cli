//! Phase 7 Slice 5: annotation search and retrieval core, matching
//! `core/jsbridge.py::search_annotations` / `get_annotations` and
//! `zotero_cli.py::item_search_annotations_command` / `item_annotations_command`. Backend-only --
//! no CLI dispatch (`cli.rs`/`lib.rs` are untouched); a later slice wires these into
//! `item search-annotations` / `item annotations`.
//!
//! Both operations are read-only and delegate to Zotero's live APIs (`Zotero.Search` and
//! `Item.getAnnotations()`) via the JS Bridge -- never Zotero's FTS SQLite tables directly.

use serde_json::Value;

use crate::bridge::client::classify_bridge_payload;
use crate::bridge::JSBridgeClient;

/// `zotero_cli.py::item_search_annotations_command`'s core. `library_id` is hardcoded to `1`,
/// matching Python's CLI layer (no `--library` option exists for this command).
///
/// Query semantics: an empty `query` searches `itemType is 'annotation'`; a non-empty `query`
/// searches `annotationText contains query` -- `annotationComment` is never searched. When
/// `colors` is non-empty, results are filtered to annotations whose `annotationColor` is in that
/// list *before* the `limit` slice is applied (matches Python's `filtered.slice(0, limit)`
/// ordering, not filter-after-slice).
///
/// Returns `(payload, is_success)` -- see `fulltext::search_fulltext` for the contract.
pub fn search_annotations(
    bridge: &JSBridgeClient,
    query: &str,
    colors: Option<&[String]>,
    limit: i64,
) -> (Value, bool) {
    let transport = bridge.search_annotations(1, query, colors, limit);
    classify_bridge_payload(&transport)
}

/// `zotero_cli.py::item_annotations_command`'s core. `library_id` is hardcoded to `1`, matching
/// Python's CLI layer (no `--library` option exists for this command).
///
/// `item_key` accepts a raw item key only -- no separate "attachment vs. bibliographic parent"
/// input distinction at this layer. If the resolved item is itself a PDF attachment, the
/// underlying JS walks up to its bibliographic parent before collecting annotations from all of
/// the parent's PDF attachments; a missing parent is its own `"ERROR: ..."` payload. Per-PDF
/// `getAnnotations()` errors are swallowed individually so one bad attachment never fails the
/// whole call.
///
/// A not-found item, or an attachment with no resolvable parent, yields a bare `"ERROR: ..."`
/// string as `payload` with `is_success == true` -- this reproduces Python's `emit_js` quirk
/// where a JS-level error *string* is still a transport-level success (exit code `0`), as opposed
/// to a transport failure or an application-level `{"ok": false, ...}` object.
pub fn get_annotations(bridge: &JSBridgeClient, item_key: &str) -> (Value, bool) {
    let transport = bridge.get_annotations(1, item_key);
    classify_bridge_payload(&transport)
}
