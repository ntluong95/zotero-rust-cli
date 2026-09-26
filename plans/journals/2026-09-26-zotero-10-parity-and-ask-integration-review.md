---
title: Zotero 10 parity and ASK integration review
date: 2026-09-26
summary: Two live reviews found Zotero 10 breaks in search and running-Zotero reads that block ASK; 9-phase fix plan created
---

# Zotero 10 parity and ASK integration review

## What happened

Reviewed zotero-rust-cli v1.0.0 against upstream cli-anything-zotero (still at the pinned `e42a930e`) and against agent-science-kit's `ask-zotero` wrapper, live on Zotero 10.0.3/10.0.4.

- `search list/get` fail on every Zotero 10 library: Zotero commit `5ade25f5` dropped `savedSearchConditions.required`, which `db.rs:859` still selects.
- With Zotero 10 running, most catalog reads (collections, item get/children/attachments, notes, tags, `session use-library`, rendering) refuse on the WAL lock. Layer B routing was specified but never wired. Only `item find` and `library list` have a Bridge fallback (`search.rs`).
- Citation, bibliography and export work in neither state: closed, they don't auto-launch; open, item resolution hits the lock.
- `item duplicates --by zotero` always returns 0 because `getSetItemsByItemID()` is called without an itemID. The bug came over unchanged from upstream.
- Delete verbs erase permanently; Zotero 10's Local API DELETE also calls `eraseTx()`.
- ASK wrapper dedupe calls `item find` without `--scope fields`, so DOI lookups never match.
- Parity harness: 47 Exact, 12 Semantic, 21 Mismatch; every mismatch is explained by design, but `compare.py` counts `Changed` rows as failures.

## Decision

User chose trash-by-default delete (following upstream PR #10), an XPI cap of `10.*` plus `appDisabled` detection in doctor, and ASK wrapper fixes as a separate phase in `~/agent-science-kit`.

## Next steps

Execute `plans/260926-1401-zotero10-parity-and-ask-integration-fixes`, starting with phases 1 → 2 → 3 (saved searches, live read fallback, rendering) to unblock ASK. Open question: release as 1.1.0 or 2.0.0.

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.
