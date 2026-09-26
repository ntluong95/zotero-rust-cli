---
phase: 7
title: "Docs, harness, and release"
status: complete
priority: P2
effort: "4h"
dependencies: [1, 2, 3, 4, 5, 6]
---

# Phase 7: Docs, harness, and release

## Goal

Make the parity harness green on its own terms, remove the documentation drift, and ship the fixes
as a release that ASK can install.

## Files

- Modify: `harness/compare.py` (line 38). Treat the `Changed` class as expected when the capture
  matches the row's documented divergence, and report it as `Changed` instead of `Mismatch`.
- Modify: `harness/commands.tsv`. Reclassify the 16 rows whose writes and Bridge calls cannot run
  against upstream's mock server: `app enable-local-api`, the collection and item write rows,
  `collection stats`, the annotation and fulltext searches, `js`, `note add`, and `sync`. Mark
  each with a note pointing to the Rust test that covers it. `app version` becomes `Changed`,
  because fork versioning differs by design.
- Modify: `plans/reports/compatibility-matrix.md`, row 6 and approved break #3.
  `app check-update` does not exist, so change the text to "Dropped: not ported" and fix the
  `Changed` and `Dropped` counts.
- Modify: `docs/ZOTERO-COMPATIBILITY.md`:
  - `search list/get` becomes LIVE VERIFIED after Phase 1.
  - Layer B read routing is now implemented (Phase 2).
  - The cap is now `10.*` (Phase 4).
  - Add a `Library.groupID` note (Phase 3).
- Modify: `docs/AGENTS.md` for the delete/restore contract, running-Zotero read support, and the
  duplicates behaviour.
- Modify: `README.md` for the write-safety section and the "Live vs. offline" paragraph.
- Modify: `Cargo.toml`, bumping the workspace version per the release decision in `plan.md`.
- Create: a release-notes entry in the release body or `CHANGELOG` if the repo adds one. Lead with
  the trash-by-default behaviour change.

## Steps

1. Update the harness classifications and `compare.py`. Run the harness until it exits 0 with no
   unexplained rows.
2. Correct the docs listed above. Verify each claim against source or a live run.
3. Run the full CI set locally.
4. After user approval, tag and publish the release through the existing `release.yml`. Then
   confirm that `scripts/install.sh` installs the new version on this Mac.

## Verification

- `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace --release`
- `python3 harness/compare.py harness/golden/python harness/current/rust`: exit 0.
- After the release: `zotero-cli --version` prints the new version, and
  `zotero-cli --json app doctor` reports `healthy`.

## Risk

The main risk is reclassifying away a real regression. Mitigation: every reclassified row must name
the Rust test that covers the behaviour.
