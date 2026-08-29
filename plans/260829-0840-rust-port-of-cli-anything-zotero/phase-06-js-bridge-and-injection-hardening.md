---
phase: 6
title: "Write Backends: Local-API-First"
status: todo
priority: P1
effort: "14-18d"
dependencies: [5, 14]
---

# Phase 6: Write Backends: Local-API-First

> **Re-planned 2026-08-29 from scratch**, after Phase 14 (Zotero 10 Compatibility Gate) merged into
> `main` (`7ee7c70`, PR #4), then **red-teamed the same day** by four adversarial reviewers
> (Security Adversary, Failure Mode Analyst, Assumption Destroyer, Scope & Complexity Critic — see
> `## Red Team Review` in `plan.md`). The previous version of this file was a self-flagged skeleton
> reconstruction; this version is a fresh design, and the red-team pass caught two factual errors in
> the first draft of this rewrite itself (a fabricated "already landed" Connector API client, and
> wrong Zotero PATCH-array semantics that would have caused real data loss) — both corrected below.
> **A second, independent Python-source audit (post-PR #5) found the Connector-routing correction
> itself was still wrong**: `add doi`/`import doi` are hybrid (JS Bridge translator first, Connector
> Crossref-BibTeX fallback second), `import pmid`/`note add` are JS-Bridge-only with no Connector
> involvement, and — most importantly — none of these commands belong to Phase 6 at all; they're
> Phase 7's ("Ingest, Attachments and PDF Cascade"). This revision removes them from §3.6's matrix,
> corrects the Overview's surface descriptions with exact `core/{imports,jsbridge,notes,add}.py`
> citations, and names the still-unimplemented Phase 5 Connector client as an explicit upstream gate
> (§3.1a, "Phase 5C") rather than assuming or silently absorbing it. It carries forward only the
> low-level mechanics independently verified against the Python source (D1's `JSON.parse` fix, D3's
> probe-caching fix, the AppleScript removal, the XPI fork mechanics).

## Overview

Zotero 10's Local API gained write support (`plan.md` Finding 3: POST/PUT/PATCH/DELETE, tag
deletion, full-text, file upload). Phase 14 (merged) already built the capability-detection
plumbing this phase needs: `RuntimeContext.server_id: Option<String>` and
`local_api_writes_available: bool` ([runtime.rs:27-33](../../crates/zotero-cli/src/runtime.rs#L27),
populated in `build_runtime_context` at [runtime.rs:90-91](../../crates/zotero-cli/src/runtime.rs#L90)
from the same round trip as the existing Local-API-availability probe). **That signal is currently
unwired** — `catalog.rs` has zero references to it — because Phase 14 was explicitly scoped to
*specify* the read-backend policy, not implement command-level routing, and write routing was
out of scope entirely. This phase wires it, for writes.

**Three write-capable surfaces exist, not two — but only one of the three has any client code
today, and a second correction (caught by an independent Python-source audit after the first
red-teamed draft) narrows which commands actually use it.** The previous version of this file only
considered Local API vs. JS Bridge, then a first correction added the Connector API but
mischaracterized its role. Re-reading the actual Python reference source
(`reference/cli-anything-zotero/cli_anything/zotero/core/{imports,jsbridge,notes,add}.py`,
`utils/zotero_http.py`) instead of inferring from `compatibility-matrix.md`'s placeholder tags shows:

- **Import/ingest commands are hybrid or Bridge-only, not uniformly Connector-routed, and they are
  not this phase's commands anyway.** `add doi`/`import doi` (`core/imports.py:880-1067`,
  `core/add.py:27-58`) try, in order: (1) library dedupe via the JS Bridge
  (`bridge.find_items_by_doi`), (2) Zotero's built-in DOI translator via the JS Bridge
  (`bridge.import_from_doi`, `core/jsbridge.py` — generates `Zotero.Translate.Search` JS), (3) only
  on translator failure, a Crossref-BibTeX-then-Connector fallback
  (`zotero_http.connector_import_text` against `/connector/import`, then
  `zotero_http.connector_update_session` against `/connector/updateSession`). Target resolution
  (`_resolve_target`, `imports.py:75-144`) can call `zotero_http.get_selected_collection` (Connector
  `/connector/getSelectedCollection`) when no explicit or session target is given. `import pmid`
  (`import_from_pmid`, `core/jsbridge.py:498`) is **JS Bridge only** — pure `Zotero.Translate.Search`
  generated JS, no Connector fallback at all; it must not be classified as Connector-routed.
  `note add` (`core/notes.py:119-159`) is **also JS Bridge only** — it builds a raw `Zotero.Item`
  creation script and calls `bridge.execute_js` directly, with zero Connector involvement; Phase
  14/the compatibility matrix's "Connector-mediated" tag on that row does not match the actual
  Python source and must not be relied on. **None of this is Phase 6's scope regardless of backend:**
  `plan.md`'s own phase table assigns `add`/`import`/`note` command families to **Phase 7**
  ("Ingest, Attachments and PDF Cascade"). The routing facts above are recorded here only so Phase
  7's planning inherits verified evidence instead of re-deriving it — Phase 6 does not implement
  these commands. See the matrix note below and Unresolved Question 9.
- **The genuinely Connector-`saveItems`-routed commands are also Phase 7's, not Phase 6's:**
  `import json` (`imports.py:636-701`, `zotero_http.connector_save_items` against
  `/connector/saveItems`) and `add url`'s generic-webpage fallback leg (`core/add.py:246-270`, same
  `connector_save_items` call, used only when the URL is neither an arXiv id nor a DOI). `import
  file` (`imports.py:547-633`) uses `/connector/import` for BibTeX/RIS/EndNote/XML/CSV file content
  — **this is what `/connector/import` actually owns; it must not be generalized to "all identifier
  imports"** (it never serves `import pmid`, and only serves `add doi`/`import doi` on the
  Crossref-fallback leg, not the preferred translator path). Attachment upload
  (`zotero_http.connector_save_attachment` against `/connector/saveAttachment`) is used by
  `_perform_attachment_upload` (`imports.py:361-502`), invoked from `import file`/`import json`'s
  attachment-manifest workflow — a different code path from `item attach`'s JS Bridge
  `attach_pdf` (`core/jsbridge.py:320`, confirmed by `zotero_cli.py:1065-1069`) and from Phase 7's
  own separate PDF-fetch cascade module (`core/pdf_fetch.py`). Keep all three distinct; do not
  conflate them.

Given the above, **Phase 6 itself has no command whose *own* CRUD logic is Connector-routed** — every
row in §3.6's matrix is Local-API-or-JS-Bridge. The one place a Connector method could matter to
Phase 6 is defensive: if a future Phase-6 write command ever grows an implicit "use the selected
collection" fallback (mirroring `_resolve_target`'s pattern), it would need
`get_selected_collection`. No currently-specified Phase 6 command has that flag today (§3.8), so this
is a documented non-dependency, not a live one — but it is exactly the kind of thing that must not be
assumed to "just work" later, which is why §3.1a below names the gate explicitly rather than leaving
it implicit.

**Correction (caught in red-team review, not true of the merged codebase, and refined further by the
Python-source audit above):** the first draft of this plan claimed Phase 5 "already landed" Connector
write methods. It did not — `grep -n "pub fn" crates/zotero-cli/src/http.rs` shows only
`connector_is_available`, `probe_local_api`, `local_api_is_available`, and `local_api_get_json`; none
of Phase 5's own declared Connector client scope (`phase-05...md:67`: "Connector API client: `ping`,
`getSelectedCollection`, `import`, `saveItems`, `saveAttachment`, `updateSession`") exists in `crates/`
beyond `ping`'s equivalent (`connector_is_available`). Phase 5's Success Criteria for the other five
methods are unchecked. **This phase must not assume that client is a settled dependency, and must
not silently build the missing transport itself either** — see §3.1a's explicit gate.

