# Zotero Version Compatibility

Support matrix, WAL rationale, and the settled read-backend routing
specification for Zotero 7–10+. This document is the condensed, honest
summary; the full evidence log lives in
[`plans/260829-0840-rust-port-of-cli-anything-zotero/phase-14-zotero-10-compatibility-gate.md`](../plans/260829-0840-rust-port-of-cli-anything-zotero/phase-14-zotero-10-compatibility-gate.md)
and
[`plans/research/zotero-10-impact-on-rust-port.md`](../plans/research/zotero-10-impact-on-rust-port.md).

Every claim below is tagged with how it was established:

- **LIVE VERIFIED** — observed directly against a real, running Zotero instance in this session.
- **SYNTHETIC** — proven deterministically via an automated test that doesn't need a live Zotero.
- **DOC-VERIFIED** — sourced from Zotero's own published documentation, not independently observed live.
- **BLOCKED** — genuinely unresolved; the reason is stated, not guessed around.

These tags describe **what is known**. They are separate from, and unaffected by, the merge-gate
status below, which describes **what blocks integrating this branch**.

## Status at v1.0.0 (2026-08-31)

The Layer A fix (the critical WAL/`immutable=1` bug), capability detection, HTTP hardening, and the
Layer B read-backend specification are complete and verified — see the sections below. Two items
that were open when this document was first written have since been resolved by Phase 6 and are
marked inline: the XPI now exists and declares `strict_max_version: 10.0.*` (Open Question 4), and
Local API write-consent handling shipped with a CLI-owned credential store (Open Question 5).

What remains genuinely unverified at v1.0.0:

| Item | Why it is still open |
|---|---|
| Live Zotero ≤9 sweep across all commands | No ≤9 instance was available in any development environment. The ≤9 code paths are structurally reasoned and covered by synthetic tests, not live-tested. |
| Migrated saved-search live verification (OQ6) | No library that has been through Zotero's saved-search migration was available. |
| `search list` / `search get` shape on a populated saved search | DOC-VERIFIED only — see the Layer B section. |

## Support matrix

| Zotero version | Journal mode | SQLite reads | Local API writes | JS-Bridge (XPI) writes |
|---|---|---|---|---|
| ≤9 | Rollback journal | Supported, `mode=ro` with `immutable=1` fallback (unchanged from pre-10 behavior) | Not available | Required — the CLI Bridge XPI is bundled in the binary and staged by `app install-plugin` |
| 10+ | WAL | Supported, `mode=ro` preferred; refuses (not silently stale) when Zotero holds the lock | Available, gated on `Zotero-Server-ID` capability detection, and on one-time human consent via `app authorize-local-api` | Available as fallback, and required for privileged operations the Local API cannot express |

## Layer A — SQLite connection policy (`db::connect_readonly`)

