---
phase: 5
title: "HTTP Surfaces, Connector, Local API and Runtime"
status: complete
priority: P1
effort: "4-6d"
dependencies: [4]
---

# Phase 5: HTTP Surfaces, Connector, Local API and Runtime

## Overview

Port the three HTTP surfaces that share port 23119, plus runtime discovery, health checks, session
state and the audit log. Completes the `find_items` Local-API path deferred from Phase 4.

Delivers `app *`, `session *`, `audit *`, `search items`, `export bib`, `item export|citation|bibliography`,
`collection use-selected`.

## Progress: Connector client slice landed

Phase 5C adds the missing Connector API transport primitives only:
`getSelectedCollection`, `connector/import`, `saveItems`, `saveAttachment`, and `updateSession`.
The implementation stays below command dispatch and does not implement Phase 6 writes, JS Bridge,
XPI, Phase 7 ingest workflows, or direct SQLite writes. Deterministic raw TCP tests verify method,
path/query, headers, bodies, status-code expectations, object/list import response normalization,
Python-style Connector error messages, transport-error path prefixes, and Zotero 10 header-hardening
constraints. This completes the Connector client criterion only; the broader Phase 5 command,
Local-API routing, latency, and stale re-resolution criteria remain open below.

## Progress: session-state slice landed, one architectural bug found and fixed

**8 of the 9 `session *` commands are landed and verified Exact**: `status`, `use-library`,
`use-collection`, `use-item`, `clear-library`, `clear-collection`, `clear-item`, `history`. Only
`session use-selected` remains, blocked on this phase's connector `getSelectedCollection` client
(same blocker as `collection use-selected`, Phase 4's `session use-selected` note, and this phase's
own Related Code Files list).

