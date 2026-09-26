# Upstream parity and Zotero 10 review

**Conducted:** 2026-09-26 15:38 CEST
**Local:** `zotero-rust-cli` @ `f92a769` (v1.0.0; no `src/` changes since the `v1.0.0` tag)
**Upstream:** `PiaoyangGuohai1/cli-anything-zotero` @ `e42a930e` (v1.2.1, last push 2026-07-28)
**Live environment:** Zotero 10.0.3 on macOS, closed during the sweep; profile has this fork's XPI active

## Verdict

The port does **not** match upstream 1:1, and in most places the differences are intentional and
documented. Upstream `main` has not moved since the commit this port is pinned to, so there is no
code drift to catch up on. All upstream activity since the pin sits in two unmerged PRs (#9 and #10)
and four open issues.

Read-side parity holds: every read command in the parity harness classifies Exact or Semantic.
However, the port also copied three upstream bugs unchanged. One of them breaks on Zotero 10 today:
**`search list` and `search get` fail on every Zotero 10 library**, which was reproduced live on
this machine. The claim that Zotero 10 is already supported is therefore true for the Bridge XPI
(PR #9's headline fix), but not for saved searches (PR #9's second fix).

## Contents

1. [Command surface](#1-command-surface)
2. [Behavioural parity (harness run)](#2-behavioural-parity-harness-run)
3. [Upstream PRs and issues against this port](#3-upstream-prs-and-issues-against-this-port)
4. [Findings that need action](#4-findings-that-need-action)
5. [Next steps](#5-next-steps)
6. [Unresolved questions](#6-unresolved-questions)

## 1. Command surface

The upstream Click tree (walked programmatically) was compared with the installed binary's clap
tree. 86 of 96 upstream leaf commands exist with identical names, positional arguments,
argument optionality, and flags. The remaining 10 differ by design, and every one is recorded in
`docs/MIGRATION.md` and `plans/reports/compatibility-matrix.md`.

| Upstream only | Disposition in this port |
|---|---|
| `docx cite`, `doctor`, `insert-citations`, `prepare-zotero-import`, `zoterify`, `zoterify-preflight`, `zoterify-probe` | Deferred to post-v1 (issue #30) |
| `repl` | Dropped; a bare invocation prints help instead |
| `app check-update` | Dropped; package managers own updates |
| `app enable-local-api` | Replaced by `app authorize-local-api` (a real consent handshake) |
| `--experimental` on `collection create`, `item add-to-collection`, `item move-to-collection` | Removed; clap rejects the flag with exit 2 |

The port also adds features upstream lacks: `app authorize-local-api`, `--output-dir` on the three
plugin commands, and `item find --all-libraries` / `--include-feeds`.

Any upstream script that passes `--experimental` to `item move-to-collection` breaks with a usage
error. Upstream *requires* that flag, so every existing caller passes it. `MIGRATION.md` §3 already
says so.

## 2. Behavioural parity (harness run)

`harness/capture.py` was run with the installed binary, and `harness/compare.py` compared the
result against `harness/golden/python`:

| Exact | Semantic | Skipped (live-only) | Mismatch | Missing |
|---|---|---|---|---|
| 47 | 12 | 21 | 21 | 0 |

All 21 mismatches trace to design decisions, not regressions:

- **16 Bridge or write commands** fail against upstream's mock server. That server does not implement
  this fork's `fork`/`id` ownership handshake or the Local API `library.id` response shape. These
  commands are covered by Rust-only tests (`write_router_integration.rs`,
  `write_backend_routing.rs`, and similar), not by Python goldens.
- **`item get (wal-mode)` and `item list (wal-mode)`** are the intentional WAL-read fix. The
  commands table already classifies them as `Changed`.
- **`app version`** reports `1.0.0` against upstream's `1.2.1`.
- **`app enable-local-api` and `item move-to-collection`** both exit 2. The first is a renamed
  command; the second is the removed `--experimental` flag.

`compare.py` exits 1 because it maps the `Changed` class to `Mismatch`
(`harness/compare.py:38`). The harness therefore cannot go green on its own terms, even though
nothing in it is an unexpected result.

Separately, an ad-hoc read-only sweep of 26 SQLite-backed commands ran against the real Zotero
10.0.3 database with Zotero closed. Every command succeeded except `search list`.

## 3. Upstream PRs and issues against this port

| Upstream item | Status in this port | Evidence |
|---|---|---|
| PR #9: XPI `strict_max_version` `9.0.*` blocks Zotero 10 (issues #8, #11) | ✅ Fixed differently: own addon id, cap `10.0.*` | `src/plugin/assets/manifest.json`; `extensions.json` shows `active: true`, `appDisabled: false` on 10.0.3 |
| PR #9: `app doctor` detects `appDisabled` | ❌ Missing | No `appDisabled` read anywhere in `src/` |
| PR #9: `savedSearchConditions.required` dropped in Zotero 10 | ❌ **Broken** | Live: `search list` exits 1 with `no such column: required`. Source: Zotero commit `5ade25f5` (2026-07-15) removed the column. |
| PR #9: e2e fixture robustness | N/A | Python test suite only |
| PR #10: `item delete` / `collection delete` erase permanently | ⚠️ Same as upstream v1.2.1 | `bridge/js/item_delete.js` and `collection_delete.js` call `eraseTx()`. Zotero 10's Local API `DELETE` also calls `eraseTx()` (`server_localAPI.js`, `deleteSingleObject`). |
| PR #10: `item duplicates --by zotero` always returns 0 | ❌ Copied from upstream | `bridge/js/find_duplicates.js`; see finding 3 |
| PR #10: DOI normalizer misses `doi:`, `doi.acm.org`, and doubled prefixes | ⚠️ Same as upstream | `hygiene.rs:62-68` strips a single prefix |
| PR #10: `hygiene.find_duplicates(by="zotero")` returns a false `ok` | ✅ Not applicable | Rust dispatches `--by zotero` straight to the Bridge (`lib.rs:825`) |
| PR #4: Windows backslash escaping in `attach_pdf` | ✅ Not applicable by construction | Bridge params are JSON-serialized twice (`bridge/templates.rs:63-64`) |
| Issue #5: `add doi` crash when no session library is set | ✅ Fixed before the pin; ported | `session::session_library_id(&session, 1)` |
| Issue #1: `item find --scope` | ✅ Present | |
| Issue #6 (MCP wrapper), issue #7 (batch mode) | Open feature requests, not implemented upstream | Not parity items |

Both PRs come from third-party contributors, and neither has a maintainer response. The upstream
maintainer has been silent since 2026-07-28, so nothing guarantees either PR will merge.

## 4. Findings that need action

Ranked by severity.

### 4.1 HIGH: `search list` and `search get` fail on every Zotero 10 library

`db.rs:859` selects the `required` column, which Zotero 10 dropped. The query fails when the
statement is prepared, so it fails even when the library has no saved searches. Both commands read
only through SQLite (`catalog.rs:430,444`), so there is no fallback path. `ZOTERO-COMPATIBILITY.md`
marks this cell DOC-VERIFIED, not LIVE VERIFIED, which is why the break went unnoticed.

**Fix:** probe `PRAGMA table_info(savedSearchConditions)` and select `NULL AS required` when the
column is absent. This matches PR #9's behaviour, keeps the JSON shape the goldens expect, and keeps
the port aligned with upstream if PR #9 merges. Add a `db.rs` unit test that uses a
Zotero-10-shaped table.

### 4.2 HIGH (data safety, needs a decision): delete verbs erase permanently

`item delete --confirm` and `collection delete --confirm` remove data unrecoverably on every
backend. This matches upstream v1.2.1 exactly. The `--confirm` help text frames the flag as a safety
guard, when it is actually the flag that authorizes the permanent erase. PR #10 changes upstream to
trash by default, puts erasure behind `--permanent --yes-erase`, and adds `item restore` and
`collection restore`.

Following PR #10 changes a public contract, so it is a user decision, not an audit fix. If adopted,
the Local API path can likely trash by sending a `PATCH` with `deleted: 1` instead of a `DELETE`.
That still needs verification against Zotero 10.

### 4.3 MEDIUM: `item duplicates --by zotero` silently reports a clean library

Upstream's JS calls `dup.getSetItemsByItemID()` with no argument. Zotero's implementation
(`xpcom/duplicates.js:89`) requires an `itemID`, so the call returns `[undefined]`. `Object.keys`
then yields `"0"`, and `.filter(Boolean)` drops it, so the result is always `count: 0` with exit 0.

PR #10 misdiagnoses the cause as `_findDuplicates()` "resolving to undefined". That method returns
nothing by design; it populates `this._sets`. The minimal fix keeps Zotero's native detector:

```js
var dup = new Zotero.Duplicates(P.libraryID);
var ids = await (await dup.getSearchObject()).search();          // public API
var items = ids.map(id => Zotero.Items.get(id))
               .filter(i => i && i.isRegularItem());
// group with dup.getSetItemsByItemID(item.id) per item
```

`getSearchObject()` creates a temporary table that the UI normally drops on unload. Calling
`_findDuplicates()` and then `dup._sets.findAll(true)` avoids that table but keeps the private
dependency. Both the port and upstream also hard-code library 1 for `--by zotero`, while
`--by doi` and `--by title` honour the session library (`lib.rs:827`).

### 4.4 MEDIUM: the version cap will silently disable the Bridge after Zotero 10.0.x

The manifest and `update.json` cap the plugin at `10.0.*`. PR #9 uses `9999.*`, and issue #11 asks
for `10.*`. When Zotero ships 10.1 or 11, the XPI will be marked `appDisabled`. `app doctor` will
then report `installed_not_loaded` and advise "Restart Zotero", which is the endless loop PR #9
fixes.

**Fix:**
- Read `appDisabled` for `cli-bridge@cli-anything-rust.dev` from `<profile>/extensions.json`.
- Add a sixth Bridge state, `app_disabled`, whose next step is to upgrade the CLI or reinstall the
  XPI.
- Decide the cap policy (see §6).

The existing `update_url` gives the fork a remote lever: republishing `update.json` with a wider
`strict_max_version` for the same version may apply as a compatibility update without a reinstall.
That is standard Gecko behaviour but has not been verified in Zotero.

### 4.5 LOW: coexistence with the upstream plugin once PR #9 merges

Both plugins register the same `Zotero.Server.Endpoints["/cli-bridge/eval"]` key. The fork's client
safely refuses a foreign endpoint, because the `__PING__` response must carry `fork` and `id`. Two
problems remain:

- `app doctor` advises reinstalling this fork's plugin, when the real fix is disabling
  `cli-bridge@cli-anything.dev`.
- Upstream's `shutdown()` unconditionally deletes the shared key, so disabling or updating the
  upstream plugin removes this fork's endpoint until Zotero restarts.

**Fix:** detect the upstream addon id in `extensions.json` and say so in `doctor` and in
`MIGRATION.md`.

### 4.6 LOW: documentation and harness drift

- `compatibility-matrix.md` row 6 and approved break #3 describe `app check-update` as existing
  with "No network poll; always reports current". The binary has no such command, which
  `AGENTS.md:203` and `MIGRATION.md` state correctly.
- `ZOTERO-COMPATIBILITY.md` should move `search list/get` from DOC-VERIFIED to a known break until
  finding 4.1 is fixed. After the fix, the cell can become LIVE VERIFIED.
- `harness/compare.py` should accept `Changed` rows and the documented fork-transport rows as
  expected, so a clean run exits 0.

## 5. Next steps

1. Fix finding 4.1 (`search list/get`) and verify it live on Zotero 10.0.3. This is the only
   finding that breaks a documented, supported command today. Ship it as v1.0.1.
2. Add `appDisabled` detection and choose the `strict_max_version` policy (finding 4.4) before
   Zotero moves past 10.0.x.
3. Replace `find_duplicates.js` with the public-API version (finding 4.3), return a non-zero exit
   or an error when the scan fails, and pass the session library through.
4. Decide on PR #10's trash semantics (finding 4.2). If adopted, add `--permanent`,
   `--yes-erase`, `item restore`, and `collection restore`, and flag the behaviour change in
   release notes.
5. Correct the docs and harness drift (finding 4.6), then add the upstream-plugin coexistence
   detection (finding 4.5).
6. Watch upstream PRs #9 and #10. If either merges, re-diff the JSON contract, for example PR #9's
   `plugin.app_disabled` field and PR #10's `groups`, `scanned`, and `citation_key_conflict`
   fields.

## 6. Unresolved questions

- Should the port follow PR #10's trash-by-default semantics now, or wait until upstream merges it?
  It changes a public contract, so this is your decision.
- Which version-cap policy should the manifest use: a tight `10.0.*` refreshed through
  `update.json`, `10.*`, or upstream's `9999.*`?
- Does Zotero apply same-version compatibility updates from `update.json`? This has not been
  verified.
- Does Zotero 10's Local API accept `PATCH {deleted: 1}` as a trash operation? This has not been
  verified.
