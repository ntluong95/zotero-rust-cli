---
phase: 5
title: "HTTP Surfaces, Connector, Local API and Runtime"
status: todo
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
- [ ] Connector 201-vs-200 expectations match Python exactly
- [ ] `local_api_is_available` produces Python-identical message strings for 200 / 403 / other (HTTP
      response cases); the fully-unreachable transport case is an accepted, narrowly-scoped divergence
      — see "Accepted divergence" above — verified via the `zotero-unreachable` harness fixture, not
      chased to byte-identity
- [ ] Group-library routing produces `/api/groups/<libraryID>` and user routing `/api/users/0`
- [ ] `session status` and `audit path` complete without building a runtime context (asserted by test: no HTTP probe issued)
- [ ] `session_library_id` returns the default for `None` and `""` — regression test for upstream issue #5
- [ ] Session file locking degrades gracefully on Windows without failing the command
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
