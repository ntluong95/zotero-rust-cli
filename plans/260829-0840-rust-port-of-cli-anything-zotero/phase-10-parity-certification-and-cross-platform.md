---
phase: 10
title: "Parity Certification and Cross-Platform Hardening"
status: waiting-for-v1-tail
priority: P1
effort: "5-7d"
dependencies: [7, 8, 9, "post-phase-7-tail"]
---

# Phase 10: Parity Certification and Cross-Platform Hardening

## Overview

Drive the full compatibility matrix to green on macOS, Windows and Linux, certify final parity, and
produce a signed parity report (`plans/reports/PARITY-REPORT.md`) that Phase 13 (Python retirement) can be decided against.

**Precondition**: This certification gate runs after the post-Phase-7 remaining v1 command implementation tail (Rendering/Export, Local App/Audit, Selection/Collection, Analysis/Hygiene) is completed.

The harness has been running since Phase 1; this phase is about **certification and the long tail**,
not first contact.

## Requirements

**Functional**
- All 34 Exact-class commands byte-identical (normalized) on all three OSes where fixture coverage exists
- All 52 Semantic-class commands schema- and exit-code-identical, with evidence class separated into fixture, live, manual, or accepted divergence
- Both Changed behaviours implemented and documented (bare invocation; `item move-to-collection`)
- Live validation against a real Zotero for `live-only` commands
- Performance regression suite

**Non-functional**
- Parity report is machine-generated and reproducible
- CI runs the harness on every push across all three OSes

## Architecture

### Test taxonomy

| Layer | Scope | Where |
|---|---|---|
| Unit | Pure functions: `note_html`, `attach_path`, `csl`, `vectors`, `result` | `#[cfg(test)]` |
| Integration | Module against fixtures | `tests/*.rs` |
| CLI golden/snapshot | stdout + stderr + exit code per command | Phase 1 harness |
| JSON contract | Schema assertions on the result envelope and every payload shape | `tests/contracts.rs` |
| SQLite | Read layer against fixture and real DBs | `tests/db_read.rs` |
| Local API | Both enabled and disabled states | Fake server + live |
| Connector | Full call-sequence assertions | Fake server |
| JS Bridge | Template rendering + injection suite + live eval | Fake server + live |
| Import/export | Partial-success matrix | `tests/imports_partial.rs` |
| Attachment | Six path-resolution branches × 3 OSes | `tests/attach_path.rs` |
| Error path | Every `CliError` variant → payload, stream, exit code | `tests/errors.rs` |
| Cross-platform path | UNC, drive letters, `file://`, spaces, Unicode, long paths | `tests/paths_platform.rs` |
| Performance | Cold start, SQLite, cosine | `benches/` |

### Cross-platform focus areas

These are where a port of this shape actually breaks:

| Area | Specific risk |
|---|---|
| Windows UNC | `\\server\share\file.pdf` from `file://server/share/...` |
| Windows drive letters | `file:///C:/Users/...` → `C:\Users\...` |
| Path separators in JSON | Python emits `str(Path)` — backslashes on Windows. **Match exactly**; do not normalize to `/` |
| Console encoding | cp1252 console + CJK titles → `backslashreplace` fallback |
| `~` expansion | `%USERPROFILE%` vs `$HOME` |
| Config dir | `~/.config/cli-anything-zotero` on Windows too (not `%APPDATA%`) |
| File locking | `flock` absent on Windows; must degrade silently |
| Line endings | JSON output must use `\n`, not `\r\n`, on Windows |
| Long paths | Windows `MAX_PATH` with deep `storage/` trees |

> The path-separator rule is easy to get wrong in the "helpful" direction. Python's JSON contains
> `"C:\\Users\\x\\Zotero"` on Windows. Emitting forward slashes would be cleaner and would break
> every agent that string-matches paths.

### Certification evidence classes

Do not collapse fixture coverage, live coverage, and manual smoke tests into one "green" claim.
`PARITY-REPORT.md` must record one evidence class per command:

| Evidence class | Meaning |
|---|---|
| `fixture` | Fully reproducible offline comparison |
| `live-read` | Compared against a detected real Zotero without writes |
| `live-write` | Compared against an explicit scratch collection |
| `manual` | Requires Word/LibreOffice/Zotero UI verification |
| `accepted-divergence` | Not equivalent; documented and accepted |

Retirement can only use claims backed by the evidence class required for that command.

### Live validation

Some commands cannot run against fixtures. Provide an opt-in live suite, gated exactly like
upstream's:

```
CLI_ANYTHING_ZOTERO_ENABLE_WRITE_E2E=1
CLI_ANYTHING_ZOTERO_IMPORT_TARGET=<collection-key>
```

Read-only live tests run whenever a real Zotero is detected. Write tests remain opt-in. Run both
implementations against the **same** live library, sequentially, and compare.

### Zotero-version-specific re-baselining (saved searches)