**LIVE VERIFIED** (2026-08-29, real Zotero 10.0.1, independently reproduced this session,
corroborating the plan's earlier live findings): Zotero holds its own database connection in
SQLite's exclusive locking mode on **every** version, not only under WAL — a plain `mode=ro` open
fails with `SQLITE_BUSY` on the first statement (a bare `SELECT 1` included) whenever Zotero is
running, at busy timeouts up to 2s, reproduced 5+ times. This is *why* `immutable=1` was chosen
originally — not for staleness tolerance, but because it was the only way to open the file at all
while Zotero ran.

Under WAL (Zotero 10 default), `immutable=1` has a second, unrelated effect: it tells SQLite the
file can never change, so SQLite never attaches `-wal` at all. Every committed-but-uncheckpointed
row silently vanishes, with exit code 0 and no error.

**Corrected policy** (implemented in `crates/zotero-cli/src/db.rs`):

1. Try `mode=ro`, with a real probe query (the lock only surfaces at statement-prepare time, not
   connection-open time — confirmed by the same live reproduction above).
2. Success → use it. This is the only path when Zotero is closed, or the database has no `-wal`
   sidecar (Zotero ≤9) — zero behavior change there.
3. `SQLITE_BUSY` and a `-wal` file exists → **refuse loudly**, with an actionable error. Never a
   silent `immutable=1` fallback on a WAL database.
4. `SQLITE_BUSY` and no `-wal` file exists → fall back to `immutable=1`. Safe unconditionally: a
   rollback-journal database has nothing in a WAL for `immutable=1` to miss.

**Scope note:** the plan's own fallback ladder allows an explicit `--allow-stale-sqlite` opt-in as
a fourth rung (fall back to `immutable=1` behind a flag even on a locked WAL database). That flag
was **not wired** in this pass — `connect_readonly`'s refusal has no bypass yet. This is stricter
than the plan requires, not looser; threading the flag through all 24 read commands' call sites is
left as explicit follow-up scope.

**Verification, in increasing order of rigor:**

- SYNTHETIC: 4 unit tests in `db.rs` (`wal_mode_immutable_fallback_silently_misses_uncheckpointed_commits`,
  `connect_readonly_reads_uncheckpointed_wal_commits_completely`,
  `connect_readonly_refuses_not_falls_back_when_wal_database_is_locked`,
  `connect_readonly_falls_back_to_immutable_when_locked_non_wal_database`) reproduce both the bug
  and the fix deterministically, with no live Zotero dependency, on every `cargo test` run.
- LIVE VERIFIED (harness): `harness/fixtures/build_fixture.py`'s new `wal-mode` state leaves item
  `WALFIX01` genuinely uncheckpointed on disk (a child process writes it and exits via `os._exit()`,
  skipping SQLite's checkpoint-on-close). Captured through the project's actual `capture.py`: Python's
  unmodified reference (`zotero_sqlite.py`, still `mode=ro&immutable=1`) returns `"Item not found"`;
  Rust's fix returns it correctly. `harness/commands.tsv` rows 100–101; golden files committed.
- LIVE VERIFIED (real Zotero): reproduced directly against the user's real `~/Zotero/zotero.sqlite`
  while Zotero 10.0.1 was running, and confirmed `mode=ro` succeeds cleanly once Zotero quits.

## Layer B — command-level read-backend routing

**Pre-v1 update (2026-08-31):** two discovery reads now have safe live routing
for the exact state that motivated this document. `item find` tries its normal
SQLite path first; only if that path refuses with the tagged WAL/busy
`DatabaseLocked` error does it ask the owned CLI Bridge to run Zotero's own
read-only quicksearch. `library list` follows the same SQLite-first rule, using
the Bridge only after the database lock refusal. No path uses `immutable=1` on a
locked WAL database, and neither command autolaunches Zotero.

The original compatibility pass did not route most SQLite-backed reads through a live backend,
per that plan's own scope (`catalog.rs` routing was separable follow-up work). It did produce
a concrete, evidence-based answer for whether the Local API can represent each SQLite-backed read
command — see the full matrix in `phase-14-zotero-10-compatibility-gate.md` §1c. Current summary:

| Backend answer | Commands |
|---|---|
| Local API confirmed sufficient (LIVE VERIFIED) | `collection list/find/get/items/tree`, `item list/get/children/notes/attachments/file`, `tag list/items` |
| Local API confirmed sufficient (already landed) | `item find` (Local-API-first since before this phase) |
| No Local API equivalent — SQLite/Bridge only, LIVE VERIFIED | `library list`, `session use-library` |
| Local API likely sufficient — DOC-VERIFIED only | `search list`/`search get` (see below) |
| No SQLite path at all (unaffected by this phase) | `search items`, `style list`, `session status`/`use-collection`/`use-item`/`clear-*`/`history` |
| Not SQLite-backed; HTTP-mediated | `session use-selected`, `collection use-selected` |

**`search list`/`get` — the one DOC-VERIFIED (not LIVE VERIFIED) cell.** No saved search exists in
the only live Zotero 10 instance available this session (confirmed empty via a read-only SQLite
query); creating one to test would write to the user's real production library, out of scope for a
read-only compatibility pass. Zotero's public Web API docs and the Pyzotero client's documented
response shape both show `GET .../searches/<key>` returning
`data.conditions: [{condition, operator, value}, ...]` — exactly the shape `SavedSearchCondition`
needs. Every sibling endpoint checked live this session (`items`, `collections`, `tags`) matched the
Web API v3 shape exactly, so the Local API mirroring the same shape for searches is assessed likely
— but this remains DOC-VERIFIED, not LIVE VERIFIED, until confirmed against a populated saved search.

Every "Local API confirmed sufficient" row needs a **compatibility renderer** before layer B could
actually route to it: the Local API's JSON shape (`key`/`version`/`library`/`links`/`meta`/`data`,
matching the public Web API v3 format) is structurally different from this port's flattened
`_normalize_item`/`Collection`/`TagSummary` shapes, not just renamed fields.

## Capability detection (`Zotero-Server-ID`)

**LIVE VERIFIED**: `GET /api/` on a real Zotero 10.0.1 instance returns a `Zotero-Server-ID` header
on **every** response, including a `403` when the Local API is disabled in preferences. This makes
header presence a reliable Zotero 10+ discriminator independent of whether the Local API is
currently usable — stronger than the plan anticipated (which only specified checking a 200
response). `RuntimeContext` now carries `server_id: Option<String>` and
`local_api_writes_available: bool` (`server_id.is_some() && local_api_available` — a 10+ install
with Local API currently disabled reports `false`, since it can't accept writes either way right
now). Both are surfaced as additive fields on `app status`; not wired to any write path.

## HTTP hardening

**LIVE VERIFIED** against the real Zotero 10.0.1 instance:

- `Host: evil.example.com` → `400 Bad Request`. `Host: localhost:23119` and `Host: 127.0.0.1:23119`
  (matching our client's actual base URL) → `200 OK`. Resolves Open Question 2.
- A `Mozilla/`-prefixed `User-Agent` → connection dropped, zero bytes of response (`curl` exit 52,
  "empty reply from server").
- Any `Origin` header at all → same silent drop, independently reproduced.
- Our own client (`ureq`'s default `ureq/x.y` UA, no `Origin`) → passes cleanly.

Locked in as a permanent regression test:
`crates/zotero-cli/tests/zotero10_conformance.rs` (3 tests, against a local fake server — it can't
re-verify Zotero's own server behavior, only prevent our client code from regressing into a header
Zotero 10 is confirmed to drop).

## Selection semantics (`session use-selected` / `collection use-selected`)

**LIVE VERIFIED**, resolving Open Question 3. `/connector/getSelectedCollection`:

- Requires `POST {}`, not `GET` (a live `GET` returns `400 Endpoint does not support method`).
- Returns the single currently-selected collection or library:
  `{libraryID, libraryName, editable, id, name, targets: [...]}`.
- **No multi-selection is reachable.** Zotero 10.0.1's collection tree does not support ctrl/cmd- or
  shift-click multi-select at all — both simply move the single selection rather than extending it.
  "Multi-selection" from Open Question 3 is not a real UI state for this endpoint.
- **No true "nothing selected" state was reachable either** — the tree always keeps exactly one row
  focused once any collection or library has been clicked in a session.

Implication: `use-selected`'s eventual implementation can assume single-collection-or-absent
semantics and never needs to handle a list.

## Local API write authorization persistence (Open Question 5)

> **Resolved in Phase 6.** `POST /api/local/authorize` re-prompts for human consent on *every*
> call, even with an existing "Always Allow" grant — so a stateless CLI process cannot rely on
> Zotero's own persistence to write unattended. The port therefore persists the issued key itself,
> scoped to a specific `Zotero-Server-ID`, in a restrictive-permission file beside `session.json`,
> and still handles the server's own 401 rejection rather than assuming a stored entry is valid
> forever. See `crates/zotero-cli/src/credentials.rs` and [`SECURITY.md`](SECURITY.md). The
> original read-only-pass finding is preserved below for the record.

**Partially observed, LIVE VERIFIED (UI only) — BLOCKED for full verification.** Zotero's Advanced
settings expose a "Clear Write Authorizations" control, enabled only after at least one write
authorization has been granted — its existence confirms Zotero persists write authorizations beyond
a single session (there would be nothing to "clear" otherwise). Fully confirming *what* survives a
restart requires attempting a real Local API write and driving the consent dialog, which this pass
did not do: "Do not start Phase 6 writes yet" was an explicit constraint, and this is the user's real
production Zotero library, not a disposable fixture. Remains open for Phase 6.

## XPI Zotero 10 compatibility (Open Question 4)

> **Resolved in Phase 6.** The fork-owned CLI Bridge XPI now exists, embedded in the binary at
> `crates/zotero-cli/src/plugin/assets/{manifest.json,bootstrap.js}`. It declares
> `strict_min_version: 6.999` / `strict_max_version: 10.0.*`, uses its own addon id
> (`cli-bridge@cli-anything-rust.dev`), and registers both `/cli-bridge/eval` and
> `/cli-bridge/ownership`. LIVE VERIFIED against Zotero 10.0.1: `app doctor` reports
> `bridge.state: healthy` with a successful privileged eval round-trip.

The original finding — no `plugin/` directory existed at the time of the read-only compatibility
pass, so there was nothing to set `strict_max_version` on and no endpoint to test Zotero 10's HTTP
hardening against — is superseded.

## Not evaluated

- Migrated saved searches (Open Question 6) — no library that has gone through Zotero's saved-search
  migration was available.
- A live Zotero ≤9 instance — none was available in any development environment; the ≤9 code paths
  are DOC-VERIFIED/structurally reasoned (see phase-14 §Success Criteria) rather than live-tested.
  This remains true at v1.0.0.
