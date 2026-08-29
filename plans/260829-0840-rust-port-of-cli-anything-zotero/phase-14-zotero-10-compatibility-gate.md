---
phase: 14
title: "Zotero 10 Compatibility Gate"
status: todo
priority: P1
effort: "3-5d"
dependencies: [4, 5]
---

# Phase 14: Zotero 10 Compatibility Gate

> **Executes between Phase 5 and Phase 6, despite its number.** `ak plan add-phase` appends
> sequentially; phase *number* has never equalled execution order in this plan (Phase 8 shipped
> before Phases 5–7 by design). The dependency graph in `plan.md` is authoritative.
>
> **This phase blocks Phase 6.** Nothing in Phase 6 may start until Gate 0 (below) passes.

## Overview

Zotero 10 shipped 2026-08-17 — after this plan was written and after 31 commands landed against
Zotero 7/8/9 assumptions. Two of its changes are **CRITICAL**: one silently corrupts results from
already-landed, already-"Exact"-verified read commands; the other makes the CLI Bridge XPI
unloadable, invalidating Phase 6's premise.

This phase makes the existing port correct on Zotero 10 **before** any write code is written, and
resolves the six open questions that Phase 6's redesign depends on.

Full evidence: [`plans/research/zotero-10-impact-on-rust-port.md`](../../research/zotero-10-impact-on-rust-port.md).

## Requirements

**Functional**
- WAL-safe SQLite reads; all 24 landed read commands return complete data on a Zotero 10 database
- **The SQLite connection policy (layer A, `connect_readonly`) and the command-level read-backend
  policy (layer B, per-command Local-API-vs-SQLite routing) are specified separately — see §1b/§1c.
  This phase's job is to produce and resolve the Read-backend matrix (§1c); wiring layer B's routing
  into `catalog.rs` is out of scope for this pass and tracked as separable follow-up work.**
- Harness gains a WAL-mode fixture state; parity re-verified against it
- XPI loads on Zotero 10 (`strict_max_version` → `10.0.*`, plus the already-planned fork changes)
- Runtime capability detection distinguishes Zotero 10+ from ≤9
- HTTP client provably never trips Zotero 10's browser-origin rejection
- Six open questions (§Open Questions) answered against a live Zotero 10
- Every `TBD` cell in the Read-backend matrix (§1c) resolved against a live Zotero 10, with the same
  live-verified discipline as the Open Questions

**Non-functional**
- Zotero 7/8/9 must keep working — this is a compatibility *gate*, not a cutover
- No regression in the 31 currently-Exact commands on either Zotero version

## Architecture

### 1. WAL-safe SQLite reads (CRITICAL — fixes silent data loss)

**Current** (`db::connect_readonly`):

```rust
let uri = format!("file:{posix_path}?mode=ro&immutable=1");
```

**The bug, reproduced 2026-08-29 with plain SQLite:**

```
journal_mode : wal          -wal size: 4152      writer sees: 5 rows
mode=ro&immutable=1  ->  1 rows    <-- CURRENT db.rs (80% silent loss, exit 0)
mode=ro              ->  5 rows    <-- correct
```

`immutable=1` promises SQLite the file cannot change, so SQLite skips WAL recovery entirely and
never reads `-wal`. Under Zotero 10's WAL mode, every uncheckpointed commit is invisible. No error
is raised.

**Why every green harness run missed it:** `harness/fixtures/build_fixture.py` builds fixtures with
Python `sqlite3.connect()`, which defaults to **rollback-journal** mode. No `-wal` file exists in
any fixture, so `immutable=1` is harmless there. Every Exact result to date was measured against a
non-WAL database. This is the same "fixture can't reach the condition" category as the
`resolve_attachment_real_path` and lazy-runtime gaps — but with the largest blast radius yet.

**Fix:** drop `immutable=1`. Open `mode=ro` and let SQLite read the WAL.

```rust
// Zotero 10 enables WAL. `immutable=1` makes SQLite ignore -wal entirely,
// silently returning only checkpointed rows. Must NOT be reintroduced.
let uri = format!("file:{posix_path}?mode=ro");
```

**Known risk requiring live verification (Open Question 1):** a read-only WAL connection needs to
map the `-shm` shared-memory file. If Zotero holds the DB in a way that prevents this, SQLite can
fail `SQLITE_CANTOPEN`. Fallback ladder, in preference order:

