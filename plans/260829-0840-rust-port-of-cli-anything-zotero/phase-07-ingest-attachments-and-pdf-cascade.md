---
phase: 7
title: "Ingest, Attachments and PDF Cascade"
status: todo
priority: P1
effort: "6-8d"
dependencies: [6]
---

# Phase 7: Ingest, Attachments and PDF Cascade

## Overview

Port the write/ingest surface: connector-mediated imports, the unified `add` entrypoints, the
open-access PDF cascade, notes, CSL-JSON conversion, citation metrics and OpenAI-backed analysis.

This is the largest remaining block (~2,600 LOC across `imports.py`, `add.py`, `pdf_fetch.py`,
`notes.py`, `csl.py`, `analysis.py`, `metrics.py`, `openai_api.py`) and has the subtlest semantics —
particularly partial-success accounting.

## Requirements

**Functional**
- `import file|json|doi|pmid` with attachment manifests, inline attachments and dedupe
- `add doi|arxiv|url|bibtex|file` with `--if-exists file|skip|duplicate`
- OA PDF cascade: Unpaywall → Europe PMC → bioRxiv/medRxiv → arXiv, with resume state
- `note get|add`
- `item metrics` (NIH iCite), `item context`, `item analyze` (OpenAI-compatible)
- CSL-JSON / Crossref / connector-format normalization

**Non-functional**
- Partial success must produce exit code 1 and a complete recovery report
- Preserve serial batch behavior for v1 parity — no `tokio`, no pre-parity concurrency flag

## Architecture

```
crates/zotero-cli/src/
  imports.rs       # connector import, manifests, inline attachments
  add.rs           # unified ingest entrypoints
  pdf_fetch.rs     # OA cascade + resume state
  notes.rs
  csl.rs
  analysis.rs      # item context + item analyze
  metrics.rs       # NIH iCite
  openai.rs        # OpenAI-compatible chat
```

### Partial-success and recovery contract

The most delicate behaviour in the phase. `import json` / `import file` can succeed at item creation
but fail later during `updateSession` or attachment upload. Python reports attachment failure as
`status: "partial_success"`, which `exit_code_for` maps to **exit 1**, but `updateSession` failure
can happen after Zotero has already created items and before the CLI has attached or reported enough
recovery context.

Agents branch on this. The rules:

| Outcome | `ok` | `status` | Exit |
|---|---|---|---|
| All items and attachments succeed | true | `success` | 0 |
| Items created, some attachments failed | true | `partial_success` | **1** |
| Items created, `updateSession` failed | true | `partial_success` | **1** |
| No items created | false | `error` | 1 |
| Item already present, `--if-exists skip` | true | `already_exists` | 0 |

Recovery detail must include `sessionID`, imported item keys/titles when known, requested target,
requested tags, failed step, and per-attachment failure detail (index, title, reason). Add a fixture
where `connector/import` or `saveItems` succeeds and `connector/updateSession` returns non-200.

### PDF cascade

`pdf_fetch.py` tries sources in order, controlled by `--pdf-sources`
(default `zotero,unpaywall,epmc,biorxiv,arxiv`):

| Source | Endpoint |
|---|---|
| `zotero` | Zotero's own `addAvailablePDF` via the bridge |
| `unpaywall` | `https://api.unpaywall.org/v2/{doi}` |
| `epmc` | `https://www.ebi.ac.uk/europepmc/webservices/rest/search` → `ptpmcrender.fcgi` / NCBI PMC |
| `biorxiv` / `medrxiv` | `https://www.biorxiv.org/content/...`, `https://www.medrxiv.org/content/...` |
| `arxiv` | `https://arxiv.org/pdf/`, `https://export.arxiv.org/pdf/` |

PDF validation accepts by **magic bytes** (`%PDF`) even when `Content-Type` is wrong — upstream
handles servers that mislabel PDFs. Preserve this.

Resume state (`load_resume_keys`/`save_resume_key`/`clear_resume_state`) lets an interrupted
collection fetch continue. Port the same on-disk format so an in-flight Python run can be resumed by
the Rust binary during migration.

### Batch execution

`collection fetch-pdfs` loops per item with `--timeout-per-item` defaulting to 45 s. Serially, 50
items is worst-case ~37 minutes.

Do not add `--concurrency` in v1. The Python contract and compatibility matrix have no concurrency
flag, and the resume file is a load-modify-write JSON file with no lock. Keep the loop serial until
Phase 10 parity is green. A later performance phase may add concurrency only with an atomic
resume-state writer and per-host rate limits.

### Network policy

All external calls must:
- send a descriptive `User-Agent` (Python uses `"Mozilla/5.0"` for iCite and
  `"cli-anything-zotero"` for the update check — normalize to an honest product UA)
