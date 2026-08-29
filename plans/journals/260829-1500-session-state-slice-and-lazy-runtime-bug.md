---
date: 2026-08-29 15:00
severity: medium
component: session-state slice (8 commands) + a real dispatch-level architectural bug found and fixed
status: in-progress
---

# The first write path, and a bug the write path was needed to expose

**Date**: 2026-08-29 15:00
**Severity**: Medium
**Component**: `crates/zotero-cli/src/{session,lib}.rs`, plan docs
**Status**: Ongoing (68 of 99 harness rows still unported)

## What Happened

Continued directly from the hardening + 15-command catalog slice (`05b4649`), per the user's own
proposed sequence: session-state commands next, since they introduce controlled local-state mutation
without yet touching the Zotero library, and are "the ideal place to make `session_library_id`
load-bearing... before the first real Zotero write" — the user's words, from the message that set
this sequence.

Ported 8 of the 9 `session *` commands: `status`, `use-library`, `use-collection`, `use-item`,
`clear-library`, `clear-collection`, `clear-item`, `history`. `session use-selected` is deferred —
it needs the connector's `getSelectedCollection` endpoint, which doesn't exist yet (`http.rs` only has
`connector_is_available`/`local_api_is_available`/`local_api_get_json` so far; POST support is
genuinely Phase 5 scope, same blocker `collection use-selected` already has). Added
`save_session_state` (locked write via a new `fd-lock` dependency — small, focused, chosen over the
broader `fs2` per the project's stated dependency-footprint principle), `append_command_history`
(reloads from disk rather than mutating a caller's copy, matching Python exactly: each CLI invocation
is a fresh process, so "current state" only ever means "what's on disk right now"), and
`build_session_payload`. `session history`'s `[-limit:]` slice needed its own helper
(`python_negative_tail_slice`) — a genuinely different shape from the `[:limit]` helper already
built for `item find`/`item list`, with its own three-way behavior (`limit == 0` returns the *entire*
list, since `-0 == 0` in Python — an easy one to get wrong by assuming symmetry with the other slice
helper).

Wired the 8 new subcommands and ran them through the parity harness. **7 of 8 came back as
`Mismatch`, not `Exact`.** The one that passed, `session use-library`, was the one command in the
group that actually needs `RuntimeContext` (it resolves the library ref via SQLite). The diff on
every failure was identical: Rust's `http_calls` showed the usual 2-call connector/Local-API probe
pair; Python's showed `[]`. `session status`, `use-collection`, `use-item`, and the three `clear-*`
commands touch only local session state in Python and issue zero HTTP calls — but `dispatch_command`
was building `RuntimeContext` (probe pair included) unconditionally at the top of every dispatch,
before routing to the specific command handler.

This was a real bug in code that predates this session — the original Phase 3 vertical slice's
`dispatch_command` — not something newly introduced. It was invisible through 23 previously-landed
commands because every single one of them (SQLite reads, HTTP-backed reads, `app status` itself)
legitimately needed the runtime, so the unconditional construction never produced a wrong answer
until the first commands that genuinely don't need it existed to prove it wrong. Fixed by turning the
eager construction into a lazy closure (`build_runtime`) called explicitly inside each of the 20 match
arms that actually use it — mechanically inserted via a small Python script rather than 20 manual
edits, then hand-verified against the diff. Re-ran the full 31-command regression set (23 previously
landed + 8 new) after the fix: all 31 Exact, zero regressions from the refactor.

Then went further than the harness could: the parity harness runs exactly one command per freshly
built fixture, so a green result on `session use-library` proves that command's own JSON output is
right, but nothing about whether a *second*, separate process reading the same state file afterward
sees what the first one wrote. Manually chained real subprocess invocations against the same
`CLI_ANYTHING_ZOTERO_STATE_DIR`: a successful `use-library 2` followed by a fresh `session status`
correctly showed `current_library: 2`; a `use-collection` + `use-item` + `status` sequence correctly
accumulated `history_count`; a deliberately-failing `use-library 999` (library not found) left prior
state completely untouched rather than partially applying the write, because the SQLite lookup errors
out via `?` before any field assignment happens. Also caught, from that manual testing, a subtle
ordering detail that matches Python exactly rather than by luck: a write command's own JSON response
reports `history_count` *before* that same command's own history line is appended (Python's `state`
local variable and `append_command_history`'s independent disk reload-and-resave are genuinely two
separate operations on two separate in-memory copies) — ported the same two-copies structure rather
than trying to "simplify" it into one, which would have silently changed the count in every write
command's own response.