| Option | Trade-off |
|---|---|
| `mode=ro` (preferred) | Correct + concurrent. Needs `-shm` access. |
| `mode=ro` + `PRAGMA query_only` | Same, belt-and-braces against accidental writes. |
| `immutable=1` **only when no `-wal` file exists** | Correct on Zotero ≤9; must refuse on 10. Requires a runtime check, not a constant. |
| Route reads through Local API | Always correct; loses the "works with Local API disabled" property that motivated SQLite reads. |

Do **not** ship a silent fallback to `immutable=1` when `-wal` is present — that reintroduces the
exact bug. If `mode=ro` fails on a WAL database, fail loudly with an actionable message.

> **CORRECTION (2026-08-29, post-recovery) — Open Question 1 is now answered, and it changes the
> default above.** Live testing against a real, running Zotero 10.0.1 instance found that `mode=ro`
> does not merely *risk* `SQLITE_CANTOPEN` under some conditions — it **reliably fails with
> `SQLITE_BUSY`** every time Zotero holds the database (5+ consecutive attempts, up to a 5 s busy
> timeout, including a bare `SELECT 1`). Full reproduction:
> [`plans/research/zotero-10-impact-on-rust-port.md`](../../research/zotero-10-impact-on-rust-port.md) §7.1.
> Zotero appears to hold its own connection in SQLite's exclusive locking mode — true on every Zotero
> version, not new in 10 — which is why `immutable=1` was chosen originally.
>
> This means the fallback ladder below is superseded by an explicit product decision (made directly by
> the user reviewing this evidence): **detect Zotero's lock via the actual `mode=ro` open attempt
> itself** (no separate HTTP probe). If it succeeds, use it — this is the fully-correct WAL-aware path,
> and it is what happens automatically whenever Zotero is not running or no `-wal` file exists (Zotero
> ≤9, zero behavior change). If it fails with `SQLITE_BUSY` specifically, **do not** fall back to
> `immutable=1` silently or by default — refuse with a clear, actionable error, and only use the
> `immutable=1` snapshot behind an explicit `--allow-stale-sqlite` opt-in flag. This is stricter than
> the ladder below (which treated a scoped `immutable=1` fallback as an acceptable rung): the corrected
> policy prefers a loud failure over a possibly-incomplete read whenever Local API is unavailable while
> Zotero runs, consistent with `plan.md`'s Local-API-first direction for Phase 6. The ladder is kept
> below as a record of what was considered before live evidence existed — it is not the shipped design.

> **Supersedes D4.** The plan's original `immutable=1` rationale was "needed so reads work while
> Zotero holds the DB." Under WAL that inverts: WAL *exists* to let readers proceed concurrently
> with a writer, so `immutable=1` no longer buys concurrency — it only buys stale data.
> The `--strict-read` flag proposed in Phase 4 is **cancelled**; `mode=ro` is now the default and
> there is nothing to opt into.

### 1b. Two separate policies — do not conflate them

**CLARIFICATION (2026-08-29, added on review before merge):** the correction above describes only
the **SQLite connection policy** inside `connect_readonly`. Read naively, it invites a wrong
conclusion — "Zotero running + Local API down ⇒ all 24 read commands fail" — that overstates the
gate's actual effect. That conclusion is wrong because it skips a layer: most commands never have to
reach `connect_readonly`'s refusal at all, because a **separate, command-level policy** should route
them to the Local API first whenever Zotero is running and the Local API can answer them. The SQLite
refusal is the *last-resort* behavior for the commands (or code paths within a command) that
genuinely have no Local API equivalent — not the default experience of using the CLI while Zotero is
open. These are two different layers with two different owners, and Phase 14 must specify both, not
just the lower one:

| Layer | Owns | Question it answers | Default when Zotero is running |
|---|---|---|---|
| **A. SQLite connection policy** (`db::connect_readonly`) | `db.rs` | "Can this specific SQLite open be trusted to be complete?" | `mode=ro` if it succeeds; otherwise **refuse**, never a silent `immutable=1` fallback. `immutable=1` survives only for non-WAL databases (Zotero ≤9, unconditionally safe there) and behind the explicit `--allow-stale-sqlite` opt-in — and only if it is kept at all; a future pass may drop it entirely once command-level Local API coverage is wide enough that no command still needs it. |
| **B. Command-level read-backend policy** (`catalog.rs` and callers, above `db.rs`) | `catalog.rs` | "Where should *this command* get its data from, given whether Zotero is running?" | **Prefer the Local API** when Zotero is running and the Local API can represent the command's result. Fall through to layer A's SQLite policy only when Zotero is closed (where `mode=ro` is safe and cheap — no lock holder), or when the command has no Local API representation at all (see the matrix below, which must give each such command an explicit, documented behavior — not a silent stale read). |

