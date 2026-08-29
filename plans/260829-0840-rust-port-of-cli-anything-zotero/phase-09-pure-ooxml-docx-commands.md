---
phase: 9
title: "Pure OOXML DOCX Commands"
status: todo
priority: P2
effort: "4-6d"
dependencies: [4]
---

# Phase 9: Pure OOXML DOCX Commands

## Overview

Port the four DOCX commands that require **no external process** — no LibreOffice, no Java, no
AppleScript. These are `zipfile` + `ElementTree` only and are genuinely portable.

The other seven DOCX commands are deferred to Phase 12 behind their own gate.

## Requirements

**Functional**
- `docx inspect-citations` — detect citation field systems and static citation text
- `docx inspect-placeholders` — find `{{zotero:KEY}}` placeholders
- `docx validate-placeholders` — resolve placeholder keys against the local library
- `docx render-citations` — static text citation rendering

**Non-functional**
- Output DOCX must open cleanly in Word and LibreOffice
- Structural comparison, never byte comparison, against Python output

## Architecture

```
crates/zotero-cli/src/
  docx/
    mod.rs
    package.rs       # OPC zip read/write, part management
    xml.rs           # quick-xml helpers, namespace handling
    inspect.rs       # inspect_citations, inspect_placeholders
    validate.rs      # validate_placeholders
    static_render.rs # render_static_citations
    working.rs       # build_working_docx
```

### Scope boundary (verified, not assumed)

Derived by static analysis of subprocess usage per function:

| Function | External process? | Phase |
|---|---|---|
| `inspect_citations` | no | **9** |
| `inspect_placeholders` | no | **9** |
| `validate_placeholders` | no | **9** |
| `render_static_citations` | no | **9** |
| `build_working_docx` | no | **9** (helper) |
| `zoterify_preflight` | yes | 12 |
| `prepare_zotero_import_document` | yes | 12 |
| `cite_document` | yes | 12 |
| `zoterify_probe` / `zoterify_doctor` / `zoterify_document` | yes | 12 |

### Why byte-comparison is the wrong test

Python registers namespace prefixes globally (`docx.py:33-35`) and `ElementTree` re-serializes the
whole document part on write. `quick-xml` will produce different — but equally valid — XML:
attribute order, self-closing tag style, namespace prefix placement, and whitespace will differ.

**Parity is defined structurally:**
- same set of OPC parts, same content types
- same relationship graph (`.rels` targets and types)
- semantically equivalent `document.xml`: same paragraph and run sequence, same visible text, same
  field instructions, same bookmarks, same custom properties
- output opens without repair prompts in Word and LibreOffice

For the three inspect/validate commands the output is **JSON**, not DOCX, so those are compared
normally against golden output and can reach a stricter class.

### Patterns to port carefully

| Pattern | Source | Note |
|---|---|---|
| `_PLACEHOLDER_RE` | `\{\{\s*zotero\s*:\s*([^}]*)\s*\}\}` (case-insensitive) | Straightforward in Rust `regex` |
| `_ZOTERO_KEY_RE` | `^[A-Z0-9]{8}$` | Trivial |
| `_AUTHOR_YEAR_RE` | Nested alternation with `&`/`and`/`et al.` | Most complex; no backtracking required — verify it compiles under Rust `regex` before assuming |
| `_NUMERIC_RE` | `\[(?:\d+(?:\s*[-,]\s*\d+)*)\]` | Fine |
| `_ZOTERO_BOOKMARK_RE`, `_ZOTERO_CUSTOM_PROP_RE` | Bookmark/property naming | Fine |

Placeholder text can be **split across multiple `w:r` runs** by Word (a `{{zotero:ABCD1234}}` may
be broken into several runs by spell-check or formatting marks). Python works on the concatenated
visible text via `_visible_text(root)`. The Rust port must reproduce that concatenation before
matching, and must handle re-splitting correctly when rewriting.

### Validation depends on the library

`validate_placeholders` calls `catalog.get_item` per unique key and reports `items`, `missing_keys`
and `errors` — so this command needs Phase 4, which is its only dependency.

## Related Code Files

- Create: `src/docx/mod.rs`, `package.rs`, `xml.rs`, `inspect.rs`, `validate.rs`, `static_render.rs`, `working.rs`
- Create: `tests/docx_inspect.rs`, `tests/docx_render.rs`, `tests/docx_structural.rs`
- Create: `harness/fixtures/docx/` — corpus of real `.docx` files
- Reference: `core/docx.py` (lines 38–150), `core/docx_static.py`, `core/docx_zoterify.py` (`build_working_docx` only)

## Implementation Steps

1. **Build the fixture corpus first.** Collect real `.docx` files covering: no citations; Zotero
   field citations; Mendeley/EndNote fields; `ZOTERO_BREF_` bookmarks; static author-year text;
   numeric citations; valid placeholders; invalid placeholders; placeholders split across runs;
   CJK content. Without this corpus the phase cannot be verified.
2. Implement `package.rs` — read OPC zip, expose parts by name, write back preserving unmodified
   parts **byte-for-byte** (only rewrite parts that actually change).
3. Implement `xml.rs` namespace-aware helpers over `quick-xml`.
4. Port `inspect_citations`: field instruction extraction, Zotero bookmark reports, system counting,
   visible-text extraction, static-citation matching, notes generation.
5. Port `inspect_placeholders`: matching, key parsing, invalid detection, context extraction,
   duplicate counting.
6. Port `validate_placeholders` on top of Phase 4's `catalog`.
7. Port `render_static_citations` and `build_working_docx`.
8. Write the structural comparator used by `docx_structural.rs`.
9. Run the three JSON-output commands through the Phase 1 harness.

## Success Criteria

- [ ] `docx inspect-citations`, `inspect-placeholders`, `validate-placeholders` match golden JSON output
- [ ] `docx render-citations` output passes structural comparison against Python output on every corpus file
- [ ] Every rendered output opens in Word **and** LibreOffice with no repair prompt
- [ ] Unmodified OPC parts are byte-identical to the input archive
- [ ] Placeholders split across multiple `w:r` runs are detected and rewritten correctly
- [ ] CJK content round-trips without mojibake
- [ ] All five regex patterns compile under Rust `regex` and match Python's results on the corpus
- [ ] No subprocess is spawned by any command in this phase (asserted by test)

## Risk Assessment

| Risk | Mitigation |
|---|---|
| Corpus too small to be meaningful | Corpus construction is step 1 and a gating deliverable, not an afterthought |
| `quick-xml` round-trip corrupts unmodified parts | Only rewrite changed parts; copy the rest verbatim from the source zip |
| Run-splitting logic loses formatting | Structural comparator asserts run-level properties (`w:rPr`) are preserved |
| `_AUTHOR_YEAR_RE` needs backtracking | Verify at step 1; fall back to `fancy-regex` **only** if genuinely required, and record why |
| Word repairs the file silently on open | Manual open test on both Word and LibreOffice is an explicit success criterion |
| Scope creep into Phase 12 | The function table above is the contract; any function invoking a subprocess belongs to Phase 12 |
