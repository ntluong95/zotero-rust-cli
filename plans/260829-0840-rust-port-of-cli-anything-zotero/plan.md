---
title: "Rust Port of cli-anything-zotero"
description: "Incremental, compatibility-first port of the cli-anything-zotero Python CLI (96 commands) to a distributable native Rust binary optimized for AI-agent use."
status: in-progress
priority: P1
effort: "13 phases"
tags: [rust, port, zotero, cli, agent-tooling, migration]
created: 2026-08-29
blockedBy: []
blocks: []
---

# Rust Port of cli-anything-zotero

## Overview

Port [`PiaoyangGuohai1/cli-anything-zotero`](https://github.com/PiaoyangGuohai1/cli-anything-zotero)
(Python, Apache-2.0, v1.2.1 @ `e42a930e`) to a native Rust CLI that ships as a standalone binary
with **no Python, pip, virtualenv, Rust, or Cargo required by end users**.

This is a **behavioural/architectural port**, not a line-by-line translation. The existing Zotero
JavaScript/XPI bridge runtime logic is reused byte-for-byte unless Phase 6 proves an ownership marker
is required for forked plugin coexistence. Migration is incremental: both implementations coexist
until parity is certified.

**Evidence base (read these first):**
- [`../reports/xia-rust-port-analysis.md`](../reports/xia-rust-port-analysis.md) — full port analysis, measured benchmarks, challenge gate, approved decisions
- [`../reports/compatibility-matrix.md`](../reports/compatibility-matrix.md) — all 96 commands classified Exact / Semantic / Changed / Deferred / Dropped
- Read-only source checkout: `reference/cli-anything-zotero/`

### Why this port is justified

Measured on the target machine (macOS ARM64, Zotero 9.0.6 running, real 112 MB library,
5754 items): typical read commands take **137–195 ms**, of which **~5 ms is actual work**.
Over **95% is Python process and import overhead**, and **55–70% of total wall time is the single
eager `import prompt_toolkit`**.

| Path | Startup | End-user requirement |
|---|---|---|
| Python today | 137–195 ms | Python 3.10+ and pip |
| Python + lazy-import fix (~10 LOC) | ~60–70 ms | Python 3.10+ and pip |
| **Rust binary (this plan)** | **~3–8 ms** | **none** |

The lazy-import fix alone captures most of the *speed* win. It cannot deliver the *distribution*
requirement. **Distribution is the load-bearing justification; startup latency is a large but
secondary benefit.** Phase 2 proves the distribution pipeline before significant code depends on it.

### What Rust will NOT improve (do not claim otherwise)

SQLite reads are already 0.06–1.93 ms on a 112 MB database. Localhost HTTP is 1.2–3.4 ms. Path
discovery is 0.40 ms. Roughly 40 of 96 commands are bounded by Zotero, LibreOffice, or remote APIs
and will show **zero** improvement — `add doi` waits up to 45 s on Zotero's translators regardless
of implementation language.

## Goals

| # | Goal | Priority |
|---|------|----------|
| 1 | Ship standalone native binaries for macOS ARM64/x86_64, Windows x86_64, Linux x86_64/ARM64 with no runtime prerequisites | P1 |
| 2 | Preserve command names, flags, JSON field names, JSON schemas, exit codes, env vars, and default paths so existing agent skills and scripts keep working | P1 |
| 3 | Reduce repeated short-invocation startup from ~140–195 ms to under 10 ms | P1 |
| 4 | Reuse the Zotero XPI bridge unchanged; port only the JavaScript *generation* layer | P1 |
| 5 | Fix two structural defects rather than reproducing them: JS string-concat injection (D1) and f-string SQL injection (D2) | P1 |
| 6 | Maintain a continuously-running Python↔Rust parity harness, not a late big-bang comparison | P1 |
| 7 | Satisfy Apache-2.0 obligations, including the §4b statement of changes and a third-party license bundle | P1 |
| 8 | Keep the dependency footprint small and the code synchronous — no `tokio` | P2 |
| 9 | Retire the Python implementation only against explicit, measurable criteria | P2 |

## Non-Goals (approved at the challenge gate)

| Excluded | Rationale |
|---|---|
| Interactive REPL (`repl` command, `prompt-toolkit`, 521-LOC `repl_skin.py`) | Largest latency cost; zero agent value. `core/session.py` **is** ported — non-interactive commands depend on it. |
| Experimental direct-SQLite **writes** (`--experimental` flag) | Bypasses Zotero sync bookkeeping, requires Zotero closed, costs a 112 MB file copy per write. All three commands remain available — see the table below for exactly what changes. |
| DOCX zoterify chain in v1 (7 commands needing LibreOffice + Java + AppleScript) | Highest port risk. Deferred to Phase 12 behind its own go/no-go gate — deferred, not permanently cut. |
| `tokio` / async Rust | No measured concurrency demand. Batch PDF loops stay serial in v1 until parity is green. |
| Rewriting the XPI plugin domain logic | 71 lines, generic `eval` endpoint, zero Zotero logic, language-agnostic. Phase 6 may add only a minimal ownership marker if needed to disambiguate forked vs upstream endpoint ownership. |

## Approved intentional breaks

| Behaviour | Python | Rust | Rationale |
|---|---|---|---|
| Bare `zotero-cli` (no subcommand) | Enters REPL, **blocks on stdin** | Prints help, exits 0 | A blocking stdin read is the worst failure mode for a non-interactive agent caller. |
| `item move-to-collection` | **Only** works with `--experimental`, direct SQLite write, Zotero must be **closed** | Works by default via one dedicated JS bridge operation with Zotero **running**; `--experimental` removed | Verified: this command has no bridge path upstream (`_require_experimental_flag` fires unconditionally). v1 implements one Zotero-side operation equivalent to add target + remove sources, with rollback/compensation requirements. Strictly more usable, but it is new work and a real behaviour change — not a like-for-like port. |
| `app check-update` | Polls upstream's version file over the network | No network poll; always reports current | A fork must not poll upstream's own version-check endpoint on the user's behalf. Package managers (Homebrew, Scoop, direct release downloads) already own the update-notification job for this distribution. |

### What actually changes for the three `--experimental` commands

| Command | Upstream default | v1 change | Impact |
|---|---|---|---|
| `collection create` | JS bridge (already default) | `--experimental` flag removed | None on the default path |
| `item add-to-collection` | JS bridge (already default) | `--experimental` flag removed | None on the default path |
| `item move-to-collection` | **No bridge path** — `--experimental` mandatory | Dedicated Zotero-side JS bridge operation | **Changed** — see table above |

## Command budget

| Bucket | Count |
|---|---|
| **Ported in v1** | **88** |
| Deferred to Phase 12 (external-process DOCX) | 7 |
| Dropped (`repl`) | 1 |
| **Total** | **96** |

Compatibility classes across all 96: **34 Exact**, **52 Semantic**, **2 Changed**, **7 Deferred**, **1 Dropped**.

## Target architecture

```
AI Agent / Human
      │
      ▼
  zotero-cli  (single native binary, clap, synchronous)
      │
      ├── db::read   → zotero.sqlite  (rusqlite, mode=ro — WAL-safe; NOT immutable=1)
      │
      ├── http       → 127.0.0.1:23119 ─┬── /connector/*     Connector API
      │                                 ├── /api/*           Local API — reads (all versions)
      │                                 │                    Local API — WRITES (Zotero 10+)
      │                                 └── /cli-bridge/eval CLI Bridge XPI (generated JS)
      │                                                      ≤9: all writes · 10+: privileged only
      │
      ├── paths      → profile discovery, prefs.js/user.js, storage/, styles/, XPI build
      ├── net        → Crossref · Unpaywall · EuropePMC · NCBI PMC · bioRxiv/medRxiv
      │                arXiv · NIH iCite · doi.org · OpenAI-compatible chat + embeddings
      ├── semantic   → cli-anything-vectors.sqlite (second, independent SQLite store)
      └── proc       → Zotero launch  [Phase 12: LibreOffice, Java, osascript]
      │
      ▼
  Zotero Desktop  ← MUST BE RUNNING (upstream non-negotiable assumption)
```

> Corrects the originally proposed diagram: the **Connector API is a distinct fourth surface**
> (not the Local API), and a **second vector SQLite store** exists.

## Phases

Ordering is evidence-driven and deviates deliberately from a naive skeleton-first sequence:
distribution is proven early (Phase 2) because it is the port's justification, and the parity
harness is built first (Phase 1) and runs continuously rather than as a late gate.

| # | Phase | Delivers | Status |
|---|-------|----------|--------|
| 1 | [Behavioural Baseline and Parity Harness](./phase-01-start.md) | Golden fixtures + normalize/diff tooling. No Rust. | Complete |
| 2 | [Distribution Spine and Release Pipeline](./phase-02-distribution-spine-and-release-pipeline.md) | 5-target CI, authenticated releases, Homebrew/Scoop — proven on a trivial binary | Complete |
| 3 | [CLI Skeleton, Result Contract and Config](./phase-03-cli-skeleton-result-contract-and-config.md) | v1 command paths, output contracts, exit codes, `--json` anywhere, paths | Complete |
| 4 | [SQLite Read Layer and Typed Models](./phase-04-sqlite-read-layer-and-typed-models.md) | 24 read commands (the Exact-class core) | Complete |
| 5 | [HTTP Surfaces and Runtime](./phase-05-http-surfaces-connector-local-api-and-runtime.md) | Connector + Local API + runtime/doctor/session/audit | Complete |
| 6 | [Write Backends: Local-API-First](./phase-06-js-bridge-and-injection-hardening.md) | **Re-planned 2026-08-29 against merged Phase 14 evidence, then red-teamed.** Local API writes (gated on the `local_api_writes_available` capability flag, not Zotero version) + JS Bridge for 4 confirmed privileged ops (up to 7 contingent); fixes D1; XPI. Merged across PRs #6, #8, #9, #10, #11. | Complete |
| 7 | [Ingest, Attachments and PDF Cascade](./phase-07-ingest-attachments-and-pdf-cascade.md) | `add`/`import`/`note`/PDF cascade/hygiene/metrics (18 public CLI commands). Merged across PRs #12-#19 (latest: PR #18 `a47006e`, PR #19 `568d90a`). | Complete |
| 8 | [Semantic Search Vector Store](./phase-08-semantic-search-vector-store.md) | 3 commands (`item build-index`, `item semantic-search`, `item similar`); fixes D2; ~73× cosine speedup. Verified complete on main. | Complete |
| 9 | [Pure OOXML DOCX Commands](./phase-09-pure-ooxml-docx-commands.md) | 4 subprocess-free DOCX commands (`inspect-citations`, `inspect-placeholders`, `validate-placeholders`, `render-citations`). Verified complete on main. | Complete |
| — | **[Post-Phase-7 Remaining V1 Implementation](#post-phase-7-remaining-v1-implementation-parity-tail)** | **ACTIVE IMPLEMENTATION TAIL.** 17 canonical commands across 4 slices (Rendering/Export, Local App/Audit, Selection/Collection, Analysis/Hygiene). | **In Progress** |
| 10 | [Parity Certification and Cross-Platform](./phase-10-parity-certification-and-cross-platform.md) | Full matrix green on 3 OSes, `PARITY-REPORT.md`, cross-platform hardening, upstream drift check. Gated on remaining v1 command implementation. | **Waiting for Remaining V1 Implementation** |
| 11 | [Agent Skill, Docs and License Compliance](./phase-11-agent-skill-docs-and-license-compliance.md) | Regenerated SKILL.md, migration guide, Apache-2.0 compliance | Pending |
| 12 | [DOCX Zoterify Chain (deferred, gated)](./phase-12-docx-zoterify-chain-deferred-gated.md) | The 7 external-process DOCX commands (gated after Phase 10) | Pending |
| 13 | [Python Retirement](./phase-13-python-retirement.md) | Criteria-driven decommission | Pending |
| 14 | [**Zotero 10 Compatibility Gate**](./phase-14-zotero-10-compatibility-gate.md) | **BLOCKS P6.** WAL-safe reads, WAL fixture, XPI 10.0.*, capability detection. Merged in PR #4 (`7ee7c70`). | Complete |

### Dependency graph

```
P1 ──┬─→ P3 ──┬─→ P4 ──→ P5 ──→ ⟦P14⟧ ──→ P6 ──→ P7 ──→ [Parity Tail] ──→ P10 ──→ P11 ──→ P13
     │        │                                              ↑                 ↑              ↑
P2 ──┘        └─→ P9 ──────────────────→ P8 ─────────────────┘                 │              │
                                                                               P12 ───────────┘
```

## Post-Phase-7 Remaining V1 Implementation (Parity Tail)

Following the completion of Phase 7 (PR #18 `a47006e`, PR #19 `568d90a`) and the post-Phase-7 parity
tail (PRs #20-25, landing the Rendering/Export, Local App/Audit, Selection/Collection, and
Analysis/Hygiene slices plus `app launch`), the canonical parity audit against upstream `e42a930e`
now finds **zero** commands missing: every one of the 17 leaf commands this table used to list as
Missing has a real Rust implementation and dispatch arm as of PR #25 (`e45010f`, "app launch (final
canonical command)").

Phase 10's certification gate (canonical-matrix reconciliation, RC1 versioning, release-artifact
verification) is underway on this basis — see `plans/reports/compatibility-matrix.md`'s "Phase 10
canonical classification" section for the authoritative per-command table and evidence pointers.
The table below is kept in sync with that canonical source; on any future disagreement, the
compatibility matrix wins.

### Canonical 96-Command Inventory Breakdown

| Status Category | Count | Command Names |
|---|:---:|---|
| **Integrated** | **86** | `add arxiv`, `add bibtex`, `add doi`, `add file`, `add url`, `app doctor`, `app install-plugin`, `app launch`, `app ping`, `app plugin-status`, `app status`, `app uninstall-plugin`, `app version`, `audit path`, `audit tail`, `collection create`, `collection delete`, `collection fetch-pdfs`, `collection find`, `collection find-pdfs`, `collection get`, `collection items`, `collection list`, `collection remove-item`, `collection rename`, `collection stats`, `collection tree`, `collection use-selected`, `docx inspect-citations`, `docx inspect-placeholders`, `docx render-citations`, `docx validate-placeholders`, `export bib`, `import doi`, `import file`, `import json`, `import pmid`, `item add-to-collection`, `item analyze`, `item annotations`, `item attach`, `item attachments`, `item bibliography`, `item build-index`, `item children`, `item citation`, `item context`, `item delete`, `item duplicates`, `item export`, `item fetch-pdf`, `item file`, `item find`, `item find-pdf`, `item get`, `item list`, `item merge`, `item metrics`, `item move-to-collection` (Changed), `item notes`, `item search-annotations`, `item search-fulltext`, `item semantic-search`, `item similar`, `item tag`, `item update`, `js`, `library list`, `note add`, `note get`, `search get`, `search items`, `search list`, `session clear-collection`, `session clear-item`, `session clear-library`, `session history`, `session status`, `session use-collection`, `session use-item`, `session use-library`, `session use-selected`, `style list`, `sync`, `tag items`, `tag list` |
| **Missing** | **0** | *(none)* |
| **Changed (Stand-alone)** | **1** | `app check-update` (fork uses package managers, does not poll upstream Python version file) |
| **Excluded (Safe Divergence)**| **1** | `app enable-local-api` (replaced by Rust-native `app authorize-local-api` and plugin staging flow) |
| **Dropped** | **1** | `repl` (challenge decision C4: non-interactive agent focus) |
| **Deferred (Phase 12 Gated)** | **7** | `docx cite`, `docx doctor`, `docx insert-citations`, `docx prepare-zotero-import`, `docx zoterify`, `docx zoterify-preflight`, `docx zoterify-probe` |
| **Total Canonical** | **96** | *(Plus 1 Rust-only utility: `app authorize-local-api`)* |

### Implementation Slices for the Parity Tail

#### Slice A: Rendering / Export (RECOMMENDED IMMEDIATE NEXT SLICE)
- **Commands**: `item citation`, `item bibliography`, `item export`, `export bib`
- **Scope**:
  - Read-only with respect to Zotero SQLite and Local API.
  - Queries Local API rendering endpoints (`/items/{key}/citation`, `/items/{key}/bibliography`, `/items/{key}/export?format=...`) or formats locally via CSL / BibTeX.
  - 4 canonical rows; offline and mock-testable; foundational for `item context`.

#### Slice B: Local App / Audit Utilities
- **Commands**: `app ping`, `app version`, `app doctor`, `audit path`, `audit tail`
- **Scope**:
  - `app ping`: Fast reachability check.
  - `app version`: Version metadata JSON / string.
  - `app doctor`: Diagnostic report across paths, ports, databases, and bridge.
  - `audit path` & `audit tail`: Command execution audit trail logger (`CLI_ANYTHING_ZOTERO_AUDIT_DIR`).

#### Slice C: Selection / Collection
- **Commands**: `collection stats`, `collection use-selected`, `session use-selected`
- **Scope**:
  - `collection stats`: Summary metrics of collection items.
  - `collection use-selected` & `session use-selected`: Connector `getSelectedCollection` / `getSelectedItems` resolution.

#### Slice D: Analysis / Hygiene
- **Commands**: `item context`, `item duplicates`, `item metrics`, `item analyze`
- **Scope**:
  - `item context`: Hydrated item context assembling metadata, notes, bibtex, and citations.
  - `item duplicates`: Duplicate detection using title/DOI/ISBN matchers.
  - `item metrics`: Citation and lookup metrics (e.g. Crossref / Semantic Scholar / PMID).
  - `item analyze`: LLM-based item synthesis utility.

*(Note: `app launch` is isolated as a standalone GUI-spawning operation with distinct safety and platform validation requirements).*

### Three-Agent Operating Model

To execute the remaining implementation slices cleanly:
- **Claude A**: Sole implementation writer (writes Rust modules, CLI arguments, and error routing).
- **Claude B**: Independent Python contract oracle + source reviewer (validates exact JSON shapes, arguments, and edge cases against Python source `cli_anything/zotero/`).
- **Gemini**: Black-box QA + canonical parity auditor (develops independent test fixtures, verifies exit codes, and audits compatibility).

- **Phase number ≠ execution order.** P14 was appended by the plan CLI but executes **between P5 and
  P6**. This graph is authoritative. (P8 already shipped before P5–P7 by the same principle.)
- **⟦P14⟧ is a hard gate.** P6 must not start until its success criteria pass. It also retro-fixes a
  CRITICAL defect in already-landed P4 code — see Zotero 10 section below.
- **"Phase 5C" (2026-08-29 addition, source-audit finding):** P5's own declared Connector client
  scope (`getSelectedCollection`, `import`, `saveItems`, `saveAttachment`, `updateSession` —
  `phase-05...md:67`) remains unimplemented in `crates/`. A source-level audit found this gates
  **P7's** import/note commands (`add doi`/`import doi`/`import pmid`/`import file`/`import json`/
  `add url`/`note add` — all P7-owned, not P6-owned, per this table's own row 7), not P6's — P6 has
  zero Connector-routed commands (see `phase-06` §3.1a/§3.6 for the full audit). The `P5 → ... → P7`
  edge already visible above covers this ordering; this note exists so the gate is never assumed
  solved just because the arrow does.
- **P1 and P2 are independent** and may run concurrently.
- **P3 blocks everything downstream** — it defines the result contract.
- **P12 is optional** for reaching P13 only if its commands are formally deprecated instead.
- **Parallelisable right now, independent of P14:** P9 (pure-OOXML DOCX) depends only on P4's read
  layer and touches no Zotero surface Zotero 10 changed.

## Zotero 10 impact (assessed 2026-08-29)

Zotero 10 shipped **2026-08-17**, after this plan was written and after 31 commands landed. Full
analysis: [`plans/research/zotero-10-impact-on-rust-port.md`](../../research/zotero-10-impact-on-rust-port.md).

**Decision: PLAN ADAPTATION REQUIRED BEFORE PHASE 6.**

| # | Finding | Class | Effect on this plan |
|---|---|---|---|
| 1 | **WAL mode enabled.** `mode=ro&immutable=1` makes SQLite ignore `-wal` → reproduced **1 of 5 rows returned**, exit 0, no warning | **CRITICAL** | Retro-fixes **landed** P4 code. D4 superseded, `--strict-read` cancelled. New `wal-mode` fixture (P1). Owned by **P14**. |
| 2 | **XPI `strict_max_version: 9.0.*`** → plugin won't load on Zotero 10 | **CRITICAL** | P6's "reuse the XPI byte-for-byte" premise fails. **Correction (2026-08-29, Phase 6 re-plan):** P14 established this requirement but never applied it — no `manifest.json` exists anywhere in the repo for P14 to have bumped. **P6 owns creating the manifest and setting/live-verifying `strict_max_version: 10.0.*`** (see `phase-06` §3.12). |
| 3 | **Local API gained write support** (POST/PUT/PATCH/DELETE + tag delete, full-text, file upload) | **OPPORTUNITY** | **P6 redesigned Local-API-first.** JS Bridge shrinks ~33 → ~10 commands. D1 scope shrinks with it. |
| 4 | Local API key + user consent dialog; `Zotero-Server-ID` required on writes; local version semantics decoupled from sync | HIGH | New auth/key-storage requirements in **P6**; Server-ID capture in **P5**. |
| 5 | HTTP hardening: `Host` allowlist; `Mozilla/` UA or any `Origin` dropped | HIGH | Conformance test in **P5**. We pass today by luck, not design. |
| 6 | Singular `ZoteroPane.getSelected*()` removed → plural; multi-selection of collections/searches/libraries | MEDIUM | `use-selected` semantics defined in **P5**, pending Open Question 3. |
| 7 | Attachment stored-paths must be bare filenames; `setType` conversions throw | HIGH | **P7** validation + error handling. |
| 8 | FTS5 rewrite; `fulltextWords`/`fulltextItemWords` dropped; saved searches auto-migrate | MEDIUM | No landed code reads those tables (verified). Re-baseline `search list/get` per Zotero version in **P10**. |
| 9 | Port, read-auth, `Zotero.Server.Endpoints`, Firefox 140 ESR base, core schema | **NO CHANGE** | Existing design valid. |

**Backward compatibility:** Local API writes are **10+ only**. Zotero 7/8/9 keep the JS Bridge write
path. The port detects capability at runtime (`Zotero-Server-ID` header presence) rather than
assuming — so this is a dual-backend design, not a cutover.

**Already-landed work is not invalidated.** All 31 commands remain logically correct; one connection
-string change plus a WAL fixture restores their correctness on Zotero 10.

## Cross-cutting technical decisions

| Decision | Choice | Rationale |
|---|---|---|
| CLI framework | `clap` v4 derive | 96 commands; needs custom global-flag handling for `--json` at any level |
| HTTP client | `ureq` (blocking, rustls) | No async need; small; no OpenSSL cross-compile pain |
| SQLite | `rusqlite` with `bundled` feature | Removes system libsqlite3 dependency from all 5 targets |
| Serialization | `serde` + `serde_json` with `preserve_order` | JSON key order must match Python `json.dumps` output |
| Errors | `thiserror` for typed domain errors, `anyhow` at the boundary | Centralized mapping to result payloads, raw outputs, streams, and exit codes |
| Logging | `tracing` + `tracing-subscriber`, stderr only, off by default | Must never pollute stdout JSON |
| Concurrency | Serial batch loops in v1 | No `tokio`; any threaded PDF fetching waits until post-parity work |
| Paths | explicit home-dir resolution and fallbacks to match Python | Python hardcodes `~/.config/cli-anything-zotero` on all platforms — **replicate exactly, do not "fix" to platform-native** |
| XML | `quick-xml` | DOCX phases only |
| ZIP | `zip` | XPI build + DOCX |

## Contracts that must be preserved verbatim

| Contract | Detail |
|---|---|
| `--json` position | Accepted at root, group **and** command level (`item find X --json`) |
| JSON error channel | In `--json` mode, errors print to **stdout** as `{"error": "..."}` — not stderr |
| Human error channel | Without `--json`: Click-style message → **stderr**; `RuntimeError` → `Error: {msg}` → stderr |
| Exit codes | `ok=false` → 1; `status` ∈ {partial_success, error, failed, timeout} → 1; else 0 |
| Result payload helper | Commands that use `core.results.result_payload` preserve `{action, ok, status, code?, error?, ...}`. Many Exact read commands emit raw arrays/objects; do not wrap them. |
| JSON encoding | `ensure_ascii=False`, `indent=2`; fall back to ASCII-escaped when stdout cannot encode |
| Binary names | `zotero-cli` (primary), `cli-anything-zotero` (alias) |
| Config dir | `~/.config/cli-anything-zotero/session.json` on **all** platforms |
| Port fallback | `23119` |
| Data dir fallback | `~/Zotero` |

Full env-var list and the complete matrix live in
[`../reports/compatibility-matrix.md`](../reports/compatibility-matrix.md).

## Defects to fix, not reproduce

| ID | Defect | Fix | Phase |
|---|---|---|---|
| D1 | JS built by string concatenation escaping only `'` — breaks on `\`, newlines, quotes in titles/tags/collection names | Serialize params with `serde_json`, pass via `JSON.parse` inside the generated JS | 6 |
| D2 | f-string SQL interpolation of `language` / `exclude_key` in `semantic.py` | Bound parameters | 8 |
| D3 | `execute_js` probes `bridge_endpoint_active()` before every call — doubles round trips on 33 commands | Cache successful probe; retry after install/launch/register | 6 |
| D4 | ~~SQLite reads use `immutable=1`; can return stale/torn data~~ **— SUPERSEDED.** Under Zotero 10's WAL mode this is not "possibly stale", it is **silent data loss**: reproduced returning 1 of 5 committed rows, exit 0. | ~~Remove `immutable=1`; open `mode=ro`.~~ **Corrected 2026-08-29 after live testing** (see `phase-14` and the research report §7.1): `mode=ro` reliably fails with `SQLITE_BUSY` while Zotero holds the DB (exclusive locking mode, true on every Zotero version). Detect via the open attempt itself; use `mode=ro` when it succeeds (Zotero not running, or no `-wal`); refuse by default when it doesn't, opt-in via `--allow-stale-sqlite` for `immutable=1`. `--strict-read` cancelled either way — there is nothing to opt into once refusal is the safe default. | **14** |
| D5 | 112 MB full-file backup before every experimental write | N/A — write path excluded from v1 | — |
| D6 | JSON-mode errors go to stdout | **Preserve deliberately.** Agents depend on it. Document as intentional. | 3 |

## Risks

| Risk | Severity | Mitigation | Phase |
|---|---|---|---|
| JSON key ordering / float formatting differs from Python and breaks agent parsers | High | `preserve_order`; golden fixtures captured in P1 before any Rust exists | 1, 3 |
| Windows path handling (UNC, `file:///C:/`, drive letters) in `resolve_attachment_real_path` | High | Dedicated cross-platform test matrix; Windows CI runner from P2 | 4, 10 |
| DOCX OOXML byte output differs from `ElementTree` | High | Compare semantic structure, never bytes; corpus of real `.docx` fixtures | 9, 12 |
| Upstream diverges during the port (active repo, v1.2.1 one month before analysis) | Medium | Pin compatibility target at `e42a930e`; the matrix is the diff surface; review upstream at P10 | 10 |
| `_safe_text_for_stdout` backslashreplace behaviour on Windows cp1252 consoles | Medium | Explicit encoding tests on the Windows runner | 3, 10 |
| Binary distribution is checksummed but not authenticated | High | Phase 2 must pick a real signing/attestation path and document verification before releases are considered shippable | 2 |
| macOS notarization blocks distribution late | Medium | Resolved in P2 on a trivial binary, before code depends on it | 2 |
| Python `re` patterns relying on backtracking not expressible in Rust `regex` | Low | Audit each of the ~11 patterns during its phase; `fancy-regex` only if genuinely required | 4, 9 |
| Local API disabled on user machines (observed `false` on the analysis machine) | Medium | Parity fixtures must cover both Local-API-on and Local-API-off states | 1, 5 |
| Local API search returns items that immutable SQLite cannot re-resolve yet | High | Phase 5 must test and define this mixed-surface failure path; never silently drop all fresh Local API results as "no results" | 4, 5 |

## Success Criteria

- [ ] All 34 **Exact**-class commands produce byte-identical normalized JSON and identical exit codes versus Python on macOS, Windows and Linux
- [ ] All 52 **Semantic**-class commands produce identical JSON schemas and exit-code semantics
- [ ] Both **Changed** behaviours (bare invocation, `item move-to-collection`) are implemented and documented
- [ ] Cold-start latency for `item find --json` is under 10 ms (from a measured 142 ms baseline)
- [ ] Prebuilt binaries install and run on all 5 targets with no Python, pip, Rust or Cargo present
- [ ] Release artifacts are authenticated, not only checksummed, with documented verification
- [ ] The Zotero XPI runtime logic is byte-identical to upstream apart from `update_url`, addon id, and any minimal Phase-6 ownership marker required to distinguish forked endpoint ownership
- [ ] D1 and D2 are fixed and covered by regression tests using adversarial inputs (backslashes, newlines, quotes, CJK)
- [ ] `SKILL.md` regenerated against the Rust CLI and validated by a real agent run
- [ ] Apache-2.0 §4b statement of changes present; third-party license bundle generated
- [ ] Python retirement criteria in Phase 13 are all met, with any Phase 12 legacy-only commands excluded from the supported Rust no-Python promise

## Open Questions

1. **Binary/package naming.** Publishing `zotero-cli` to Homebrew/Scoop may collide with upstream's
   PyPI console script if a user has both. Resolve in Phase 2: distinct package name with
   `zotero-cli` as the installed binary, or a fully distinct binary name?
2. **Fork vs. contribution.** Should the Rust CLI be proposed upstream, or maintained as an
   independent fork? Affects the addon id and `update_url` decision in Phase 6.
3. **macOS code signing.** Requires a paid Apple Developer account for notarization. If unavailable,
   the fallback is documented `xattr -d com.apple.quarantine` instructions — decide in Phase 2.
4. **Phase 12 go/no-go.** Whether to port the LibreOffice/Java/AppleScript DOCX chain at all, or
   formally deprecate those 7 commands. Decide after Phase 10 with real usage data.

### Zotero 10 open questions — all require a live Zotero 10 instance (blocking Phase 14)

The dev machine runs **Zotero 9.0.6** (rollback journal, no `-wal`). None of these can be answered
from documentation. They are listed in full in
[`plans/research/zotero-10-impact-on-rust-port.md`](../../research/zotero-10-impact-on-rust-port.md) §6.

5. **Does `mode=ro` (no `immutable`) open a live Zotero 10 DB** while Zotero holds it? WAL readers
   must map `-shm`; a read-only open can fail `SQLITE_CANTOPEN`. Determines which rung of Phase 14's
   fallback ladder we land on. **Highest-risk unknown in the whole adaptation.**
6. **Does `Host: 127.0.0.1:23119` (with port) pass** Zotero 10's new Host allowlist?
7. **Does `/connector/getSelectedCollection` still exist in 10, and what does it return under
   multi-selection?** Determines `use-selected` semantics (Phase 5 decision matrix).
8. **Does `/cli-bridge/eval` still pass** the hardened browser-origin check without
   `allowRequestsFromUnsafeWebContent`?
9. **Does "Always Allow" survive a Zotero restart, and where is the key stored?** Determines whether
   unattended agent writes are viable at all on Zotero 10.
10. **Do 10-migrated saved searches** (`childNote` → `note`+`resultLevel`) change `search list/get`
    JSON versus the Python baseline on the same library?

## Red Team Review

### Session — 2026-08-29
**Findings:** 14 (14 accepted, 0 rejected)
**Severity breakdown:** 2 Critical, 9 High, 3 Medium

| # | Finding | Severity | Disposition | Applied To |
|---|---|---|---|---|
| 1 | Universal result envelope overstates Python contract | Critical | Accept | plan.md, Phase 3 |
| 2 | Python-only fallback contradicts no-Python retirement | Critical | Accept | Phase 12, Phase 13 |
| 3 | Deferred/dropped command stubs become public traps | High | Accept | Phase 3, Phase 11 |
| 4 | Release signing is claimed but optional | High | Accept | plan.md, Phase 2 |
| 5 | Bridge-composed move is non-atomic | High | Accept | Phase 6 |
| 6 | Forked addon id cannot prove endpoint ownership | High | Accept | plan.md, Phase 6, Phase 13 |
| 7 | Fake bridge cannot certify real JS semantics | High | Accept | Phase 6 |
| 8 | Post-import updateSession failure loses recovery context | High | Accept | Phase 7 |
| 9 | Local API plus immutable SQLite can drop fresh results | High | Accept | plan.md, Phase 4, Phase 5 |
| 10 | Phase 10 overclaims parity coverage for live-only commands | High | Accept | Phase 10 |
| 11 | Raw privileged `js` command lacks agent-facing warning | High | Accept | Phase 11 |
| 12 | `--concurrency` is pre-parity scope creep and resume-race risk | Medium | Accept | Phase 7 |
| 13 | Env-configured AI endpoints can exfiltrate item context and keys | Medium | Accept | Phase 7, Phase 11 |
| 14 | Endpoint probe cache can turn transient inactive bridge into process-wide failure | Medium | Accept | Phase 6 |

#### Adjudication Notes
- Finding 1 verified by `zotero_cli.py:738-777`, `zotero_cli.py:980-1029`, and `core/results.py:8-37`: raw read commands are not result envelopes.
- Finding 5 verified by `utils/zotero_sqlite.py:735-775` versus `core/jsbridge.py:607-629`: upstream move is transactional; bridge primitives are separate saves.
- Finding 6 verified by `plugin/zotero-cli-bridge/bootstrap.js:41-67` and `manifest.json:6-11`: addon ids differ, but both register/delete the same endpoint key.
- Finding 10 is a certification wording issue, not a command cut: fixture, live, manual, and accepted-divergence evidence must be separated in `PARITY-REPORT.md`.

### Session — 2026-08-29 (Zotero 10 adaptation)
**Findings:** 5 (4 accepted, 1 withdrawn on review)
**Severity breakdown:** 2 Critical, 3 High
**Reviewer:** inline adversarial pass, not a spawned subagent (per standing instruction). Stated so the
provenance of this review is not overclaimed.

| # | Finding | Severity | Disposition | Applied To |
|---|---|---|---|---|
| 15 | WAL + `immutable=1` silently drops uncheckpointed rows in **already-landed, already-"Exact"** read commands | Critical | Accept | plan.md (D4), Phase 4, Phase 14 |
| 16 | XPI `strict_max_version: 9.0.*` makes the plugin unloadable on Zotero 10, invalidating Phase 6's premise | Critical | Accept | Phase 6, Phase 14 |
| 17 | ~~Local-API-first knowingly downgrades ~10 write commands Exact → Semantic~~ **— finding withdrawn, superseded by 17b** | High | **Rejected on review** | — |
| 17b | Finding 17 conflated *backend returns different bytes* with *CLI must emit different bytes*. Parity is a property of the observable CLI contract. Exact remains achievable behind a compatibility renderer; the real risk is the opposite — a renderer that makes two backends print identical JSON while leaving Zotero in **different states** | High | Accept | Phase 6 §C2 |
| 18 | Local API consent dialog may make unattended agent writes impossible if "Always Allow" doesn't persist — would reverse the redesign | High | Accept, gate on OQ9 | Phase 6 §C1, Phase 14 |

#### Adjudication Notes
- Finding 15 verified **empirically**, not from documentation alone: a WAL database with uncheckpointed
  commits returned **1 of 5 rows** under `mode=ro&immutable=1` versus 5 of 5 under `mode=ro`
  (reproduction in `plans/research/zotero-10-impact-on-rust-port.md` §1.1). Vendor doc independently
  confirms WAL is enabled in Zotero 10.
  **Nuance worth stating:** SQLite auto-checkpoints (~1000 pages), so the corruption window is
  intermittent rather than constant. That makes it *worse* for an agent tool, not better — an
  intermittently-wrong read is harder to detect than a consistently-wrong one.
- Finding 16 verified against `plugin/zotero-cli-bridge/manifest.json` (`strict_max_version: "9.0.*"`)
  and the Zotero 10 developer guidance to bump it to `10.0.*`.
- **Finding 17 was withdrawn on review.** It asserted that changing the write backend forces a
  parity downgrade. It does not: parity describes the **observable CLI contract**, not the transport.
  Python is `item update → JS Bridge → Zotero → construct JSON`; Rust can be
  `item update → Local API → Zotero → construct the SAME JSON`. With a compatibility renderer in
  front, Exact stays achievable for keys, types, status fields, stdout/stderr, exit codes and
  semantics. The correct rule — **attempt Exact first, downgrade only on demonstrated evidence** —
  replaces it, and an adapter is required for correctness anyway, since Zotero 10's local object
  `version` deliberately means something different from the Web API's.
- **Finding 17b** is the inverted, real risk that survives: forcing output parity can mask a genuine
  behavioural difference. Mitigated by requiring **post-write state parity**, not just output parity.
- **Finding 18 (C1) is the only genuine reversal trigger.** If "Always Allow" does not persist across
  a Zotero restart, unattended agent writes are impossible via Local API and bridge-first is correct
  again. Gated on Open Question 9 in Phase 14.
- **Not accepted as a finding:** "Phase 14's number is out of execution order." Real, but cosmetic —
  the plan CLI appends sequentially and the dependency graph is already authoritative (P8 shipped
  before P5–P7 under the same rule). Renumbering 8 phase files to fix presentation would risk more
  than it clarifies.

### Whole-Plan Consistency Sweep (2026-08-29, Zotero 10 adaptation session)
- Files reread: plan.md, phase-01 through phase-13.
- Decision deltas checked: result payload scope, release authentication, plugin endpoint ownership, move atomicity, deferred command visibility, live-only parity evidence, Python-only retirement wording, concurrency removal, AI endpoint threat model, SSRF policy, mixed Local API/SQLite stale reads, probe cache invalidation.
- Reconciled stale references: 12.
- Unresolved contradictions: 0.

### Session — 2026-08-29 (Phase 6 re-plan, four-lens hostile review)
**Findings:** 28 raw (from 4 parallel reviewers: Security Adversary, Failure Mode Analyst, Assumption Destroyer, Scope & Complexity Critic), deduplicated to 15 distinct findings (15 accepted, 0 rejected — every finding carried a file:line evidence citation and passed the evidence filter)
**Severity breakdown:** 7 Critical, 6 High, 2 Medium (post-dedup)
**Reviewers:** 4 parallel `code-reviewer` subagents, one per lens, each assigned a Full-tier verification role (Fact Checker, Flow Tracer, Scope Auditor, Contract Verifier) per this project's plan-review protocol.

| # | Finding | Severity | Disposition | Applied To |
|---|---|---|---|---|
| 19 | Connector API `saveItems`/`saveAttachment` were claimed "already landed" in Phase 5; grep confirms zero matches anywhere in `crates/` and Phase 5's own file is still `status: todo` | High | Accept | Phase 6 Overview, §3.6, Unresolved Q9 |
| 20 | `item add-to-collection`'s design PATCHed `data.collections` as if the Local API appends; Zotero's Web API v3 treats array properties as complete-replacement lists — as written, this would silently remove the item from every other collection (real data loss) | Critical | Accept | Phase 6 §3.6 row 47/68/26 |
| 21 | `item duplicates`/`merge`/`sync` were labeled "LIVE VERIFIED absence" from the Local API; Phase 14 never actually probed anything duplicates/merge/sync-shaped — the label overclaimed evidence that doesn't exist | Critical | Accept | Phase 6 §3.6 rows 58/66/94 |
| 22 | "No silent Bridge fallback on write-denial" does not address the double-write risk when a caller retries a non-idempotent write (`collection create`) after an ambiguous consent-denial status, per the plan's own fail-fast message instructing a re-run | Critical | Accept | Phase 6 §3.3 |
| 23 | Write-fixture test design ("restore by file copy between runs") contradicts the plan's own live-verified proof that Zotero holds SQLite in exclusive locking mode — a file-copy race against a running Zotero produces flaky, non-reproducible failures | Critical | Accept | Phase 6 §Testing Strategy |
| 24 | §3.6's matrix was keyed on Zotero version rather than the actual `local_api_writes_available` capability flag, silently breaking on the plan's own documented "Local API disabled on a 10+ machine" risk | Critical | Accept | Phase 6 §3.6 |
| 25 | No slice was assigned to build the ≤9 JS-Bridge CRUD command templates the matrix requires — a baseline gap independent of the write-consent spike's outcome | Critical | Accept | Phase 6 §3.8a, new Slice 1b |
| 26 | Single-plugin XPI ownership policy only checks ownership at install time; a later out-of-band upstream reinstall silently reclaims the shared endpoint with no runtime signal | High | Accept | Phase 6 §3.12, Unresolved Q5 |
| 27 | The disposable write-consent spike's "scratch library" isolation relied on an unenforced environment-variable convention; `http.rs`'s `base_url` has no library/instance identity check, risking a write to a real production Zotero | High | Accept | Phase 6 §3.2 |
| 28 | Diagnostic `server_id` persistence into `session.json` had no defined consumer, would be silently dropped by `save_session_state`'s hardcoded exact-4-key contract (or break Python-schema parity if wired in), and would turn previously read-only `app status`/`app doctor` into racy writers of a shared, non-atomically-locked file | High | Accept (cut from scope) | Phase 6 §3.4 |
| 29 | Slice 6's shared write-interface contract (success/auth-denied/conflict/transport-error taxonomy) was left undefined until merge time instead of fixed upfront | High | Accept | Phase 6 §3.13 (new) |
| 30 | Fail-fast/no-polling write-auth design gives an unattended AI-agent caller no machine-distinguishable signal to avoid blind retry loops | High | Accept | Phase 6 §3.3 |
| 31 | No-SQLite-write regression guard's proposed grep pattern (`rusqlite::Connection::open`) does not match the codebase's actual unqualified `Connection::open` call style — the guard would not have caught a real violation | High | Accept | Phase 6 §3.7 |
| 32 | "`catalog.rs` is the one shared file" was factually wrong — it has no dispatch logic at all; the real shared dispatch surface both agents' new commands land in is `cli.rs`/`lib.rs`, which the original plan never assigned an owner to | High | Accept | Phase 6 Related Code Files, §Agent split |
| 33 | No enforcement test existed for "backend identity must never leak into stdout JSON," relying only on cross-backend diffing that never exercises single-backend commands (`js`, `sync`, `item duplicates`, `item merge`) | Medium | Accept | Phase 6 §3.5, §Testing Strategy |

#### Additional smaller findings folded into the same edit pass (evidence-backed, Medium severity, not given individual table rows to keep this table to a manageable size)
- Stale "~10 privileged ops" estimate contradicted by the matrix's actual confirmed count (4, up to 7) — corrected in this file's phase table (row 6) and in phase-06's §3.6.
- A new dedicated `plans/research/phase-6-write-consent-spike.md` would have fragmented the existing, established `zotero-10-impact-on-rust-port.md` evidence trail — redirected to append as a new §8 there instead.
- Success Criteria required every matrix cell fully resolved while Risk Assessment called the same resolution "a bonus, not a requirement" — reconciled to "committed backend required, evidence-tier upgrade optional."
- The 14-18d effort estimate and the two-agent parallel split were carried forward from the user's explicit request (not reversed per this project's "do not silently undo explicit user decisions" rule) but now carry an explicit stated justification/scheduling assumption instead of an unstated one.
- Slice 7's real dependency on Slice 3's matrix resolution existed only in prose; added as a soft dependency in the structured slice table.
- Added Slice-0 open question on whether a `server_id` change should be correlated into the 401/403 error message, and whether repeated denied-write attempts stack Zotero's consent dialog.