Layer A is already specified above and is small in scope (one function, `connect_readonly`). Layer B
is genuinely per-command and is the actual determinant of whether an agent calling `item list` while
Zotero is open gets a fast, fresh Local API answer (the common, desired case) or ever sees layer A's
refusal at all (which should be rare — only for the commands the matrix below marks as
SQLite-only). **Do not implement layer B's routing in this pass** (see the matrix below) — this
phase's job is to specify the decision, not code it; Phase 14's own success criteria track the
specification, and wiring it into `catalog.rs` is separable follow-up work once the matrix's open
items are resolved.

### 1c. Read-backend matrix (new Phase-14 deliverable — specification only, not implemented here)

Required output of this phase: a complete command-by-command matrix covering every one of the 24
landed SQLite-backed read commands, so no command is left to "whatever `connect_readonly` happens to
do" by default. Columns, as specified:

`Command | Zotero running | Zotero closed | stale fallback | parity impact`

Draft below, built from the current `catalog.rs`/`db.rs` implementation and the Local API surface
described in `plan.md`'s Zotero 10 findings — **not yet verified against Zotero 10's actual
documented Local API capabilities for each shape**, which is why several cells are marked `TBD`.
Resolving every `TBD` (empirically, against a live Zotero 10, the same discipline as the Open
Questions) is part of this phase's Implementation Steps and Success Criteria below, not a
prerequisite to recording the matrix itself.

| Command | Zotero running | Zotero closed | Stale fallback | Parity impact |
|---|---|---|---|---|
| `library list` | `TBD`: verify whether Local API exposes a library list with the same fields `fetch_libraries` needs (`editable`, `filesEditable`, `storageVersion`, `archived`); if not, SQLite via layer A | SQLite `mode=ro` (layer A) | `--allow-stale-sqlite` only if layer A's `mode=ro` itself is denied | None if Local API can match the shape; needs a compatibility renderer otherwise |
| `collection list` / `find` / `get` | `TBD`: Local API exposes collections, but `itemCount` aggregation and the exact field set need verification | SQLite `mode=ro` | Same as above | Same as above |
| `collection items` | Local API `GET /collections/<key>/items` likely satisfies the item-listing half; the collection-ref resolution half still needs library/collection lookup (Local API or SQLite, `TBD`) | SQLite `mode=ro` | Same | Needs compatibility renderer (item shape) — see Phase 6 §C2 |
| `collection tree` | `TBD`: Local API's parent/child representation for the tree walk is unverified | SQLite `mode=ro` | Same | None if verified equivalent |
| `item list` / `get` | Local API `GET /items` / `GET /items/<key>` can very likely satisfy this | SQLite `mode=ro` | Same | Needs compatibility renderer (Phase 6 §C2) — Local API's item shape is not `_normalize_item`'s shape |
| `item find` | **Already Local-API-first in landed code** (`catalog::find_items`); SQLite title-search is the existing fallback when Local API returns nothing or `--exact-title` is set | SQLite `mode=ro` title search (existing fallback path) | Extend so the *existing* SQLite fallback also honors layer A's refuse/opt-in policy, not just a fresh direct call | Already handled by the landed Local-API-first branch; no new parity risk beyond what layer A adds to its own SQLite leg |
| `item children` / `notes` / `attachments` | Local API can likely list an item's children; resolving the parent item itself still needs `resolve_item`-equivalent lookup (`TBD` whether fully Local-API-representable) | SQLite `mode=ro` | Same | Needs compatibility renderer; `attachments`' `resolvedPath` is a **local filesystem computation** regardless of backend — never sourced from either API |
| `item file` | Same as `item attachments`, plus a local filesystem `exists()` check that is backend-independent | SQLite `mode=ro` | Same | Same as attachments |
| `search list` / `get` | `TBD` — **highest-priority unresolved item**: Local API's representation of saved-search *conditions* (the actual query definition, not just results) is unverified and may not exist at all. If it does not, this command has **no Local API equivalent** and must get an explicit documented behavior under layer B rather than being silently routed to layer A's refusal | SQLite `mode=ro` | If no Local API equivalent exists, this command needs the `--allow-stale-sqlite` opt-in to remain usable at all while Zotero runs and Local API can't help — that is a real product decision to make explicitly, not default into | None if SQLite-only is the accepted answer; flag in `docs/ZOTERO-COMPATIBILITY.md` either way |
| `search items` | **Already Local-API-only** in landed code (`catalog::search_items` errors today if Local API is unavailable, regardless of whether Zotero is running or closed) | Errors today — no SQLite fallback exists; `TBD` whether Phase 14 should add one or accept the existing hard dependency | N/A — no SQLite path exists to fall back to | No SQLite involvement at all currently; unaffected by layer A |
| `tag list` / `items` | `TBD`: Local API's tag+item-count aggregation shape is unverified | SQLite `mode=ro` | Same | Same |
| `style list` | **No backend decision needed at all** — pure local filesystem CSL parsing, no SQLite and no HTTP in either Zotero state | Same | N/A | None |
| `session status` / `use-collection` / `use-item` / `clear-*` / `history` | **No backend decision needed** — pure local JSON state file, zero SQLite and zero HTTP | Same | N/A | None |
| `session use-library` | Needs library resolution (same `TBD` as `library list`) | SQLite `mode=ro` | Same as `library list` | None if resolved via Local API cleanly |
| `session use-selected` / `collection use-selected` | Not SQLite-backed at all — HTTP-mediated via the connector's `getSelectedCollection`, gated on Open Question 3 (Phase 5's decision matrix). Listed here only to state explicitly that layer A's SQLite policy does not apply to these two commands | Selection state only exists while Zotero runs — command is inherently Zotero-running-only | N/A | Already Semantic/HTTP-mediated class; unaffected by the SQLite connection policy |

