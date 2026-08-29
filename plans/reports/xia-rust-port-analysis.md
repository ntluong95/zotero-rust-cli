# Xia Port Analysis — cli-anything-zotero (Python) → Rust CLI

Date: 2026-08-29
Mode: `--port` (behavioural/architectural port, not line-by-line translation)
Analyst: Xia phases 1–4 (Recon → Map → Analyze → Challenge)

---

## 1. Source manifest

| Field | Value |
|---|---|
| Source repo | `https://github.com/PiaoyangGuohai1/cli-anything-zotero` |
| Resolved commit | `e42a930e9374422c9966a38e477adec71436a61e` |
| Ref / release | `main` @ `v1.2.1` (2026-07-28) |
| License | **Apache-2.0** (no `NOTICE` file present) |
| Language | Python 100% (660 KB) |
| Runtime deps | **`click>=8.0.0`, `prompt-toolkit>=3.0.0` — nothing else** |
| Dev deps | `pytest`, `pytest-cov` |
| Python floor | `>=3.10` |
| Local reference checkout | `reference/cli-anything-zotero/` (read-only) |
| Upstream activity | Active — 13 commits Jul 2026, release 1 month before analysis |
| Stars / forks | 131 / 13 |

### Size

| Area | LOC |
|---|---|
| Production Python | ~13,500 |
| Tests (Python) | ~3,050 (82 tests) |
| Zotero XPI plugin (JS) | **71** |
| Docs / skills | ~1,600 |
| **Total Python** | 16,557 |

### Command surface

**96 leaf commands** across **14 groups** (`add`, `app`, `audit`, `collection`, `docx`, `export`,
`import`, `item`, `library`, `note`, `search`, `session`, `style`, `tag`) plus 3 root-level
commands (`js`, `sync`, `repl`).

---

## 2. Source anatomy

### 2.1 Module map

| Module | LOC | Responsibility | Key deps |
|---|---|---|---|
| `zotero_cli.py` | 2681 | Click command tree, `--json` propagation, `emit`/`emit_js`, REPL loop, `dispatch`/`entrypoint` | click, prompt_toolkit |
| `core/imports.py` | 1067 | Connector import (file/JSON), attachments manifest, DOI ingest, Crossref BibTeX | urllib, hashlib |
| `core/docx_zoterify.py` | 937 | Placeholder → live Zotero LibreOffice fields | zipfile, ET, subprocess, osascript |
| `core/docx.py` | 848 | DOCX inspection, placeholder validation, preflight, Java/LO probing | zipfile, ET, subprocess |
| `core/jsbridge.py` | 816 | **Generates JS source strings**, POSTs to `/cli-bridge/eval`; AppleScript fallback | urllib, subprocess |
| `utils/zotero_sqlite.py` | 782 | All read queries + 3 experimental write helpers | sqlite3 |
| `utils/repl_skin.py` | 521 | ANSI branding, banner, prompt session | prompt_toolkit |
| `core/pdf_fetch.py` | 514 | OA PDF cascade (Unpaywall→EPMC→preprint→arXiv) | urllib |
| `core/add.py` | 491 | Unified ingest (`doi`/`arxiv`/`url`/`bibtex`/`file`) | urllib |
| `core/hygiene.py` | 489 | Duplicate detection + merge preview/execute | — |
| `utils/zotero_paths.py` | 398 | Profile/data-dir/executable discovery, prefs parsing, XPI build/install | configparser, zipfile, re |
| `core/semantic.py` | 259 | Embedding API + f32 vector SQLite + **pure-Python cosine** | sqlite3, struct, urllib |
| `core/catalog.py` | 258 | Read facade over SQLite + Local API; CSL style listing | ET |
| `core/docx_static.py` | 256 | Static (text) citation rendering fallback | zipfile, ET |
| `utils/zotero_http.py` | 240 | Connector + Local API HTTP client | urllib |
| `core/docx_pipeline.py` | 235 | One-shot `docx cite` orchestration | — |
| `core/csl.py` | 196 | CSL-JSON ↔ connector item conversion | — |
| `core/experimental.py` | 175 | Guards for direct-SQLite writes | — |
| `core/notes.py` | 172 | Note read/add via connector | html, re |
| `core/discovery.py` | 167 | `RuntimeContext`, launch, readiness waits | subprocess |
| `core/analysis.py` | 166 | `item context` + `item analyze` (OpenAI) | — |
| `core/doctor.py` | 158 | Health checks | — |
| `core/session.py` | 125 | Persisted session state + flock | fcntl |
| `core/audit.py` | 123 | Append-only write audit log | — |
| `core/rendering.py` | 98 | `export`/`citation`/`bibliography` via Local API | — |
| `core/results.py` | 55 | `result_payload`, `exit_code_for` | — |
| `core/metrics.py` | 35 | NIH iCite lookup | urllib |
| `utils/openai_api.py` | 84 | OpenAI-compatible chat call | urllib |

