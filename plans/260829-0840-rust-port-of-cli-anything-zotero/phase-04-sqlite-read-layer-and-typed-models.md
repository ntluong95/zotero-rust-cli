---
phase: 4
title: "SQLite Read Layer and Typed Models"
status: complete
priority: P1
effort: "5-7d"
dependencies: [3]
---

# Phase 4: SQLite Read Layer and Typed Models

## Overview

Port the read half of `utils/zotero_sqlite.py` (~600 of its 782 lines) and the read facade
`core/catalog.py`. Delivers the 24 SQLite-backed commands — the bulk of the **Exact** compatibility
class and the most frequently used commands in agent workflows.

### Progress via the vertical slice (Phase 3 slice 1, this slice 2)

**23 of this phase's 24 commands are now landed and verified Exact.** Only `session use-selected`
remains (it needs `connector/getSelectedCollection`, an HTTP call — arguably Phase 5 scope by
dependency, kept here only because the original command inventory placed it in this phase's list).

- **Slice 1** (`phase-03`'s vertical slice): `item list`, `item get`, `item find`, `collection list`.
- **Slice 2** (this update): `library list`, `collection find/get/items/tree`, `item children/notes/
  attachments/file`, `search list/get/items`, `tag list/items`, `style list`.

All SQL and normalization logic lives in `crates/zotero-cli/src/db.rs` (flat, not yet split into
`db/mod.rs` + `db/read.rs` + `db/models.rs` as sketched below — split only if/when this becomes
unwieldy; at ~1000 lines with two full slices in it, still readable). New this slice: `find_collections`,
`build_collection_tree`, `fetch_item_children/notes/attachments`, `resolve_attachment_real_path`,
`fetch_saved_searches`, `resolve_saved_search`, `fetch_tags`, `fetch_tag_items` — plus, in
`catalog.rs`, the domain wrappers for all of the above, `list_libraries`, `get_search`, `search_items`
(Local API passthrough), and `list_styles` (real namespace-aware CSL XML parse via `quick-xml`,
added as a genuine new dependency rather than a regex heuristic — see its module doc comment).

**`get_collection`'s signature was corrected mid-slice**, not left as originally shipped: the Phase 3
version took a required `&str` and skipped Python's `ref: None -> session.current_collection ->
error` fallback, because its only caller at the time (`find_items`) always supplied `Some`. Once
`collection get`/`collection items` needed that fallback too, `get_collection` was changed to
`Option<&str>` with the fallback moved inside — matching `get_item`'s already-correct, established
pattern, and Python's own `catalog.py:68-80` — rather than duplicating the fallback logic at each new
call site.

**Verification beyond the golden fixtures (repeating the Phase 3 slice's discipline, not skipping it
because the first attempt was green):** all 15 new commands classified Exact on the first harness run
against every existing fixture — which, per the Phase 3 slice's own lesson, proves the happy path, not
correctness. A direct check of what those fixtures actually exercise found real gaps in the two riskiest
new functions:

- `resolve_attachment_real_path` (the highest cross-platform risk area in this phase, per the
  Architecture section below) — the only fixture item with a `storage:` attachment path is queried by
  every `item file`/`item attachments` golden row; the item with a `file:///C:/...` drive-letter path
  is a *different* item that no fixture command ever queries, and no fixture attachment uses a
  non-localhost `file://` host at all. **4 of 6 branches had zero coverage from a green harness run.**
  Closed with 8 direct unit tests in `db.rs` covering all six branches, including the UNC and
  drive-letter cases synthetically.
- `build_collection_tree`'s orphan-root case (a `parentCollectionID` pointing outside the result set,
  e.g. filtered by library scope) — the only nested collection in the fixture data has its parent in
  the same result set, so the "orphan becomes a root" branch (`zotero_sqlite.py:216-219`) was
  untested. Closed with 2 direct unit tests.
- `resolve_saved_search`'s ambiguous-reference error path — no existing fixture queried a saved-search
  key that's duplicated across libraries (`DUPSEARCH` exists for both). Manually verified against the
  live Python reference first (`search get DUPSEARCH` → exit 1, exact error text), then added as a new
  standing golden fixture (`harness/commands.tsv` row 99, `search get (ambiguous)`) rather than left as
  an ad-hoc check — now part of the permanent parity suite, not one-off verification.

**Known gap, not yet closed:** `resolve_collection`'s own pre-existing ambiguous-key path (duplicate
collection keys across libraries, e.g. `DUPCOLL1`) has the same kind of untested-ambiguity gap as
`resolve_saved_search` had — this is Phase 3 slice 1's code, not new this slice, but flagged here
since it was found while auditing the sibling function. Not fixed in this pass; tracked for the next
hardening round rather than silently left implicit.