- respect the per-call timeouts already in the source
- never send `OPENAI_API_KEY` or `ZOTERO_EMBED_KEY` to any host other than the configured endpoint
- warn that configured AI endpoints receive Zotero item context and bearer tokens; require HTTPS
  unless the host is loopback or an explicit insecure override is set
- validate attachment URLs before fetching: default allow `https` plus loopback fixture hosts only;
  block private, link-local, multicast, and metadata-service IP ranges unless an explicit unsafe
  flag is provided for local testing

### `item analyze`

Requires `OPENAI_API_KEY`; endpoint overridable via `CLI_ANYTHING_ZOTERO_OPENAI_URL`. When the key
is missing, Python fails with a specific error — reproduce the message. `item context` is the
model-independent alternative and must work with no key at all.

## Related Code Files

- Create: `src/imports.rs`, `add.rs`, `pdf_fetch.rs`, `notes.rs`, `csl.rs`, `analysis.rs`, `metrics.rs`, `openai.rs`
- Create: `tests/imports_partial.rs`, `tests/pdf_cascade.rs`, `tests/csl_normalize.rs`
- Reference: `core/imports.py`, `core/add.py`, `core/pdf_fetch.py`, `core/notes.py`, `core/csl.py`, `core/analysis.py`, `core/metrics.py`, `utils/openai_api.py`

## Implementation Steps

1. Port `csl.rs` first — pure data transformation, no I/O, fully unit-testable. Covers the CSL type
   map, `looks_like_csl_item` heuristics, Crossref `message` unwrapping, and the four format
   detection branches in `normalize_import_json_payload`.
2. Port `imports.rs`: connector session lifecycle, `saveItems`, `updateSession` with tags,
   attachment manifest parsing with index/title validation, inline attachment extraction and
   stripping, dedupe within a request, and the partial-success accounting table above.
3. Port `notes.rs` including HTML/markdown note payload construction.
4. Port `pdf_fetch.rs` source-by-source, each with its own fixture-backed test. Implement magic-byte
   validation and resume state.
5. Port `add.rs` — the five entrypoints, `--if-exists` policy handling, arXiv id normalization, and
   the optional post-import PDF cascade.
6. Port `metrics.rs` and `openai.rs`.
7. Port `analysis.rs`: `build_item_context` aggregation (links, notes, exports) and `analyze_item`.
8. Keep batch PDF commands serial for v1; record concurrency as post-parity backlog only.
9. Extend the fake HTTP server with all external scholarly endpoints so the cascade is testable
   offline.

## Success Criteria

- [ ] `import json` and `import file` reach their target class, including inline and manifest attachments
- [ ] Partial-success produces `status: "partial_success"` **and exit code 1**, with complete per-attachment failure detail
- [ ] Duplicate attachments within one request are skipped idempotently, matching Python
- [ ] Manifest index-out-of-range and title-mismatch produce Python-identical errors
- [ ] Created-but-not-assigned/tagged failure returns `partial_success` with recovery context
- [ ] PDF accepted by magic bytes when `Content-Type` is wrong
- [ ] Each cascade source has an offline fixture test
- [ ] Resume state file is format-compatible with Python (a Python-started run resumes under Rust)
- [ ] `--if-exists file|skip|duplicate` behaves identically for all five `add` entrypoints
- [ ] `item analyze` without `OPENAI_API_KEY` produces the Python error message; `item context` works with no key
- [ ] No `--concurrency` flag in v1; compatibility matrix remains the CLI contract
- [ ] No credential is transmitted to any host other than the configured endpoint (asserted by test)
- [ ] AI endpoint warnings and HTTPS/loopback policy covered by tests
- [ ] Attachment URL validation blocks private/link-local/metadata IPs by default

## Risk Assessment

| Risk | Mitigation |
|---|---|
| Partial-success accounting subtly wrong — silent data loss for agents | Highest-priority test area; port the accounting table before the code; mirror upstream's own test cases from `test_core.py::ImportCoreTests` |
| Connector session semantics (`sessionID` reuse, ordering) misunderstood | Fake connector server records the full call sequence; assert order matches Python's |
| External APIs change or rate-limit during development | All tests run offline against fixtures; live calls are opt-in only |
| Hammering free scholarly APIs with future concurrency | Out of v1; any later threaded mode needs cap and per-host serialization |
| Resume-state format drift breaks mid-migration users | Byte-format compatibility test against a Python-produced file; keep v1 serial because the Python resume writer is not lock-safe |
| `add bibtex` parsing differs | BibTeX handling is delegated to Zotero's translators via the connector, not parsed locally — verify this assumption before writing a parser |
| Attachment URL fetch becomes SSRF/local-network read primitive | Block unsafe target ranges by default; require explicit unsafe override for local tests |
| Env-configured AI endpoint exfiltrates private library context | Document exact payload sent; require HTTPS or loopback unless explicitly overridden |
