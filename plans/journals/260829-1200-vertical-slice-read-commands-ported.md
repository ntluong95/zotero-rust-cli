---
date: 2026-08-29 12:00
severity: medium
component: runtime foundation + 5 read-only commands (Phases 3-5 of Rust port)
status: in-progress
---

# Vertical-slice pivot: runtime foundation shipped, 5/96 commands ported and byte-verified

**Date**: 2026-08-29 12:00
**Severity**: Medium
**Component**: `crates/zotero-cli/src/{paths,session,http,db,catalog,runtime,output,error,cli,lib}.rs`, `plans/reports/compatibility-matrix.md`, phase-03/04/05 plan docs
**Status**: Ongoing (Phases 3-5 in-progress, 91 of 96 commands unported)

## What Happened

User directive at session start: accept Phase 2 (distribution spine) as done, defer Homebrew/Scoop/clean-machine verification, and abandon the plan's original phase-by-phase sequencing — stub all 96 commands first, then fill them in — in favor of a vertical slice: build the whole reusable runtime once, then port a small read-only command set completely (Python-parity-verified), before touching the other 91.

Scouted the Python reference (`reference/cli-anything-zotero/cli_anything/zotero/`) with an Explore subagent, then personally re-verified its findings line-by-line against source — exact SQL text, exact JSON field ordering, exact error-message strings — cross-checked against the golden fixtures already captured in `harness/golden/python/` from the Phase 1 session. Did not take the subagent's summary on faith.

Built the runtime as flat modules: `paths.rs` (install/profile/data discovery, porting `utils/zotero_paths.py`), `session.rs`, `http.rs` (`ureq`-based connector/Local-API client), `db.rs` (read-only SQLite via `rusqlite`, `mode=ro&immutable=1`, every query transcribed verbatim from `utils/zotero_sqlite.py`), `catalog.rs` (domain layer over db+http+session, porting `core/catalog.py`), `runtime.rs`, `output.rs` (matches Python's `emit()` exactly, including that human-mode list output is *deliberately not valid JSON*), `error.rs` (domain-error to exit-1), `cli.rs` (clap derive — `--json` as `global = true` replaces Python's bespoke `_JsonAwareGroup` propagation hack with one line), `lib.rs` (dispatch). Both binaries now call into this shared lib.

Ported 5 read-only commands end-to-end: `app status`, `item list`, `item get`, `item find` (dual-path Local-API-first, SQL-fallback search), `collection list`. Ran them through the existing parity harness (`harness/capture.py` + `compare.py`) against the same fixtures used for the Python golden captures: **5/5 Exact** — byte-identical stdout, exit codes, and recorded HTTP call sequences — on the first real attempt, confirmed with a manual `diff` on top of the harness's own verdict.