```
Write-capable surfaces (SQLite excluded — see §3.7, no direct SQLite writes)
┌──────────────────────┬──────────────────────────┬──────────────────────────────────┐
│ Surface               │ Available on              │ Owns                              │
├──────────────────────┼──────────────────────────┼──────────────────────────────────┤
│ Connector API         │ all Zotero versions       │ Phase 7's ingest commands (file/  │
│ (/connector/*)        │ (not gated by server_id)  │ JSON import, generic add-url,     │
│                       │                           │ attachment upload, selected-      │
│                       │                           │ collection target resolution) —   │
│                       │                           │ Phase 5's job to build, NOT yet   │
│                       │                           │ landed (§3.1a gate). Not used by  │
│                       │                           │ any command Phase 6 itself owns.  │
│ Local API             │ gated by                  │ CRUD: item/collection field       │
│ (/api/*)              │ local_api_writes_available│ updates, tag add/remove,          │
│                       │ (capability flag, NOT     │ create/rename/delete, membership  │
│                       │ simply "Zotero 10+" — can │                                    │
│                       │ be false on 10+ too, see  │                                    │
│                       │ §3.6)                     │                                    │
│ JS Bridge             │ all Zotero versions       │ privileged/internal-only ops with │
│ (/cli-bridge/eval)    │ (XPI required; greenfield │ no HTTP equivalent (raw js, sync,  │
│                       │ in this phase); fallback  │ duplicate merge, ...); fallback   │
│                       │ whenever local_api_writes_│ for every CRUD op whenever        │
│                       │ available is false        │ Local API writes aren't available │
└──────────────────────┴──────────────────────────┴──────────────────────────────────┘
```

**The single highest-stakes open item is authorization persistence** (`plan.md` red-team Finding 18,
"the only genuine reversal trigger"): Zotero 10's Local API write path requires a user-facing consent
dialog. Phase 14 confirmed *some* persistence mechanism exists (a "Clear Write Authorizations"
control in Advanced settings, grayed out until an authorization exists —
`plans/research/zotero-10-impact-on-rust-port.md` §7.3, OQ5) but explicitly could not drive a real
write to find out what survives a restart, where it's stored, or what HTTP status a denied/revoked
write returns — assigned to "Phase 6's first disposable write spike" (`docs/ZOTERO-COMPATIBILITY.md:28`).
§3.2 below is that spike's design. **Nothing past §3.2 should be implemented against production code
until the spike's findings are in.**

## Architecture

### 3.1 What Phase 14 already gives this phase

