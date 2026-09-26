---
phase: 2
title: "Live read fallback for catalog reads"
status: pending
priority: P1
effort: "1.5d"
dependencies: []
---

# Phase 2: Live read fallback for catalog reads

## Goal

When Zotero 10 is running and holds the WAL lock, serve the catalog reads ASK depends on through
the owned Bridge. Use exactly the SQLite-first, retry-on-`DatabaseLocked` pattern that
`search.rs` already uses for `item find` and `library list`.

## Context

These commands were live-verified to refuse on Zotero 10.0.4 while Zotero is running:

`session use-library`, `collection list/find/get/items`, `item get/children/notes/attachments`,
`note get`, `tag list`, and `item context`.

Four more share the same resolvers and are fixed with them: `item list`, `item file`,
`collection tree`, and `tag items`.

`docs/ZOTERO-COMPATIBILITY.md` records the underlying "Layer B" routing as specified but never
wired.

## Design

- New module `crates/zotero-cli/src/live_catalog.rs` exposes one entry point per read, for
  example:

  ```rust
  pub fn collection_items(runtime, bridge, ...) -> anyhow::Result<(Vec<Item>, SearchSource)>
  ```

  Each entry point tries SQLite first, returns early unless the error is
  `db::is_database_locked`, and then calls a read-only Bridge template. When the Bridge cannot
  answer, it returns the **original refusal** unchanged.
- `catalog.rs` and `session.rs` callers route through `live_catalog`. For the offline path, keep
  `catalog.rs` as the SQLite implementation.
- Library resolution while locked goes through `search::bridge_libraries`: `resolve_library_id`,
  `default_library`, and `local_api_scope`'s library-kind lookup. Extract a shared
  `resolve_library_live()` helper.
- Build the result shapes from Bridge data with the same struct constructors as the SQLite path:
  `Item`, `Collection`, `Tag`, and the note and attachment payloads. Where a field is unavailable
  live, leave it empty exactly as `search::live_item` already does, and document each gap in a
  test.
- Use the session library when it is set. Otherwise use Zotero's `Zotero.Libraries.userLibraryID`,
  mirroring `live_current_library_id`.

## Files

- Create: `crates/zotero-cli/src/live_catalog.rs`.
- Create: read-only templates in `crates/zotero-cli/src/bridge/js/`:
  - `live_collections.js`: list, find, get, and tree for one library
  - `live_collection_items.js`
  - `live_item.js`: get plus children, notes, attachments, and file path
  - `live_note.js`
  - `live_tags.js`: list and items
- Modify: `crates/zotero-cli/src/bridge/templates.rs` to add `render_live_*` functions with
  JSON-serialized params only (see line 63).
- Modify: `crates/zotero-cli/src/catalog.rs`, `session.rs`, `notes.rs`, `analysis.rs`
  (`build_item_context`), `lib.rs`, and the dispatch arms, which must pass the Bridge client.
- Modify: `crates/zotero-cli/src/search.rs` to move `bridge_libraries` and `parse_bridge_json`
  where `live_catalog` can share them. Change visibility only; no behaviour change.

## Steps

1. Extract the library resolution helper and add the `session use-library` fallback first. This is
   the smallest change and unblocks ASK's isolated-session flow.
2. Add the collection reads, then the item reads, notes, tags, and `item context`.
3. Extend the existing `live_templates_contain_no_mutation_verbs` test to cover every new
   template. Forbidden verbs are `saveTx`, `eraseTx`, `merge`, `trash`, `setField`, and
   `addToCollection`.
4. Add a unit test per command for three cases:
   - (a) the offline path issues no Bridge call;
   - (b) a locked database with a healthy Bridge produces the Bridge result with an identical shape;
   - (c) a locked database with no Bridge returns the original refusal text verbatim.

   Use the fixtures in `tests/common`, with the WAL-locked setup already used by the
   `db.rs` locking tests.
5. Add an integration test (`tests/live_read_fallback.rs`) that uses the fake Bridge from
   `bridge_transport.rs` to cover the ASK command set end to end.

## Verification

- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- Harness: every read row stays Exact or Semantic. The offline path must not change.
- Live check on Zotero 10.0.4 **running**: every command in the ASK review §3.1 table exits 0.
  Run the same commands with Zotero **closed** and diff the outputs; key sets must match.

## Risk

- **Shape drift between the Bridge and SQLite outputs.** Mitigation: tests (b) and (c), plus the
  closed/open diff in the live check.
- **Performance of large collections over the Bridge.** Mitigation: page inside the template and
  respect `--limit`.
- **Rollback:** revert the dispatch arms. SQLite behaviour is untouched throughout.