Then, because passing 5 golden fixtures each covering exactly one input/state combination does not prove the untested branches are correct, spawned two parallel background subagents before calling this done: a code-review pass (line-by-line diff against Python source, focused on branches the fixtures don't exercise) and a dynamic-testing pass (23 constructed scenarios: SQL-fallback paths, ambiguous-reference errors, numeric-ref edge cases including whitespace padding and i64 overflow, empty/group-library fixture states, real HTML note content). Both found real bugs:

- `is_numeric_ref` didn't trim whitespace before parsing — Python's `int(str(value))` does — so refs like `" 5 "` were misclassified as key lookups instead of numeric ones. Found by dynamic testing, fixed at all 3 call sites (`resolve_item`, `resolve_collection`, the tag filter in `fetch_items`).
- `connect_readonly` built the SQLite `file:` URI with native path separators, which is a broken URI on Windows (SQLite's URI spec requires `/`, not `\`). Every read command would have failed on Windows and nothing here would have caught it — dev and CI are Darwin-only. Found by the code-review agent by checking SQLite's own URI documentation, not by guessing.
- `find_items`' negative `--limit` used a clamped `.truncate()` instead of Python's `list[:limit]` negative-slice semantics.
- `session_library_id` — dead code today, but the exact function a future write-command phase will need — silently defaulted on a corrupted session value instead of erroring. A landmine: wired into a write path later, this would point the tool at the wrong Zotero library with zero error output.
- Minor: regex recompiled per note-conversion call instead of cached; `note_preview` recomputed `note_html_to_text`'s work instead of reusing it; an HTTP body-read failure silently defaulted to an empty string instead of propagating.

All fixed, 7 unit tests added as regression coverage, full parity suite re-run: still 5/5 Exact, no regression introduced by the fixes.

Two divergences from Python were identified and left as documented limitations, not silently "fixed": legacy semicolonless HTML entities (`&nbsp` without `;`) aren't decoded — Zotero's own note editor never emits them, and the `html-escape` crate doesn't implement that WHATWG legacy table anyway — and connector/Local-API error text can't be made byte-identical between Python's `urllib` and Rust's `ureq` when Zotero is completely unreachable. That second one is flagged as needing a product decision on harness coverage, not resolved unilaterally.

Updated `plans/reports/compatibility-matrix.md` with a live "Migration Progress" tracker, and updated `plan.md` plus `phase-03`/`phase-04`/`phase-05` docs to record the vertical-slice resequencing explicitly. Also resolved an apparent contradiction in phase-05's text — "runtime context built lazily" — against verified Python behavior: every command handler calls `current_runtime()` unconditionally, so both HTTP probes always fire per-command in practice. "Lazy" correctly means "not eagerly at process startup," not "skipped when unneeded." Real ambiguity in the original plan text, resolved by evidence, not escalated as a blocker.

Regenerated `THIRD-PARTY-LICENSES.md`/`about.toml` for the new runtime deps (clap, serde, serde_json, thiserror, anyhow, dirs, regex, html-escape), accepting one new license type: MPL-2.0 via `option-ext`/`dirs-sys`, file-level weak copyleft, safe for an unmodified dependency.

Committed as `1749f08` (implementation) and `88c8273` (docs/matrix/license sync), got explicit user confirmation before pushing since the repo is public, pushed. CI run `33246731188` was still `in_progress` at the time of writing — not yet confirmed green, though the same 5-target matrix + license-bundle generation already proved reliable in the Phase 2 session.

## The Brutal Truth

The genuinely satisfying part of this session is that the discipline paid off measurably: the parity harness passed 5/5 on the first attempt, which could easily have been mistaken for "done." It wasn't. Two of the bugs the review/dynamic-testing passes caught — the whitespace-trim bug and the Windows URI bug — are exactly the kind of thing that ships silently in an "AI wrote code, tests passed, merge it" workflow, because the existing golden fixtures each exercise exactly one code path per command. A green harness run proves the happy path is byte-identical; it says nothing about the fallback search path, the error path, or platforms nobody's laptop can reproduce. Spawning a second pair of eyes specifically to hunt for what the fixtures don't cover is not overhead — it is the only thing standing between "looks done" and "is done." That's satisfying to confirm in practice, not just to say in principle.

The uncomfortable part: the Windows bug is *fixed but unverified*. It was corrected by reading SQLite's URI spec, not by running on an actual Windows machine, because there is no Windows CI leg exercising `db.rs` yet. That's a confidence gap I'm choosing to accept and document rather than pretend is closed — it would be dishonest to write "fixed" without also writing "never actually run on the platform it fixes." `session_library_id` is the other loose thread worth sitting with: it's dead code right now, no command calls it, so the "silently defaults on corrupted session data" bug has zero live impact today. But it's a landmine armed and waiting for whoever wires in the first write command, and if that person doesn't re-read this file first, the fix might get "refactored away" as unreachable code before it's ever exercised.

## Technical Details

- Parity result: `for cmd in "app status" "collection list" "item find" "item get" "item list"; do harness/capture.py --impl <rust binary> --command "$cmd"; done` then `harness/compare.py` → 5/5 Exact against `harness/golden/python/*.json` (byte-identical stdout, exit codes, HTTP call sequences).
- Bug: `is_numeric_ref` on `" 5 "` classified as a key lookup instead of numeric — Python's `int(str(value))` strips whitespace; the Rust equivalent didn't. Fixed at 3 call sites: `resolve_item`, `resolve_collection`, the tag filter in `fetch_items`.
- Bug: SQLite connection URI built with native path separators on Windows — SQLite's `file:` URI spec requires `/`; would break every read command on that OS, invisible on Darwin-only CI.
- Bug: `find_items` negative `--limit` used `.truncate()` (clamped) instead of matching Python's `list[:limit]` negative-slice semantics.
- Bug: `session_library_id` defaulted silently on corrupted session state instead of returning an error — dead code today, load-bearing once a write command calls it.
- 7 unit tests added covering `is_numeric_ref`, `note_html_to_text`/`note_preview`, and the negative-limit slice helper.
- New deps this session: `clap`, `serde`, `serde_json`, `thiserror`, `anyhow`, `dirs`, `regex`, `html-escape`; new accepted license: MPL-2.0 (`option-ext` via `dirs-sys`).
- Commits: `1749f08` (runtime + 5 commands), `88c8273` (plan/matrix/license sync). CI run `33246731188` in progress at write time, not yet confirmed green.

## What We Tried

- **Trusting the Explore subagent's summary of the Python reference as-is** — rejected; re-verified every claim (SQL text, JSON field order, error strings) against source and the existing golden fixtures before writing a line of Rust against it.
- **Calling the command done after 5/5 Exact on the parity harness** — rejected. The fixtures each cover one input/state combination per command; a second review pass and a dynamic-testing pass specifically targeting untested branches were run before accepting the slice as complete, and both found real bugs the harness could not have caught.
- **Silently resolving the phase-05 "lazy runtime" wording and the vertical-slice resequencing without a trace** — rejected. Both were genuine deviations from/ambiguities in the written plan; both are now documented explicitly in the plan docs rather than left implicit.

## Root Cause Analysis

The Windows URI bug and the whitespace-trim bug share the same root cause: porting behavior from a dynamically-typed, exception-driven language (Python) into Rust exposes every implicit coercion and platform assumption Python quietly handled for free. `int(str(value))` trims whitespace as a side effect of CPython's number parsing; Rust's `str::parse::<i64>()` does not, and nothing in the type system flags that gap — it's a semantic difference invisible in the type signatures. Similarly, Python's SQLite bindings and OS-path handling absorb the `\` vs `/` distinction in ways a hand-built `file:` URI in Rust does not. Neither bug is a logic error in the traditional sense; both are the tax of porting behavior, not just types, and the tax only gets paid when someone actively tests the divergence rather than trusting that "the code compiles and matches the type signature" is equivalent to "the code matches the behavior."

## Lessons Learned

- A parity harness with one fixture per command proves the happy path, not correctness. Treat 5/5 Exact as a necessary, not sufficient, signal — pair it with a review pass and a dynamic-testing pass aimed specifically at the branches the fixtures don't exercise, every time, not just when something feels risky.
- When porting from Python, audit every place a Rust type's parse/format semantics might differ from Python's implicit coercions (whitespace handling, negative-index slicing, int overflow) — these differences don't show up as compiler errors, and they don't show up in a single-fixture parity test either.
- Platform-specific bugs (the Windows URI separator) are structurally invisible on single-OS CI. If ported code touches paths, URIs, or line endings, either get a CI leg on that platform or explicitly flag the gap in writing — don't let a Darwin-only green run imply cross-platform correctness.
- Dead code that exists ahead of a future phase (`session_library_id`) is still worth fixing now and documenting loudly, because "we'll catch it when we wire it in" is exactly the kind of promise that gets forgotten three phases later.
- Deviating from a written plan (resequencing phases, resolving an ambiguous requirement) is fine and sometimes necessary — but only if it's written down in the plan docs themselves, not just decided in a session and left to be reconstructed from git history later.

## Next Steps

- Owner: whoever continues the vertical slice. 91 of 96 commands remain unported.
- `item find`'s SQL-fallback path has no dedicated golden fixture yet — only ad-hoc dynamic-testing coverage. Add one to `harness/golden/python/` so it's covered by the standing parity suite, not one-off verification.
- No fixture exists for "Zotero completely unreachable" — needed both to cover the connector/Local-API error-path divergence and to give the harness real coverage of that branch. Needs a product decision on whether the error text divergence between `urllib` and `ureq` is acceptable or must be normalized.
- No Windows CI leg has executed `db.rs` yet; the URI fix is spec-verified, not runtime-verified. Add a Windows leg to the CI matrix (or at minimum a targeted single-OS job) before trusting this on real Windows.
- Original Phase 3 scope not yet started: full 17-environment-variable config inventory, `NOT_IMPLEMENTED` stubs for unported v1 commands, deferred/dropped-command visibility filtering.
- Confirm CI run `33246731188` (or its successor) actually finishes green before treating this push as verified, not just pushed.
