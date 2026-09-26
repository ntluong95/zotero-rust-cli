---
phase: 9
title: "Live end-to-end verification"
status: in-progress
priority: P1
effort: "3h"
dependencies: [7, 8]
---

# Phase 9: Live end-to-end verification

## Goal

Prove on this Mac, running Zotero 10.0.4 with the Bridge installed, that every finding is fixed.
Check both the CLI directly and ASK's workflows.

## Preconditions

- The released CLI is installed through `scripts/install.sh`, and the updated XPI is installed via
  Tools → Plugins.
- The ASK wrapper from Phase 8 is installed.
- **Write steps:** the user approves every write step before it runs. All writes target a
  throwaway collection named `ASK verification (delete me)` in library 1.

## Matrix

Run each row with Zotero **closed** and with Zotero **running**. Expected result: exit 0 in both
states, unless noted.

| # | Check | Expected |
|---|---|---|
| 1 | `search list`, `search get <key>` | exit 0; `required` key present |
| 2 | `session use-library 1` (isolated `CLI_ANYTHING_ZOTERO_STATE_DIR`) | exit 0 |
| 3 | `collection list/find/get/items`, `item get/children/notes/attachments`, `note get`, `tag list`, `item context` | exit 0; the key sets match between the closed and running runs |
| 4 | `item citation/bibliography/export`, `export bib --output` | exit 0. With Zotero closed, the command auto-launches Zotero. With `ZOTERO_CLI_NO_AUTOLAUNCH=1`, it exits 1 with a clear message. |
| 5 | Group-library `item bibliography` | exit 0 |
| 6 | `app doctor` | `healthy`; `checks.plugin.app_disabled == false` |
| 7 | `item duplicates --by zotero` on a seeded duplicate pair (write) | one group reported |
| 8 | `item delete --confirm` → `item restore --confirm` → `--permanent --yes-erase` on a throwaway item (write) | trashed → restored → gone |
| 9 | ASK `probe`, `libraries`, `find --all-libraries`, `find --library 1`, `collection-items`, `attachments`, `note-get`, `bbt-probe` | `ok: true`; BBT on port 23120 |
| 10 | ASK `export` plan, then `--confirmed` with 3 records (DOI, PMID, arXiv) into the throwaway collection (write) | `status: complete`, `verified: true` |
| 11 | Rerun row 10 unchanged (write) | `imported: 0`, `reused: 3`; no new items or collections |
| 12 | Clean up: trash the throwaway items and collection (write) | back to the pre-test state |

## Deliverable

Write `plans/reports/smoke-test-<date>-zotero10-ask-integration.md` with every command, exit code,
and relevant output excerpt. Update `docs/ZOTERO-COMPATIBILITY.md` tags from DOC-VERIFIED to
LIVE VERIFIED where this run proves them.

## Verification

Every row passes. Any failure reopens the owning phase. Do not patch around it in this phase.