### 2.2 Backend distribution across the 96 commands

| Backend | Commands | Share |
|---|---|---|
| SQLite (read) | 24 | 25% |
| JS Bridge (`/cli-bridge/eval`) | 33 | 34% |
| Session/state/local-only | 12 | 13% |
| DOCX (OOXML manipulation) | 10 | 10% |
| Connector HTTP (`/connector/*`) | 6 | 6% |
| Local API (`/api/*`) | 4 | 4% |
| External network APIs | 6 | 6% |
| Direct SQLite **write** (experimental) | 3 | 3% |

### 2.3 Actual runtime architecture (verified, corrects the proposed diagram)

```
AI Agent / Human
      │
      ▼
  zotero-cli
      │
      ├── Zotero SQLite  (read: mode=ro&immutable=1) ── ~/Zotero/zotero.sqlite
      ├── Zotero SQLite  (WRITE, experimental, 3 cmds, requires Zotero CLOSED)
      │
      ├── HTTP :23119 ─┬── /connector/*     ← Connector API   (import, saveItems,
      │                │                       saveAttachment, updateSession,
      │                │                       getSelectedCollection, ping)
      │                ├── /api/*           ← Local API       (export, citation,
      │                │                       bibliography, saved-search items)
      │                └── /cli-bridge/eval ← CLI Bridge XPI  (arbitrary JS eval)
      │
      ├── Filesystem  (profile discovery, prefs.js/user.js, storage/, styles/, XPI build)
      ├── External scholarly APIs (Crossref, Unpaywall, EuropePMC, NCBI PMC,
      │                            bioRxiv/medRxiv, arXiv, NIH iCite, doi.org)
      ├── OpenAI-compatible APIs  (chat: item analyze; embeddings: semantic search)
      └── External processes      (LibreOffice `soffice`, Java/`javac`,
                                   macOS `osascript`, Zotero launch)
      │
      ▼
  Zotero Desktop  ← MUST BE RUNNING (upstream: "non-negotiable runtime assumption")
```

**Three corrections to the proposed target diagram:**

1. **The Zotero Connector API is a distinct fourth surface**, missing from the proposal. It is
   *not* the Local API. It handles all non-bridge writes (`import file`, `import json`,
   `note add`, `note get`, `collection use-selected`, `app enable-local-api`). All three HTTP
   surfaces share port 23119 but have different auth/versioning/status semantics
   (`/connector/import` returns **201**, Local API needs `Zotero-API-Version: 3`).
2. **LibreOffice + Java + AppleScript are hard external-process dependencies** for the
   `docx zoterify` chain — the diagram shows no process-execution edge.
3. **A separate vector SQLite database** (`~/Zotero/cli-anything-vectors.sqlite`, overridable
   by `ZOTERO_VECTOR_DB`) is a second, independent SQLite store.

### 2.4 The XPI bridge — the single most consequential finding

`plugin/zotero-cli-bridge/bootstrap.js` is **71 lines** and is a **generic `eval` endpoint**:

```js
init: async function (options) {
  var result = await eval("(async () => {" + options.data + "})()");
  return [200, "application/json", JSON.stringify(result)];
}
```

It contains **zero Zotero domain logic**. All 33 bridge-backed commands work by having Python
**build JavaScript source strings** and POST them as the request body.

Consequences:

- The XPI is **completely language-agnostic**. A Rust CLI reuses it byte-for-byte. Rewriting it
  would be pure churn.
- **The real bridge work is the ~500 lines of JS-string templates inside `core/jsbridge.py`**, and
  those must be reproduced exactly in Rust — this is porting *JavaScript source generation*, not
  porting JavaScript.
- The XPI grants **arbitrary privileged code execution** inside Zotero to any local process that
  can reach `127.0.0.1:23119`. That is upstream's existing security posture; the port inherits it.

### 2.5 Defects found in the source (port should not replicate)

