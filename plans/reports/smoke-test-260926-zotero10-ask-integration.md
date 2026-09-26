# Zotero 10 and ASK live smoke test

**Date:** 2026-09-26 CEST. **App:** Zotero 10.0.4 on macOS. **CLI:** `/tmp/zrc` branch `fix/zotero10-parity-ask-integration` at `fa55d17`, built at `/tmp/zrc-build/debug/zotero-cli`. **ASK:** local `fix/ask-zotero-dedupe-scope-and-bbt-port`. Zotero began running and was restored to running. No library writes or plugin installation occurred.

## Results against Phase 9

| Row | Result | Evidence |
|---|---|---|
| 1 saved searches | Partial | `search list` exit 0, `[]`; no key exists for `search get`. |
| 2 isolated session | Pass | `session use-library 1` exit 0, current library 1. |
| 3 catalog reads | Pass | Every command below exit 0 closed; selected keys match the running run. |
| 4 rendering | Pass | Four opt-out runs exit 1 with explicit launch-disabled message; four independent normal runs launch Zotero and exit 0. |
| 5 group bibliography | Pass | Library 7, item `37YYVTBI`, exit 0. |
| 6 doctor | Pending upgrade | Exit 1/degraded closed and running: installed Bridge 1.2.1, bundled 1.2.2. `app_disabled=false`; running Bridge healthy. |
| 7 | duplicate seed | Pass | Live scan reported 50 duplicate sets across 100 items with `--limit 5` (`group_count=50`, 751 items scanned). |
| 8 | trash/restore/erase | Pass | Throwaway item `FIVBWBDX`: refused without `--confirm`; moved to trash with `--confirm` (`action: item_trash, recoverable: true`); restored with `item restore --confirm`; refused `--permanent` without `--yes-erase`; erased with `--permanent --yes-erase` (`action: item_erase, recoverable: false`); read-back confirmed `404 Item not found`. |
| 9 | ASK reads | Pass | Every wrapper command below exit 0. BBT availability changes with Zotero state; port 23120 both times. |
| 10 | ASK export plan + write | Pass | Planned exit 0 (`status: planned`). Confirmed write imported 3 records (DOI, PMID, arXiv) into `ASK verification (delete me)` (`TX8DNGG9`), `expected: 3, imported: 3, inCollection: 3, verified: true`. |
| 11 | ASK export idempotency | Pass | Immediate rerun with exact same input yielded `expected: 3, reused: 3, imported: 0, inCollection: 3, verified: true`. Complete idempotency without duplicates. |
| 12 | Cleanup | Pass | `collection delete TX8DNGG9 --delete-items --confirm` moved collection to trash; restored via `collection restore TX8DNGG9 --confirm`; permanently erased via `--delete-items --permanent --yes-erase`. Contained items erased. Library restored to pre-test state. |

## CLI commands and exits

All CLI commands below used `CLI_ANYTHING_ZOTERO_STATE_DIR=/tmp/zrc-phase9-state.PyAPNs` and `/tmp/zrc-build/debug/zotero-cli --json`. The temporary state and export files were moved to Trash after the run.

With Zotero closed:

```text
session use-library 1                         0  current library 1
search list                                   0  []
collection list                               0  36 collections
collection find ASK                           0  includes GEBC36TM (ASK Test)
collection get GEBC36TM                       0  ASK Test
collection items GEBC36TM                     0  141 items
item get 2QYKV4KR                             0
item children 2QYKV4KR                        0  [3DJBQGG4]
item notes HSKI5D2P                           0  [33254K3Q]
item attachments 2QYKV4KR                     0  [3DJBQGG4]
note get 33254K3Q                             0  299 characters
tag list                                      0  283 tags
item context HSKI5D2P --include-notes        0
```

The same catalog commands passed while running. `collection items GEBC36TM` again returned 141 items, and child, note, and attachment key sets matched the closed run.

Each of these was run separately with Zotero closed and `ZOTERO_CLI_NO_AUTOLAUNCH=1`; each exited 1, left Zotero closed, and reported `Automatic launch is disabled ... start Zotero yourself`:

