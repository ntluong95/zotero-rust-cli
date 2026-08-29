---
phase: 4
title: "SQLite Read Layer and Typed Models"
status: todo
priority: P1
effort: "5-7d"
dependencies: [3]
---

# Phase 4: SQLite Read Layer and Typed Models

## Overview

Port the read half of `utils/zotero_sqlite.py` (~600 of its 782 lines) and the read facade
`core/catalog.py`. Delivers the 24 SQLite-backed commands — the bulk of the **Exact** compatibility
class and the most frequently used commands in agent workflows.

## Requirements

**Functional**
- All read queries: libraries, collections, collection tree, items, item fields/creators/tags,
  notes, attachments, annotations, saved searches, tags, tag-linked items
- Library-aware reference resolution with `AmbiguousReferenceError` semantics
- Attachment real-path resolution including Windows UNC and drive-letter forms
- Note HTML→text conversion and preview truncation
- CSL style listing from `styles/*.csl`
- Delivers: `library list`, `collection list|find|get|items|tree`, `item list|find|get|children|notes|attachments|file`, `search list|get`, `tag list|items`, `style list`, `session use-selected`

**Non-functional**
- Read connection must not block or corrupt a running Zotero
- Query latency within 2× of Python's measured 0.06–1.93 ms

## Architecture

```
crates/zotero-cli/src/
  db/
    mod.rs
    read.rs          # port of zotero_sqlite.py read functions
    models.rs        # typed structs, serde
    note_html.rs     # note_html_to_text, note_preview
    attach_path.rs   # resolve_attachment_real_path
  catalog.rs         # port of core/catalog.py
```

### Connection semantics (D4)

Python opens `file:{path}?mode=ro&immutable=1` with a 1 s timeout
(`zotero_sqlite.py:25-32`). `immutable=1` tells SQLite the file cannot change, so it skips WAL
recovery and locking entirely. That is what lets reads succeed while Zotero holds the database —
and it is also why reads can observe stale or torn state during a Zotero write.

**Decision: preserve the default behaviour** (removing it would break the core use case), **but**:
- document it in `docs/ARCHITECTURE.md` and in `app doctor` output;
- add an opt-in `--strict-read` global flag that opens `mode=ro` **without** `immutable=1`, which
  will fail cleanly under lock contention rather than returning inconsistent data.

`--strict-read` is a new flag, additive, and does not change any default — it is compatible.

### Typed models

Replace Python's `dict[str, Any]` rows with typed structs, but the **serialized JSON must match
exactly**, including these normalizations from `_normalize_item` (`zotero_sqlite.py:388-404`):

| Field | Rule |
|---|---|
| `hasPdf` | SQLite `EXISTS` returns 0/1 → serialize as **bool** |
| `DOI` | `NULL` → `""` (empty string, not null) |
| `fields`, `creators`, `tags` | `{}` / `[]` when `include_related` is false — present but empty, never omitted |
| `isAttachment`, `isNote`, `isAnnotation` | Derived from `typeName` |
| `parentItemID` | `COALESCE(attachment, note, annotation)` parent |
| `noteText`, `notePreview` | Always present, `""` for non-notes |

`resolve_item` uses `include_related = true`; `fetch_items` and `find_items_by_title` use `false`.
Getting this wrong changes the JSON shape for every list command.

### Note HTML → text

Port `note_html_to_text` (`zotero_sqlite.py:85-95`) in order:
`<br>`→`\n`, `</p>`→`\n\n`, `</div>`→`\n`, strip all tags, HTML-unescape, normalize CRLF, collapse
3+ newlines to 2, trim.

`note_preview` truncates to 160 chars and appends `…` (U+2026) after right-trimming — off-by-one
sensitive: `text[: max(0, limit - 1)].rstrip() + "…"`.

Use a maintained HTML-entity crate for unescaping; Python's `html.unescape` handles the full HTML5
named-entity table including entities without trailing semicolons.

### Attachment path resolution

`resolve_attachment_real_path` (`zotero_sqlite.py:552-574`) has four branches:

| Prefix | Behaviour |
|---|---|
| `storage:` | `data_dir/storage/<item.key>/<filename>` |
| `file://` with non-localhost netloc | UNC: `\\host\path` with `/`→`\` |
| `file://` with `/C:/...` | Strip leading `/`, treat as Windows path |
| `file://` otherwise | Percent-decoded POSIX path |
| bare absolute | as-is |
| bare relative | `data_dir/<path>` |