**What this table is not:** a claim that any of the `TBD` cells are resolved, or that layer B is
implemented. It is the enumeration of decision points layer B must resolve — each `TBD` becomes a
finding the same way Open Questions 1–6 do, with the same "live-verified, not guessed" discipline.

### 2. WAL-mode harness fixture (makes the fix permanently testable)

Add `wal-mode` to `harness/fixtures/build_fixture.py`:

```python
def _enable_wal(sqlite_path: Path) -> None:
    with sqlite3.connect(sqlite_path) as conn:
        conn.execute("PRAGMA journal_mode=WAL")
        # Leave uncheckpointed commits in -wal so a reader that ignores
        # the WAL observably misses them -- that is the whole point.
        conn.execute("INSERT INTO items VALUES (91, 1, ...)")
```

The fixture must leave rows **in the WAL, uncheckpointed**. A fixture that checkpoints on close
proves nothing.

**Gate criterion:** capture the Python baseline for this state, then run Rust against it. Rust must
match. Then deliberately re-add `immutable=1` and confirm the harness **fails** — a fixture that
cannot fail is not coverage.

### 3. XPI Zotero 10 compatibility (CRITICAL)

Current manifest declares `strict_max_version: "9.0.*"` → the add-on manager refuses it on Zotero
10 → `/cli-bridge/eval` never registers → every bridge command fails.

Combine with the fork changes Phase 6 already required:

| Field | Current | Target | Reason |
|---|---|---|---|
| `strict_max_version` | `9.0.*` | `10.0.*` | Zotero 10 support |
| `strict_min_version` | `6.999` | keep | Zotero 7/8/9 back-compat |
| `update_url` | upstream repo | fork's own, or remove | Must not let upstream push updates to fork users |
| addon `id` | `cli-bridge@cli-anything.dev` | fork-specific | Avoid clobbering an installed upstream plugin |

Zotero's guidance: *"If no changes are required, you can simply update `strict_max_version` in your
plugin's update manifest without releasing a new version."* — but we are changing id/update_url
anyway, so a real release is required.

**Also verify (Open Question 4):** `/cli-bridge/eval` is a custom endpoint receiving a
`text/plain` POST. Zotero 10 drops browser-looking requests (`Mozilla/` UA or any `Origin`) unless
they carry `Zotero-Allowed-Request`, and this check "previously applied only to CORS-simple content
types." Confirm our POST still passes; if not, the endpoint needs
`allowRequestsFromUnsafeWebContent = true` or we send `Zotero-Allowed-Request`.

