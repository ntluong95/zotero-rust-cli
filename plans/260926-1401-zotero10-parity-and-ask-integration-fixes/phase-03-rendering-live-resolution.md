---
phase: 3
title: "Rendering and export on Zotero 10"
status: pending
priority: P1
effort: "4h"
dependencies: [2]
---

# Phase 3: Rendering and export on Zotero 10

## Goal

Make `item citation`, `item bibliography`, `item export`, and `export bib` work with Zotero 10
both open and closed.

## Context

- **Closed:** these commands call `build_runtime()` (for example `lib.rs:737,749`) instead of
  `live_runtime(lifecycle::Backend::LocalApi)`. They fail with "Local API is not available" and
  never auto-launch Zotero.
- **Open:** item resolution goes through SQLite and hits the WAL-lock refusal before the Local API
  is ever called.
- The Local API returned HTTP 200 for `items/<key>?format=bibtex` while Zotero was running.
- `catalog::local_api_scope` builds `/api/groups/{libraryID}`. The Local API expects the
  **groupID**, which is a different number from the libraryID.

## Files

- Modify: `crates/zotero-cli/src/lib.rs`, in the dispatch arms for `ItemCommands::Export`,
  `Citation`, and `Bibliography`, and `ExportCommands::Bib`.
- Modify: `crates/zotero-cli/src/rendering.rs` (`require_local_api`, `fetch_render_payload`, and
  item resolution).
- Modify: `crates/zotero-cli/src/catalog.rs` (`local_api_scope`).
- Modify: `crates/zotero-cli/src/db.rs` if `Library` does not yet carry `groupID`. Read it from the
  `groups` table in SQLite, or from `Zotero.Groups.getGroupIDFromLibraryID` over the Bridge.

## Steps

1. Switch the four dispatch arms to `live_runtime(lifecycle::Backend::LocalApi)?` so that they
   auto-launch Zotero and respect `ZOTERO_CLI_NO_AUTOLAUNCH`.
2. Resolve items and collections through `live_catalog` from Phase 2 instead of SQLite directly.
3. Fix `local_api_scope` for group libraries so it uses the groupID. Add a unit test in which the
   libraryID and groupID differ, and live-verify against one group library.
4. Keep the `{"error": ...}` payload shape and exit code 1 for real failures.

## Verification

- `cargo test -p zotero-cli rendering`, and the `rendering.rs` integration tests.
- Live, with Zotero **closed**: `zotero-cli --json item citation <KEY>` launches Zotero and exits 0.
  With `ZOTERO_CLI_NO_AUTOLAUNCH=1` set, it exits 1 with the explicit message from `lifecycle.rs`.
- Live, with Zotero **open**:
  - `item export <KEY> --format bibtex` exits 0.
  - `export bib --items <KEY> --output /tmp/x.bib` writes a non-empty file.
  - `item bibliography` works for one group-library item.

## Risk

The group-scope change alters which URL is requested. The harness `http_calls` rows for rendering
use user libraries only, so re-check that they stay Exact.