This is the highest cross-platform risk in the phase. It needs its own test table covering all six
branches on all three OSes.

### Local API preference in `find_items`

`catalog.find_items` (`catalog.py:98-144`) prefers the Local API when available and not doing an
exact-title search, then **re-resolves each returned key against SQLite**, and falls back to SQLite
title search when the API returns nothing. Both paths must be ported; the Local API client arrives
in Phase 5, so implement the SQLite path now behind a trait and wire the API path in Phase 5.

### Mixed-surface stale-read failure

`immutable=1` means SQLite may not observe the freshest Zotero writes, while the Local API can return
fresh item keys immediately. When Phase 5 wires Local API search, do not silently discard a non-empty
Local API response just because every key failed SQLite re-resolution. Return a diagnostic partial
result or fall back with a warning field that records `local_api_count`, `sqlite_resolved_count`, and
the unresolved keys. Add a fixture where Local API returns a just-created key absent from the SQLite
snapshot.

## Related Code Files

- Create: `src/db/mod.rs`, `read.rs`, `models.rs`, `note_html.rs`, `attach_path.rs`
- Create: `src/catalog.rs`
- Create: `tests/db_read.rs`, `tests/attach_path.rs`, `tests/note_html.rs`
- Reference: `reference/.../utils/zotero_sqlite.py` (lines 1–652), `core/catalog.py`

## Implementation Steps

1. Add `rusqlite` (already vendored in Phase 2). Implement `connect_readonly` with the exact URI and
   the `--strict-read` variant.
2. Port `_base_item_select()` verbatim — it is a large correlated-subquery SELECT; transcribe the SQL
   as a Rust constant rather than reconstructing it.
3. Implement `models.rs` with the normalization table above.
4. Port library/collection functions, including `build_collection_tree` (orphan parents become roots).
5. Port item functions: `fetch_items`, `find_items_by_title`, `resolve_item`, children/notes/attachments.
6. Implement `AmbiguousReferenceError` → a `CliError` variant producing the same message text:
   `"Ambiguous {kind} reference: {ref}. Matches found in L1, L2. Set the library with \`session use-library <id>\` and retry."`
7. Port `note_html.rs` and `attach_path.rs` with their dedicated test tables.
8. Port saved searches (including nested `conditions`) and tags.
9. Port `catalog.rs` including `_default_library` resolution order (session → `default_library_id`).
   Leave a documented integration point for Phase 5 to report Local-API/SQLite re-resolution gaps.
10. Port `list_styles` — parse `*.csl` XML for `id` and `title`, emit `{path, id, title, valid}`,
    with `valid: false` and `title: <stem>` on parse failure.
11. Wire all 24 commands; run the Phase 1 harness and drive them to Exact.

## Success Criteria

- [ ] All 24 SQLite-backed commands classified **Exact** against the Phase 1 golden outputs
- [ ] `hasPdf` serializes as bool; `DOI` null→`""`; empty `fields`/`creators`/`tags` present not omitted
- [ ] Ambiguous-key resolution reproduces Python's message and behaviour on the group-library fixture
- [ ] `resolve_attachment_real_path` passes all six branches on macOS, Windows and Linux
- [ ] `note_preview` truncation is character-identical, including the `…` and the off-by-one
- [ ] Reads succeed against a **live, running** Zotero with a 112 MB database
- [ ] `--strict-read` fails cleanly under lock contention instead of returning torn data
- [ ] Local-API/SQLite mixed-surface stale-read behavior has an explicit fixture and does not produce a silent false-negative result
- [ ] Query latency within 2× of the Python baseline
- [ ] `immutable=1` trade-off documented in `docs/ARCHITECTURE.md` and surfaced by `app doctor`

## Risk Assessment

| Risk | Mitigation |
|---|---|
| SQL transcription errors in the large `_base_item_select` | Copy verbatim; add a test comparing column names and row counts against Python on the same fixture |
| Windows path branches wrong | Dedicated six-branch test table run on the Windows CI runner |
| HTML entity coverage differs from `html.unescape` | Property test over the HTML5 named-entity list |
| `COLLATE NOCASE` ordering differs between bundled SQLite and system SQLite | `rusqlite bundled` pins the SQLite version; assert ordering in tests |
| Typed models omit fields Python emits as null | Golden comparison is strict on key presence, so this is caught by the harness |
| Reading a live DB during Zotero writes yields flaky test results | Live tests assert schema and types, not exact row content |