`session.rs` gained `save_session_state` (locked write via `fd-lock`, a new dependency — small,
focused, matches Python's best-effort `fcntl.flock` semantics exactly: a lock failure does not fail
the write, only skips the mutual exclusion it would have provided), `append_command_history`
(reloads state from disk rather than using a caller-supplied copy, matching Python — each CLI
invocation is a fresh process, so "current state" only ever means "what's on disk right now"), and
`build_session_payload`. Added `python_negative_tail_slice` for `session history`'s `[-limit:]` slice
shape — a *different* shape from `python_slice_to_limit`'s `[:limit]` (used by `item find`/`item
list`), with its own three-way behavior (`limit == 0` returns the entire list, since `-0 == 0` in
Python) — 6 unit tests.

**A real architectural bug was found and fixed, not just new commands added.** `dispatch_command`
built `RuntimeContext` — including the 2-call connector/Local-API HTTP probe — unconditionally for
*every* command, before this slice. This was invisible through 23 previously-landed commands because
every one of them legitimately needed the runtime (SQLite path or HTTP). The first commands that
don't (`session status`, `use-collection`, `use-item`, `clear-*`, `history` — pure local-state
operations with zero Zotero dependency in Python) immediately surfaced it as a real harness
`Mismatch`: Rust emitted 2 `http_calls` where Python's golden shows `http_calls: []`. Fixed by making
`runtime` construction a lazy closure (`build_runtime`) called only inside the match arms that
actually use it — 20 call sites now call it explicitly, matching Python's `current_runtime(ctx)`
being invoked per-command-handler rather than unconditionally at the top of dispatch. All 23
previously-landed commands re-verified Exact after the fix (zero regressions); `session use-library`
correctly still shows 2 `http_calls` (it needs SQLite lookup via `catalog::resolve_library_id`,
routed through the same runtime as everything else).

**Verified beyond the golden fixtures, again, on purpose:** the harness runs one command per fresh
fixture — it cannot prove that a *separate* subsequent process reads back what an earlier process
wrote. Manually verified real cross-process persistence: `session use-library 2` in one invocation,
then a fresh `session status` invocation against the same `CLI_ANYTHING_ZOTERO_STATE_DIR` correctly
shows `current_library: 2`; a sequence of `use-collection`/`use-item`/`status` correctly accumulates
`history_count`; a failing `use-library 999` (library not found) correctly leaves prior state
untouched rather than partially corrupting it (the SQLite lookup errors via `?` before any field
assignment or save happens); `history_count` in a write command's *own* response reflects the count
*before* that command's own history line is appended (matching Python's exact ordering — `state` is
saved and returned as a local copy, while `append_command_history` reloads-and-re-saves internally,
so the write's own echoed payload doesn't yet include its own history entry).

## Requirements

**Functional**
- Connector API client: `ping`, `getSelectedCollection`, `import`, `saveItems`, `saveAttachment`, `updateSession`
- Local API client with `Zotero-API-Version: 3`
- Runtime context: probe both surfaces, build status payload
- Zotero launch with platform-specific handling and readiness polling
- Session state with advisory locking and 50-entry command history
- Append-only audit log
- `app doctor` health aggregation

**Non-functional**
- Probe cost within 2× of Python's measured 1.2–3.4 ms
- Runtime context built lazily — commands that need no runtime must not pay for probes

  **Clarified during the Phase 3 vertical slice:** verified against both Python source
  (`current_runtime()`, `zotero_cli.py:235-244`) and every golden fixture in `harness/golden/python/`
  that "lazily" means *built on first access, not eagerly at process startup* — not *skipped for
  commands that don't use the HTTP result*. Every command handler calls `current_runtime(ctx)`
  unconditionally as its first action (even pure-SQLite ones like `item list`/`item get`/
  `collection list`, because they need `environment.sqlite_path`), so the two probes fire on every
  real command invocation in Python, confirmed by `http_calls` always containing exactly
  `[connector/ping, /api/]` regardless of command. The Rust vertical slice (`runtime.rs`) replicates
  this exactly — probes run once per invocation for any real command, skipped only for `--help`/
  bare invocation, which never build a runtime at all. Making probes conditional on whether a command
  *uses* the HTTP result would diverge from Python's observed `http_calls` and break the Exact-class
  parity bar (`compare.py` treats `http_calls` as part of the equality check), so that reading of
  "lazy" is not implemented and should not be for Exact-class commands.

## Architecture

```
crates/zotero-cli/src/
  http/
    mod.rs           # request builder, HttpResponse, error mapping
    connector.rs     # /connector/*
    local_api.rs     # /api/*
  runtime.rs         # RuntimeContext, launch, readiness waits
  session.rs         # session state + file lock
  audit.rs           # append-only JSONL
  doctor.rs          # health aggregation
