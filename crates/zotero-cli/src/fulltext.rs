//! Phase 7 Slice 5: full-text search core, matching `core/jsbridge.py::search_fulltext` /
//! `zotero_cli.py::item_search_fulltext_command`. Backend-only -- no CLI dispatch (`cli.rs`/
//! `lib.rs` are untouched); a later slice wires this into `item search-fulltext`.
//!
//! Delegates entirely to Zotero's live `Zotero.Search` engine (`fulltextContent contains query`)
//! via the JS Bridge -- never Zotero's FTS SQLite tables directly. Frozen live evidence (verified
//! against a real Zotero 10.0.1 instance, fixture item key `EMI3S3GJ` / attachment key
//! `RZ694UHL`): a `fulltextContent` search returns the matching PDF *attachment* item, not its
//! bibliographic parent -- `{"key": "RZ694UHL", "title": "PDF", "date": ""}`. This module MUST
//! NOT resolve results to parents, substitute parent metadata, add snippets/scores, or wait/poll
//! for indexing; it returns exactly what Zotero's search returns.

use serde_json::Value;

use crate::bridge::client::classify_bridge_payload;
use crate::bridge::JSBridgeClient;

/// `zotero_cli.py::item_search_fulltext_command`'s core. The Python CLI layer never passes a
/// `--library` option for this command -- `library_id` is hardcoded to `1` here, exactly
/// reproducing that parity quirk rather than exposing an override this port's callers never had.
///
/// Returns `(payload, is_success)`: `payload` is what a later CLI-integration slice should print
/// as-is (mirrors `emit_js`'s `data`-or-envelope choice), and `is_success` is the exit code
/// classification (`0` vs `1`).
pub fn search_fulltext(bridge: &JSBridgeClient, query: &str, limit: i64) -> (Value, bool) {
    let transport = bridge.search_fulltext(1, query, limit);
    classify_bridge_payload(&transport)
}
