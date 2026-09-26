---
phase: 6
title: "Trash-by-default delete + restore"
status: pending
priority: P2
effort: "1d"
dependencies: []
---

# Phase 6: Trash-by-default delete and restore

## Goal

Make the delete verbs recoverable by default, matching the Zotero UI and upstream PR #10. Permanent
erasure stays available behind a flag nobody types by reflex.

## Context

- **Today:** `item delete --confirm` and `collection delete --confirm` erase permanently. The Bridge
  templates call `eraseTx()` (`bridge/js/item_delete.js`, `collection_delete.js`), and the Zotero 10
  Local API `DELETE` also calls `eraseTx()` (`server_localAPI.js`, `deleteSingleObject`).
- **User decision:** trash by default. This is a breaking change.

## Target contract

| Invocation | Effect |
|---|---|
| `item delete KEY` | Preview only, with zero mutation. This is the existing behaviour without `--confirm`. |
| `item delete KEY --confirm` | Move to trash (`deleted = true`, then save). |
| `item delete KEY --permanent --yes-erase` | Permanent erase. |
| `item delete KEY --permanent` without `--yes-erase` | Refused with exit 1 and no prompt. The CLI is non-interactive, so it never blocks on stdin. |
| `item restore KEY [--confirm]` | Untrash, previewing without `--confirm`. |
| `collection delete` / `collection restore` | Same shape. `--delete-items` trashes the contained items instead of erasing them. |

`--confirm` never authorizes an erase.

## Files

- Modify: `crates/zotero-cli/src/cli.rs` to add `--permanent`, `--yes-erase`,
  `ItemCommands::Restore`, and `CollectionCommands::Restore`, and to update help text.
- Modify: `crates/zotero-cli/src/bridge/js/item_delete.js` and `collection_delete.js`: branch on
  `P.permanent`, using `item.deleted = true; await item.saveTx();` for trash.
- Create: `crates/zotero-cli/src/bridge/js/item_restore.js` and `collection_restore.js`.
- Modify: `crates/zotero-cli/src/bridge/templates.rs`, `lib.rs` (`item_delete_command` around
  line 1895, and collection delete around line 2528), and `write_router.rs`. For the Local API
  trash path, send `PATCH` with `{"deleted": 1}` if Phase 6 step 1 confirms it works. Otherwise use
  the Bridge.
- Modify: the delete verification in `write_router.rs`. After a trash, verify
  `data.deleted == 1` via `GET ...?includeTrashed=1` instead of a 404.
- Modify: `tests/write_router_integration.rs`, `write_backend_routing.rs`, and
  `write_output_denylist.rs`.
- Modify: `docs/AGENTS.md`, `docs/SECURITY.md`, and `docs/MIGRATION.md` to document the new
  contract and the behaviour change from upstream v1.2.1.

## Steps

1. **Spike (read-only first):** check in the Zotero source (`server_localAPI.js` PATCH handler and
   `Zotero.Item.fromJSON`) whether `PATCH {deleted: 1}` trashes an item and a collection.
   Live-verify it on one throwaway item only after approval. Record the result in this phase.
2. Implement the flags and refusal rules in the CLI. Add tests proving `--permanent --confirm`
   without `--yes-erase` never erases, and that no code path reads stdin.
3. Implement trash and restore on both backends, with audit-log entries using action names
   `item_trash`, `item_restore`, and `item_erase`.
4. Update the post-write verification for trash semantics.
5. Update the docs and the write-safety section of `README.md`.

## Verification

- `cargo test --workspace`. The new tests cover the refusal matrix and both backends through fakes.
- Live on Zotero 10.0.4, with approval, on a throwaway item in a scratch collection:
  1. delete with `--confirm`, then confirm the item is in the trash;
  2. `restore --confirm`, then confirm it is back;
  3. `--permanent --yes-erase`, then confirm it is gone.

## Risk

- **Scripts that relied on erase semantics change behaviour.** Mitigation: release notes, a
  `MIGRATION.md` entry, and an `"action": "item_trash"` field in the output so callers can detect
  the change.
- **Rollback:** revert this phase alone; nothing else depends on it.