**Cross-platform verification closed the same day:** pushed as `05b4649`; CI run `33249509720`
completed green on all 5 targets, including `x86_64-pc-windows-msvc`. The 8 new
`resolve_attachment_real_path` branch tests — including the UNC and drive-letter branches — are now
genuinely verified on real Windows, not just unit-tested on macOS and assumed portable.

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

### Connection semantics (D4) — **SUPERSEDED by Phase 14, see note**

> **[RECONSTRUCTED 2026-08-29 — not a verbatim recovery.]** The paragraphs below replace this
> section's original `--strict-read`-based decision, which is cancelled. They are rebuilt from
> `plan.md`'s D4 entry and `phase-14-zotero-10-compatibility-gate.md` §1 (both recovered verbatim
> from session context), not from this file's own lost diff — the exact original wording of this
> section's Zotero-10 update is not recoverable. Treat `phase-14-zotero-10-compatibility-gate.md` as
> the authoritative spec for `connect_readonly`; this section is a pointer, not a duplicate source of
> truth.

Python opens `file:{path}?mode=ro&immutable=1` with a 1 s timeout
(`zotero_sqlite.py:25-32`). `immutable=1` tells SQLite the file cannot change, so it skips WAL
recovery and locking entirely. That is what lets reads succeed while Zotero holds the database on
Zotero ≤9 (rollback-journal mode) — and on a rollback-journal database that is a safe trade, since
there is no separate `-wal` file for it to miss.

**Zotero 10 defaults to WAL journal mode**, which changes the trade-off's shape entirely: reads
against a WAL database opened with `immutable=1` can silently return **only the last-checkpointed
snapshot**, missing any committed-but-uncheckpointed row with no error and exit code 0 (reproduced:
1 of 5 rows returned). This is `plan.md`'s Defect D4, superseded — see there for the full writeup —
and it retroactively affects every one of this phase's 24 already-landed read commands, since they
all go through `connect_readonly`.

**Original decision here (`--strict-read` opt-in flag) is CANCELLED.** It assumed the only cost of
dropping `immutable=1` was "fails under lock contention instead of returning inconsistent data" —
i.e. that WAL's normal reader/writer concurrency model would apply. Phase 14 must verify this
empirically against a live Zotero 10 before it can be assumed (see Phase 14's Open Question 1 — this
is flagged there as the single highest-risk unknown in the whole Zotero 10 adaptation, precisely
because SQLite's WAL concurrency guarantees do not automatically survive a writer that holds its own
connection in exclusive locking mode). The fix — dropping `immutable=1` unconditionally to `mode=ro`
as the *default*, with no flag to opt into — is owned by **Phase 14**, executes before Phase 6, and
retro-fixes this phase's landed code without changing its command surface. Do not reintroduce
`--strict-read`; there is nothing left to opt into once `mode=ro` is the default.

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
      (**23/24** — only `session use-selected` remains, blocked on Phase 5's connector HTTP client)
- [ ] `hasPdf` serializes as bool; `DOI` null→`""`; empty `fields`/`creators`/`tags` present not omitted
- [ ] Ambiguous-key resolution reproduces Python's message and behaviour on the group-library fixture
- [ ] `resolve_attachment_real_path` passes all six branches on macOS, Windows and Linux
- [ ] `note_preview` truncation is character-identical, including the `…` and the off-by-one
- [ ] Reads succeed against a **live, running** Zotero with a 112 MB database
- [ ] ~~`--strict-read` fails cleanly under lock contention instead of returning torn data~~ **cancelled — superseded by Phase 14's `mode=ro`-by-default fix; see Connection semantics (D4) above**
- [ ] Local-API/SQLite mixed-surface stale-read behavior has an explicit fixture and does not produce a silent false-negative result
- [ ] Query latency within 2× of the Python baseline
- [ ] `immutable=1` / WAL trade-off documented in `docs/ZOTERO-COMPATIBILITY.md` (Phase 14) and surfaced by `app doctor`

## Risk Assessment

| Risk | Mitigation |
|---|---|
| SQL transcription errors in the large `_base_item_select` | Copy verbatim; add a test comparing column names and row counts against Python on the same fixture |
| Windows path branches wrong | Dedicated six-branch test table run on the Windows CI runner |
| HTML entity coverage differs from `html.unescape` | Property test over the HTML5 named-entity list |
| `COLLATE NOCASE` ordering differs between bundled SQLite and system SQLite | `rusqlite bundled` pins the SQLite version; assert ordering in tests |
| Typed models omit fields Python emits as null | Golden comparison is strict on key presence, so this is caught by the harness |
| Reading a live DB during Zotero writes yields flaky test results | Live tests assert schema and types, not exact row content |
