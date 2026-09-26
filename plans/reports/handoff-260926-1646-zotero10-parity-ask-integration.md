# Handoff: Zotero 10 parity and ASK integration fixes

**Written:** 2026-09-26 16:48 CEST, at the end of a session cut short by credit limits.
**Plan:** [`plans/260926-1401-zotero10-parity-and-ask-integration-fixes/plan.md`](../260926-1401-zotero10-parity-and-ask-integration-fixes/plan.md)
**Source reports:**
- [`research-260926-1538-upstream-parity-and-zotero-10-review.md`](research-260926-1538-upstream-parity-and-zotero-10-review.md)
- [`research-260926-1538-ask-zotero-integration-review.md`](research-260926-1538-ask-zotero-integration-review.md)

## Outcome

The code work for Phases 1–8 is done, tested, and committed on local branches. Nothing has been
pushed, tagged, or released. The remaining work is:

1. Get the branch into this repo.
2. Decide the version number.
3. Release.
4. Run the write rows of the Phase 9 live verification, each after user approval.

## Where the work is

| Repo | Branch | State |
|---|---|---|
| `zotero-rust-cli` | `fix/zotero10-parity-ask-integration` (7 commits on top of `main` `f92a769`) | In the clean clone at `/tmp/zrc`, plus a verified bundle at `~/zotero-rust-cli-fix-zotero10-parity.bundle`. **Not yet in this OneDrive checkout** (see below). |
| `~/agent-science-kit` | `fix/ask-zotero-dedupe-scope-and-bbt-port` (2 commits on top of `main` `7cba4e9`) | Committed locally. The skill was reinstalled with `ask install -global -target claude-code -skills ask-zotero`, so `~/.claude/skills/ask-zotero` already runs the fix. |

**Why the branch is not in this checkout.** `git fetch` fails here with
`failed to read .git/worktrees/zotero-rust-cli-phase7-pdf-cascade/commondir: Operation timed out`.
The cause is OneDrive dehydrating a stale worktree's metadata. To fix it:

1. In Finder, set the folder to "Always Keep on This Device", or run `git worktree prune`.
2. Then import the branch:

```bash
git fetch ~/zotero-rust-cli-fix-zotero10-parity.bundle fix/zotero10-parity-ask-integration:fix/zotero10-parity-ask-integration
```

Work from `/tmp/zrc` in the meantime. It is a clean clone; build with
`CARGO_TARGET_DIR=/tmp/zrc-build`, because a local hook blocks any command path containing `target`.

## Commits (zotero-rust-cli)

| Commit | Phase | Change |
|---|---|---|
| `50f213d` | 1, 2 | `search list/get` work on Zotero 10 (the dropped `required` column is emitted as `null`). While a running Zotero holds its WAL lock, `db::connect_readonly` falls back to a read-only in-memory catalog copy read through the owned Bridge (`src/live_snapshot.rs`). |
| `39aa896` | 3 | The Local API addresses groups by **groupID**, not libraryID. A personal-library 404 no longer ends Local API target resolution. Rendering commands auto-launch Zotero. |
| `b232017` | 4 | XPI 1.2.2 with `strict_max_version: 10.*`. `app doctor` gains the `app_disabled` and `upstream_plugin_conflict` states. |
| `95111b7` | 5 | `item duplicates --by zotero` reports real duplicate sets and fails loudly on error. The DOI normalizer is shape-based. |
| `548372a` | 6 | **Breaking:** `delete --confirm` now moves items and collections to the trash. `--permanent --yes-erase` erases permanently. Adds `item restore` and `collection restore`. |
| `19d7bf7` | 7 | Docs and matrix corrected. The harness honours `Changed` rows, and `compare.py` exits 0. |
| `fa55d17` | 8 (found in 9) | `item find --all-libraries` now honours `--scope fields/everything`. |

## Evidence

- `cargo fmt --check`, `cargo clippy -D warnings`, and `cargo test --workspace` all pass. The
  parity harness reports 47 Exact, 12 Semantic, 21 Changed, 21 Skipped, and 0 Mismatch.
- Live checks against Zotero 10.0.4 (running):
  - Every read in the ASK review matrix succeeds.
  - The live snapshot equals a direct read across all 21 tables. Run it with
    `ZOTERO_LIVE_PORT=23120 ZOTERO_LIVE_SQLITE=~/Zotero/zotero.sqlite cargo test -p zotero-cli --test live_snapshot_zotero -- --ignored`.
  - Group-library citation, bibliography, and export work. v1.0.0 returned 404 on
    `/api/groups/7`.
  - `duplicates --by zotero` found 50 sets. v1.0.0 found 0.
  - ASK `find` by DOI works for one library and across all libraries.
  - ASK `bbt-probe` uses port 23120.
- ASK: `node skills/ask-zotero/scripts/zotero.test.mjs` passes 8 of 8. Against the old wrapper,
  5 of those tests fail.

## Next steps for the next agent

1. **Import the branch** as described above, and push it only when the user approves.
2. **Ask the user whether this is 1.1.0 or 2.0.0.** Trash-by-default is breaking. The plan's
   default is 1.1.0 with a release-notes callout. Bump `[workspace.package] version` in
   `Cargo.toml`, then tag and release through `release.yml` only after approval.
3. **Install the new XPI.** After the release, run `zotero-cli app install-plugin`, then install it
   in Zotero via Tools → Plugins → Install Add-on From File. The doctor currently reports
   "Upgrade CLI Bridge 1.2.1 → 1.2.2".
4. **Finish Phase 9** (`phase-09-live-verification.md`):
   - Remaining read rows: the closed-Zotero run, and `ZOTERO_CLI_NO_AUTOLAUNCH=1` for rendering.
   - Write rows 7, 8, and 10–12: seeded duplicates, trash → restore → erase, and running ASK
     `export --confirmed` twice into the collection `ASK verification (delete me)`.

   Each write needs explicit user approval. Then write the smoke-test report and flip the
   remaining DOC-VERIFIED tags in `docs/ZOTERO-COMPATIBILITY.md`.
5. **Open a PR for ASK** from `fix/ask-zotero-dedupe-scope-and-bbt-port`, with the user's approval.
6. Update the phase statuses in the plan: 1–8 done, 9 partial.

## Design notes for continuity

- **Why a snapshot, not per-command Bridge templates (Phase 2).** Zotero exposes query rows only
  by index. Copying the catalog tables into an in-memory SQLite database reuses every existing
  SQL query unchanged, so the output is byte-identical by construction.
  - Implementation details: rows are fetched in 3 MB pages because ureq limits bodies to 10 MB.
    Foreign keys are off because rusqlite's bundled build enables them. The copy is made
    read-only with `PRAGMA query_only`, because SQLite ignores the READ_ONLY flag on
    shared-cache memory databases.
  - `item find` and `library list` keep their single-query Bridge path through
    `live_snapshot::without_snapshot`.
- **Delete contract.** `--confirm` never erases, and `--permanent` without `--yes-erase` is refused
  before any request is made. The Local API trash path is `PATCH {"deleted": 1}`, verified from
  Zotero's `fromJSON` source; it has not been live-tested, which is Phase 9 row 8.

## Unresolved questions

- Should the release be 1.1.0 or 2.0.0?
- May the branch be pushed and the PRs opened for both repos?
