---
phase: 1
title: "Saved searches on Zotero 10"
status: pending
priority: P1
effort: "2h"
dependencies: []
---

# Phase 1: Saved searches on Zotero 10

## Goal

Make `search list` and `search get` work on Zotero 10, which dropped the
`savedSearchConditions.required` column, without changing the JSON shape on Zotero 9 or earlier.

## Context

- Zotero commit `5ade25f5` (2026-07-15) dropped the column.
- Reproduced live on 10.0.3: `search list` exits 1 with `no such column: required`.
- The failing query is in `db.rs:859`. Both commands read only from SQLite (`catalog.rs:430,444`).
- Upstream PR #9 fills the missing column with `NULL`. Mirror that so the fork stays aligned with
  upstream if the PR merges.

## Files

- Modify: `crates/zotero-cli/src/db.rs`. In `fetch_saved_searches` (around line 833), make
  `SavedSearchCondition.required` (line 111) an `Option<i64>`.
- Modify: any serializer or consumer of `required` that `grep -rn "\.required" src/` finds.

## Steps

1. Probe once per connection with `PRAGMA table_info(savedSearchConditions)` and check for a
   column named `required`.
2. Build the SELECT with `required` when the column exists, or `NULL AS required` when it does not.
   Keep `ORDER BY searchConditionID`.
3. Change the field to `Option<i64>` so it serializes `null` on Zotero 10 and the integer on
   Zotero 9 or earlier. Keep the key present in both cases.
4. Add unit tests in `db.rs`:
   - `saved_searches_read_zotero10_schema_without_required_column` builds the table without
     `required` and expects `required: null`.
   - `saved_searches_keep_required_on_legacy_schema` expects the value to be preserved.
5. Also check `search items`, which has no SQLite path; it should need no change.

## Verification

- `cargo test -p zotero-cli saved_searches`
- The harness rows `search list`, `search get`, and `search get (ambiguous)` stay Exact:
  `python3 harness/capture.py --impl ./target/release/zotero-cli --output harness/current/rust --clean && python3 harness/compare.py harness/golden/python harness/current/rust`
- Live, with Zotero closed: `zotero-cli --json search list` exits 0.

## Risk

The risk is low. The change is purely additive in the SQL, and legacy fixtures cover the Zotero 9
shape.
