---
phase: 13
title: "Python Retirement"
status: todo
priority: P2
effort: "2-3d"
dependencies: [11]
---

# Phase 13: Python Retirement

## Overview

Decommission the Python implementation — but only against explicit, measurable criteria, and only
after a coexistence period during which both implementations are installable side by side.

Retirement is a **decision**, not a deletion task. This phase defines the decision, executes the
communication, and only then removes anything.

## Retirement criteria

Every criterion must be met. Any unmet criterion blocks retirement.

| # | Criterion | Evidence |
|---|---|---|
| 1 | All 34 Exact-class commands pass on macOS, Windows and Linux | `PARITY-REPORT.md` |
| 2 | All 52 Semantic-class commands pass on all three, and both Changed behaviours are documented | `PARITY-REPORT.md` |
| 3 | Every divergence is fixed or has a written accepted justification | `PARITY-REPORT.md` |
| 4 | Prebuilt binaries install and run on all five targets with no Python, pip, Rust or Cargo | Phase 2 validation, re-run |
| 5 | Homebrew and Scoop channels working | Manual verification |
| 6 | `SKILL.md` regenerated and validated by a real agent session | Phase 11 |
| 7 | Phase 12 either complete in Rust, or its 7 commands formally deprecated/excluded from supported Rust end-user workflows with users notified | Gate decision recorded |
| 8 | Apache-2.0 obligations discharged | Phase 11 checklist |
| 9 | Coexistence period elapsed with no unresolved blocking issue | Issue tracker |
| 10 | Migration guide published and linked from the README | Phase 11 |

> Criterion 7 is satisfiable **either** by completing Phase 12 **or** by deprecating those commands.
> Phase 12 is not on the critical path.

## Coexistence design

The two implementations must be installable simultaneously throughout the migration.

| Concern | Approach |
|---|---|
| Binary name collision | Rust binary installed as `zotero-cli`; Python remains available as `cli-anything-zotero`, or is invoked as `python -m cli_anything.zotero`. Resolve per plan Open Question 1. |
| Session state | Both read `~/.config/cli-anything-zotero/session.json` in the same format — deliberately shared, so a user can switch mid-session |
| Vector database | Shared format (Phase 8); do not run `build-index` concurrently from both |
| PDF resume state | Shared format (Phase 7); a Python-started batch resumes under Rust |
| Zotero XPI | **Only one plugin should be active.** If the addon id changed (Phase 6), both can be installed — `plugin-status` must verify endpoint ownership over HTTP or require uninstalling the upstream plugin |
| Audit log | Same JSONL file and format; entries interleave harmlessly |

The shared on-disk formats are what make coexistence real rather than nominal. They are specified in
Phases 5, 7 and 8 and must be verified here end-to-end.

## Retirement sequence

1. **Announce.** Publish the migration guide; mark the Python package as maintenance-only.
2. **Coexist.** Recommend Rust as the default; keep Python installable. Collect issues.
3. **Evaluate.** Re-check all ten criteria. Any failure returns to the owning phase.
4. **Decide.** Record the decision, evidence, and any accepted divergences.
5. **Deprecate.** Mark the Python package deprecated with a pointer to the Rust CLI. Do not delete.
6. **Archive.** Move Python sources under `legacy/` or a tagged branch — preserved, not destroyed.

> Step 6 is deliberately archival. The Python implementation is the parity oracle: the Phase 1
> harness needs it to detect regressions. Deleting it would remove the ability to verify the port
> that replaced it.

## Requirements

**Functional**
- Documented, evidence-backed retirement decision
- Verified coexistence across all shared on-disk formats
- Python sources archived and still runnable for parity checks

**Non-functional**
- Every supported Rust end-user workflow works without Python; deprecated/legacy-only commands are labeled as outside that support promise

## Related Code Files

- Create: `docs/RETIREMENT-DECISION.md`
- Modify: `README.md` (Rust as default, Python status)
- Modify: `docs/MIGRATION.md` (final state)
- Modify: `harness/README.md` (how to keep running parity against archived Python)
- Move: `reference/cli-anything-zotero/` → retained as the parity oracle

## Implementation Steps

1. Verify coexistence end-to-end: run a workflow that starts under Python and finishes under Rust,
   exercising shared session state, resume state and the vector DB.
2. Verify `plugin-status` correctly reports plugin state and endpoint ownership when both the
   upstream and forked XPI are installed, or verify the installer blocks that state.
3. Publish the migration guide and announce maintenance-only status.
4. Run the coexistence period; triage incoming issues against the criteria table.
5. Re-run the full parity harness and the Phase 2 clean-machine install validation.
6. Evaluate all ten criteria; write `docs/RETIREMENT-DECISION.md` with evidence per criterion.
7. If all pass: deprecate the Python package, update the README, archive the sources.
8. If any fail: record which, return to the owning phase, and re-evaluate later.
9. Ensure the harness still runs against the archived Python so future Rust changes remain verifiable.

## Success Criteria

- [ ] All ten retirement criteria evaluated with evidence recorded in `docs/RETIREMENT-DECISION.md`
- [ ] Coexistence verified: a workflow spanning both implementations completes correctly
- [ ] Shared session, resume and vector formats confirmed interoperable in both directions
- [ ] `plugin-status` unambiguous when both XPIs are installed, or installer enforces a single-plugin policy
- [ ] Migration guide published and linked from the README
- [ ] Every previously-working command has a documented path forward (ported in Rust, formally deprecated/excluded, or legacy-only outside the no-Python promise)
- [ ] Python sources archived and still executable for parity runs
- [ ] Parity harness continues to run post-retirement
- [ ] No Python, pip, Rust or Cargo required for any supported Rust end-user path

## Risk Assessment

| Risk | Mitigation |
|---|---|
| Retiring Python destroys the parity oracle | Archive, never delete; harness explicitly keeps running against it |
| Users on the 7 deferred DOCX commands are stranded | Criterion 7 requires either a port or an explicit deprecation with notice |
| Shared on-disk formats silently diverge | Verified end-to-end in step 1, in both directions |
| Both XPIs installed causing confusing behaviour | Endpoint ownership verification or single-plugin enforcement is an explicit criterion |
| Retirement declared on optimism rather than evidence | Ten criteria, each requiring a named evidence artifact |
| Upstream continues evolving after retirement | Accepted: the fork's compatibility target is pinned and recorded; divergence becomes intentional |
