---
phase: 1
title: "Behavioural Baseline and Parity Harness"
status: in-progress
priority: P1
effort: "3-4d"
dependencies: []
---

# Phase 1: Behavioural Baseline and Parity Harness

## Overview

Capture the exact observable behaviour of the Python CLI across all 96 commands **before any Rust
exists**, and build the tooling that will compare Python and Rust outputs continuously for the rest
of the port. No Rust code is written in this phase.

This is deliberately first. A parity harness built after the port would be written with knowledge of
the Rust implementation and would encode its bugs as expectations.

## Requirements

**Functional**
- Reproducible fixture environment (fake Zotero data dir, profile, SQLite DB, HTTP services) that
  works identically on macOS, Windows and Linux
- Golden output capture for every command that can run against fixtures
- Normalizer that strips non-deterministic fields
- Diff tool that reports Exact / Semantic / Mismatch per command
- Both Local-API-enabled and Local-API-disabled fixture states

**Non-functional**
- Harness must run without a real Zotero installation (CI-safe)
- Must be runnable against a real Zotero for opt-in live validation
- Fixture generation must be deterministic — same seed, same bytes

## Architecture

Reuse and extend the upstream test fixtures rather than inventing new ones.
`reference/cli-anything-zotero/cli_anything/zotero/tests/_helpers.py` already builds:

- a complete fake Zotero SQLite schema (`create_sample_environment`, lines 37–120+) with
  `libraries`, `items`, `itemData`, `collections`, `itemAttachments`, `itemNotes`,
  `itemAnnotations`, `savedSearches`, and a user + group library
- a fake `profiles.ini`, `prefs.js`, and `application.ini`
- a threaded `ThreadingHTTPServer` standing in for Connector and Local API
- `sample_pdf_bytes()` for attachment tests

```
harness/
  fixtures/
    build_fixture.py        # wraps _helpers.create_sample_environment, deterministic seed
    states/
      local-api-on/         # prefs.js with localAPI.enabled = true
      local-api-off/        # prefs.js with localAPI.enabled = false  (observed real-world state)
      empty-library/
      group-library/
      unicode-cjk/          # titles/tags with CJK, quotes, backslashes, newlines  → D1 regression
      wal-mode/             # journal_mode=WAL with a row left uncheckpointed in -wal → Zotero 10 regression, owned by Phase 14 [reconstructed 2026-08-29, see note below]
  capture.py                # run <impl> <command> --json  → golden/<state>/<command>.json
  normalize.py              # strip non-deterministic fields
  compare.py                # classify Exact | Semantic | Mismatch
  commands.tsv              # the 96 commands + args + expected class (from compatibility-matrix.md)
  golden/                   # committed Python baseline outputs
```

### Normalization rules

Fields that legitimately vary and must be normalized before comparison:

| Field pattern | Rule |
|---|---|
| Absolute paths (`path`, `resolvedPath`, `state_path`, `plugin_path`, `db_path`, `xpi_path`, `*_dir`) | Replace fixture root with `<ROOT>`; normalize separators to `/` |
| `dateAdded`, `dateModified`, `clientDateModified`, `checked_at`, `lastSync` | Replace with `<TIMESTAMP>` |
| `pid` | `<PID>` |
| `backupPath` | `<BACKUP>` |
| Generated Zotero keys (8-char `[23456789A-Z]`) created during the run | `<KEY>` |
| `package_version` | `<VERSION>` |
| `zotero_version` | `<ZOTERO_VERSION>` |
| Float scores (`score`) | Round to 4 dp (matches Python `round(score, 4)`) |

Everything else — including **key order**, key names, types, nesting and null-vs-absent — is
compared strictly for Exact-class commands.

> Key order matters. Python `json.dumps` preserves dict insertion order; the Rust port uses
> `serde_json` with `preserve_order`. Capturing order now is what makes that verifiable later.

> **[RECONSTRUCTED 2026-08-29 — not a verbatim recovery.]** This `wal-mode` fixture-state row and the
> paragraph below were lost to an uncoordinated concurrent git operation before being committed, and
> are rebuilt here from cross-references still present in `plan.md` and
> `phase-14-zotero-10-compatibility-gate.md` (which cite it as "New `wal-mode` fixture (P1)"), not
> from the original file content. Treat the specifics (exact row insert, exact assertions) as
> Phase 14's responsibility to finalize against `phase-14`'s own fixture spec, not as settled history.

### `wal-mode` fixture state (owned by Phase 14)