### 4. Capability detection (new requirement — enables 7/8/9 back-compat)

Local API **writes are Zotero 10+ only**. The port must detect, not assume:

```
GET /api/  →  response carries `Zotero-Server-ID` header?
    yes → Zotero 10+  → Local-API-first write routing (Phase 6)
    no  → Zotero ≤9   → JS-Bridge write routing (XPI required)
```

`Zotero-Server-ID` presence is a documented, behavioural 10+ discriminator — preferable to parsing
`environment.version`, which reflects the *installed binary* found on disk and can disagree with the
*running* instance the HTTP port actually belongs to.

Extend `RuntimeContext` with `local_api_writes_available: bool` and `server_id: Option<String>`,
populated during the existing Local-API probe (no extra round trip). Surface both in `app status`
and `app doctor` — **additive JSON fields**, which is a compatibility change to a currently-Exact
command and therefore requires re-baselining `app status` goldens (see §Compatibility below).

### 5. HTTP-hardening conformance

Zotero 10 hardening (all documented):
- `Host` must be `localhost` / `127.0.0.1` / `[::1]` → else `400`
- `User-Agent` starting `Mozilla/`, or **any** `Origin` header → **dropped without response**
- Custom endpoints may opt out with `allowRequestsFromUnsafeWebContent = true`

Our client currently passes (`ureq` default UA is `ureq/x.y`; we set no `Origin`). This is luck, not
design — lock it in with a regression test asserting outbound Zotero-local requests carry neither a
`Mozilla/` UA nor an `Origin` header.

> Cross-check performed: the Python reference sets `User-Agent: Mozilla/5.0` in `metrics.py`, but
> only for **NIH iCite** (external). Safe to port verbatim; must never be generalised to
> Zotero-local requests. Phase 7 must not "unify" the UA across clients.

### 6. Live Zotero 10 verification

The dev machine runs **Zotero 9.0.6** (`zotero.sqlite-journal` present, no `-wal`). Nothing in this
phase can be signed off from documentation alone. This phase **requires** access to a Zotero 10
install — a VM, a second machine, or an upgrade with a backed-up data directory.

## Related Code Files

- Modify: `crates/zotero-cli/src/db.rs` (`connect_readonly` — remove `immutable=1`)
- Modify: `crates/zotero-cli/src/runtime.rs` (capability detection, `server_id`)
- Modify: `crates/zotero-cli/src/http.rs` (capture `Zotero-Server-ID`; UA/Origin guard)
- Modify: `harness/fixtures/build_fixture.py` (`wal-mode` state)
- Modify: `harness/commands.tsv` (WAL-mode rows for representative read commands)
- Modify: `plugin/assets/manifest.json` (XPI version/id/update_url) — created in Phase 6
- Modify: `harness/golden/python/app__status*.json` (re-baseline for new fields)
- Create: `crates/zotero-cli/tests/zotero10_conformance.rs`
- Create: `docs/ZOTERO-COMPATIBILITY.md`
- Create/maintain: the Read-backend matrix (§1c of this file) — kept here as the living
  specification while its `TBD` cells are resolved; a condensed version is published into
  `docs/ZOTERO-COMPATIBILITY.md` once settled, but this file remains the source of truth during
  Phase 14 itself. **No `catalog.rs` routing code is created from this row in this phase** — that is
  Phase 6-or-later follow-up work once the matrix has no open `TBD`s.

## Implementation Steps

1. **Obtain a Zotero 10 instance.** Back up `~/Zotero` first. Everything below is unverifiable
   without it.
2. Answer Open Questions 1–6 empirically. Record answers in
   `plans/research/zotero-10-impact-on-rust-port.md` §6, converting each from question to finding.
3. Fix `connect_readonly` per OQ1's answer. Add the "never silently fall back to `immutable=1` when
   `-wal` exists" guard.
4. Add the `wal-mode` fixture state; capture Python baselines; verify Rust matches.
5. **Prove the fixture can fail**: re-add `immutable=1`, confirm harness reports Mismatch, revert.
6. Re-run the full 31-command parity suite on both the existing fixtures **and** `wal-mode`.
7. Implement capability detection; extend `app status`; re-baseline its goldens on both Zotero
   versions.
