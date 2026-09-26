---
title: Zotero 10 live reads made fast and delete semantics aligned with Zotero
date: 2026-09-26
summary: "Cached VACUUM INTO snapshot cut live reads from ~3 s to 0.14 s; collection trash/restore/erase now follow Zotero's source; ASK PMID dedupe no longer false-matches"
---

# Zotero 10 live reads made fast and delete semantics aligned with Zotero

## What happened

Review of PR #32 (zotero-rust-cli v2.0.0) and agent-science-kit PR #5 found the live read path too slow and several behaviours unverified against Zotero itself. Zotero 10.0.4's own source (`omni.ja`) was used as the reference.

- Live reads while Zotero runs took about 3 s per command. Each CLI process re-copied 21 catalog tables through paged JSON over the Bridge, and every `connect_readonly` call waited a 1 s SQLite busy timeout against Zotero's permanent exclusive lock (1–3 opens per command).
- Collection restore in Zotero (`ZoteroPane#restoreSelectedItems`) restores trashed descendant collections but never items. Collection trash (`save({deleteItems})`) cascades to every item in the subtree, not only direct children.
- `collection delete --delete-items --permanent --yes-erase` never touched the items (inherited from upstream). One test item from the earlier smoke run (`CWR2MUQX`) was left in the user's trash.
- The planned duplicates guard (`scanned == 0`) could never work, because `DisjointSetForest.findAll()` only returns items that were paired.
- Test fakes returned Bridge readbacks as objects, while the real Bridge returns `JSON.stringify` strings. This hid the readback bug found live, and also a latent `item merge` verification bug.
- ASK matched PMIDs by substring over the full item JSON, so a PMID could match an `itemID` or a longer PMID.

## Decision

- Live snapshot rebuilt. Zotero writes the copy with `VACUUM INTO` (as its own `vacuum()` does). The CLI caches it on disk under the state dir (`0700`/`0600`), keyed by Zotero connection token + `_commitCount` + `total_changes()`. WAL lock probe is now 100 ms, and a process skips re-probing once it has checked a copy. Live on a 25 MB library: 0.13–0.15 s per warm read, about 0.8 s on the first read after a change. Output is identical.
- Collection trash, restore and erase follow Zotero exactly (user decision). `collection restore --with-items` was removed. Erase with `--delete-items` now erases the subtree's items.
- The duplicates guard now checks Zotero internals (`_findDuplicates`, `getSetItemsByItemID`, a populated `_sets`).
- Readback fakes now return strings, via a new `ScriptedResponse::bridge_json`. Reverting either live fix now fails tests.
- `--include-feeds --scope fields` searches feeds by title instead of silently downgrading every library.
- ASK: PMID and arXiv are matched as whole identifiers in string values, verification stops after two matches, and an unverifiable candidate fails the record.

## Next steps

- Commit and push both branches (not yet authorized).
- With approval, run one throwaway live write sequence to prove cache invalidation after a write and to exercise the new collection trash/erase templates.
- Install Bridge 1.2.2 and rerun `app doctor` (Phase 9 row 6).
- User decides whether to empty or erase `CWR2MUQX` from the trash.
- Reinstall the ASK skill (`ask install`) after merging PR #5.

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.