```

### Zotero 10 additions to this phase (added 2026-08-29)

Three items land here rather than in Phase 14, because they extend surfaces this phase already owns.
Evidence: [`plans/research/zotero-10-impact-on-rust-port.md`](../../research/zotero-10-impact-on-rust-port.md).

**1. Capture `Zotero-Server-ID` on the Local API probe.** Zotero 10 returns this header on every
local API response. It is (a) the cleanest 10+ capability discriminator and (b) mandatory on writes
in Phase 6. Capture it during the probe already performed here — no extra round trip. Extend
`RuntimeContext` with `server_id: Option<String>` and `local_api_writes_available: bool`.

Prefer header presence over `environment.version` for capability detection: the version field
reflects the *installed binary discovered on disk*, which can disagree with the *running instance*
that owns port 23119 (multiple installs, portable builds, a stale `application.ini`).

**2. HTTP hardening conformance.** Zotero 10 requires `Host` ∈ {`localhost`, `127.0.0.1`, `[::1]`}
(else `400`), and **drops without response** any request with a `Mozilla/`-prefixed `User-Agent` or
*any* `Origin` header, unless it carries `Zotero-Allowed-Request`. This previously applied only to
CORS-simple content types, so a JSON POST that worked before may now be rejected.

Our client passes today by accident, not design (`ureq` default UA, no `Origin`). Add a regression
test asserting no `Mozilla/` UA and no `Origin` on Zotero-local requests, so a future change fails
loudly. **Do not** generalise the `Mozilla/5.0` UA that `metrics.py` uses for NIH iCite — that is an
external host and must stay scoped to it.

**3. `use-selected` under Zotero 10 multi-selection.** Zotero 10 lets users select multiple
collections, saved searches, **and libraries** simultaneously. `ZoteroPane.getSelectedCollection()`,
`getSelectedLibraryID()` and `getSelectedSavedSearch()` are **removed** — the singular getters now
throw, naming their plural replacements.

This does **not** automatically break `collection use-selected` / `session use-selected`, because
Python reaches selection through the **connector** endpoint `/connector/getSelectedCollection`, not
`ZoteroPane`. Whether that endpoint still exists in 10, and what it returns under multi-selection,
is **Open Question 3** in Phase 14 and must be answered before these two commands are implemented.

Decision matrix, to be resolved by OQ3's answer:

| If the connector endpoint… | Then `use-selected` should… |
|---|---|
| still returns a single collection | keep current semantics; document that it reflects Zotero's own choice under multi-selection |
| returns an array / errors on multi-select | take the **first** selected collection, and emit a `selection_count` field so an agent can detect ambiguity rather than silently acting on one of several |
| no longer exists | reimplement via JS Bridge using `ZoteroPane.getSelectedCollections()` (plural), same first-plus-count semantics |

In all three cases: **never silently pick one row out of several without signalling it.** Zotero's
own rationale for removing the singular getters was "so that plugins don't silently try to operate
on one arbitrary row" — the CLI should honour that intent. Also note collections and saved searches
can now be selected together, so a selected row that is not a library is **not necessarily** a
collection.

### Three surfaces, three contracts

| Surface | Base | Headers | Success status | Notes |
|---|---|---|---|---|
| Connector | `/connector/*` | `Content-Type` varies | **201** for `import` and `saveItems`; 200 for others | `saveAttachment` sends raw PDF bytes with an `X-Metadata` JSON header |
| Local API | `/api/*` | `Zotero-API-Version: 3` | 200 | 403 means "disabled", distinct from unavailable |
| CLI Bridge | `/cli-bridge/eval` | `text/plain` | 200 | Phase 6 |

`local_api_is_available` distinguishes three states (`zotero_http.py:196-205`):
`200` → available; `403` → `"local API disabled"`; anything else → `"local API returned HTTP {n}"`.
Connection failure → the transport error string. These message strings appear in `app status` JSON
and are part of the contract.

> The analysis machine had `connector_available=true` with `local_api_available=false`. This is a
> normal, common state — not an error. Both must be first-class.

### Accepted divergence: transport error text when Zotero is completely unreachable (decided)

**Decision (2026-08-29), made directly by the user reviewing the Phase 3 vertical-slice journal:**
when nothing is listening on the Zotero port at all — as opposed to Zotero running but the Local API
disabled (403), which stays byte-identical per the table above — `connector_message` and
`local_api_message` carry the underlying transport library's exception text (Python's
`urllib.error.URLError` string form vs Rust's `ureq::Error` `Display`). These are OS- and
library-specific (`Connection refused`, `WinError 10061`, errno numbers, wrapper class names) and
**must not** be chased to byte-identity — doing so would mean reimplementing one transport library's
exception prose inside another language's HTTP client, which is not a real compatibility contract.

**Scope of the accepted divergence — narrow, not a downgrade of `app status`:**

- A new standing parity fixture, `zotero-unreachable`, added to `harness/commands.tsv` and
  `harness/fixtures/build_fixture.py`/`capture.py`: no fake HTTP server is started at all; the CLI
  under test is pointed at a real closed TCP port, producing a genuine OS-level connection refusal
  for both implementations.
- `harness/normalize.py` normalizes **only** the `connector_message` and `local_api_message` JSON
  field values, and **only** for captures whose `fixture_state == "zotero-unreachable"`, to the
  shared token `<CONNECTION_REFUSED>`. No other field, and no other fixture state, is touched by this
  rule — it must never be widened into a general "normalize error text" behavior.
- Everything else about the fixture stays a real, unweakened comparison: JSON key set and order,
  `connector_available: false` / `local_api_available: false`, `http_calls: []`, and exit code 0 are
  all required to match exactly.
- `app status`'s existing deterministic fixtures (`local-api-off`, etc., where a real fake server
  responds with a real HTTP status) are **unaffected** and remain **Exact** with no normalization.

Implemented and verified: `harness/golden/python/app__status__(unreachable).json` captured against
the real Python reference, then re-captured against the Rust vertical-slice binary and confirmed
**Exact** via `harness/compare.py` (both message fields normalize to the same token; everything else
matches natively). This closes the divergence identified in the Phase 3 vertical-slice journal
(`plans/journals/260829-1200-vertical-slice-read-commands-ported.md`) as a "product decision," not
left open.

### Timeouts

Transcribe Python's per-call timeouts exactly; they are behavioural:

| Call | Timeout |
|---|---|
| `connector_ping`, `local_api_root` | 3 s |
| `getSelectedCollection`, generic request | 5 s |
| `updateSession` | 15 s |
| `local_api_get_json` | 10 s |
| `local_api_get_text` | 15 s |
| `saveItems` | 20 s |
| `saveAttachment` | 60 s |
| `connector_import` | 120 s |

### Lazy runtime

Python builds `RuntimeContext` lazily via `current_runtime(ctx)` and caches it on the root context
(`zotero_cli.py:235-244`). Commands like `session status` and `audit path` never touch it.

Reproduce this: build the runtime on first use, cache in a `OnceCell`. Doing it eagerly would add
~4 ms to every command — small, but it is exactly the kind of regression this port exists to avoid.

### Session state

Port `core/session.py` faithfully:
- state at `~/.config/cli-anything-zotero/session.json` on **all** platforms (see Phase 3)
- `command_history` capped at the last 50 entries
- `session_library_id()` returns the default when `current_library` is `None` **or empty string** —
  this exact bug was fixed upstream in v1.2.1 (issue #5); do not reintroduce it
- advisory lock: Python uses `fcntl.flock` and silently continues when unavailable (Windows). Use
  `fs2`/`fd-lock` and preserve the best-effort semantics — a lock failure must not fail the command

### Zotero launch

`discovery.launch_zotero` (`discovery.py:61-96`): on macOS, walk parents of the executable to find
the enclosing `.app` bundle and launch via `open`; otherwise spawn the executable directly. Then poll
`/connector/ping` and, only if `local_api_enabled_configured`, poll `/api/`.

## Related Code Files

- Create: `src/http/mod.rs`, `connector.rs`, `local_api.rs`
- Create: `src/runtime.rs`, `session.rs`, `audit.rs`, `doctor.rs`
- Modify: `src/catalog.rs` (wire the Local API path in `find_items`)
- Create: `tests/http_surfaces.rs`, `tests/session.rs`, `tests/runtime.rs`
- Reference: `utils/zotero_http.py`, `core/discovery.py`, `core/session.py`, `core/audit.py`, `core/doctor.py`, `core/rendering.py`

## Implementation Steps

1. Implement `http/mod.rs` on `ureq`: URL building with `doseq`-style repeated params, header merge,
   body handling, and error mapping. Match Python's behaviour of returning `HttpResponse` for HTTP
   error statuses but raising a transport error for connection failures.
2. Implement `connector.rs` with the exact status-code expectations (201 vs 200).
3. Implement `local_api.rs` with the version header and three-state availability.
4. Implement `runtime.rs`: lazy `RuntimeContext`, `to_status_payload`, launch, readiness waits.
5. Implement `session.rs` with locking, history cap and the `session_library_id` null/empty guard.
6. Implement `audit.rs` — append-only JSONL, honour `ZOTERO_CLI_AUDIT_DIR`, and reproduce the
   write-action prefix detection from `zotero_cli.py:263-289`.
7. Implement `doctor.rs` and wire `exit_code_for` on its payload.
8. Port `core/rendering.py`: `item export|citation|bibliography` and `export bib` via Local API,
   including `local_api_scope` user (`/api/users/0`) vs group (`/api/groups/<id>`) routing.
9. Complete `catalog.find_items`: Local API first, re-resolve keys against SQLite, fall back to
    SQLite title search when empty.
10. Add the mixed-surface stale-read fixture: Local API returns a key that immutable SQLite cannot
    resolve. The result must expose the unresolved key count or warning instead of silently returning
    the SQLite fallback as if Local API found nothing.
11. Extend the Phase 1 fake HTTP server to cover all three surfaces and both Local-API states.

## Success Criteria

- [ ] All Phase-5 commands reach their target class against golden outputs, in **both** Local-API-on and Local-API-off fixture states
- [x] Connector 201-vs-200 expectations match Python exactly — Phase 5C transport primitives
      implemented and covered by `crates/zotero-cli/tests/connector_http.rs`: `connector/import`
      and `saveItems` require 201, `saveAttachment` accepts 200/201, `updateSession` requires 200,
      and `getSelectedCollection` requires 200. No higher-level `add`/`import`/`note` command
      dispatch is claimed here.
- [ ] `local_api_is_available` produces Python-identical message strings for 200 / 403 / other (HTTP
      response cases); the fully-unreachable transport case is an accepted, narrowly-scoped divergence
      — see "Accepted divergence" above — verified via the `zotero-unreachable` harness fixture, not
      chased to byte-identity
- [ ] Group-library routing produces `/api/groups/<libraryID>` and user routing `/api/users/0`
- [x] `session status` completes without building a runtime context — verified the hard way: an
      earlier version built it unconditionally, and this exact criterion is what the resulting
      `http_calls` harness `Mismatch` caught. Fixed (`build_runtime` lazy closure); `audit path` not
      yet ported, so only half this criterion's named commands exist yet.
- [x] `session_library_id` returns the default for `None` and `""`, and errors (not silently
      defaults) on a corrupted value — regression test for upstream issue #5, added the prior session
- [x] Session file locking degrades gracefully without failing the command — 2 direct tests
      (`save_and_load_and_append_history_round_trip_through_a_real_locked_file`,
      `append_command_history_caps_at_50_entries`) exercise the real `fd-lock`-backed write path
      through `cargo test`, so CI's Windows leg proves this, not just documents the intent
- [ ] `command_history` capped at 50
- [ ] Probe latency within 2× of the Python baseline
- [ ] `find_items` Local-API path and SQLite fallback both exercised and Exact
- [ ] `find_items` stale Local-API re-resolution gap is exercised and reported without silent false negatives

## Risk Assessment

| Risk | Mitigation |
|---|---|
| `ureq` treats 4xx/5xx as errors while `urllib` returns a response object | Explicitly map `ureq::Error::Status` back into `HttpResponse`; test against 403 and 500 |
| Timeout drift changes behaviour under slow Zotero | Timeouts are transcribed into named constants and unit-asserted |
| Eagerly building runtime regresses startup | Test asserts zero HTTP traffic for state-only commands |
| Windows lacks `flock` semantics | Best-effort locking preserved; tested on Windows runner |
| Query-param encoding differs from `urlencode(doseq=True)` | Test repeated params (`?tag=a&tag=b`) explicitly |
| Local API disabled on the dev machine hides bugs in that path | Both states are mandatory fixture states from Phase 1 |
