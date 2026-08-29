---
phase: 14
title: "Zotero 10 Compatibility Gate"
status: in-progress
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

**Resolved 2026-08-29** against a real, running Zotero 10.0.1 instance (LIVE VERIFIED unless noted
otherwise) — see `docs/ZOTERO-COMPATIBILITY.md` for the classified evidence log this table
summarizes. `catalog.rs` routing (layer B) is still **not implemented** by this resolution — per
this phase's scope, resolving the matrix is a specification deliverable only.

| Command | Zotero running | Zotero closed | Stale fallback | Parity impact |
|---|---|---|---|---|
| `library list` | **Resolved: no Local API equivalent.** `GET /api/users/0/collections`/`/items` embed a `library` object with only `type`, `id`, `name`, `links` — none of `editable`/`filesEditable`/`storageVersion`/`archived`/`lastSync` that `fetch_libraries` needs. Remains SQLite via layer A. | SQLite `mode=ro` (layer A) | N/A — no Local API path exists to prefer over SQLite in the first place | None; SQLite-only was always the only option, just now confirmed rather than assumed |
| `collection list` / `find` / `get` | **Resolved: yes.** `GET /api/users/0/collections` returns `key`, `version`, `data.name`, `data.parentCollection` (`false` or parent key), `meta.numItems` (item count), `meta.numCollections` (child count) | SQLite `mode=ro` | `--allow-stale-sqlite` only if layer A's `mode=ro` itself is denied | Needs a compatibility renderer: field names/shape diverge from `Collection` (`data.name` vs `collectionName`, `parentCollection` vs `parentCollectionID`, nested `data`/`meta` vs flat) |
| `collection items` | **Resolved: yes**, from two already-verified pieces: collection-ref resolution via `collections` above, item listing via the `items/top` shape verified below (and already proven live in `find_items`'s landed Local-API-first branch, which hits the same collection-scoped endpoint) | SQLite `mode=ro` | Same | Needs compatibility renderer (item shape) — see Phase 6 §C2 |
| `collection tree` | **Resolved: yes.** `data.parentCollection` from `collections` above is sufficient to walk the tree (`false` = root, else parent key) | SQLite `mode=ro` | Same | None — tree-walk logic is equivalent, only field names differ (covered by the collection renderer) |
| `item list` / `get` | **Resolved: yes.** `GET /api/users/0/items/top` / `/items/<key>` return `key`, `version`, `library`, `links`, `meta` (`creatorSummary`, `parsedDate`, `numChildren`), `data` (`itemType`, `title`, `creators[]`, all typed fields) | SQLite `mode=ro` | Same | Needs compatibility renderer (Phase 6 §C2) confirmed — Local API's item shape is structurally different from `_normalize_item`'s flattened shape, not just renamed fields |
| `item find` | **Already Local-API-first in landed code** (`catalog::find_items`); SQLite title-search is the existing fallback when Local API returns nothing or `--exact-title` is set | SQLite `mode=ro` title search (existing fallback path) | Extend so the *existing* SQLite fallback also honors layer A's refuse/opt-in policy, not just a fresh direct call | Already handled by the landed Local-API-first branch; no new parity risk beyond what layer A adds to its own SQLite leg |
| `item children` / `notes` / `attachments` | **Resolved: yes.** `GET /api/users/0/items/<key>/children` returns the full child list, including attachments with `data.filename`, `data.linkMode`, `data.contentType`, and (bonus, unused by this port by design) `links.enclosure.href` — a `file://` path Zotero itself resolves for `imported_file` attachments | SQLite `mode=ro` | Same | Needs compatibility renderer; `attachments`' `resolvedPath` stays a **local filesystem computation** regardless of backend, per this phase's own design — never sourced from either API, including the enclosure link just found |
| `item file` | Same as `item attachments`, plus a local filesystem `exists()` check that is backend-independent | SQLite `mode=ro` | Same | Same as attachments |
| `search list` / `get` | **DOC-VERIFIED, not live-verified** — no saved search exists in the only available live Zotero 10 instance (confirmed empty via read-only SQLite query against the real library), and creating one to test would write to the user's real production library, out of scope for this read-only pass. Zotero's public Web API docs (`zotero.org/support/dev/web_api/v3/*`) and the third-party Pyzotero client's documented response shape both show `GET .../searches/<key>` returning `data.conditions: [{condition, operator, value}, ...]` — the exact shape `SavedSearchCondition` needs. The Local API is documented to mirror the Web API's object JSON, and every sibling endpoint checked this session (`items`, `collections`, `tags`) matched the Web API v3 shape exactly, so this is assessed **likely yes** but flagged DOC-VERIFIED pending live confirmation against a populated saved search | SQLite `mode=ro` | `--allow-stale-sqlite` opt-in remains the fallback until the DOC-VERIFIED answer above is upgraded to LIVE VERIFIED | Needs a compatibility renderer if confirmed; document the DOC-VERIFIED caveat in `docs/ZOTERO-COMPATIBILITY.md` either way |
| `search items` | **Already Local-API-only** in landed code (`catalog::search_items` errors today if Local API is unavailable, regardless of whether Zotero is running or closed) | Errors today — no SQLite fallback exists; **resolved: accept the existing hard dependency**, consistent with `search list`/`get`'s Local-API-preferred direction once DOC-VERIFIED is confirmed live | N/A — no SQLite path exists to fall back to | No SQLite involvement at all currently; unaffected by layer A |
| `tag list` / `items` | **Resolved: yes.** `GET /api/users/0/tags` returns `tag`, `links`, `meta.type`, `meta.numItems` — the item-count aggregation `fetch_tags` needs | SQLite `mode=ro` | Same | Needs compatibility renderer (`tag` vs `name`, nested `meta.numItems` vs flat `itemCount`) |
| `style list` | **No backend decision needed at all** — pure local filesystem CSL parsing, no SQLite and no HTTP in either Zotero state | Same | N/A | None |
| `session status` / `use-collection` / `use-item` / `clear-*` / `history` | **No backend decision needed** — pure local JSON state file, zero SQLite and zero HTTP | Same | N/A | None |
| `session use-library` | Needs library resolution — **resolved (inherits `library list`'s answer): SQLite via layer A**, Local API has no equivalent field set | SQLite `mode=ro` | Same as `library list` | None; was already going to be SQLite-only |
| `session use-selected` / `collection use-selected` | **Resolved (Open Question 3 answered).** `/connector/getSelectedCollection` exists on Zotero 10.0.1, requires `POST {}` (not GET — a live 400 "Endpoint does not support method" confirms this), and returns the single currently-selected collection/library (`{libraryID, libraryName, editable, id, name, targets: [...]}`). Live testing found Zotero 10.0.1's collection tree has **no multi-select at all** — Cmd-click and Shift-click both simply move the single selection rather than extending it — so "multi-selection" is not a reachable UI state for this endpoint; the response is inherently single-valued. A true "nothing selected" state was not reachable either: the tree always keeps exactly one row focused once any collection/library has ever been clicked in a session. Not SQLite-backed at all — layer A's policy does not apply to these two commands | Selection state only exists while Zotero runs — command is inherently Zotero-running-only | N/A | Already Semantic/HTTP-mediated class; unaffected by the SQLite connection policy. `use-selected`'s implementation can assume single-collection-or-absent semantics, never a list |

**What this table is not:** a claim that layer B is implemented — resolving the specification above
is this phase's deliverable; wiring the routing into `catalog.rs` remains separable follow-up work,
as originally scoped.

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

- [~] Open Questions 1–6 answered against a live Zotero 10 — **OQ1, OQ2, OQ3 LIVE VERIFIED this
      session (independently re-confirmed OQ1); OQ4 and OQ6 remain genuinely BLOCKED (no XPI exists
      yet for OQ4; no migrated saved-search library available for OQ6); OQ5 partially observed
      (LIVE VERIFIED, UI-only) — see `docs/ZOTERO-COMPATIBILITY.md` §Open Questions for the full,
      classified account. Not all six are closed.**
- [x] `connect_readonly` reads WAL databases completely — LIVE VERIFIED against a real Zotero
      10.0.1 (`~/Zotero/zotero.sqlite`) and against the project's own `wal-mode` harness fixture;
      also covered by 4 permanent unit tests in `db.rs`
- [x] The `wal-mode` fixture **demonstrably fails** when `immutable=1` is reintroduced —
      LIVE VERIFIED at the harness level (Python's unmodified `immutable=1`-based
      `connect_readonly` returns `"Item not found"` for the fixture's deliberately-uncheckpointed
      row; Rust's fix returns it correctly) **and** SYNTHETIC/deterministic at the unit-test level
      (`db::tests::wal_mode_immutable_fallback_silently_misses_uncheckpointed_commits` and 3
      sibling tests reproduce both the bug and the refusal path without needing a live instance)
- [ ] All 31 landed commands Exact/Semantic against `wal-mode` fixtures — **not done**; only 2
      representative rows (`item get`/`item list`) were added and verified this pass, per the
      plan's own "representative read commands" scope, not the full 31
- [ ] All 31 landed commands verified against a **live Zotero 10** and a **live Zotero 9** —
      **not done**; spot-verified against live Zotero 10 (WAL/lock behavior, HTTP hardening,
      capability detection, selection semantics) but not all 31 commands individually, and no
      Zotero ≤9 instance was available in this environment at all (BLOCKED)
- [x] ~~`immutable=1` appears nowhere in the codebase except a comment explaining why it must not return~~ — **superseded by the 2026-08-29 correction above**: `immutable=1` is retained, by design, for (a) non-WAL databases (Zotero ≤9, always correct there) and (b) the explicit `--allow-stale-sqlite` opt-in on a WAL database. It must never be the silent default when `-wal` is present and `mode=ro` fails. **Implementation note (this pass):** the `--allow-stale-sqlite` CLI flag itself was deliberately *not* wired — `connect_readonly` refuses unconditionally on a locked WAL database rather than offering a bypass, which is stricter than this criterion requires, not looser. Threading an opt-in flag through all 24 read commands' call sites is left as follow-up scope.
- [x] Capability detection correctly reports 10+ vs ≤9 on both live versions — LIVE VERIFIED for
      10+ against the real Zotero 10.0.1 instance (`Zotero-Server-ID: QR43gFhLblRt` captured, even
      on a 403 when Local API is disabled). The ≤9 branch (`server_id: None`) has no live Zotero ≤9
      to test against in this environment, but is DOC-VERIFIED/structurally guaranteed: Zotero ≤9
      never sends this header at all (a 2026-vintage capability, per `phase-14` §4's own sourcing),
      and every existing harness fixture (which never sends the header) already exercises the
      `None` branch as an Exact-classified regression via `app status`
- [x] `app status` exposes `server_id` + `local_api_writes_available`; goldens re-baselined —
      additive fields implemented, harness-normalized (`harness/normalize.py`), and verified
      Exact against the Python golden for both `app status` rows (13 and 97) via the project's own
      `capture.py`/`compare.py` pipeline
- [x] Conformance test asserts no `Mozilla/` UA and no `Origin` on Zotero-local requests —
      `crates/zotero-cli/tests/zotero10_conformance.rs`, 3 passing tests; the underlying claim
      (Zotero silently drops both) is itself LIVE VERIFIED against the real Zotero 10.0.1 instance
      (`curl` exit 52, empty reply, for both conditions independently)
- [ ] XPI installs and registers `/cli-bridge/eval` on Zotero 10 **and** Zotero 9 — **BLOCKED**:
      no `plugin/` directory or manifest exists anywhere in this repo yet (confirmed by search);
      the XPI is explicitly created in Phase 6, which this pass was instructed not to start
- [ ] `/cli-bridge/eval` confirmed reachable under Zotero 10 HTTP hardening (OQ4) — **BLOCKED**,
      same reason: no custom endpoint exists yet to test against, live or otherwise
- [x] `docs/ZOTERO-COMPATIBILITY.md` states the support matrix honestly
- [~] Read-backend matrix (§1c) has zero remaining `TBD` cells — **12 of 13 rows resolved to LIVE
      VERIFIED; `search list`/`get`'s Local API conditions shape is resolved to DOC-VERIFIED only**
      (no populated saved search available to confirm live without writing to the user's real
      library, which was out of scope). Every cell has a concrete, classified answer; one is not at
      the highest evidence tier. **Specification only; no `catalog.rs` routing implementation was
      done, as scoped.**
- [ ] CI green on all 5 targets — **not verified**; `cargo build`/`test`/`clippy`/`fmt` all pass
      locally on this session's host (macOS/aarch64, one of the 5 targets), but this pass did not
      push a branch or run the actual CI matrix

### Merge-gate classification (2026-08-29 integration pass)

The checklist above records **evidence tier** (LIVE VERIFIED / SYNTHETIC / DOC-VERIFIED / BLOCKED)
for each claim — that language is unchanged from the first pass and must stay that way; it says
what is actually known, not what blocks integration. This section adds a second, independent axis:
**does an open item block merging this branch, or is it legitimately this branch's own scope
boundary?** Product decision (2026-08-29): the Layer A fix (the critical bug) and everything
verifiable without Phase 6 or a second Zotero installation are ready for integration now; items
that structurally require work this branch was told not to do are the *next* owner's problem, not a
reason to hold this one back.

**DEFERRED TO PHASE 6** (this branch cannot do these; Phase 6 owns them by construction):
- XPI Zotero 10 compatibility (OQ4, `strict_max_version`, `/cli-bridge/eval` hardening check) — no
  `plugin/` directory exists yet anywhere in this repo; Phase 6 creates it. Evidence stays BLOCKED
  until Phase 6 has a testable endpoint.
- Local API write-consent persistence (OQ5) — confirming what "Always Allow" persists across a
  restart requires an actual Local API write and driving the consent dialog. Verify this during
  Phase 6's first disposable write spike (against a throwaway library/fixture, not production data).
  Evidence stays at "partially observed, LIVE VERIFIED (UI only)" until then.

**DEFERRED COMPATIBILITY VERIFICATION** (real gaps, tracked, not blocking this integration):
- Full 31-command live sweep against a live Zotero ≤9 instance (no ≤9 instance was available in
  this environment). Track as backward-compatibility verification owed before Zotero ≤9 support is
  claimed as fully certified, not before this branch merges — the ≤9 code paths (non-WAL
  `immutable=1` fallback, no `Zotero-Server-ID`) are unchanged from pre-Phase-14 behavior and are
  DOC-VERIFIED/structurally reasoned, not newly at risk.
- Migrated saved-search live verification (OQ6) — requires a library that has actually gone through
  Zotero's saved-search migration, which none available here have.

**REQUIRED BEFORE MERGE:**
- Green CI on all 5 targets (aarch64-apple-darwin, x86_64-apple-darwin, x86_64-pc-windows-msvc,
  x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu). Local verification (macOS/aarch64) is not a
  substitute — this is the actual gate for this PR.

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