8. Add the UA/`Origin` conformance test.
9. Update the XPI manifest (version, id, update_url); verify it installs and registers on Zotero 10
   **and still on 9**.
10. Live-run all 31 landed commands against real Zotero 10 and real Zotero 9; diff.
11. Resolve every `TBD` in the Read-backend matrix (§1c) against the same live Zotero 10 instance:
    for each, determine whether the Local API can represent the command's result and record the
    finding — do **not** wire any routing code yet, only settle the specification.
12. Write `docs/ZOTERO-COMPATIBILITY.md` documenting the support matrix, the WAL rationale, and the
    settled Read-backend matrix (layers A and B both, clearly distinguished).

## Success Criteria

- [ ] Open Questions 1–6 answered against a live Zotero 10, recorded in the research report
- [ ] `connect_readonly` reads WAL databases completely — verified by the `wal-mode` fixture
- [ ] The `wal-mode` fixture **demonstrably fails** when `immutable=1` is reintroduced
- [ ] All 31 landed commands Exact/Semantic against `wal-mode` fixtures
- [ ] All 31 landed commands verified against a **live Zotero 10** and a **live Zotero 9**
- [x] ~~`immutable=1` appears nowhere in the codebase except a comment explaining why it must not return~~ — **superseded by the 2026-08-29 correction above**: `immutable=1` is retained, by design, for (a) non-WAL databases (Zotero ≤9, always correct there) and (b) the explicit `--allow-stale-sqlite` opt-in on a WAL database. It must never be the silent default when `-wal` is present and `mode=ro` fails.
- [ ] Capability detection correctly reports 10+ vs ≤9 on both live versions
- [ ] `app status` exposes `server_id` + `local_api_writes_available`; goldens re-baselined
- [ ] Conformance test asserts no `Mozilla/` UA and no `Origin` on Zotero-local requests
- [ ] XPI installs and registers `/cli-bridge/eval` on Zotero 10 **and** Zotero 9
- [ ] `/cli-bridge/eval` confirmed reachable under Zotero 10 HTTP hardening (OQ4)
- [ ] `docs/ZOTERO-COMPATIBILITY.md` states the support matrix honestly
- [ ] Read-backend matrix (§1c) has zero remaining `TBD` cells, each resolved against a live Zotero
      10 with the same discipline as the Open Questions — **specification only; no `catalog.rs`
      routing implementation is required by this criterion**
- [ ] CI green on all 5 targets

## Compatibility Impact

| Change | Class | Note |
|---|---|---|
| WAL-safe reads | **Bug fix** | Restores correctness; on non-WAL DBs output is unchanged |
| `app status` gains `server_id`, `local_api_writes_available` | **Changed** (additive) | Third approved intentional break. Python has no equivalent — cannot be Exact. Reclassify `app status` to **Semantic**, or normalise the two new fields in the harness (preferred: normalise, keeping the rest Exact — same narrow technique already approved for `<CONNECTION_REFUSED>`). |
| `--strict-read` flag | **Cancelled** | Never implemented; rationale superseded. Remove from Phase 4. |
| XPI id / update_url | **Changed** | Already required by the fork decision; users may hold both plugins — `plugin-status` must disambiguate (already a Phase 6 criterion). |

## Risk Assessment

| Risk | Severity | Mitigation |
|---|---|---|
| `mode=ro` fails to open a live WAL DB (`-shm` unavailable) | **High** | OQ1 is step 2, before any code change. Fallback ladder defined above; four options, no silent degradation. |
| No Zotero 10 available → phase stalls | **High** | Explicit step 1. If genuinely unobtainable, the *only* honest outcome is to ship a loud runtime error when a `-wal` file is detected, and document Zotero 10 as unsupported — **not** to guess. |
| Upgrading dev machine to 10 loses the Zotero 9 baseline | Medium | Back up `~/Zotero` first; keep a 9 instance (VM/second machine) for back-compat verification. |
| Re-baselining `app status` masks an unrelated regression | Medium | Re-baseline in an isolated commit that touches nothing else; diff must show only the two new fields. |
| Fixing WAL surfaces *other* latent read bugs the old fixtures hid | Medium | Desirable, not a risk. Budget time for it; treat new mismatches as real findings. |
| Phase 6 starts before this gate passes | **High** | `plan.md` dependency graph makes P14 block P6; success criteria above are the gate. |