> **[RECONSTRUCTED 2026-08-29 — not a verbatim recovery.]** Rebuilt from `plan.md`'s Zotero 10 impact
> table, Finding 8 ("FTS5 rewrite; `fulltextWords`/`fulltextItemWords` dropped; saved searches
> auto-migrate" → "MEDIUM → No landed code reads those tables (verified). Re-baseline `search
> list/get` per Zotero version in P10"), recovered verbatim from session context. The original diff
> for this file (+23 lines per the pre-loss `git diff --stat`) is not recoverable.

Zotero 10 rewrites full-text search storage (FTS5) and drops the `fulltextWords`/`fulltextItemWords`
tables outright — verified during the Zotero 10 impact assessment that no landed Rust code reads
either table, so this alone is not a regression. Separately, Zotero 10 auto-migrates saved searches
on database upgrade (e.g. `childNote` condition type becomes `note` + a `resultLevel` condition, per
Open Question 10). Because `search list`/`search get` serialize a saved search's `conditions` array
directly from `savedSearchConditions` (Phase 4's `fetch_saved_searches`), a library that has gone
through Zotero 10's migration can legitimately produce a **different, correct** JSON shape than the
same library captured against the Python reference on Zotero ≤9 — this is not a Rust bug, but the
existing golden fixtures were captured pre-migration and would falsely read as a Mismatch.

Certification must therefore re-baseline `search list`/`search get` goldens **per Zotero version**
(a Zotero ≤9 golden and a separate Zotero 10-migrated golden), and record which evidence class
(`fixture`/`live-read`/etc., per the taxonomy above) backs each. Do not silently merge the two into
one golden or treat the Zotero 10 shape as a divergence to "fix" — Open Question 10 in `plan.md`
must be answered against a real migrated library before this can be finalized, and this phase should
capture that answer as part of the drift check below rather than treating it as a separate task.

### Upstream drift check

Upstream was active as recently as one month before this analysis. Before certifying:

1. `git fetch` the reference checkout and diff against pinned `e42a930e`
2. For each upstream change, classify: irrelevant / port-it-now / record-as-known-divergence
3. Update `compatibility-matrix.md`
4. Record the new pinned compatibility target in the parity report

## Related Code Files

- Create: `tests/contracts.rs`, `errors.rs`, `paths_platform.rs`
- Create: `benches/startup.rs`, `benches/sqlite.rs`, `benches/cosine.rs`
- Create: `harness/report.py` → `PARITY-REPORT.md`
- Modify: `.github/workflows/ci.yml` (harness on all three OSes)
- Modify: `plans/reports/compatibility-matrix.md` (actual vs expected class)

## Implementation Steps

1. Wire the Phase 1 harness into CI for macOS, Windows and Linux.
2. Fill remaining test layers from the taxonomy above.
3. Build the cross-platform path test table and run it on all three runners.
4. Add the Windows console-encoding test.
5. Add the performance regression suite with thresholds: cold start < 10 ms; `item find` < 15 ms
   total; cosine < 10 ms at 5,754 × 768.
6. Run the live suite against a real Zotero on macOS and Windows.
7. Perform the upstream drift check.
8. Generate `PARITY-REPORT.md`: per-command expected class, actual class, evidence class, platform
   coverage, and any divergence with justification.
9. Triage every divergence into: fix now / accept and document / defer to Phase 12.

## Success Criteria

- [ ] 34/34 Exact-class commands pass on macOS, Windows and Linux for fixture-backed commands; any live-only Exact command is labeled separately
- [ ] 52/52 Semantic-class commands have schema/exit-code evidence with platform coverage explicitly labeled
- [ ] Both Changed behaviours verified and documented; `item move-to-collection` works with Zotero running
- [ ] Every divergence or live-only gap is either fixed or has a written, accepted justification in `PARITY-REPORT.md`
- [ ] Path separators in JSON match Python per-platform (backslashes on Windows)
- [ ] Config dir resolves to `~/.config/cli-anything-zotero` on Windows
- [ ] JSON output uses `\n` line endings on all platforms
- [ ] CJK output survives a cp1252 Windows console via the documented fallback
- [ ] Live read-only suite passes against a real Zotero on macOS and Windows
- [ ] Opt-in live write suite passes at least once against a scratch collection
- [ ] Performance thresholds met and enforced in CI
- [ ] Upstream drift reviewed; compatibility target re-pinned and recorded
- [ ] `PARITY-REPORT.md` generated and committed

## Risk Assessment

| Risk | Mitigation |
|---|---|
| Certification overstates what fixtures can prove | `PARITY-REPORT.md` carries evidence class and platform coverage per command |
| Long tail of small divergences consumes the schedule | Triage explicitly; "accept and document" is a valid outcome for cosmetic differences |
| Windows CI cannot run a real Zotero | Live tests are opt-in and run manually; fixtures cover the automated path |
| Upstream has diverged substantially | Drift check happens here, before certification, so it informs the retirement decision rather than surprising it |
| Performance thresholds flaky in shared CI | Use generous thresholds and median-of-N; the goal is catching regressions, not micro-benchmarking |
| Live write tests damage a real library | Require an explicit scratch-collection target; never default; never run in CI |