#### Adjudication Notes
- All 15 primary findings passed the evidence filter (grep/read-verified file:line citations); none were rejected for lack of evidence.
- No finding was rejected on the merits either — this session's four reviewers found convergent, non-overlapping structural issues (three independently flagged variations of the `session.json`/`server_id` diagnostic-field problem from different angles: schema-invariant violation, missing consumer, and a new write-race exposure — triangulated into one clean "cut it" decision rather than three separate patches).
- The two-agent parallel split (Assumption/Scope Critic's "is this worth the coordination cost?" challenge) was **not** reversed: the user's original `/ak-fix` request explicitly required an "Agent A / Agent B parallel split" as a deliverable. Per this project's decision-authority rule, an audit concern about a user-specified deliverable gets surfaced with its trade-off, not silently overridden — the plan now states the scheduling assumption the split depends on explicitly, so the trade-off is visible rather than resolved unilaterally.

### Whole-Plan Consistency Sweep (2026-08-29, Phase 6 re-plan session)
- Files reread: plan.md (this file, including the phase table and both prior Red Team Review sessions), phase-06-js-bridge-and-injection-hardening.md (fully rewritten), phase-05, phase-14 (cross-checked for §C1/§C2/§C3 anchor consistency — those labels are preserved in phase-06's new section headers specifically so existing citations from phase-14/plan.md continue to resolve).
- Decision deltas checked: Connector-API-landed assumption (reversed), PATCH array semantics (corrected), matrix keying (version → capability flag), evidence-tier discipline (LIVE VERIFIED downgraded where unearned), session.json diagnostic field (cut), shared-file identification (catalog.rs → cli.rs/lib.rs), write-interface contract (deferred → fixed upfront), effort/scope justification (added), "~10 privileged ops" estimate (corrected to 4-7).
- Reconciled stale references: 1 (plan.md phase table row 6).
- Unresolved contradictions: 0.

### Post-review correction (2026-08-29, user-directed, before commit)
User caught a Phase 14/Phase 6 ownership error the four automated reviewers missed: the draft above
still said "Phase 14 already bumped `strict_max_version` to `10.0.*`," which contradicts Phase 14's
own merge-gate classification ("no `plugin/` directory exists yet anywhere in this repo; Phase 6
creates it") — confirmed by `find crates -iname "manifest.json"` returning nothing. Corrected in both
files: Phase 14 established the Zotero-10-compatibility *requirement*; Phase 6 creates the manifest
and owns setting/live-verifying the bump (`phase-06` §3.12; `plan.md` Zotero 10 impact table, Finding
2). Also resolved, per explicit user decision, the §3.12 XPI ownership trade-off that the plan had
left open: **ownership marker**, not an install-time-only single-plugin policy — an upstream plugin
reinstalled later can silently reclaim the shared `/cli-bridge/eval` endpoint under the single-plugin
option with no runtime signal, whereas a minimal marker lets `plugin-status` verify ownership
continuously through the endpoint itself. Unresolved Question 5 in `phase-06` is now marked resolved.
- Reconciled stale references: 2 (`phase-06` §3.12 body + Requirements bullet; `plan.md` Zotero 10
  impact table row 2).
- Unresolved contradictions: 0.

<!-- slug: rust-port-of-cli-anything-zotero -->