Zotero 10 defaults `zotero.sqlite` to WAL journal mode. The three existing fixture states above are
all built with Python's `sqlite3.connect()`, which defaults to rollback-journal mode — no `-wal` file
is ever produced, so no existing fixture can exercise WAL-specific read behavior. `wal-mode` closes
that gap: it enables `PRAGMA journal_mode=WAL` on the fixture database and leaves at least one
committed row **uncheckpointed** in `-wal` (see `phase-14-zotero-10-compatibility-gate.md` §2 for the
mechanics and the "must demonstrably fail under the old `immutable=1` behavior" gate criterion). This
is a regression fixture, not a general-purpose one — it exists specifically to make Finding 15 (WAL
silently drops uncheckpointed rows under `immutable=1`) permanently testable, not just documented.

## Related Code Files

- Create: `harness/fixtures/build_fixture.py`
- Create: `harness/capture.py`, `harness/normalize.py`, `harness/compare.py`
- Create: `harness/commands.tsv`
- Create: `harness/README.md`
- Create: `harness/golden/**` (generated, committed)
- Read-only reference: `reference/cli-anything-zotero/cli_anything/zotero/tests/_helpers.py`
- Read-only reference: `plans/reports/compatibility-matrix.md`

## Implementation Steps

1. Create an isolated Python venv **outside the repo** and install the reference package in editable
   mode. This is the only place Python tooling is set up; it is never a user-facing requirement.
2. Write `build_fixture.py` wrapping `_helpers.create_sample_environment` with a fixed RNG seed.
   Extend it with the `unicode-cjk` state: item titles, tag names and collection names containing
   `\`, `'`, `"`, newline, `<script>`, and CJK — these are the D1 regression inputs.
3. Derive `commands.tsv` from `compatibility-matrix.md`: command path, representative arguments,
   expected compatibility class, required fixture state.
4. Write `capture.py`: sets `ZOTERO_DATA_DIR`, `ZOTERO_PROFILE_DIR`, `ZOTERO_HTTP_PORT`,
   `CLI_ANYTHING_ZOTERO_STATE_DIR` to fixture paths; runs the command; records **stdout, stderr and
   exit code separately**.
5. Write `normalize.py` implementing the table above.
6. Write `compare.py` producing a per-command verdict and a summary table.
7. Capture the Python baseline for all four fixture states. Commit `golden/`.
8. Record which commands **cannot** be exercised against fixtures (needing real Zotero, LibreOffice,
   or live network) and mark them `live-only` in `commands.tsv`.
9. Document the harness in `harness/README.md`, including how to run it against a real Zotero.

## Success Criteria

- [ ] Fixture environment builds byte-identically on macOS, Windows and Linux from a fixed seed
- [x] Four fixture states exist, including `local-api-off` and `unicode-cjk`
- [x] Golden outputs captured for every command not marked `live-only`
- [x] `compare.py` reports zero mismatches when comparing the Python baseline against itself
- [x] `compare.py` correctly detects an injected regression (deliberately mutate one golden file and confirm it fails)
- [x] stdout, stderr and exit code are captured and compared independently
- [x] `commands.tsv` accounts for all 96 commands with an explicit class or `live-only` marker
- [ ] Harness runs in CI without Zotero installed

## Local Verification

- `python3 -m py_compile harness/fixtures/build_fixture.py harness/normalize.py harness/capture.py harness/compare.py`
- `python3 harness/capture.py --impl python --output harness/golden/python --clean` captured 75 fixture-safe commands and skipped 21 live-only commands with zero harness failures.
- `python3 harness/compare.py harness/golden/python harness/golden/python` reported 75 Exact, 21 Skipped, 0 Semantic, 0 Mismatch, 0 Missing.
- A fresh repeat capture in `/tmp` compared as 75 Exact, 21 Skipped, 0 Semantic, 0 Mismatch, 0 Missing.
- A deliberately mutated `item get` capture returned nonzero and reported 1 Mismatch.

## Risk Assessment

| Risk | Mitigation |
|---|---|
| Golden outputs encode machine-specific paths | Normalizer developed alongside capture; verified by running capture on two different machines/roots |
| Fixture schema drifts from real Zotero schema | Fixtures come from upstream's own test helpers, which are validated against Zotero 7.0.32; add a documented note that fixture schema is a subset |
| Commands with side effects pollute fixtures | Rebuild the fixture from seed before every command capture; never reuse a mutated fixture |
| `live-only` set is large enough to leave real gaps | Enumerate explicitly and revisit in Phase 10 with opt-in live runs against a real library |