| # | Location | Defect | Impact |
|---|---|---|---|
| D1 | `core/jsbridge.py` throughout | JS built by string concat, escaping **only `'`** (`update_item_fields`, `manage_tags`, `create_collection`, `_build_post_import_js`) | A title/tag/name containing `\`, newline, or `</script>` produces broken or injected JS. `attach_pdf` had to be patched for Windows `\` (issue #4) — the general class remains. **Fix in port: `serde_json` the parameters and `JSON.parse` them in JS.** |
| D2 | `core/semantic.py:58-59` | f-string SQL interpolation of `language` and `exclude_key` | SQL injection from CLI args. **Fix: bound parameters.** |
| D3 | `core/jsbridge.py:289` | `execute_js` calls `bridge_endpoint_active()` first — an **extra HTTP POST before every bridge call** | Doubles round trips on all 33 bridge commands. |
| D4 | `utils/zotero_sqlite.py:29` | Reads use `immutable=1` | Tells SQLite the file cannot change; ignores WAL/locking. With Zotero running and writing, reads can be stale or torn. Deliberate trade-off upstream, but undocumented. |
| D5 | `utils/zotero_sqlite.py:664` etc. | `backup_database()` full-file copy before **every** experimental write | Measured **98 ms** for a 112 MB DB; scales linearly. Also writes `synced=0, version=0`, bypassing Zotero's sync bookkeeping. |
| D6 | `zotero_cli.py:2665` | In `--json` mode, errors are printed to **stdout**, not stderr | Contract quirk agents depend on; must be preserved deliberately, not "fixed". |

---

## 3. Measured performance baseline (the evidence for/against Rust)

Measured on this machine: macOS (Darwin 25.5.0), Apple Silicon, Python 3.14.3,
**live Zotero 9.0.6 running**, real library — `zotero.sqlite` **112 MB, 5754 items,
121 collections, 11 libraries**.

### 3.1 End-to-end command latency (live Zotero)

| Command | Wall time |
|---|---|
| `app status --json` | 163 ms |
| `library list --json` | 195 ms |
| `collection list --json` | 161 ms |
| `item list --json --limit 10` | 147 ms |
| `item find "PM2.5" --json --limit 5` | 142 ms |
| `tag list --json` | 137 ms |
| `--help` | 116 ms |

### 3.2 Where that time actually goes

| Phase | Cost | Share of a 142 ms `item find` |
|---|---|---|
| Python interpreter boot | ~27 ms | 19% |
| `import click` | ~6–17 ms | ~8% |
| **`import prompt_toolkit`** | **~73–151 ms** | **~55–70%** |
| Remaining module imports | ~20 ms | 14% |
| `build_environment` (fs stat + prefs parse) | **0.40 ms** | 0.3% |
| Connector HTTP probe | **1.18 ms** | 0.8% |
| Local API HTTP probe | **3.37 ms** | 2.4% |
| SQLite `connect` (112 MB, ro) | **0.06 ms** | 0.04% |
| SQLite `fetch_collections` | **0.42 ms** | 0.3% |
| SQLite `find_items_by_title` | **1.93 ms** | 1.4% |
| SQLite `fetch_tags` | **1.16 ms** | 0.8% |
| **Total useful work** | **~5 ms** | **~3.5%** |

> **>95% of wall time on a typical read command is Python process/import overhead.
> Roughly 55–70% of total wall time is the single `import prompt_toolkit` statement**, which is
> executed eagerly at module top (`zotero_cli.py:18`, `repl_skin.py`) even for one-shot
> `--json` commands that never open a REPL.

### 3.3 The only genuinely CPU-bound code path

`core/semantic.py` computes cosine similarity in pure Python over every stored vector:

| Library size × dim | Pure-Python cosine cost |
|---|---|
| 1,000 × 768 | ~44 ms |
| 5,754 × 768 | ~261 ms |
| 5,754 × 1536 | ~580 ms |

Affects 2 commands (`item semantic-search`, `item similar`). Even here the operation is
preceded by a network embedding call (10 s timeout), so end-to-end gain is partial.

### 3.4 I/O-bound costs Rust cannot fix

| Operation | Cost | Bound by |
|---|---|---|
| `backup_database` (112 MB copy) | 98 ms | Disk I/O |
| `add doi` / `import doi` bridge call | `wait_seconds=45` | Zotero translators + remote publisher |
| `item find-pdf` | `timeout=30` | Zotero `addAvailablePDF` + remote |
| `collection fetch-pdfs` | 45 s **per item** | Remote OA services |
| `docx zoterify` | seconds–minutes | LibreOffice + Java + GUI automation |
| `sync` | `wait_seconds=30` | Zotero sync server |

---

## 4. Component disposition (A–E)

### A. Rewrite in Rust — high value, low risk

| Component | Rust target | Rationale |
|---|---|---|
| `zotero_cli.py` command tree | `clap` (derive) | 96 commands; startup-dominant; the whole point |
| `utils/zotero_paths.py` | `paths.rs` (+`zip`, hand-rolled INI/pref regex) | Pure fs+parse, fully deterministic, easy to test |
| `utils/zotero_sqlite.py` (read) | `db/read.rs` (`rusqlite` bundled) | 24 commands; mechanical SQL port; typed models |
| `utils/zotero_http.py` | `http.rs` (`ureq`, blocking) | Thin urllib wrapper; three distinct surfaces |
| `core/catalog.py` | `catalog.rs` | Read facade |
| `core/discovery.py` | `runtime.rs` | Process launch + readiness polling |
| `core/session.py` | `session.rs` (`fs2`/`fd-lock`) | JSON state + advisory lock |
| `core/results.py` | `result.rs` | Result envelope + `exit_code_for` — port **first**, it defines the contract |
| `core/audit.py` | `audit.rs` | Append-only JSONL |
| `core/doctor.py` | `doctor.rs` | Health aggregation |
| `core/csl.py` | `csl.rs` (`serde_json`) | Pure data mapping |
| `core/rendering.py` | `rendering.rs` | Local API passthrough |
| `core/notes.py` | `notes.rs` | HTML↔text + connector payloads |
| `core/metrics.py` | `metrics.rs` | 35 LOC HTTP+JSON |
| `core/jsbridge.py` **JS templates** | `bridge/mod.rs` + `bridge/js/*.js` | Port the *templates*; fix D1 via `serde_json`+`JSON.parse` |
| `core/semantic.py` cosine | `semantic.rs` | The one real CPU win (~50×); also fixes D2 |

### B. Keep as Zotero JavaScript / XPI — do not rewrite

| Component | Disposition |
|---|---|
| `plugin/zotero-cli-bridge/bootstrap.js` (71 LOC) | **Ship byte-identical.** Language-agnostic generic eval endpoint. Rewriting it in Rust is impossible (must run inside Zotero) and rewriting it *at all* is unjustified. |
| `manifest.json` | Reuse, but **must change `update_url`** (currently points at upstream's `main`) and consider a distinct addon ID to avoid clobbering an installed upstream plugin. |
| All 33 bridge JS payload bodies | Remain JavaScript. Rust generates them; Zotero executes them. Zotero's item model, translators, `Zotero.Search`, `Zotero.Duplicates`, and `addAvailablePDF` have **no non-JS API**. |

### C. Keep as external service / integration

| Component | Disposition |
|---|---|
| Zotero Connector API, Local API | Keep — HTTP contracts, just re-clientize |
| Crossref, Unpaywall, EuropePMC, NCBI PMC, bioRxiv/medRxiv, arXiv, NIH iCite | Keep — pure HTTP+JSON |
| OpenAI-compatible chat & embeddings | Keep — HTTP; keep `OPENAI_API_KEY`, `ZOTERO_EMBED_*` env contract |
| LibreOffice (`soffice`), Java/`javac`, `osascript` | Keep as subprocess invocations; **do not** attempt to reimplement |
| Zotero desktop launch | Keep as `std::process::Command` |

### D. Remove or simplify

| Component | Recommendation |
|---|---|
| `utils/repl_skin.py` (521 LOC ANSI branding) + `prompt-toolkit` | **Drop from the hot path.** This is the single largest latency cost and delivers zero agent value. If a REPL is retained at all, use `reedline`/`rustyline` and keep it out of one-shot startup. |
| AppleScript GUI-automation bridge fallback (`_execute_applescript`, `_MENU_PATHS`) | **Drop.** Already deprecated upstream; macOS-only; drives menus by localized string; superseded by the XPI. |
| `app check-update` (GitHub version poll) | Simplify or drop — a fork must not poll upstream's version file, and native packaging (Homebrew/Scoop) handles updates. |
| Direct SQLite **writes** (`experimental.py`, 3 commands) | **Do not port the `--experimental` path.** Bypasses Zotero sync bookkeeping (D5), needs Zotero closed, costs a 112 MB copy per write. Two of the three (`collection create`, `item add-to-collection`) already default to the bridge, so nothing is lost. The third (`item move-to-collection`) has no bridge path and must be composed from `add_to_collection` + `remove_from_collection` — see §10.1. |
| `docx prepare-zotero-import` | Upstream's own CLI text says it "has failed in Zotero 9 + LibreOffice testing". Port last or deprecate. |

### E. Needs further investigation

| Item | Question |
|---|---|
| `docx_zoterify.py` + `docx.py` + `docx_static.py` (2,041 LOC) | OOXML field/bookmark surgery + LibreOffice + AppleScript. Highest port risk. Needs a real corpus of `.docx` fixtures before committing. |
| `imports.py` attachment manifest semantics (1,067 LOC) | Partial-success accounting and dedupe rules are subtle; needs golden fixtures. |
| Local API availability | On this machine `local_api_available=False` while connector was `True`. Fallback paths need explicit parity fixtures for both states. |
| Group libraries (11 present here) | `local_api_scope` user-vs-group routing needs multi-library fixtures. |
| Fork divergence | Upstream is active (v1.2.1, 2026-07-28). Need a policy for tracking upstream changes. |

---

## 5. Challenge phase

### C1 — "Rust will make this faster."

**Partly true, and true for the reason that matters — but not for the reasons usually given.**

- **True:** startup is ~95% of wall time for the most-used commands. A Rust binary starts in
  ~3–8 ms vs 137–195 ms. For an agent issuing 20 CLI calls in a session, that is
  ~2.8 s → ~0.15 s. This is a real, defensible, *repeated-invocation* win.
- **Overstated:** SQLite is **not** slow (0.06–1.93 ms on a 112 MB DB). `rusqlite` will not
  produce a measurable user-visible gain. Claiming "faster queries" would be false.
- **Overstated:** localhost HTTP is **not** slow (1.2–3.4 ms). `reqwest`/`ureq` gain nothing.
- **Overstated:** path discovery is **not** slow (0.40 ms).
- **False:** any command bounded by Zotero, LibreOffice, or a remote API (~40 of 96) will show
  **zero** improvement. `add doi` waits up to 45 s on Zotero's translators regardless of language.
- **Genuine CPU win, narrow scope:** `semantic.py` cosine (261–580 ms → ~5 ms), 2 commands.

**Risk if wrong:** building a 13.5k-LOC rewrite to chase a win that a 10-line Python change
mostly delivers.

### C2 — Is a full Rust rewrite justified, or would a cheaper fix capture most of it?

**A cheaper fix captures most of the startup win.** Making `prompt_toolkit` a lazy import
(deferred into `run_repl`) would cut ~73–151 ms — i.e. roughly **55–70% of total wall time** —
for a change of about ten lines, with zero compatibility risk.

| Option | Startup | Effort | Distribution |
|---|---|---|---|
| Python today | 137–195 ms | — | requires Python + pip |
| Python, lazy `prompt_toolkit` | ~60–70 ms | ~10 LOC | still requires Python + pip |
| Rust port | ~3–8 ms | large | **single binary, no runtime** |

**Conclusion:** the startup argument *alone* does not justify the rewrite — the lazy-import fix
takes it from 195 ms to ~65 ms. What the lazy-import fix **cannot** deliver is the user's stated
hard requirement: **no Python/pip/venv on the end-user machine.** That distribution requirement,
not raw speed, is the load-bearing justification.

### C3 — Is command compatibility more important than architectural redesign?

**Yes, decisively.** The project ships a `SKILL.md` (285 lines) consumed by Claude/Cursor agents,
and a `skill_generator.py` that regenerates it. Existing agent prompts, skills, and scripts encode
exact command names, flags, JSON field names, and exit codes. Redesigning the CLI surface would
silently break every deployed agent skill with no compile-time signal. **Compatibility wins;
architectural cleanup is confined to internals.**

Two specific quirks must be preserved *deliberately*, not "fixed":
- `--json` accepted at **any** level (`item find X --json`), via `_JsonAwareGroup`.
- In `--json` mode, errors go to **stdout** as `{"error": "..."}` (D6).

One quirk should be **changed**: bare `zotero-cli` with no subcommand launches the **REPL** and
blocks on stdin. For an agent this is a hang. Recommend: bare invocation prints help and exits 0
(or `--repl`/`repl` required), documented as an intentional break.

### C4 — Is interactive prompt-toolkit functionality worth porting?

**No.** It is the single largest latency cost, it is 521 LOC of ANSI branding, and the stated
optimization target is non-interactive agent use. Recommend: **do not port the REPL in the
initial scope.** If demanded later, add it behind `reedline` in a way that cannot affect one-shot
startup. `session` state (used by non-interactive commands) is separate and **must** be ported.

### C5 — Is async Rust necessary?

**No.** Measured concurrency demand is nil for 94 of 96 commands: one or two sequential localhost
round trips of 1–3 ms each. `tokio` would add compile time, binary size, and complexity for no
measurable gain. Use **blocking `ureq`**.

Two commands (`collection fetch-pdfs`, `collection find-pdfs`) loop per item with a 45 s timeout
each and would benefit from parallelism — but a bounded **thread pool** (`std::thread` or
`rayon`) serves that without an async runtime. **Decision: synchronous, no `tokio`.**

### C6 — Should direct SQLite access remain?

**Split the answer.**

- **Reads: yes, keep.** 24 commands depend on it, it is fast (0.06–1.93 ms), and it is the only
  surface that works when the Local API is disabled — which was the case on this very machine
  (`local_api_available=False`). Removing it would break the tool's core inventory function.
  Carry over `mode=ro&immutable=1`, but **document D4** explicitly.
- **Writes: no, do not port the `--experimental` path.** All three commands bypass Zotero's sync
  bookkeeping, require Zotero closed, and cost a 112 MB file copy each. Two of the three already
  default to the JS bridge, so dropping the flag costs nothing. The third,
  `item move-to-collection`, is SQLite-write-only and must be reimplemented by composing the
  existing bridge primitives — new work, and an approved behaviour change (§10.1).

### C7 — Which Python functionality is expensive or risky to reproduce in Rust?

| Risk | Detail | Mitigation |
|---|---|---|
| **DOCX OOXML surgery** (2,041 LOC) | `xml.etree` round-trips with namespace re-registration, bookmark/field/custom-property rewriting. `quick-xml` is not a drop-in; byte-level output will differ. | Fixture corpus first; compare **semantic** DOCX structure, not bytes. Port last. |
| **`configparser` semantics** | `profiles.ini` parsing incl. `IsRelative`, duplicate-key and case behaviour | Port with explicit fixtures from `_helpers.py` |
| **Python `re` vs Rust `regex`** | `_AUTHOR_YEAR_RE` etc. — no backtracking in Rust `regex` | Audit each pattern; most are simple |
| **`html.unescape`** | Full HTML5 named-entity table | Use `html-escape`/`htmlescape` crate; test round-trip |
| **Text encoding fallbacks** | `_read_pref_file` tries utf-8 → utf-8-sig → latin-1 | Replicate explicitly (`encoding_rs`) |
| **`_safe_text_for_stdout`** | `backslashreplace` on non-encodable stdout (Windows cp1252) | Replicate; Windows console fixtures |
| **Windows paths** | UNC (`\\host\share`), `file:///C:/...`, drive letters in `resolve_attachment_real_path` | Dedicated cross-platform path tests |
| **`fcntl.flock`** | POSIX-only advisory lock, silently skipped on Windows | `fs2`/`fd-lock`; preserve "best-effort" semantics |

### C8 — Legal / licensing

- Source is **Apache-2.0**; a Rust port derived from reading and translating this code is a
  **derivative work**. Obligations: ship the Apache-2.0 text, retain copyright/patent/trademark
  notices, and **state prominently that files were changed** (§4b).
- There is **no `NOTICE` file**, so no NOTICE-propagation obligation.
- Redistributing `bootstrap.js`/`manifest.json` verbatim carries the same Apache-2.0 terms.
- **`manifest.json` `update_url` points at upstream's repository.** A fork that ships it unchanged
  would have users silently auto-updated to *upstream's* plugin. **Must be changed.**
- Apache-2.0 permits static linking into a distributed binary; Rust crate licenses
  (MIT/Apache-2.0 typically) are compatible. Generate a third-party license bundle at release.
- Recommend a distinct binary name to avoid PyPI/command collision, with `zotero-cli` as an
  opt-in alias.

### C9 — Fork divergence

Upstream shipped v1.2.1 one month before this analysis and averages meaningful monthly activity.
A Rust fork accrues a permanent tracking cost. The port must either (a) declare a pinned
compatibility target (v1.2.1) and track deliberately, or (b) accept drift. **Recommend (a),
with the command-compatibility matrix as the diff surface.**

---

## 6. Decision matrix

| # | Decision | Source's way | Recommended for Rust | Confidence |
|---|---|---|---|---|
| 1 | Zotero write path | JS bridge (33 cmds) + experimental SQLite (3) | **Keep JS bridge; drop experimental SQLite writes from v1** | High |
| 2 | XPI plugin | 71-LOC generic eval endpoint | **Reuse byte-identical; change `update_url` + addon id** | High |
| 3 | JS payload generation | Python f-strings, `'`-only escaping | **Rust templates + `serde_json` params via `JSON.parse`** (fixes D1) | High |
| 4 | SQLite reads | `sqlite3`, `mode=ro&immutable=1` | **Keep; `rusqlite` bundled**; document D4 | High |
| 5 | HTTP client | stdlib `urllib`, blocking | **`ureq` blocking; no `tokio`** | High |
| 6 | CLI framework | `click` + custom `_JsonAwareGroup` | **`clap` derive + global `--json` on every subcommand** | High |
| 7 | REPL | `prompt-toolkit` + 521-LOC skin, eager import | **Omit from v1**; if added, `reedline`, lazily | High |
| 8 | Async | n/a | **Synchronous; bounded threads for PDF batch loops** | High |
| 9 | Error model | `ClickException`/`RuntimeError`, JSON→stdout | **`anyhow`+`thiserror`; preserve stdout-JSON quirk & exit codes** | High |
| 10 | Command surface | 96 commands, 14 groups | **Preserve names/flags/JSON keys exactly**; break only bare-invocation REPL | High |
| 11 | DOCX chain | `zipfile`+ET+LibreOffice+Java+AppleScript | **Defer to final phase**; keep external processes | Medium |
| 12 | Semantic search | Pure-Python cosine, f-string SQL | **Rust cosine (~50×); bound SQL params** (fixes D2) | High |
| 13 | Scope shape | — | **Incremental port, not big-bang**; Python stays until parity | High |
| 14 | Distribution | PyPI + pip | **GitHub Actions → prebuilt binaries; Homebrew/Scoop; no Rust for users** | High |

---

## 7. Answers to the eleven required questions

1. **Is the Rust port technically worthwhile?** — **Yes, but the justification is distribution,
   not speed.** A single self-contained binary removes Python/pip/venv from the end-user path
   entirely. The startup win is real (137–195 ms → 3–8 ms) but ~55–70% of it is also reachable
   from Python with a ten-line lazy-import fix.

2. **What concrete improvements will Rust provide?** — (a) no runtime install; (b) ~20–40×
   startup, compounding across repeated agent invocations; (c) ~50× on semantic cosine;
   (d) compile-time-enforced JSON schemas via `serde`; (e) fixes D1 (JS injection) and D2 (SQL
   injection) structurally rather than by patching.

3. **Which expected improvements are overstated?** — SQLite speed (0.06–1.93 ms already), HTTP
   speed (1.2–3.4 ms on localhost), path discovery (0.40 ms), and *anything* bounded by Zotero,
   LibreOffice, or remote APIs (~40 of 96 commands gain nothing).

4. **What should remain JavaScript?** — The 71-LOC XPI verbatim, and every bridge payload body.
   Zotero's item model, translators, `Zotero.Search`, `Zotero.Duplicates`, and `addAvailablePDF`
   have no non-JS API.

5. **What should remain an external integration?** — Connector API, Local API, all scholarly APIs,
   OpenAI-compatible endpoints, LibreOffice, Java, `osascript`, Zotero launch.

6. **Full rewrite, incremental port, or thin frontend?** — **Incremental port.** A thin Rust
   frontend shelling out to Python would *keep* the Python dependency and *add* a process spawn,
   defeating both goals. Big-bang risks the 2,041-LOC DOCX chain.

7. **Minimum viable Rust port?** — `results` + `paths` + `sqlite(read)` + `http` + `catalog` +
   `session` + `bridge` + the 24 SQLite read commands + the ~20 highest-value bridge commands
   ≈ **45–50 of 96 commands, ~30% of the LOC, covering the overwhelming majority of agent calls.**

8. **Highest-risk modules?** — `docx_zoterify.py` (937), `docx.py` (848), `imports.py` (1067),
   `docx_static.py` (256); then Windows path handling and `jsbridge` escaping semantics.

9. **What migrates first?** — `core/results.py`. It defines the result envelope and exit-code
   contract every other command depends on. Then `zotero_paths` → `zotero_sqlite`(read) →
   `zotero_http` → `catalog` → read-only commands.

10. **What constitutes behavioural parity?** — Per command: identical exit code; identical JSON
    keys/types/nesting after normalizing non-deterministic fields (paths, timestamps, PIDs,
    generated keys, backup filenames); stdout/stderr routing preserved *including* the
    JSON-errors-to-stdout quirk; identical env-var and default-path resolution.

11. **When can Python be removed?** — When all commands classified *exact* or *semantic* pass the
    parity harness on macOS + Windows + Linux; the DOCX chain either reaches parity or is formally
    deprecated; prebuilt binaries ship for all target triples; and `SKILL.md` + `skill_generator.py`
    are regenerated against the Rust CLI. Realistically **at the end of the phased plan, not before.**

---

## 8. Risk score

| Dimension | Score (1–5) | Note |
|---|---|---|
| Behavioural surface | **5** | 96 commands, agent-visible JSON contracts |
| Algorithmic complexity | 2 | Mostly I/O orchestration; little real logic |
| External coupling | **5** | Zotero + 3 HTTP surfaces + LibreOffice + Java + 8 remote APIs |
| Test transferability | 2 (good) | 82 tests + fake SQLite/HTTP fixtures directly reusable |
| Cross-platform risk | 4 | Windows UNC/drive-letter paths, console encoding, `flock` |
| Upstream divergence | 3 | Active upstream |
| **Overall** | **3.7 / 5 — Medium-High** | Driven by surface breadth and external coupling, not by algorithmic difficulty |

---

## 9. Recommended scope for `ak:plan`

**Incremental port, compatibility-first, distribution-driven.** Proposed phase reshaping vs. the
user's straw-man (changes justified by the evidence above):

- **Merge** proposed Phases 1+2 (skeleton + config/models) — `paths` and `results` are small and
  are prerequisites for everything.
- **Promote** parity fixtures to Phase 0 and keep the harness running continuously, not as a
  late Phase 8 gate.
- **Demote** the REPL (proposed Phase 7) to *out of scope for v1*, per C4.
- **Demote** experimental SQLite writes out of v1 entirely, per C6.
- **Split** the DOCX work into its own late phase with its own go/no-go gate, per C7.
- **Add** an explicit phase for the JS-template port with an injection-hardening objective (D1).
- **Move** cross-platform packaging earlier — it is the *reason* for the port and should be
  proven on a trivial binary before 13.5k LOC depend on it.

---

## 10. Challenge gate — APPROVED decisions (2026-08-29)

| # | Decision | Approved outcome |
|---|---|---|
| 1 | **Port scope** | **Incremental port, full-parity goal.** Python retained until parity proven. MVP milestone at ~45–50 commands, then continue. |
| 2 | **REPL** | **Dropped from v1 entirely.** `prompt-toolkit` and `repl_skin.py` are not ported. `core/session.py` **is** ported (non-interactive commands depend on it). Revisit behind `reedline` post-v1 only on demand. |
| 3 | **v1 exclusions** | (a) Experimental direct-SQLite writes — 3 commands; (b) `docx prepare-zotero-import`; (c) the **full DOCX zoterify chain** (LibreOffice + Java + AppleScript). |
| 4 | **Bare invocation** | **Intentional break approved.** `zotero-cli` with no subcommand prints help and exits 0 instead of entering a blocking REPL. |

### 10.1 Exclusion boundary — exact command lists

Derived by static analysis of external-process usage per function (not guessed).

**Excluded from v1 — the `--experimental` direct-SQLite write path (3 commands affected):**

> **Correction.** An earlier draft of this report stated that all three commands have working
> JS-bridge equivalents upstream. Verification against the source shows that is true for only two
> of them. The three are **not** uniform:

| Command | Upstream default path | Effect of dropping `--experimental` |
|---|---|---|
| `collection create` | JS bridge is already the default (`zotero_cli.py:810-829`); `--experimental` is opt-in | **None on the default path.** Only the offline SQLite mode is lost. |
| `item add-to-collection` | JS bridge is already the default (`zotero_cli.py:1348-1353`) | **None on the default path.** |
| `item move-to-collection` | **No bridge path at all.** `_require_experimental_flag` fires unconditionally (`zotero_cli.py:1371`); implemented solely as a direct SQLite write requiring Zotero closed | **The command would have no implementation.** v1 must compose it from the existing bridge primitives `add_to_collection` + `remove_from_collection`. |

The composed implementation is strictly more usable — it works while Zotero is running and needs no
`--experimental` flag — but it is **new work and a real behaviour change**, not a like-for-like port.
It is therefore classified **Changed**, and is the second approved intentional break alongside the
bare-invocation change.

All three commands remain available in v1.

**Excluded from v1 — LibreOffice / Java / AppleScript dependent (7):**
`docx zoterify` · `docx zoterify-preflight` · `docx zoterify-probe` · `docx doctor` ·
`docx cite` · `docx insert-citations` · `docx prepare-zotero-import`
(backing fns: `zoterify_document`, `zoterify_preflight`, `zoterify_probe`, `zoterify_doctor`,
`cite_document`, `prepare_zotero_import_document`)

**Retained in v1 — pure-OOXML DOCX (4 commands):**
`docx inspect-citations` · `docx inspect-placeholders` · `docx validate-placeholders` ·
`docx render-citations`
(backing fns: `inspect_citations`, `inspect_placeholders`, `validate_placeholders`,
`render_static_citations`, `build_working_docx` — none invoke a subprocess)

> These four are `zipfile` + `ElementTree` only. They are genuinely portable, useful to agents,
> and carry none of the GUI-automation risk. Excluding them would have been over-broad.

### 10.2 Resulting v1 command budget

| Bucket | Count |
|---|---|
| **Ported in v1** | **88** |
| Deferred to the gated post-v1 phase — external-process DOCX | 7 |
| Dropped — `repl` | 1 |
| **Total** | **96** |

Compatibility classes across all 96: **34 Exact**, **52 Semantic**, **2 Changed**
(bare invocation; `item move-to-collection`), **7 Deferred**, **1 Dropped**.

### 10.3 Standing constraint for planning

Full parity remains the **eventual** goal (decision 1). The 7 excluded DOCX commands are therefore
a **deferred post-v1 phase with its own go/no-go gate**, not a permanent cut. Until that gate
passes, those commands remain Python-only and must be documented as such.
