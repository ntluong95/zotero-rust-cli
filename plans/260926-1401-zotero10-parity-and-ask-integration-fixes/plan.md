---
title: "Zotero 10 parity and ASK integration fixes"
description: "Fix every finding from the 2026-09-26 upstream-parity and ASK-integration reviews so zotero-cli works end to end on Zotero 10 and under agent-science-kit."
status: in-progress
priority: P1
effort: 5d
branch: main
tags: [bugfix, backend, zotero-10, ask]
blockedBy: []
blocks: []
created: 2026-09-26
---

# Zotero 10 parity and ASK integration fixes

## Overview

This plan fixes every finding in the two reports from 2026-09-26. When it is done, zotero-cli will
work end to end on Zotero 10 whether Zotero is open or closed, and agent-science-kit (ASK) will be
able to discover papers, read them, export bibliographies, and export to a collection idempotently.

Source reports:
- [research-260926-1538-upstream-parity-and-zotero-10-review.md](../reports/research-260926-1538-upstream-parity-and-zotero-10-review.md)
- [research-260926-1538-ask-zotero-integration-review.md](../reports/research-260926-1538-ask-zotero-integration-review.md)

**Repos:**
- `zotero-rust-cli`, this repo (root `crates/zotero-cli/`).
- `~/agent-science-kit`, only for Phase 8. Its `ask-portable/` directory is gitignored build
  output, so do not edit it.

## Decisions (user-approved 2026-09-26)

| Topic | Decision |
|---|---|
| Delete semantics | Trash by default, following upstream PR #10. `--confirm` moves to trash. Permanent erase requires `--permanent --yes-erase`. Adds `item restore` and `collection restore`. This is a breaking change and must be called out in the release notes. |
| XPI version cap | `strict_max_version: 10.*`, plus `appDisabled` detection in `app doctor`. |
| ASK wrapper | Fixed in a separate phase in `~/agent-science-kit`, after the CLI fixes ship. |

## Architecture: live read fallback (Phases 2 and 3)

This reuses the pattern `search.rs` already uses for `item find` and `library list`. SQLite is
always tried first. Only a real `DatabaseLocked` refusal triggers a retry through the owned Bridge.
Offline behaviour therefore stays byte-identical, and harness parity holds.

```text
read command
   │
   ├─ SQLite (db::connect_readonly) ── ok ───────────────► result (unchanged path)
   │
   └─ Err(DatabaseLocked)
        ├─ owned Bridge healthy ─► read-only JS template ─► same struct shape ─► result
        └─ no Bridge ───────────────────────────────────────► original refusal, verbatim
```

The Local API is not used as the fallback read backend. It cannot enumerate group libraries while
SQLite is locked, as `search.rs` explains in its header. Rendering commands keep using the Local API
for the rendered output itself; only their item and library resolution gains the fallback.

## Phases

| # | Phase | Priority | Effort | Depends on | Status |
|---|---|---|---|---|---|
| 1 | [Saved searches on Zotero 10](./phase-01-saved-search-zotero10-schema.md) | P1 | 2h | — | Complete |
| 2 | [Live read fallback for catalog reads](./phase-02-live-read-fallback.md) | P1 | 1.5d | — | Complete |
| 3 | [Rendering and export on Zotero 10](./phase-03-rendering-live-resolution.md) | P1 | 4h | 2 | Complete |
| 4 | [Bridge version cap, appDisabled, coexistence](./phase-04-bridge-compat-doctor.md) | P1 | 4h | — | Complete |
| 5 | [`item duplicates --by zotero` fix](./phase-05-duplicates-native-detector.md) | P2 | 4h | — | Complete |
| 6 | [Trash-by-default delete + restore](./phase-06-trash-by-default-delete.md) | P2 | 1d | — | Complete |
| 7 | [Docs, harness, and release](./phase-07-docs-harness-release.md) | P2 | 4h | 1–6 | Complete (v2.0.0, PR #32) |
| 8 | [ASK wrapper fixes](./phase-08-ask-wrapper-fixes.md) | P1 | 2h | 7 (released CLI) | Complete (PR #5) |
| 9 | [Live end-to-end verification](./phase-09-live-verification.md) | P1 | 3h | 7, 8 | In progress: row 6 needs Bridge 1.2.2 installed; post-review write checks LIVE VERIFIED |

Phases 1, 2, 4, 5 and 6 are independent and can be done in any order. To unblock ASK fastest, do
1 → 2 → 3 first.

## Finding coverage

| Finding (report § ) | Phase |
|---|---|
| Parity 4.1: `search list/get` fail on the dropped `required` column | 1 |
| Parity 4.2: delete verbs erase permanently | 6 |
| Parity 4.3: `duplicates --by zotero` always returns 0, and hard-codes library 1 | 5 |
| Parity 4.4: `10.0.*` cap, missing `appDisabled` check | 4 |
| Parity 4.5: coexistence with the upstream plugin | 4 |
| Parity 4.6: matrix `check-update` drift, compat doc, `compare.py` `Changed` handling | 7 |
| PR #10: DOI normalizer misses `doi:`, `doi.acm.org`, doubled prefixes | 5 |
| ASK 3.1: reads refuse while Zotero 10 runs; `session use-library` blocked | 2 |
| ASK 3.1: citation/bibliography/export fail open and closed | 3 |
| ASK 3.2: wrapper DOI lookup uses the wrong scope | 8 |
| ASK 3.3: wrapper `bbt-probe` checks the wrong port | 8 |
| ASK 3.4: ASK reference docs still say "RC2" | 8 |

## Success criteria

- [x] With Zotero 10 **running** and the owned Bridge healthy, every command in the ASK review
      matrix succeeds.
- [x] With Zotero 10 **closed**, every read still succeeds through SQLite, and rendering commands
      auto-launch Zotero (or fail with an explicit status when `ZOTERO_CLI_NO_AUTOLAUNCH=1`).
- [x] `search list/get` work on Zotero 10 and still emit a `required` key.
- [x] `app doctor` reports `app_disabled` and `upstream_plugin_conflict` with correct next steps.
- [x] `item duplicates --by zotero` finds a known duplicate pair and exits non-zero on scan failure.
- [x] `item delete --confirm` moves an item to the trash, and `item restore` brings it back.
- [x] `cargo fmt --check`, `cargo clippy -D warnings`, and `cargo test --workspace` pass.
      `harness/compare.py` exits 0.
- [x] Running ASK `export --confirmed` twice into a throwaway collection converges: the second run
      imports 0 records.

## Resolved questions

- Version: 2.0.0 (breaking change for safe delete semantics).
- Pushed and PRs opened:
  - zotero-rust-cli: PR #32
  - agent-science-kit: PR #5