Added two direct Rust tests exercising the real `fd-lock`-backed write path end-to-end (round-trip
through `save_session_state`/`load_session_state`/`append_command_history`, and the 50-entry history
cap) — not because the manual testing above wasn't convincing, but because manual testing on one
machine doesn't get re-run by CI on every push, and the phase doc's own success criterion ("session
file locking degrades gracefully... without failing the command") needed something CI's Windows leg
would actually execute, not just a documented claim.

`cargo test --workspace` (debug) passed clean. Before pushing, ran `cargo test --workspace --release`
specifically because that's CI's actual invocation, not debug — and it failed, intermittently, on
exactly the two new tests. Both set the same process-global env var
(`CLI_ANYTHING_ZOTERO_STATE_DIR`) to point at their own temp directory; Rust's default test runner
runs tests in parallel threads within one process, so the two tests raced on that shared global,
and the first test's own safety comment ("no other test in this crate reads this env var") was false
the moment the second test was written — self-contradicted within the same commit, not caught because
debug mode's slower, differently-scheduled execution happened not to overlap the two tests' critical
sections, while release mode's faster execution did. Fixed with a `static Mutex<()>` held for the
full duration of any test that touches that env var, with poison-recovery
(`unwrap_or_else(|p| p.into_inner())`) so one test's panic while holding the lock can't cascade into
spurious failures in the other. Re-ran `cargo test --workspace --release` 8 times back to back after
the fix, all green, plus 4 more debug-mode runs for good measure.

## The Brutal Truth

The satisfying part: this is exactly the kind of bug the project keeps saying it's watching for, and
it got caught the same way every time so far — not by extra scrutiny applied because something felt
risky, but by the mechanical fact of adding commands with different needs than the ones already
there. Nobody had to be clever to catch this; the harness's `http_calls` field being part of the exact
match was enough. That's the harness design paying for itself, three sessions running.

The uncomfortable part: this bug lived in `dispatch_command` since the very first Phase 3 vertical
slice, survived a code-review pass and a dynamic-testing pass in that session, survived a second
independent review-adjacent audit in the hardening session, and 23 Exact command results across two
slices never surfaced it — not because it was subtle, but because nobody had yet ported a command
that would disagree with it. That's worth sitting with honestly: "Exact on every command ported so
far" was never actually evidence this particular piece of architecture was correct. It was evidence
that every command ported so far happened not to exercise the bug. Those are different claims, and
conflating them is exactly the trap the project's own stated lesson ("a green harness run proves the
happy path... not correctness") is trying to name — this is a case of the same trap wearing a
different disguise: not "the fixture doesn't cover this branch of this function," but "no command
we've ported yet is shaped in a way that would disagree with this shared piece of plumbing." Worth
naming as its own category, not just filed under the same lesson as before.

## Technical Details

- Bug: `dispatch_command` (`lib.rs`) built `RuntimeContext` unconditionally before the `match`,
  instead of only inside arms that use it. Fix: `let build_runtime = || { ... };` closure, called
  explicitly as `let runtime = build_runtime();` inside 20 of 21 match arms (all except the 7
  runtime-free `session *` arms).
- Verification: full 31-row harness regression (23 pre-existing + 8 new) after the fix — 31/31 Exact,
  0 mismatch.
- New dependency: `fd-lock` 4.0.4 (Apache-2.0, `RwLock<T: AsOpenFile>`/`try_write()` API, already
  accepted in `about.toml`'s license list — no new acceptance decision needed).
- `session history`'s slice: `python_negative_tail_slice` — `limit == 0` -> whole list, `limit > 0` ->
  last `limit` (clamped), `limit < 0` -> drop first `-limit` (clamped to empty past list length).
  6 unit tests.
- 2 new tests exercise the real locked-write path: round-trip through save/load/append against a real
  temp-dir `CLI_ANYTHING_ZOTERO_STATE_DIR`, and the 50-entry cap. Both use `unsafe { set_var/remove_var
  }` (env-var mutation is `unsafe` as of this toolchain) — safe in practice since no other test in the
  crate reads that env var, documented as a constraint on adding a third such test casually.
- Tests: 27 -> 35 (8 new: 6 for the slice helper, 2 for the write path).
- Manually verified (not harness-capturable): cross-process persistence, history-count-before-its-own-
  append ordering, failed-write-doesn't-corrupt-state.

## What We Tried

- **Assuming the 15-command catalog slice's "Exact on the first attempt" pattern would hold for
  session commands too, and moving straight to the next slice on a first green harness run** —
  rejected on contact: 7 of 8 came back `Mismatch` immediately. The pattern held for finding problems
  quickly, not for confirming there weren't any.
- **Fixing the lazy-runtime bug by special-casing just the `session *` arms that don't need it, rather
  than restructuring all 20 arms** — considered and rejected: that would have meant every *future*
  runtime-free command needed someone to remember the special case exists, versus the closure approach
  where "does this arm need the runtime" is a visible, per-arm decision at every call site, matching
  Python's own `current_runtime(ctx)`-called-inside-each-handler structure.
- **Skipping the two write-path unit tests since the manual cross-process testing already proved the
  behavior correct** — rejected: manual testing done once on one machine isn't the same claim as "CI
  proves this on every push, including Windows," and the phase doc's own success criterion specifically
  asked for the latter.
- **Trusting `cargo test --workspace` (debug mode) as sufficient before pushing** — rejected once
  `--release` was run specifically because it's what CI actually invokes, and it caught a real,
  self-introduced race the debug run's scheduling happened to hide.

## Root Cause Analysis

The `dispatch_command` bug and the two previous sessions' bugs (Windows URI separators,
`resolve_attachment_real_path`'s untested branches) are structurally different from each other, worth
distinguishing rather than filing under one generic "watch for edge cases" lesson. The URI and
attachment-path bugs were both *branch coverage* problems — a function with N possible input shapes,
tested against fewer than N fixture inputs. This bug is a *shared-state assumption* problem — a
service built once and reused by every command handler, whose correctness depended on an invariant
("every caller needs this") that was true of every caller that existed at the time it was written, and
stopped being checked once it stopped being re-examined per new caller. The fix category is also
different: branch-coverage gaps are closed by adding more test inputs against the same code; this gap
was closed by changing *where* the shared construction happens, not by testing it harder in place.

## Lessons Learned

- "Every command ported so far passes" is evidence about the commands ported so far, not about the
  shared plumbing they all happen to route through — a piece of infrastructure can be silently wrong
  in a way that's invisible until a caller shaped differently from all previous callers exercises it.
  When adding a genuinely new *category* of command (not just another instance of a category already
  ported), specifically ask what assumptions the shared code makes that this new category might break,
  rather than only checking the new category's own logic.
- The harness's one-command-per-fixture design is deliberate and has a real edge: it cannot prove
  cross-process persistence, ordering between a write's own response and its side effects, or any
  multi-step sequence. The first slice that introduces state mutation is exactly the point where that
  gap starts mattering, and it's worth manually chaining real invocations specifically because the
  standing fixture design structurally can't.
- A `#[cfg(test)]` claim in a phase doc's success criteria ("locking degrades gracefully") is not
  satisfied by a doc comment or by manual testing that isn't re-run — it needs an actual `cargo test`
  entry that CI executes on every push, on every platform, or the criterion is aspirational, not met.
- A test's own inline safety comment ("no other test reads this") is a claim that expires the moment
  a second test is added, not a fact fixed at the time it was written. Any test that mutates
  process-global state (env vars, `chdir`, global statics) needs an explicit serialization mechanism
  from the start — a shared mutex, not a comment — because the second such test is often added in the
  same sitting as the first, by the same author, who is exactly the person least likely to notice the
  comment just became false.
- `cargo test` in debug mode and `cargo test --release` can disagree on a genuine race, because
  optimization changes thread scheduling and timing enough to change whether two racing tests'
  critical sections overlap. If CI's actual invocation is `--release` (it is, here), that's the
  command to run locally before pushing — not the faster debug default.

## Next Steps

- Owner: whoever continues the vertical slice. 68 of 99 harness rows (68 of 96 real commands) remain
  unported.
- Per the user's stated sequence, the JS-bridge/write slice is next — the first slice that writes to
  the actual Zotero library rather than local session state, and the first place
  `session_library_id` (regression-tested two sessions ago, load-bearing as of nothing yet) finally
  gets a real caller.
- `session use-selected` and `collection use-selected` both remain blocked on the connector
  `getSelectedCollection` POST client — worth landing early in the write slice since two commands are
  waiting on it specifically.
- `resolve_collection`'s ambiguous-key gap (flagged, not fixed, in the previous session) is still open.
- Given this session found a bug in *shared dispatch infrastructure* rather than in a single function,
  worth a deliberate pass before the write slice: are there other assumptions in `runtime.rs`,
  `catalog.rs`, or `session.rs` that happen to hold for every command ported so far but haven't been
  checked against what a *write* command will actually need (e.g. does anything currently assume reads
  are the only kind of operation)?
