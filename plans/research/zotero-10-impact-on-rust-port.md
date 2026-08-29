# Zotero 10 Impact on the Rust Port

> ## ⚠️ RECONSTRUCTED 2026-08-29 — NOT A VERBATIM RECOVERY
>
> This file was destroyed by an uncoordinated concurrent git operation (`git clean`/checkout from a
> parallel session) before it was ever committed. Unlike `plan.md`, `phase-05`, and `phase-14`, it was
> **never read in full** during the session that is rebuilding it — only its existence and line count
> (344 lines) were observed before the loss. Nothing here should be treated as a byte-for-byte
> recovery of the original.
>
> What follows is reconstructed from two sources only:
> 1. Every direct citation, quoted figure, and structural reference to this file found in the
>    recovered verbatim content of `plan.md` and `phase-14-zotero-10-compatibility-gate.md` (both
>    fully recovered — see those files' own provenance notes).
> 2. **New evidence gathered in the same session that discovered the data loss**, specifically a live
>    read-only test against a real Zotero 10.0.1 instance running on the dev machine — see §7. This is
>    additional to, not a recreation of, whatever the original report's own live-verification section
>    contained (the original was written when the dev machine still ran Zotero 9.0.6, per `plan.md`'s
>    own citation of it: "The dev machine runs Zotero 9.0.6 (rollback journal, no `-wal`)").
>
> The original almost certainly contained more detail (exact vendor-doc quotes and links, additional
> reproduction steps, narrative not captured in the shorter citations above). Treat structural claims
> here as reliable (they are direct quotes/citations from recovered documents) and treat the absence
> of something as "not recoverable," not as "did not exist."

## §0. Context

Zotero 10 shipped **2026-08-17**, after `plan.md` was originally written and after 31 of 96 commands
had already landed against Zotero 7/8/9 assumptions. This report assesses the delta and feeds
`plan.md`'s "Zotero 10 impact" table and `phase-14-zotero-10-compatibility-gate.md`.

At the time this report was originally researched, the primary dev machine ran **Zotero 9.0.6**
(rollback journal, no `-wal` file) — no live Zotero 10 instance was available, so several findings
were necessarily desk/documentation research pending live verification (§6, Open Questions). By the
time of this reconstruction, the dev machine had been upgraded to **Zotero 10.0.1**, which is what
made the live verification in §7 possible.

## §1. WAL mode enabled — CRITICAL

**Finding** (recovered verbatim via `plan.md`'s Zotero 10 impact table, row 1): Zotero 10 enables
SQLite WAL journal mode. The port's `db::connect_readonly` opens `file:{path}?mode=ro&immutable=1`.
`immutable=1` tells SQLite the file cannot change, so it never attaches to `-wal` — under WAL, any
committed-but-uncheckpointed row becomes invisible, silently, with exit code 0 and no warning.

### §1.1 Reproduction (cited directly by `plan.md`'s red-team Finding 15 adjudication note)

> "Finding 15 verified **empirically**, not from documentation alone: a WAL database with
> uncheckpointed commits returned **1 of 5 rows** under `mode=ro&immutable=1` versus 5 of 5 under
> `mode=ro`... Vendor doc independently confirms WAL is enabled in Zotero 10."

`phase-14-zotero-10-compatibility-gate.md` §1 gives the exact reproduction shape (recovered
verbatim):

```
journal_mode : wal          -wal size: 4152      writer sees: 5 rows
mode=ro&immutable=1  ->  1 rows    <-- CURRENT db.rs (80% silent loss, exit 0)
mode=ro              ->  5 rows    <-- correct
```

**Nuance** (recovered verbatim, red-team adjudication note): "SQLite auto-checkpoints (~1000 pages),
so the corruption window is intermittent rather than constant. That makes it *worse* for an agent
tool, not better — an intermittently-wrong read is harder to detect than a consistently-wrong one."

**Fix, and why the naive fix is not the whole story:** see §7 below — live testing performed in the
same session that reconstructed this document found that the straightforward fix ("just drop
`immutable=1`, use `mode=ro`") does not merely risk `SQLITE_CANTOPEN` under some conditions (the
original's stated risk, per Open Question 1) — it **reliably fails outright** while Zotero holds the
database, which is the primary supported state. This is significant enough that it changes the
recommended default behavior; see §7 for the full account and the corrected fallback policy that
resulted from it.

## §2. XPI `strict_max_version: 9.0.*` — CRITICAL

**Finding** (recovered verbatim via `plan.md` row 2 and `phase-14` §3): the CLI Bridge XPI's
`manifest.json` declares `strict_max_version: "9.0.*"`. Zotero's add-on manager refuses to load any
plugin whose `strict_max_version` doesn't cover the running version, so the plugin — and therefore
every JS-Bridge-routed command — is unloadable on Zotero 10 as shipped. This invalidates the
original Phase 6 premise that the XPI would be "reused byte-for-byte."

Zotero's own developer guidance (quoted in `phase-14` §3, recovered verbatim): *"If no changes are
required, you can simply update `strict_max_version` in your plugin's update manifest without
releasing a new version."* This port cannot take that shortcut regardless, because the addon `id`
and `update_url` must also change for the fork (see Phase 6), which does require a real release.

## §3. Local API write support — OPPORTUNITY

**Finding** (recovered verbatim via `plan.md` row 3): Zotero 10's Local API gained write support —
POST/PUT/PATCH/DELETE, tag deletion, full-text operations, and file upload — none of which existed
in the read-only Local API the original design was built against. This is the basis for Phase 6's
redesign to Local-API-first write routing, shrinking the JS Bridge's command count from ~33 to ~10
privileged operations Local API cannot express. See `phase-06-js-bridge-and-injection-hardening.md`'s
own (reconstructed) redesign section for how this plays out architecturally.

## §4. Local API authorization, `Zotero-Server-ID`, and local version semantics — HIGH

**Finding** (recovered verbatim via `plan.md` row 4 and `phase-14` §4): Local API write access
requires a key and a user-facing consent dialog; writes must carry a `Zotero-Server-ID` header;
local object `version` semantics are explicitly decoupled from the Web API's/sync's notion of
version (this is why `phase-06`'s §C2 compatibility-renderer discussion warns against a naive
pass-through of the Local API's `version` field into CLI output).

`Zotero-Server-ID` is also the recommended **capability discriminator** (recovered verbatim,
`phase-14` §4): "`Zotero-Server-ID` presence is a documented, behavioural 10+ discriminator —
preferable to parsing `environment.version`, which reflects the *installed binary* found on disk and
can disagree with the *running* instance the HTTP port actually belongs to."

The **consent-persistence question is the single highest-stakes open item this report identified**:
if "Always Allow" does not survive a Zotero restart, unattended agent writes via Local API are not
viable at all, and the entire Phase 6 redesign direction reverses back to JS-Bridge-first. This is
Open Question 5 below (cited elsewhere as "Open Question 9" under `plan.md`'s own consolidated,
whole-plan open-questions numbering — see the numbering note in §6).

## §5. HTTP hardening — HIGH

**Finding** (recovered verbatim via `plan.md` row 5 and `phase-14` §5): Zotero 10 requires `Host` to
be `localhost`, `127.0.0.1`, or `[::1]` (else HTTP 400), and **drops without any response** requests
carrying a `Mozilla/`-prefixed `User-Agent` or **any** `Origin` header, unless the request carries
`Zotero-Allowed-Request` or the target endpoint sets `allowRequestsFromUnsafeWebContent = true`.
Recovered verbatim, `phase-14` §3: "this check... previously applied only to CORS-simple content
types" — meaning a JSON POST that worked against Zotero ≤9 may now be silently dropped on 10.

The port's `ureq`-based HTTP client passes today "by accident, not design" (`phase-14` §5, recovered
verbatim): `ureq`'s default `User-Agent` is not `Mozilla/`-prefixed, and no code sets an `Origin`
header. This needs to become a locked-in regression test (Phase 5/14), not an accident that a future
change could silently break.

**Explicit cross-check preserved from the original** (recovered verbatim, `phase-14` §5): "the Python
reference sets `User-Agent: Mozilla/5.0` in `metrics.py`, but only for **NIH iCite** (external). Safe
to port verbatim; must never be generalised to Zotero-local requests. Phase 7 must not 'unify' the UA
across clients." — this is a specific, actionable landmine worth preserving exactly.

## §6. Open Questions (require a live Zotero 10 instance)

All six require live verification and could not be answered from documentation alone at the time of
original research. Numbered 1–6 here to match `phase-14-zotero-10-compatibility-gate.md`'s own
references ("resolves the six open questions," "Answer Open Questions 1–6 empirically"). **Numbering
note:** `plan.md`'s consolidated, whole-plan Open Questions list renumbers these as its own Q5–Q10
(continuing after its four pre-existing, non-Zotero-10 questions), and separately, `plan.md`'s
red-team log cites "Open Question 9" for the consent-persistence question — that citation uses
`plan.md`'s consolidated numbering (where it is Q9), which corresponds to **this report's OQ5**. Both
numbering schemes are correct within their own document; this note exists so the two are not mistaken
for a contradiction during the cross-document consistency review.

1. **Does `mode=ro` (no `immutable`) open a live Zotero 10 DB while Zotero holds it?** WAL readers
   must map `-shm`; a read-only open can fail `SQLITE_CANTOPEN`. Originally flagged (recovered
   verbatim, `plan.md`) as "the highest-risk unknown in the whole adaptation." **Answered — see §7,
   and independently re-confirmed 2026-08-29 in a separate session — see §7.3.**
2. **Does `Host: 127.0.0.1:23119` (with port) pass** Zotero 10's new Host allowlist? **Answered
   2026-08-29 — see §7.3: yes, both `Host: localhost:23119` and `Host: 127.0.0.1:23119` pass; an
   unrelated `Host` value gets a real `400`.**
3. **Does `/connector/getSelectedCollection` still exist in 10, and what does it return under
   multi-selection?** Determines `use-selected` semantics — see `phase-05`'s recovered decision
   matrix. **Answered 2026-08-29 — see §7.3: the endpoint exists (requires `POST`, not `GET`), and
   Zotero 10.0.1's collection tree has no multi-select at all, so "under multi-selection" is not a
   reachable state to answer for.**
4. **Does `/cli-bridge/eval` still pass** the hardened browser-origin check without
   `allowRequestsFromUnsafeWebContent`? Not yet answered — no XPI/plugin exists in this repository
   yet to test against (Phase 6 has not started), so this question is currently untestable regardless
   of live Zotero access, and stays open until Phase 6 produces a testable endpoint.
5. **Does "Always Allow" survive a Zotero restart, and where is the key stored?** Determines whether
   unattended agent writes are viable at all on Zotero 10 (see §4). Requires driving Zotero's own
   consent-dialog UI. **Partially observed 2026-08-29 (UI only, no live write attempted — see
   §7.3): Zotero's Advanced settings expose a "Clear Write Authorizations" control, gated on at
   least one authorization existing, confirming *some* persistence mechanism exists. What exactly
   survives a restart, and where it's stored, still requires driving the consent dialog via an
   actual write, which was deliberately not attempted (real production library, writes out of
   scope for this pass).**
6. **Do Zotero-10-migrated saved searches** (e.g. `childNote` → `note` + `resultLevel`) change
   `search list`/`search get` JSON versus the Python baseline on the same library? Not yet answered;
   requires a library that has actually gone through Zotero's migration, not merely a fresh Zotero 10
   install.

## §7. New evidence gathered during reconstruction (2026-08-29, this session)

This section did not exist in the original report — it is genuinely new, gathered live against a
real, running Zotero 10.0.1 instance on the dev machine, immediately before the data-loss incident
that necessitated this reconstruction. Recorded honestly as new findings, not retrofitted into the
original's voice.

### §7.1 OQ1 — `mode=ro` vs. `immutable=1` against a live, running Zotero 10.0.1

Direct `sqlite3` CLI probes against the real `~/Zotero/zotero.sqlite` (WAL confirmed active: `-wal`
and `-shm` files present) while Zotero 10.0.1 was running:

```
mode=ro&immutable=1              -> succeeds, 5754 rows (SELECT count(*) FROM items)
mode=ro (no immutable)           -> FAILS every time: "database is locked" (SQLITE_BUSY, code 5)
mode=ro, busy_timeout=3000..5000 -> still fails; not a transient/timing issue
mode=ro&nolock=1                 -> fails differently ("unable to open database file") — not a fix
```

Reproduced 5+ times across several seconds, including a bare `SELECT 1` (no table access at all), to
rule out a table-specific lock. The failure is **consistent, not transient**.

**Interpretation:** Zotero appears to hold its own primary database connection in SQLite's exclusive
locking mode — a long-standing pattern in Firefox/mozStorage-based applications, predating Zotero 10
and independent of journal mode. This directly explains why the original tool chose `immutable=1` in
the first place: not merely "tolerable staleness," but the *only* way to open the file at all while
Zotero runs, on every Zotero version. WAL changed what `immutable=1` misses (uncheckpointed commits
in `-wal`); it did not change Zotero's locking discipline, so a normal WAL reader cannot attach
concurrently the way SQLite's WAL design would otherwise allow.

**Consequence for the fix (supersedes `phase-14-zotero-10-compatibility-gate.md`'s literal
`connect_readonly` diff, which was written before this evidence existed):** unconditionally removing
`immutable=1` in favor of `mode=ro` does not merely risk occasional `SQLITE_CANTOPEN` — it reliably
makes every SQLite read fail while Zotero is running, which is the tool's primary supported state.
Shipping that literal fix would be a worse regression than the staleness bug it targets.

**Corrected policy adopted as a result (this session, by explicit product decision):** detect whether
Zotero is running via the actual `mode=ro` open attempt itself (a short busy timeout, not a separate
HTTP probe): if it succeeds, use it (this is the fully-correct, WAL-aware path — the common case when
Zotero is not running, or a `-wal` file simply isn't present, i.e. Zotero ≤9). If it fails with
`SQLITE_BUSY` specifically, do **not** silently fall back to `immutable=1` — refuse with a clear,
actionable error by default, and only fall back to the `immutable=1` snapshot behind an explicit
`--allow-stale-sqlite` opt-in. This is stricter than `phase-14`'s original fallback ladder (which
still allowed a documented `immutable=1` fallback as an acceptable rung) — the corrected policy
treats "Zotero is running and Local API is unavailable" as a hard stop by default, consistent with
`plan.md`'s Local-API-first direction for Phase 6, rather than quietly degrading to a
possibly-incomplete read.

### §7.2 Other live checks attempted, and why they are incomplete

A live HTTP probe pass was started against the same Zotero 10.0.1 instance (connector `ping`, Local
API root + `Zotero-Server-ID` header, `Host`/`User-Agent`/`Origin` hardening, `getSelectedCollection`)
but the Zotero process quit partway through — independently of the git data-loss incident — before
OQ2–OQ6 could be answered. `lsof`/`ps` confirmed the process was no longer running; this was not a
sandboxing artifact (an earlier attempt at the same probes failed differently, with connection
refusal from a sandboxed shell that does not share the host's loopback network namespace by default,
which was resolved before the process-quit issue occurred). OQ2–OQ6 remain open pending a fresh
Zotero 10 relaunch and a repeat pass; §6 above marks each accordingly.

### §7.3 Completed repeat pass (2026-08-29, separate session, zotero10-compat-impl branch)

The relaunch this section anticipated happened, in a later session implementing the Zotero 10
compatibility gate against a real, running Zotero 10.0.1 instance on the same dev machine. Recorded
here as genuinely new, independently-gathered evidence — not a recovery or continuation of §7.1/§7.2
(this session never saw their content beforehand) — and it happens to corroborate §7.1's OQ1 finding
independently, which is worth noting given §7.1 itself was reconstructed rather than a verbatim
recovery.

**OQ1, independently re-confirmed.** Direct `sqlite3` CLI probes against the real
`~/Zotero/zotero.sqlite` (confirmed WAL-active: non-empty `-wal`/`-shm` files, growing while Zotero
ran) while Zotero 10.0.1 held it open:

```
mode=ro&immutable=1                    -> succeeds, 5754 rows
mode=ro (no immutable), timeout=2000ms -> FAILS every time: "database is locked" (SQLITE_BUSY)
```

Same failure on a bare `SELECT 1`. After quitting Zotero (WAL auto-checkpointed to 0 bytes on
clean exit), `mode=ro` succeeded and matched `immutable=1`'s count exactly. Matches §7.1's finding
and its interpretation (Zotero holds an exclusive-locking-mode connection, independent of journal
mode) precisely.

**OQ2, answered.** `Host: localhost:23119` → `200`. `Host: 127.0.0.1:23119` (this port's actual
client base URL) → `200`. `Host: evil.example.com` → real `400 Bad Request`.

**HTTP hardening, independently confirmed beyond OQ2.** `curl` with a `Mozilla/`-prefixed
`User-Agent`, and separately with any `Origin` header, both produced exit code 52 ("empty reply
from server") against `/connector/ping` and `/api/` — a genuine dropped connection, not a JSON
error response. A plain `ureq/x.y`-style UA with no `Origin` passed cleanly.

**Capability detection.** `GET /api/` carried `Zotero-Server-ID: QR43gFhLblRt` on **every**
response observed, including a `403 Forbidden` (`"Nothing to see here."` body) when the Local API
was disabled in preferences at the time — a stronger discriminator than the original text
anticipated (header presence, not `200`-gated).

**OQ3, answered.** `/connector/getSelectedCollection` requires `POST {}`; a `GET` returns
`400 Endpoint does not support method`. With a single collection selected, it returns
`{libraryID, libraryName, editable, id, name, tags: {...}, targets: [...]}` — `id` an integer, not
the `"C<n>"` form used inside `targets`. Attempting to reproduce "multi-selection" via both
Cmd-click and Shift-click on Zotero 10.0.1's collection tree found **no multi-select support at
all**: both gestures simply moved the single selection rather than extending it. A true
zero-selection state was not reachable either — the tree keeps exactly one row focused once
anything has been clicked in the session.

**OQ5, partially observed (UI only).** Zotero's Advanced settings exposed a "Clear Write
Authorizations" control once the Local API was enabled — grayed out until at least one write
authorization exists, confirming persistence of *something* beyond the current session, but no
write was actually attempted (deliberately out of scope), so what survives a restart and where it's
stored remain open.

**Local API endpoint shapes, live-verified for the read-backend matrix.** Against the real library
(Local API temporarily enabled for testing, then reverted): `GET /collections` returns
`key`/`version`/`data.name`/`data.parentCollection`/`meta.numItems`/`meta.numCollections`;
`GET /items/top` and `/items/<key>` return the standard Web-API-v3 item shape; `GET /items/<key>/children`
returns child items including attachments with `data.filename`/`linkMode`/`contentType` and a
`links.enclosure.href` file path; `GET /tags` returns `tag`/`meta.type`/`meta.numItems`. No
saved search existed in this library to verify `GET /searches/<key>`'s conditions shape live; the
Web API's public docs and Pyzotero's documented response format describe `data.conditions:
[{condition, operator, value}]`, assessed DOC-VERIFIED rather than LIVE VERIFIED for this reason.
Full detail in `docs/ZOTERO-COMPATIBILITY.md` and `phase-14-zotero-10-compatibility-gate.md` §1c.

**Not re-attempted:** OQ4 (still no XPI in this repo to test) and OQ6 (no migrated saved-search
library available). Both remain open for the same reasons §6 already states.