- `RuntimeContext.server_id: Option<String>` / `.local_api_writes_available: bool` — capability
  signal, re-probed every process invocation. Confirmed single call site: `dispatch_command`
  ([lib.rs:65](../../crates/zotero-cli/src/lib.rs#L65)) calls `build_runtime_context` exactly once
  per process — there is no daemon, so "restart handling" is mostly free (§3.3).
- `http::probe_local_api` ([http.rs:65](../../crates/zotero-cli/src/http.rs#L65)) captures
  `Zotero-Server-ID` on every response, including a `403` when Local API is disabled — confirmed
  live against Zotero 10.0.1.
- HTTP hardening is already locked in by `tests/zotero10_conformance.rs` — write requests inherit
  this for free.
- The read-backend matrix (`phase-14-zotero-10-compatibility-gate.md` §1c) settles which surface
  serves which *read*. Post-write state parity (§3.5) reuses that matrix.
- Zero session/auth/consent persistence exists yet (`session.rs` has no token/consent/key handling)
  and zero write path exists in `db.rs` (every `pub fn` is a reader; the only `INSERT`s are
  `#[cfg(test)]` fixture setup using a separate direct connection, not `connect_readonly`).
- **Important correction:** `local_api_writes_available` is a *capability* flag
  (`server_id.is_some() && local_api_available`, [runtime.rs:90-91](../../crates/zotero-cli/src/runtime.rs#L90)),
  not a Zotero-version flag. `plan.md`'s own Risks table already documents an observed real machine
  where Local API was disabled entirely. **A Zotero 10+ install with Local API disabled in
  preferences has `local_api_writes_available == false`** — every write must key off this flag
  directly, never off a version check, or that machine gets zero working write backend (§3.6).

### 3.1a Phase 5C — Connector client completion (upstream gate, not a Phase 6 slice)

**Added per source-level audit finding: do not silently absorb this transport work into Phase 6.**
Phase 5's own plan file already declares this scope (`phase-05...md:67`) — five Connector methods
beyond the already-landed `ping`: `getSelectedCollection`, `import`, `saveItems`, `saveAttachment`,
`updateSession`. None exist in `crates/zotero-cli/src/http.rs` today. Naming the remaining,
unimplemented slice of Phase 5's own declared scope **Phase 5C** (not a new numbered phase — a
checkpoint within Phase 5) makes the gate explicit instead of letting Phase 6 or Phase 7 assume it
exists:

```
Phase 5C — Connector client completion (Phase 5's own scope, not yet built)
  get_selected_collection · connector_import_text · connector_save_items ·
  connector_save_attachment · connector_update_session
                    │
                    ▼
      Connector-dependent Phase 7 commands (import file/json, add url,
      add doi's Crossref-fallback leg, attachment-manifest upload)
```

**Phase 6's actual exposure to this gate, stated precisely (per the Overview's Python-source audit
above):** zero of Phase 6's own commands are Connector-routed today. The gate is recorded here
defensively, not because a currently-specified Phase 6 command needs it, but so that (a) nobody
building Phase 6's write-routing infrastructure mistakes the DOI/PMID/note-import commands for
in-scope work needing a Connector client, and (b) if a future Phase 6 write command ever grows an
implicit "use the selected collection" fallback (§3.8), it gates on Phase 5C rather than assuming
`getSelectedCollection` "just exists." **This phase's own Slice 3 (Local API write client) has no
dependency on Phase 5C** — Local API and Connector API are different HTTP surfaces with independent
clients; nothing here blocks Phase 6's Local-API/JS-Bridge work on Phase 5C landing.

**Follow-up flagged, not actioned in this PR:** Phase 5's own plan file (`phase-05...md`) should
gain a matching explicit "Phase 5C" checkpoint marking these five methods as a distinct,
trackable sub-scope with their own completion gate, so Phase 7's planning (which genuinely depends
on all five) can cite it directly instead of a generic "Phase 5's Success Criteria." Left as a
recommendation rather than made here, since this PR's scope is Phase 6's plan and `plan.md`.

### 3.2 (§C1) Slice 0 — Disposable write-consent spike (must run first, answers §OQ9/OQ5)

**Purpose:** answer, empirically, against a live Zotero 10 instance and a **throwaway/scratch
library** — never the user's production `~/Zotero` data:

1. Does "Always Allow" on the Local API's write-consent dialog survive a Zotero **restart**?
2. Where does the authorization live, as precisely as observable from the outside?
3. What exact HTTP status does a **write** return before any consent has been granted?
4. What exact HTTP status does a write return **after** "Clear Write Authorizations" (revocation)?
5. Does `Zotero-Server-ID` stay constant across a restart on the same profile, or rotate per-launch?
6. Does one "Always Allow" grant authorize all future writes, or is it scoped per request/endpoint?
7. **Does a repeated write attempt while the first is still pending consent stack a second dialog,
   dedupe into the same one, or replace it?** (Added in red-team review — without this, an agent
   harness that retries on failure could stack unbounded dialogs in Zotero's UI with no human ever
   notified through any channel the agent controls; see §3.3.)

**Method:** build a disposable test harness — not part of the shipped binary — e.g.
`crates/zotero-cli/examples/write_consent_spike.rs` or a `#[ignore]`-gated live integration test.
**Correction (red-team finding: an environment variable alone cannot guarantee target isolation).**
`http.rs`'s `base_url` ([http.rs:11-18](../../crates/zotero-cli/src/http.rs#L11)) takes only a bare
port — it has no concept of which library or profile is bound to that port, and nothing in
`paths.rs`'s env overrides (`ZOTERO_DATA_DIR`/`ZOTERO_PROFILE_DIR`/`ZOTERO_HTTP_PORT`) prevents the
harness from talking to whichever Zotero instance actually answers on that port at test time — a
developer's normal, production-loaded Zotero if that's what happens to be running. The spike must
therefore **verify before writing, not just configure and trust**: `GET` the target item first and
assert its title/key exactly matches a pre-registered scratch-fixture marker value; abort with a
loud error if it doesn't match, rather than relying on the environment-variable convention alone.

Record every finding — including negative ones — as **a new `§8` section appended to the existing**
`plans/research/zotero-10-impact-on-rust-port.md` (not a new file: that document already has the
established append-as-you-learn-more pattern for this exact investigation thread — see its own
`§7.3 Completed repeat pass` precedent — and OQ5/OQ9 are already tracked there; splitting the answer
across two files risks the two drifting or contradicting unnoticed). Use the same LIVE VERIFIED /
DOC-VERIFIED / BLOCKED evidence tiers Phase 14 used.

**Outcome gate:** if finding 1 comes back "no, consent does not survive a restart," stop and bring
the reversal to the user before writing any Local API write code. The rest of this file assumes the
spike answers "yes," but §3.9-§3.12 (D1/D3/AppleScript/XPI mechanics) and the ≤9 JS-Bridge CRUD
template slice (§3.8a) stay valid either way — see §Unresolved Questions.

### 3.3 (§C1, continued) Authorization, non-idempotent retries, and 401/403/revocation behavior

Design, pending the spike's exact status codes:

- **First-time write, not yet authorized:** fail fast with an actionable message and exit 1; no
  polling loop (matches `plan.md`'s "bare `zotero-cli` must not block on stdin" philosophy). A
  polling/wait flag is explicit YAGNI scope creep unless a real workflow demonstrates the need.
- **Revoked:** treat identically to "not yet authorized" unless the spike finds a genuinely
  distinguishable status/body.
- **No silent fallback to JS Bridge on a write-auth failure**, to avoid double-applying a write.
- **Machine-distinguishable "needs human action" signal (added in red-team review).** This CLI's
  stated primary caller is an unattended AI agent (`plan.md` line 3). A generic exit-1 message is
  indistinguishable, to an agent's default retry-on-failure logic, from any other transient failure.
  Every write-auth failure must set a distinct, documented signal an agent can branch on without
  parsing prose — e.g. a dedicated exit code (not the generic `1` used for `ok=false`) or a
  `"needs_human_action": true` field in the `--json` error payload. **Do not retry automatically
  inside the CLI either** — surfacing the signal once and stopping is safer than looping, given
  finding 7 above (unknown whether repeated attempts stack dialogs).
- **Non-idempotent create commands need explicit duplicate-write protection (added in red-team
  review).** `collection create` (§3.6 row 18) is a `POST`, not idempotent. If Slice 0 finds that a
  write can physically land in Zotero *before* an error is returned for the pending-consent state
  (not yet confirmed either way — this is itself part of what Slice 0 must determine, beyond the
  status code alone), then the fail-fast message's implicit "re-run after approving" instruction
  risks a second, duplicate `POST` from a caller who retries as told. Before Slice 8 implements any
  create-class command, confirm whether the Local API supports a client-supplied idempotency key or
  whether a pre-create existence check is needed; do not ship a create command whose retry story was
  never verified against this failure mode.
- **Restart, in general:** because every CLI invocation is a fresh process (§3.1), "restart
  handling" is almost entirely "re-probe `local_api_writes_available` every time," which already
  happens. No new restart-detection code, and — per §3.4 below — no new persisted state either.

### 3.4 Zotero-Server-ID: diagnostic value only, no persistence (scope cut in red-team review)

`server_id` is a **capability discriminator**, not a credential, and must not become one:

- Never persist `server_id` as part of any authorization or write-gating decision.
- **Cut from this plan: persisting `server_id` into `session.json` for diagnostics.** The first
  draft of this plan proposed this as "additive, low-risk." Three independent red-team findings
  showed it is neither: (1) `session.rs`'s own `save_session_state` doc comment states it "rebuilds
  exactly these 4 keys in this order, discarding anything else that might be in `state`"
  ([session.rs:99-101](../../crates/zotero-cli/src/session.rs#L99)) — a deliberate byte-for-byte
  match to Python's schema that `plan.md`'s Goal 2 requires preserving exactly, so a naively-added
  struct field would be silently dropped on every save, and a *correctly*-added field would break
  the exact-4-key parity contract; (2) no command or Success Criterion in this phase actually
  consumes the value — it would ship with no defined reader; (3) `app status`/`app doctor` are
  today pure reads with no call to `save_session_state` at all (confirmed: no such call exists in
  their `dispatch_command` arms) — turning them into writers of `session.json`'s already
  best-effort-locked, full-object-overwrite file
  ([session.rs:90-94](../../crates/zotero-cli/src/session.rs#L90)) introduces a real lost-update
  race against every existing session-mutating command, purely to support a diagnostic nobody reads.
  `app status`'s existing **per-invocation** `server_id` field ([runtime.rs:44-46](../../crates/zotero-cli/src/runtime.rs#L44))
  is sufficient for "did Zotero restart since I last checked" — no persistence needed.
- If Slice 0 finds `server_id` rotates per-launch and this correlates with authorization state
  resetting, that is useful context for the error message in §3.3 — see Unresolved Questions; it
  does not change the no-persistence decision above.

### 3.5 (§C2) Compatibility renderer and post-write state parity

**Rule: a write command must never construct its own output shape.** After a write commits, re-read
the affected object through the **existing read path** (whatever Phase 14's read-backend matrix
already says serves that object type) and render it through the same normalizer the corresponding
read command already uses.

- **Backend identity must never leak into stdout JSON.** This must be an enforced test, not just a
  documented rule (red-team finding: nothing in the original Testing Strategy actually asserted
  this for single-backend commands like `js`/`sync`/`item duplicates`, which have no dual path to
  diff against). Add a standing schema check across **every** write command's JSON output for a
  denylist of keys (`backend`, `server_id`, raw Local API `version`) that runs regardless of how
  many backends a given command has — see §Testing Strategy.
- **Local API `version` is not the Web API's/sync's `version`.** Never pass it through. Any
  concurrency-control header a write needs (`If-Unmodified-Since-Version` on PATCH/DELETE) must be
  fetched fresh via a Local API `GET` immediately before the write, never reused from SQLite.
- **Delete- and merge-class commands need their own explicit sub-rule, not a fallout of the
  general one (red-team finding).** `item delete`/`collection delete` leave nothing to re-read by
  definition; `item merge` destroys source items and can change the survivor's fields. For these:
  - Delete: re-`GET` the deleted key and assert absence (404, or the appropriate trash/tombstone
    state) — do not skip the verification step just because there's "nothing to read."
  - Merge: re-`GET` the surviving item **and** assert every merged-away key is gone or redirects,
    not just that the survivor's JSON renders correctly.
- **Post-write state parity must be an enforced runtime check, not only a test-time comparison
  (red-team finding).** The original draft only checked this via a pre-ship "cross-backend
  consistency" test. In production, if a Local API PATCH partially applies (e.g. `title` commits
  but a malformed `date` field is silently dropped in the same request — Local API transactionality
  across mixed-validity fields is unconfirmed, see Unresolved Questions), the re-read would silently
  show the partial result as a normal, exit-0 success with no signal to the caller. Every write must
  diff its requested fields against the post-write observed fields and surface a distinct
  warning/error status on mismatch — not rely solely on pre-ship test coverage to catch this class
  of bug.

### 3.6 (§C3) Command/backend matrix

Columns are keyed on the **actual capability flag**, `local_api_writes_available`, not on Zotero
version (red-team finding: version-keyed columns silently break on a documented real machine state
— Zotero 10+ with Local API disabled in preferences — where the flag is `false` despite the version
being 10+). Whenever the flag is `false`, **every** CRUD row falls back to the JS Bridge column,
regardless of why (Zotero ≤9, or Zotero 10+ with Local API off).

Evidence tiers, corrected per red-team findings: **LIVE VERIFIED** (confirmed against real Zotero
10.0.1), **DOC-VERIFIED** (Zotero's public Web API v3 docs, not directly tested against this port),
**DOC-VERIFIED-BY-OMISSION** (no such endpoint appears in Zotero's documented API surface, but this
was never an actual live probe of that specific shape — weaker than LIVE VERIFIED, must not be
conflated with it), **VERIFY IN SLICE 3** (genuinely unknown, must be live-tested before shipping).

| # | Command | `local_api_writes_available == false` | `== true` | Evidence | Notes |
|---|---|---|---|---|---|
| 75 | `item update` | JS Bridge | Local API `PATCH /items/<key>` | DOC-VERIFIED | Straightforward field patch |
| 74 | `item tag --add/--remove` | JS Bridge | Local API `PATCH /items/<key>` (`data.tags`, full-array-replace — read current tags first, see row 47's correction) | DOC-VERIFIED | Not the dedicated library-wide tag-delete endpoint (Finding 3) — a different, rarer operation; **VERIFY IN SLICE 3** whether `item tag` ever needs that form |
| 57 | `item delete` | JS Bridge | Local API `DELETE /items/<key>` | DOC-VERIFIED, needs `If-Unmodified-Since-Version` (§3.5) | Post-write check is "assert absence," not a re-read of a live object (§3.5) |
| 50 | `item attach` | JS Bridge | **VERIFY IN SLICE 3** — Local API file upload is a documented multi-step Web API protocol (create attachment item, upload to storage, register). Defaults to JS Bridge; resolving to Local API is a bonus, not required (see Success Criteria) | High implementation risk if forced onto Local API prematurely |
| 47 | `item add-to-collection` | JS Bridge (already default upstream) | **Corrected in red-team review — was wrong in the first draft.** Zotero's Web API v3 treats array properties (`data.collections`, `data.tags`) as **complete replacement lists on PATCH**: any key omitted from the submitted array is removed. A naive `PATCH {"collections": ["<new-key>"]}` would silently strip the item from every other collection it belongs to — a real data-loss bug for a command whose whole purpose is *additive* membership. Correct implementation: `GET` the item's current `data.collections` (with `If-Unmodified-Since-Version`), compute the union client-side, `PATCH` the full array | DOC-VERIFIED (Zotero Web API v3 "Write Requests": array properties are complete lists, not merged) | This is now DOC-VERIFIED, not "unconfirmed" — do not re-litigate the semantics, only confirm the specific request shape live |
| 68 | `item move-to-collection` | JS Bridge, one dedicated add+remove transaction (no bridge path exists upstream — red-team Finding 5: upstream bridge primitives are separate saves, not atomic) | Same full-array-replace mechanism as row 47, computing the new set (remove `--from` sources / all-other-collections, add target) client-side before one `PATCH` | DOC-VERIFIED (same API fact as row 47) | Keep the JS Bridge transactional design for the `false` column regardless |
| 18 | `collection create` | JS Bridge (already default) | Local API `POST /collections` | DOC-VERIFIED | Non-idempotent — see §3.3's duplicate-write protection requirement before implementing |
| 27 | `collection rename` | JS Bridge | Local API `PATCH /collections/<key>` | DOC-VERIFIED | |
| 19 | `collection delete` | JS Bridge | Local API `DELETE /collections/<key>` | DOC-VERIFIED | Post-write check is "assert absence" (§3.5) |
| 26 | `collection remove-item` | JS Bridge | Same full-array-replace mechanism as row 47/68 | DOC-VERIFIED | |
| 58 | `item duplicates` (find) | JS Bridge (`Zotero.Duplicates` internal object) | JS Bridge | **DOC-VERIFIED-BY-OMISSION, corrected from an overclaimed "LIVE VERIFIED absence" in the first draft.** No duplicates-shaped request was ever actually sent during Phase 14's live probing — the six endpoints it probed (`/collections`, `/items/top`, `/items/<key>`, `/items/<key>/children`, `/tags`, `/connector/getSelectedCollection`) are unrelated. Absence from that list is not a tested negative. Slice 3 must attempt an actual plausible request shape (e.g. a duplicates-related path) and observe a real 404 before this can be upgraded to LIVE VERIFIED — it does not block shipping to JS Bridge (the safe default either way), only the evidence-tier label | Privileged, version-independent either way |
| 66 | `item merge` | JS Bridge (Zotero merge logic touches multiple items transactionally) | JS Bridge | Same DOC-VERIFIED-BY-OMISSION correction as row 58 | Post-write check per §3.5's merge sub-rule |
| 94 | `sync` | JS Bridge (`Zotero.Sync.Runner`) | JS Bridge | Same DOC-VERIFIED-BY-OMISSION correction as row 58 | Privileged, version-independent |
| 76 | `js` | JS Bridge (by definition) | JS Bridge | N/A | Never a Local/Connector API candidate |
| 9, 12, 14, 8 | `app install-plugin` / `app plugin-status` / `app uninstall-plugin` / `app enable-local-api` | XPI + local filesystem (this phase builds the `plugin/` module) | Same | This phase owns fully | See §3.12 |

**Removed from this matrix (source-audit correction — these are not Phase 6 commands at all, not
merely mis-routed):** `add doi` / `import doi` / `import pmid` / `note add` / `import file` /
`import json` / `add url` all belong to **Phase 7** ("Ingest, Attachments and PDF Cascade" per
`plan.md`'s phase table), regardless of backend. An earlier draft of this matrix incorrectly listed
the first three as Phase-6-owned "Connector API" rows. Verified backend facts, recorded here for
Phase 7's benefit rather than asserted as a Phase 6 routing decision (see Overview for full
citations): `add doi`/`import doi` are hybrid (JS Bridge dedupe → JS Bridge translator → Connector
Crossref-BibTeX fallback); `import pmid` and `note add` are JS-Bridge-only with no Connector
involvement; `import file`/`import json`/`add url`'s generic-webpage leg are the actual
Connector-`saveItems`/`/connector/import`-routed commands. All of this depends on §3.1a's Phase 5C
gate, not on anything Phase 6 builds.

**Explicitly out of this phase's scope:** `collection stats`, `item annotations` / `note get`
(reads, Phase 5/14 territory), `item search-annotations` / `item search-fulltext` (no confirmed
Local/Connector API equivalent found in any evidence gathered — a genuine open question, not
silently assumed JS-Bridge, see Unresolved Questions), `item find-pdf` / `item fetch-pdf` /
`collection find-pdfs` / `collection fetch-pdfs` / `add doi` / `import doi` / `import pmid` /
`import file` / `import json` / `add url` / `note add` (all Phase 7's territory per `plan.md`'s
phase table).

**Reconciling the "~10 privileged ops" estimate:** `plan.md`'s phase table (line 147) and the
research doc still say "~10 privileged operations." This matrix's confirmed floor is **4** rows that
are JS-Bridge-only regardless of the capability flag (`js`, `sync`, `item duplicates`, `item merge`),
with up to **3** more contingent on unfavorable Slice 3 resolutions (`item attach`, plus `item tag`'s
library-wide-delete variant if needed) — a ceiling of 7, not ~10. `plan.md`'s line has been updated
to match (see that file's own changelog); this matrix is the authoritative, current count.

### 3.7 No direct SQLite writes (invariant, not a new decision)

Already true in the merged codebase — every `pub fn` in `db.rs` is a reader; the only `INSERT`s live
in `#[cfg(test)]` fixture setup using a separate direct `rusqlite::Connection`, never
`connect_readonly`. This phase must not introduce the first one.

**Corrected regression guard (red-team finding: the original grep pattern would not have caught the
codebase's own coding style).** `db.rs` imports `use rusqlite::{Connection, ...}` and calls the
**unqualified** `Connection::open(...)` / `Connection::open_with_flags(...)` throughout — never the
fully-qualified `rusqlite::Connection::open`. A guard that matches only the fully-qualified form
would report green while a real write path using the file's own established style slips through.
The regression test must match the unqualified call sites actually in use (`\bConnection::open\b`,
`\.execute\(`) outside `#[cfg(test)]` blocks and outside declared read-only helpers — or, more
robustly, enforce this at compile time via a newtype wrapper around `rusqlite::Connection` that only
exposes query methods in non-test builds, rather than relying on a source-text grep at all.

### 3.8 Plural selection APIs and `use-selected` semantics

Phase 14 answered Open Question 3: `/connector/getSelectedCollection` requires `POST {}`, and Zotero
10.0.1's collection tree has **no multi-select at all** — the response is inherently single-valued
(`targets: [...]` is plural-*shaped* but always length 1). `session use-selected` / `collection
use-selected` are Phase 5's commands, **not re-implemented here**.

**Corrected scope (source audit finding): no currently-specified Phase 6 command actually needs
this.** Every Phase 6 CRUD command's Python signature takes an explicit collection/item reference
(`item add-to-collection <ITEM_REF> <COLLECTION_REF>`, `collection rename <COLLECTION_KEY> --name`,
etc. — none show a `--use-selected` flag in `compatibility-matrix.md`'s command-signature column).
The implicit "no explicit target → fall back to session → fall back to connector-selected → fall
back to default library" chain (`_resolve_target`, `imports.py:75-144`) is used only by Phase 7's
import commands, not by anything Phase 6 owns. This section's guidance is retained as **defensive
policy for future work, not a current Phase 6 requirement**: if a future Phase 6 write command ever
does grow an implicit "use the selected collection/item" target, it must consume the connector
response defensively as a plural shape — take the single element, and treat `targets.len() != 1` as
an error rather than silently taking `targets[0]` — and it must gate on §3.1a's Phase 5C completing
first, since `get_selected_collection` doesn't exist in the Rust codebase yet either.

### 3.8a The ≤9 JS-Bridge CRUD template set (new slice, added in red-team review)

**Gap found in red-team review:** the first draft of this plan assigned Agent A only the
already-privileged bridge commands (`js`, `sync`, `item duplicates`, `item merge`) in Slice 7, and
assigned Agent B the Local-API-first commands in Slice 8 — but **nobody was assigned the ≤9
JS-Bridge implementation of the CRUD rows** (`item update`, `item tag`, `item delete`, `item attach`,
`item add-to-collection`, `item move-to-collection`'s bridge transaction, `collection
create`/`rename`/`delete`/`remove-item`) that §3.6's matrix requires whenever
`local_api_writes_available` is `false`. This work exists **regardless of Slice 0's outcome** — it
is not itself the write-consent spike's contingency, it is baseline scope the matrix always requires.
It is now **Slice 1b**, owned by Agent A (touches `bridge/`, which Agent A already owns), scheduled
alongside Slice 1/2 with no dependency on Slice 0.

### 3.9 D1 fix — parameters via `JSON.parse`, not string concatenation (JS Bridge commands only)

Applies to whatever's on the JS-Bridge column of §3.6's matrix. Python builds JS by string
interpolation, escaping only `'` (`core/jsbridge.py:360-374`); a title containing `C:\Users\x` or a
newline produces malformed or injected JavaScript. Pass every parameter as one JSON-encoded blob:

```rust
let params = serde_json::json!({ "libraryID": library_id, "key": item_key, "fields": fields });
let code = format!(
    "const P = JSON.parse({});\n{}",
    serde_json::to_string(&serde_json::to_string(&params)?)?,
    include_str!("js/item_update.js")
);
```

```js
// js/item_update.js — no interpolation anywhere
var item = Zotero.Items.getByLibraryAndKey(P.libraryID, P.key);
if (!item) { return 'ERROR: item ' + P.key + ' not found'; }
for (const [k, v] of Object.entries(P.fields)) { item.setField(k, v); }
await item.saveTx();
return 'OK: updated ' + item.getField('title').substring(0, 60);
```

`serde_json` produces a correctly-escaped JSON string literal for any input, so the injection class
disappears rather than narrows. **Return values must stay byte-identical** to Python's (`'OK: '`,
`'ERROR: '`, `'FOUND: '`, `'NOT_FOUND: '`, `'TIMEOUT: '`, `'DELETED: '`).

A structural side benefit: **any command routed through Local API or Connector API instead of the JS
Bridge eliminates the entire D1 injection class for that command by construction** — those surfaces
take JSON request bodies natively. D1's blast radius is exactly the JS-Bridge column, nothing more.

### 3.10 D3 fix — cache positive probes only

`execute_js` in the Python reference calls `bridge_endpoint_active()` before every call
(`jsbridge.py:289`), doubling round trips. Cache only a **successful** probe in a `OnceCell` for the
process lifetime; never cache a negative result permanently.

### 3.11 AppleScript fallback — dropped

`_execute_applescript` (`jsbridge.py:153-212`) drives Zotero's "Run JavaScript" dialog via
`osascript`, keyed on localized menu names. macOS-only, deprecated upstream, superseded by the XPI.
When the bridge endpoint is inactive, fail loudly with Python's existing non-macOS message rather
than silently automating the GUI. Document as an intentional macOS-only behavior change.

### 3.12 XPI packaging and the fork problem

**Ownership correction (caught in final review before commit): Phase 14 established the
requirement, it did not implement it.** Phase 14's file specifies the target
(`strict_max_version: "9.0.*" → "10.0.*"`) as part of documenting what Zotero 10 compatibility
requires, but its own "Merge-gate classification" section is explicit that this was never actually
applied: *"XPI Zotero 10 compatibility (OQ4, `strict_max_version`, ...) — no `plugin/` directory
exists yet anywhere in this repo; Phase 6 creates it. Evidence stays BLOCKED until Phase 6 has a
testable endpoint."* There is no `manifest.json` anywhere in this repository today for anything to
have bumped — `find crates -iname "manifest.json"` returns nothing. **This phase creates
`manifest.json` from scratch and sets `strict_max_version: "10.0.*"` itself; Phase 14 owns the
requirement, Phase 6 owns the implementation and the live verification.** The bump must not be
treated as verified, tested, or already-done anywhere in this plan until Slice 2's XPI actually
installs and `/cli-bridge/eval` responds against a live Zotero 10 instance — the same discipline
Phase 14 itself used for every other claim (LIVE VERIFIED vs. DOC-VERIFIED vs. BLOCKED).

`manifest.json` upstream (`reference/cli-anything-zotero/plugin/zotero-cli-bridge/manifest.json`)
declares `"id": "cli-bridge@cli-anything.dev"` and an `update_url` pointing at upstream's repo. Both
must change so upstream cannot push updates to this fork's users. The addon id determines the
installed XPI filename, so a fork can coexist with an installed upstream plugin rather than
clobbering it — but `plugin-status` must then disambiguate which is active.

**Decision (resolved before commit, was left open in the prior draft): ownership marker, not
single-plugin policy.** A single-plugin policy (detect and require uninstalling an installed
upstream XPI before installing the fork) only gates at install time. `bootstrap.js` is byte-identical
to upstream except id/`update_url` (red-team Finding 6, verified against
`plugin/zotero-cli-bridge/bootstrap.js:41-67` and `manifest.json:6-11`: both extensions
register/delete the identical `Zotero.Server.Endpoints['/cli-bridge/eval']` key), so a later
out-of-band upstream reinstall (profile restore, manual drag-in, the original Python CLI's own
`install-plugin`) silently reclaims the endpoint on the next Zotero restart with zero code-level
signal — `plugin-status` would report "active" with no way to know which code is actually executing
arbitrary JS on the caller's behalf. An install-time-only gate cannot detect this. **Chosen design:**
add a minimal endpoint ownership/version marker to the forked `bootstrap.js` — e.g. the eval
endpoint's response includes a small, fixed marker field (or a dedicated lightweight
`/cli-bridge/ownership` companion endpoint) identifying the fork's id/version — so `plugin-status`
can verify ownership **continuously, per invocation, through the endpoint itself**, not only by
checking XPI files on disk at install time. This is a deliberate, documented, minimal deviation from
"byte-identical to upstream" (previously an open trade-off; now a resolved decision): keep the
marker to the smallest change that makes ownership machine-verifiable, hash-test everything else in
`bootstrap.js` against upstream to prove the deviation is scoped to exactly that marker and nothing
else, and document the deviation explicitly in this phase's XPI packaging notes and in
`docs/ZOTERO-COMPATIBILITY.md`.

**OQ4** (`/cli-bridge/eval` under Zotero 10's hardened Host/UA/Origin checks) has been BLOCKED since
Phase 14 — no `plugin/` directory exists anywhere in this repo yet. This phase produces the first
testable endpoint — resolving OQ4 live is an early Slice 2 task, alongside the `strict_max_version`
bump itself and the ownership-marker implementation above.

**Security note, unchanged:** the XPI grants arbitrary privileged code execution inside Zotero to
any local process reaching `127.0.0.1:23119`. Keep the endpoint bound to loopback, keep
`permitBookmarklet: false`, document the exposure in `docs/SECURITY.md`. The raw `js` command must
be marked privileged in generated agent skill docs.

### 3.13 Shared write-interface contract (defined now, added in red-team review)

**Gap found in red-team review:** the first draft deferred the interface between Agent A's bridge
functions and Agent B's Local/Connector-API functions to "an Agent B implementation decision,"
discoverable only when Slice 6 merges both tracks — the point in the schedule with the least slack
to rework either side. Fixing the contract now, before either track starts:

```rust
/// Every write path (Bridge, Local API, Connector API) returns this, regardless of backend.
/// Slice 6's dispatch in `lib.rs`/`cli.rs` matches on `WriteOutcome`, never on backend-specific types.
pub enum WriteOutcome {
    /// Write applied. `affected_key` feeds §3.5's post-write re-read; the renderer produces the
    /// stdout JSON from that re-read, never from this variant's own data.
    Applied { affected_key: String },
    /// Local/Connector API specific: consent not yet granted, or revoked. Maps to §3.3's
    /// machine-distinguishable exit code / `needs_human_action` field. Never triggers a Bridge
    /// fallback (§3.3).
    AuthorizationDenied { detail: String },
    /// Version/precondition mismatch (`If-Unmodified-Since-Version` conflict, or Bridge-side
    /// equivalent). Caller re-reads and may retry with a fresh version — does not imply the write
    /// landed.
    Conflict { detail: String },
    /// Transport/unexpected failure. Distinct from `AuthorizationDenied` so §3.3's retry-vs-stop
    /// logic can tell "ask a human" apart from "genuinely broken."
    TransportError { detail: String },
}
```

Both agents implement functions returning `anyhow::Result<WriteOutcome>` (or an equivalent
project-consistent error type — the point is the shared `WriteOutcome` enum, not the wrapping error
type) from day one, not whatever ad hoc shape each track finds convenient. Slice 6 becomes a
mechanical `match` over this enum plus the §3.5 renderer, not a design exercise.

## Requirements

**Functional**
- Slice 0's disposable write-consent spike completed and its findings appended to
  `plans/research/zotero-10-impact-on-rust-port.md` §8 before any Local API write code merges
- §3.6's matrix fully resolved to a committed backend for every row (defaulting to JS Bridge is an
  acceptable final answer — see Success Criteria; only the evidence *tier* for the DOC-VERIFIED-BY-
  OMISSION rows needs upgrading, not necessarily the routing decision)
- All CRUD write commands routed per the resolved matrix, keyed on `local_api_writes_available`
- Post-write state parity: every write command re-reads and renders through the existing read
  normalizer, with delete/merge-specific verification per §3.5, and a runtime requested-vs-observed
  diff that surfaces mismatches rather than silently succeeding
- XPI build/install/uninstall/version-detection/`plugin-status`, forked id/`update_url`,
  `strict_max_version: 10.0.*` set and live-verified by **this phase** (Phase 14 established the
  requirement only — no `manifest.json` existed for it to have bumped, §3.12), the ownership-marker
  design (§3.12, resolved — not single-plugin policy), and Zotero ≤9 back-compat via the full ≤9
  JS-Bridge CRUD template set (§3.8a)
- `item move-to-collection` implemented as one Zotero-side operation on whichever backend §3.6
  resolves it to, using full-array-replace semantics on Local API (not append)

**Non-functional**
- No direct SQLite writes anywhere (§3.7's corrected regression guard passes)
- Parameters containing `\`, `'`, `"`, newlines, `</script>`, CJK must not corrupt generated JS on
  any JS-Bridge-routed command
- Backend identity never appears in stdout JSON — enforced by a standing denylist test (§3.5), not
  only by cross-backend diffing
- One endpoint probe per process for the JS Bridge, not per call
- `Zotero-Server-ID` never used as, or stored as, a credential, and never persisted (§3.4)
- Write-auth failures surface a machine-distinguishable signal an agent caller can branch on (§3.3)

## Related Code Files

- Create: `crates/zotero-cli/src/bridge/mod.rs`, `bridge/templates.rs`, `bridge/js/*.js`
- Create: `crates/zotero-cli/src/plugin/mod.rs`, `plugin/assets/{manifest.json,bootstrap.js}`
- Create: `crates/zotero-cli/src/write_router.rs` (Local API write client — **not** a Connector
  client, corrected per §3.1a/Overview: Phase 6 has no Connector-routed commands — the `WriteOutcome`
  enum from §3.13, and the compatibility renderer)
- Modify: `crates/zotero-cli/src/http.rs` (extend with `PATCH`/`POST`/`DELETE` write methods,
  `If-Unmodified-Since-Version` handling)
- Modify: `crates/zotero-cli/src/catalog.rs` (item/collection normalizers reused by §3.5's renderer)
- **Modify: `crates/zotero-cli/src/cli.rs`** (new `ItemCommands`/`CollectionCommands` variants —
  `Update`/`Delete`/`Tag`/`Attach`/`AddToCollection`/`Create`/`Rename`/`RemoveItem` — currently
  absent) **and `crates/zotero-cli/src/lib.rs`** (new arms in `dispatch_command`'s match block,
  [lib.rs:52](../../crates/zotero-cli/src/lib.rs#L52)). **Correction (red-team finding): these two
  files, not `catalog.rs`, are the actual shared dispatch surface both agents' new commands land in.**
  `catalog.rs` has no command-dispatch or routing logic at all — it is pure domain logic. `cli.rs`
  and `lib.rs` are the single-owner Slice 6 serialization boundary; treat them with the same
  no-parallel-edits discipline originally described (incorrectly) for `catalog.rs` alone.
- Create: `crates/zotero-cli/examples/write_consent_spike.rs` (Slice 0, disposable, never shipped)
- Create: `crates/zotero-cli/tests/{bridge_templates.rs,bridge_injection.rs,plugin_xpi.rs,write_backend_routing.rs,write_output_denylist.rs}`
- Modify: `docs/SECURITY.md` (eval-endpoint exposure), `docs/ZOTERO-COMPATIBILITY.md` (write-path
  section, condensed §3.6 matrix)
- Append: `plans/research/zotero-10-impact-on-rust-port.md` §8 (Slice 0 findings — not a new file)

## Implementation Slices and Dependencies

```
                                                    (Phase 5C — Connector client completion,
                                                     §3.1a. Outside Phase 6; gates only a
                                                     hypothetical future §3.8 command, not
                                                     any currently-specified Phase 6 slice.)

Slice 0 (Agent B, blocking) ──┬─→ Slice 3 (Agent B) ─→ Slice 4 (Agent B) ─→ Slice 5 (Agent B) ─┐
                               │                                                                 │
Slice 1  (Agent A, parallel) ─┼─→ Slice 2 (Agent A) ────────────────────────────────────────────┤
Slice 1b (Agent A, parallel) ─┘                                                                  ├─→ Slice 6 (joint,
                                                                                                  │   serial merge into
                                        Slice 7 (Agent A, after 1) ── soft-depends on Slice 3 ───┤   cli.rs/lib.rs)
                                        Slice 8 (Agent B, after 3+4) ─────────────────────────────┘
                                                        ↓
                                        Slice 9 (joint) ─→ Slice 10 (joint)
```

The `WriteOutcome` contract (§3.13) is fixed in this document before any slice starts — it is not a
Slice 6 deliverable, it is a precondition for Slices 3, 7, and 8 to be interface-compatible.
**None of Slices 0-10 below depend on Phase 5C** — Local API and JS Bridge are independent HTTP
surfaces from the Connector API, and §3.6's audit found zero Phase-6-owned commands are
Connector-routed. Phase 5C is tracked in §3.1a purely so it is never silently assumed to exist by
whoever picks up the deferred §3.8 future-work item, or by Phase 7's planning.

| Slice | Owner | Depends on | Delivers |
|---|---|---|---|
| 0 | Agent B | Phase 14 merged | **COMPLETE 2026-08-29** — Disposable write-consent spike; answers OQ9/OQ5, 401/403/revocation, `Zotero-Server-ID` stability. Dialog-stacking behavior (§3.2 finding 7) remains untested — see `plans/research/zotero-10-impact-on-rust-port.md` §8.3 item 7. Full evidence: §8 of that document. Outcome: **SLICE 0 PASSED — LOCAL-API-FIRST REMAINS VALID** |
| 1 | Agent A | — | Bridge transport, D1/D3 fixes, JS template extraction infrastructure |
| 1b | Agent A | 1 | The ≤9 JS-Bridge CRUD template set (§3.8a) — baseline scope, independent of Slice 0's outcome |
| 2 | Agent A | 1 | XPI packaging/fork changes: create `manifest.json`/`bootstrap.js` from scratch, set `strict_max_version: 10.0.*`, implement §3.12's ownership marker (resolved decision, not single-plugin policy); OQ4 resolved against a real Zotero 10 |
| 3 | Agent B | 0 | Local API write client (PATCH/POST/DELETE, version header, full-array-replace helper, capability gate). **No Connector-client dependency** (corrected — Slice 3 is Local API only; the Connector client is Phase 5C's scope, gated separately per §3.1a, not smoke-checked from within this slice) |
| 4 | Agent B | 3 | Compatibility renderer / post-write state parity (§3.5), reusing Phase 4/14 normalizers |
| 5 | Agent B | 0, 3 | 401/403/revocation error handling, `AuthorizationDenied` signal wiring (§3.3) |
| 6 | Joint (single owner) | 2, 4, 5, resolved §3.6 matrix | Wire final routing into `cli.rs`/`lib.rs` — serialized on purpose, shared files |
| 7 | Agent A | 1; soft-depends on 3 (matrix's "VERIFY IN SLICE 3" resolutions may add rows to this slice's scope) | Implement confirmed privileged bridge-only commands (`js`, `sync`, `item duplicates`, `item merge`, `item move-to-collection`'s ≤9 transaction) |
| 8 | Agent B | 3, 4 | Implement Local-API-first write commands per resolved matrix, using the full-array-replace helper for rows 47/68/26 |
| 9 | Joint | 6, 7, 8 | Injection regression suite, conformance tests, golden re-baseline, cross-backend consistency tests, backend-identity denylist test |
| 10 | Joint | 9 | `docs/SECURITY.md`, `docs/ZOTERO-COMPATIBILITY.md` write-path section, migration guide |

## Testing Strategy

- **Injection regression suite:** adversarial inputs (`\`, `'`, `"`, newline, `</script>`, `${}`,
  CJK) round-trip correctly through every JS-Bridge-routed write command, using the Phase 1
  `unicode-cjk` fixture. Same inputs demonstrably **break** the Python implementation.
- **Offline template linting:** every static JS template renders and lints without a live Zotero.
- **Bridge smoke tests:** against a real Zotero, or a JS harness exposing the required Zotero object
  methods — the existing fake server is not sufficient on its own.
- **Local API write tests:** a mock HTTP server for deterministic 401/403/revocation status-code
  handling, plus the live spike's one-time real-write verification for actual behavior.
- **Cross-backend consistency:** for every command §3.6 resolves to "both backends possible," assert
  both paths render identical CLI JSON for the same logical write.
- **Backend-identity denylist test (new, red-team finding):** a standing test that checks **every**
  write command Phase 6 itself implements — including single-backend commands with no dual path to
  diff against (`js`, `sync`, `item duplicates`, `item merge`, `item attach` if it stays Bridge-only)
  — for a denylist of keys (`backend`, `server_id`, raw Local API `version`). This closes the gap
  where cross-backend diffing alone would never exercise these commands. (Corrected scope: `note
  add`/`import doi`/`import pmid` removed from this list — they are Phase 7's commands, not Phase
  6's; Phase 7 should adopt the same denylist pattern for its own write commands when it lands.)
- **No-SQLite-write regression guard** (§3.7, corrected pattern): matches the unqualified
  `Connection::open`/`.execute(` calls the codebase actually uses, outside `#[cfg(test)]` and
  declared read-only helpers.
- **Write-fixture harness (corrected in red-team review — the original design was unsafe).** The
  first draft proposed restoring a scratch fixture "by file copy between harness runs," matching
  `harness/fixtures/build_fixture.py`'s existing pattern. That pattern was built for a static file
  nothing has open; write testing requires a **live Zotero process** attached to the same profile
  (for Bridge smoke tests and Slice 2's OQ4 verification), and `db.rs`'s own live-verified tests
  (`connect_readonly_refuses_not_falls_back_when_wal_database_is_locked`) already prove Zotero holds
  its SQLite connection in exclusive locking mode. Copying a file out from under a running Zotero
  process risks a sharing-violation failure (Windows) or a silent divergence between Zotero's
  in-memory state and the file on disk (macOS/Linux), producing flaky, non-reproducible pollution
  across runs rather than a clean failure. **Corrected approach:** reset write-test state through
  the API itself (delete/reset the scratch items via Local API or Bridge calls, which Zotero
  observes), not a raw file swap; if a full fixture reset is genuinely needed, do it with Zotero
  fully closed between runs, not concurrently with the smoke-test steps that require Zotero running.

## Agent A / Agent B Parallel Split

The `WriteOutcome` contract (§3.13) is fixed before either track starts, closing the "discovered
only at merge time" risk from the first draft. The two files agents must not edit concurrently are
**`cli.rs` and `lib.rs`** (corrected from `catalog.rs`, which has no dispatch logic at all — see
Related Code Files).

**Agent A — JS Bridge & Plugin track.** Owns: `bridge/`, `plugin/`, `tests/bridge_templates.rs`,
`tests/bridge_injection.rs`, `tests/plugin_xpi.rs`. Slices 1, 1b, 2, and (mostly) 7 do not depend on
Slice 0's findings — D1/D3 fixes, the full ≤9 JS-Bridge CRUD template set (§3.8a), XPI fork
packaging, and the already-confirmed-privileged commands can all start immediately. Only Slice 7's
*final* scope (whether any "VERIFY IN SLICE 3" row joins the permanently-bridge-only set) needs
Agent B's matrix resolution — tracked as a soft dependency in the slice table, not a hard blocker.

**Agent B — Local API Write Path & Authorization track.** Owns: `http.rs` extensions,
`write_router.rs`, the disposable spike (appended to the existing research doc, not a new file), the
compatibility renderer. Runs Slice 0 first, which requires live Zotero 10 instance access — the same
hard constraint Phase 14 had.

**Scheduling assumption, stated explicitly (added in red-team review, since the first draft never
justified the split's overhead against a sequential baseline):** this split is worth its
coordination cost only if Agent A's Slices 1/1b/2/7 provide enough independently-schedulable work
(estimated several days) to absorb whatever delay Agent B hits securing live Zotero-instance access
for Slice 0 — a scheduling/logistics delay, not a technical one, but a real one based on Phase 14's
own experience needing a second machine/VM. This is an estimate, not a guarantee: if instance access
turns out to be immediate and Agent A's independent work is short, the split saves less time than
assumed. Track actual wall-clock against this assumption in plan status updates rather than treating
the split as self-evidently faster than a single sequential implementer working the same dependency
order.

**Sync points:** after Slice 0 (Agent B publishes findings; both agents re-check whether the
authorization-persistence answer changes scope), and before Slice 6 (both agents' primitives must
exist and conform to §3.13's `WriteOutcome` contract before the shared `cli.rs`/`lib.rs` wiring step).

## Success Criteria

- [ ] Slice 0's spike findings are appended to `plans/research/zotero-10-impact-on-rust-port.md` §8
      and the authorization-persistence question is answered before any Local API write code merges
- [ ] Every §3.6 matrix row has a **committed backend** for both capability-flag states (JS Bridge
      as the final answer for a "VERIFY IN SLICE 3" row is acceptable — upgrading its evidence tier
      beyond DOC-VERIFIED-BY-OMISSION is a bonus, not a blocking requirement; this reconciles the
      Success Criteria with the Risk Assessment, which the first draft of this plan contradicted)
- [ ] All CRUD commands route per the resolved matrix, keyed on `local_api_writes_available`, not
      Zotero version; SQLite write path count remains zero
- [ ] Post-write state parity: every write command's JSON comes from the same normalizer its
      corresponding read command uses, with delete/merge-specific verification (§3.5) and a runtime
      requested-vs-observed diff that surfaces mismatches
- [ ] `item add-to-collection`/`move-to-collection`/`remove-item`/`item tag` use full-array-replace
      (read-modify-write), never a naive append PATCH
- [ ] Injection suite passes; the same adversarial inputs demonstrably break Python
- [ ] Bridge return strings byte-identical to Python
- [ ] Successful bridge probe cached at most once per process; negative probes retried after
      install/launch/register
- [ ] XPI builds, installs, and registers `/cli-bridge/eval` on Zotero 10 **and** Zotero 9;
      `plugin-status` correctly reports installed/active/version and endpoint ownership continuously,
      not only at install time
- [ ] OQ4 resolved live (no longer BLOCKED)
- [ ] 401/403/revocation behavior matches Slice 0's empirically-confirmed status codes; write-auth
      failures carry a machine-distinguishable "needs human action" signal
- [ ] `collection create`'s duplicate-write risk on retry-after-consent is resolved (idempotency key
      or existence check), not left as an unverified assumption
- [ ] `item move-to-collection` works via one Zotero-side operation with Zotero running, no longer
      requires `--experimental`
- [ ] Backend-identity denylist test passes on every write command, including single-backend ones
- [ ] `docs/SECURITY.md` documents the eval-endpoint exposure; `docs/ZOTERO-COMPATIBILITY.md` gains
      a condensed write-path section

## Risk Assessment

| Risk | Mitigation |
|---|---|
| Authorization does not survive a restart (Slice 0 finds "no") | Explicit outcome gate in §3.2: stop and bring the reversal to the user; §3.8a-3.12 (Bridge-side content) stay valid either way |
| A "VERIFY IN SLICE 3" cell resolves unfavorably | Matrix already defaults those cells to JS Bridge as the safe fallback; resolving to Local API is a bonus (Success Criteria reconciled to match) |
| `cli.rs`/`lib.rs` merge conflict between the two agent tracks | Slice 6 is a single-owner serialized step by design; §3.13's `WriteOutcome` contract is fixed upfront so neither track's primitives need reworking at merge time |
| A JS template is transcribed with subtly different semantics | Extract templates as whole units, diff each against the Python f-string with interpolation removed |
| Return-string drift breaks callers | Golden tests assert exact prefixes |
| Endpoint ownership reclaimed by a later out-of-band upstream XPI install | Resolved by design choice: §3.12 uses an ownership marker verified continuously through the endpoint itself, not an install-time-only single-plugin check |
| Local API `version` field leaks into CLI output | §3.5's renderer rule forbids passing it through; enforced by the standing denylist test, not only cross-backend diffing |
| Write-fixture harness state leaks or corrupts under a running Zotero's exclusive lock | Reset via API calls, not file-copy, per the corrected Testing Strategy |
| Non-idempotent write (`collection create`) duplicated on caller retry after ambiguous consent status | §3.3/Success Criteria requires this resolved (idempotency key or pre-check) before shipping the command |
| Disposable spike accidentally targets a real production Zotero library | §3.2's corrected method requires positive verification (GET-and-match a scratch marker) before any write, not just an unenforced env var |
| Phase 7 (or a future Phase 6 iteration) silently assumes the Connector client (`getSelectedCollection`/`import`/`saveItems`/`saveAttachment`/`updateSession`) already exists because it's "just an HTTP call" | §3.1a names this Phase 5C explicitly and states it is unimplemented (`grep -n "pub fn" http.rs` confirms); flagged as a follow-up for Phase 5's own plan file to formalize |
| A future contributor re-adds `add doi`/`import doi`/`import pmid`/`note add` to Phase 6's matrix, repeating this correction's mistake | §3.6 explicitly documents the removal and cites Phase 7 ownership with source-verified routing facts, not just a routing label, so the reasoning is preserved alongside the decision |

## Unresolved Questions

1. **Authorization persistence (OQ9/OQ5)** — the load-bearing unknown; Slice 0 answers it.
2. **`item attach`'s Local API upload protocol** — unverified multi-step flow; defaults to JS Bridge.
3. **Local API PATCH transactionality across mixed-validity fields** — if one field in a multi-field
   `PATCH` is invalid, does the whole request reject, or can it partially apply? §3.5's runtime diff
   check is the safety net either way, but knowing the answer would simplify error messaging.
4. **`item search-annotations`/`item search-fulltext`'s backend** — no Local or Connector API
   endpoint was found in any evidence gathered. Slice 3/7 should actively search for a documented
   endpoint before defaulting to Bridge.
5. ~~Single-plugin-policy vs. ownership-marker~~ — **Resolved before commit**: ownership marker
   chosen (§3.12). An upstream plugin reinstalled later can reclaim the shared endpoint under a
   single-plugin policy with zero runtime signal; the marker lets `plugin-status` verify ownership
   continuously through the endpoint itself. Remaining implementation-only question: the exact
   marker shape (response field vs. companion endpoint) — a Slice 2 detail, not a product decision.
6. **Does one "Always Allow" authorize all future write shapes, or is it scoped per request/endpoint
   (Slice 0 finding 6)?**
7. **Does a repeated write attempt during a pending consent dialog stack, dedupe, or replace it
   (Slice 0 finding 7, added in red-team review)?** Affects whether an agent's retry-on-failure
   default behavior is safe or actively harmful.
8. **Does a `server_id` change correlate with an authorization reset** (added in red-team review)?
   If Slice 0 finds it does, the §3.3 error message for a 401/403 should reference "Zotero appears
   to have restarted since your last successful write" as a probable cause rather than a generic
   "approve the dialog" message that may not apply if no dialog is actually pending.
9. ~~Does Phase 5 actually ship a working `saveItems`/`saveAttachment` Connector client before this
   phase starts implementing Slice 8's Connector-routed rows?~~ — **Superseded by a source-level
   audit**: Slice 8 has no Connector-routed rows at all — the commands originally listed here
   (`add doi`/`import doi`/`import pmid`/`note add`) are Phase 7's, not Phase 6's (§3.6, Overview).
   The real open item is now tracked as **Phase 5C** (§3.1a): Phase 5's own declared Connector
   client scope (`getSelectedCollection`, `import`, `saveItems`, `saveAttachment`, `updateSession`)
   is unimplemented, and Phase 7's planning — not Phase 6's — needs this gate before it can build
   its import commands. Whether Phase 5's own plan file should formally name this checkpoint is
   flagged as a follow-up recommendation (§3.1a), not resolved in this PR.