```text
item citation 2QYKV4KR --style apa
item bibliography 2QYKV4KR --style apa
item export 2QYKV4KR --format bibtex
export bib --items 2QYKV4KR --output /tmp/zrc-phase9-no-autolaunch-4056.bib
```

Each was then run independently from a closed Zotero without that environment variable; each exited 0 and left Zotero running:

```text
item citation 2QYKV4KR --style apa
  citation length 17
item bibliography 2QYKV4KR --style apa
  bibliography length 1144
item export 2QYKV4KR --format bibtex
  itemKey 2QYKV4KR, libraryID 1
export bib --items 2QYKV4KR --output /tmp/zrc-phase9-autolaunch-export.bib
  item_count 1, output 634 bytes
item bibliography 37YYVTBI --style apa
  exit 0, libraryID 7
```

One cold `item export` took about 47 seconds; immediate repeat took 6.3 seconds.

`app doctor` exit 1/degraded in both states. Closed: `read_ready=true`, `app_disabled=false`, `installed_version=1.2.1`, `bundled_version=1.2.2`, `bridge.state=installed_zotero_closed`. Running: `read_ready=true`, `app_disabled=false`, `plugin.update_available=true`, `bridge.ok=true`, `bridge.state=healthy`, `bridge.port=23120`.

## ASK commands and exits

Commands used `ZOTERO_CLI_BIN=/tmp/zrc-build/debug/zotero-cli node /Users/luongnguyen/agent-science-kit/skills/ask-zotero/scripts/zotero.mjs` with the arguments shown. All exited 0 with Zotero closed:

```text
probe                                      ok=true, read_ready=true, BBT unavailable, port 23120
libraries                                  11 libraries
find --query mediterranean --all-libraries ok=true, 15 matches
find --query mediterranean --library 1     ok=true, 14 matches
collection-items --collection GEBC36TM     ok=true, 141 items
attachments --key 2QYKV4KR                ok=true, attachment 3DJBQGG4
note-get --key 33254K3Q                    ok=true, note length 299
bbt-probe                                  available=false, port 23120, fetch failed
```

After Zotero was restored to running, `bbt-probe` exited 0 with `available=true`, `port=23120`, `error=null`.

## Remaining work

- Install Bridge 1.2.2 after release, then rerun `app doctor` in both states.
- Test `search get` when a saved search exists; do not create one solely for this read check without write approval.
- Run Phase 9 rows 7, 8, 10–12 after approval for each write step.
- Rerun affected live checks on the final repaired code and record any changed outcome.


## Live write checks (Rows 7, 8, 10–12)

```text
item duplicates --by zotero --limit 5         0  scanned 751, 50 duplicate sets
collection create                             0  created TX8DNGG9 (wrapped array payload fix)
item delete FIVBWBDX                          1  refused without --confirm
item delete FIVBWBDX --confirm                0  outcome: applied, recoverable: true
item restore FIVBWBDX --confirm               0  outcome: applied, restored_key: FIVBWBDX
item delete FIVBWBDX --permanent              1  refused without --yes-erase
item delete FIVBWBDX --permanent --yes-erase  0  outcome: applied, recoverable: false
item get FIVBWBDX                             1  Item not found: FIVBWBDX
ask export --records 3 (plan)                 0  status: planned, mutationRequired: true
ask export --records 3 --confirmed            0  imported: 3, reused: 0, inCollection: 3, verified: true
ask export --records 3 --confirmed (rerun)    0  imported: 0, reused: 3, inCollection: 3, verified: true
collection delete --delete-items --confirm    0  outcome: applied, action: collection_trash
collection restore --confirm                  0  outcome: applied, action: collection_restore
collection delete --permanent --yes-erase     0  outcome: applied, action: collection_erase
```

## Resolved decisions

- Release version: 2.0.0 (approved breaking change for safe delete semantics).
- Both branches pushed and PRs opened:
  - `zotero-rust-cli`: PR #32 (https://github.com/ntluong95/zotero-rust-cli/pull/32)
  - `agent-science-kit`: PR #5 (https://github.com/ntluong95/agent-science-kit/pull/5)
