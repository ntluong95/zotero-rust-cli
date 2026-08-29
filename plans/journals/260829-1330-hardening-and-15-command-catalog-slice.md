---
date: 2026-08-29 13:30
severity: low
component: unreachable-Zotero divergence, stub-requirement supersession, 15-command catalog slice (Phase 3-5 vertical slice, continued)
status: in-progress
---

# Hardening the vertical slice, then 5/96 -> 23/99 commands ported

**Date**: 2026-08-29 13:30
**Severity**: Low
**Component**: `harness/{capture,normalize,commands.tsv}`, `crates/zotero-cli/src/{db,catalog,cli,lib,paths,session}.rs`, plan docs
**Status**: Ongoing (76 of 99 harness rows still unported)

## What Happened

Two product decisions arrived from the user, made in response to the previous session's journal
(`260829-1200-vertical-slice-read-commands-ported.md`), which had explicitly flagged them as needing
a call rather than deciding unilaterally:

1. **Accept the unreachable-Zotero transport-error divergence**, scoped narrowly: add a real
   "nothing listening on the port" fixture, normalize only `connector_message`/`local_api_message`
   for that one fixture state, keep every other `app status` fixture Exact and unweakened.
2. **Drop the plan's leftover requirement to stub all 91 remaining commands as `NOT_IMPLEMENTED`**
   before porting them — the project already abandoned that sequencing for the vertical-slice
   approach, but `phase-03`'s text hadn't caught up. Exposing commands that only fail is actively
   worse for an agent than not exposing them at all.

Implemented both before touching anything else, since the user explicitly wanted the decision closed
first. The unreachable fixture: added a `zotero-unreachable` `fixture_state` to `capture.py` that
skips starting the fake HTTP server entirely and instead binds-then-closes a real TCP port, so both
implementations hit a genuine OS-level connection refusal rather than a mocked status. Verified
end-to-end — captured the Python golden, then the same command against the Rust binary, confirmed
**Exact** via `compare.py` (both message fields collapse to `<CONNECTION_REFUSED>`; everything else,
including the two `false` reachability booleans and `http_calls: []`, matches natively, no
normalization). Updated `phase-05` and `compatibility-matrix.md` to record the decision as resolved,
not just noted.

The stub-supersession was a documentation-only fix: `phase-03.md` still had "every unimplemented v1
leaf returns `NOT_IMPLEMENTED`" as a requirement, an implementation step, and a success criterion,
none of which the actual shipped code does or should do (nothing in `cli.rs` for an unported command
exists at all — `clap`'s own unrecognized-subcommand error is what a user or agent sees, which is
more honest than a clean-looking JSON failure for something that was never real). Rewrote those three
sections to say so explicitly, added a "Superseded requirement" subsection naming the rationale, and
was careful to distinguish this from the separate, still-valid Deferred/Dropped command visibility
requirement (the 7 DOCX-chain commands and `repl`, which are a different category — permanently out of
v1 scope, not "not yet ported").

Then moved to the next vertical slice, per the user's own proposed sequencing: 15 more read-only
catalog commands (`library list`; `collection find/get/items/tree`; `item attachments/children/
file/notes`; `search list/get/items`; `style list`; `tag list/items`), chosen because they build on
`catalog.rs`/`db.rs` infrastructure that already exists rather than introducing a new backend. Ported
each function from the Python reference line-for-line (`zotero_sqlite.py`'s `find_collections`,
`build_collection_tree`, `fetch_item_children/notes/attachments`, `resolve_attachment_real_path`,
`fetch_saved_searches`, `resolve_saved_search`, `fetch_tags`, `fetch_tag_items`; `catalog.py`'s
wrappers for all of the above plus `list_libraries`, `get_search`, `search_items`, `list_styles`),
wired 15 new `clap` subcommands and `lib.rs` dispatch arms, and added `quick-xml` as a new dependency
for `style list`'s CSL parsing — a deliberate choice over a regex heuristic, since a real
namespace-aware parse is what the plan actually calls for and the DOCX phases will need the same
crate later anyway.

All 15 classified **Exact** against the existing golden fixtures on the first harness run. Per the
previous session's own explicit lesson ("a green harness run proves the happy path is byte-identical;
it says nothing about the fallback search path, the error path, or platforms nobody's laptop can
reproduce"), did not stop there. Checked what each fixture row's arguments actually exercise before
calling any of it done, and found three real, load-bearing gaps:

- `resolve_attachment_real_path` — the plan's own docs flag this as the highest cross-platform risk
  area in the port. The only fixture item with a `storage:`-prefixed attachment is the one every
  `item file`/`item attachments` golden row queries; the fixture item with a `file:///C:/...`
  drive-letter path is a *different* item that no golden command ever queries, and no fixture
  attachment uses a non-localhost `file://` host at all. **4 of the function's 6 branches had zero
  coverage from a fully green harness run.** Closed with 8 direct unit tests exercising all six
  branches synthetically, including the UNC and drive-letter cases the fixtures can't reach.
- `build_collection_tree`'s orphan-root case (a `parentCollectionID` pointing outside the result set
  becomes a root, not silently dropped) — the fixture's only nested collection has its parent inside
  the same result set, so this branch was untested. Closed with 2 direct unit tests.
- `resolve_saved_search`'s ambiguous-reference error path — a saved-search key (`DUPSEARCH`) exists
  in both the user and group library in the fixture data, but no existing command row ever queried it
  ambiguously. Verified the expected behavior against the live Python reference first (exit 1, exact
  error text with both library IDs), then added it as a **new standing golden fixture** (row 99,
  `search get (ambiguous)`) rather than a one-off check, so it's part of the permanent parity suite.

Also found and fixed an architectural inconsistency while wiring `collection get`/`collection items`:
the Phase 3 slice's `get_collection` took a required `&str` and skipped Python's `ref: None ->
session.current_collection -> error` fallback, because its only caller at the time (`find_items`)
always supplied `Some`. `get_item` already has this fallback built in correctly. Rather than
duplicating the fallback-and-error logic at each new call site that needed it, changed
`get_collection`'s signature to match `get_item`'s established, correct pattern.

Flagged but did not fix: `resolve_collection` (Phase 3 slice 1's code, not new this session) has the
same class of untested-ambiguity gap `resolve_saved_search` had, for duplicate collection keys across
libraries. Recorded in `phase-04.md` as a known gap for the next hardening pass rather than silently
left for someone to rediscover.

Regenerated `THIRD-PARTY-LICENSES.md` for the new `quick-xml` dependency (MIT, already in
`about.toml`'s accepted list, no new license-acceptance decision needed). Verified `cargo fmt`,
`cargo clippy -D warnings`, `cargo test`, and `cargo build --release` all clean, release binary still
3.15 MB (well under the 15 MB CI budget) with no unexpected dynamic dependencies. Re-ran the full
99-row Python self-capture against the committed goldens to confirm none of the harness changes
(`capture.py`'s new `zotero-unreachable` branch, `normalize.py`'s new substitution) altered behavior
for the other 96 rows — still 100% Exact/Skipped, zero regressions.

## The Brutal Truth

The genuinely satisfying part: checking "what does this fixture actually exercise" before declaring
15 green commands done, rather than after, caught real gaps in exactly the function the plan itself
already flagged as highest-risk (`resolve_attachment_real_path`). That's the previous session's
discipline holding up under repetition — it would have been easy to treat "23/23 Exact on the first
attempt, twice in a row now" as proof the pattern doesn't need re-checking every time. It does.

The honest gap this session leaves open: `resolve_collection`'s pre-existing ambiguous-key path is
now a *known*, *named*, *documented* gap rather than an invisible one — which is real progress — but
it's still an open gap. Finding a sibling function's bug while auditing the one you just wrote and
then not stopping to fix it immediately is a defensible scoping call (it's Phase 3 slice 1's code,
out of this session's stated scope, and the fix pattern is now well-understood from
`resolve_saved_search`), but it's also exactly the kind of thing that's easy to flag-and-forget. It's
written down in `phase-04.md` specifically so that doesn't happen.

## Technical Details

- Unreachable fixture: `harness/capture.py`'s `_run_unreachable()` binds `("127.0.0.1", 0")`, reads
  back the OS-assigned port, closes the socket, then points `ZOTERO_HTTP_PORT` at that now-closed
  port. No fake server, no mocked status — a real connection refusal on every platform.
- `normalize.py`'s `CONNECTOR_MESSAGE_RE`/`LOCAL_API_MESSAGE_RE` substitution is gated on
  `fixture_state == "zotero-unreachable"` specifically — verified it does not touch any of the other
  98 rows by re-running the full Python self-capture and diffing against the committed goldens.
- `resolve_attachment_real_path`: percent-decoding implemented as a ~15-line manual decoder rather
  than a new dependency (the only call site); the UNC and drive-letter branches are built as direct
  string formatting rather than emulating Python's `PureWindowsPath`, since for the two exact input
  shapes those branches produce, `PureWindowsPath`'s output is equivalent to the literal string
  construction Python's own code already does before wrapping it — verified against Python source
  line-by-line, not assumed.
- `style list`: `quick-xml` 0.42's API differs from older versions assumed at design time —
  `BytesText::unescape()` doesn't exist in this version; used `xml10_content()` instead (EOL
  normalization, not entity-unescaping, but CSL `id`/`title` content is plain text/URLs in practice).
  `LocalName::as_ref()` returns `&str` in this version, not `&[u8]` — compiler caught both on first
  build attempt.
- New standing fixture rows: 97 (`app status (unreachable)`), 98 (`item find (sql-fallback)`), 99
  (`search get (ambiguous)`) — `harness/commands.tsv` now has 99 rows for the original 96 commands.
- Tests: 17 -> 27 (10 new: 8 for `resolve_attachment_real_path`, 2 for `build_collection_tree`).
- New dependency this session: `quick-xml` 0.42 (MIT).

## What We Tried

- **Declaring the 15-command slice done on 23/23 Exact against existing fixtures** — rejected, for
  the same reason the previous session rejected it at 5/5: checked what those fixtures' *arguments*
  actually exercise before trusting the green result, not just that they passed.
- **Duplicating the session-fallback-and-error logic at the `collection get`/`items` call sites in
  `lib.rs`** — rejected once written; moved it into `get_collection` itself to match `get_item`'s
  already-correct pattern and avoid two copies of the same fallback logic.
- **Fixing `resolve_collection`'s sibling ambiguity gap in the same pass since it was already found**
  — rejected as scope creep for this session; documented instead so it isn't lost.

## Root Cause Analysis

The `resolve_attachment_real_path` coverage gap has the same root cause as last session's Windows URI
bug: a golden fixture captured against *one* representative input per command proves that one input's
code path, and nothing about the others. The fixture data for this port was built with a handful of
sample items covering a handful of scenarios, but no single item in the base fixture happens to
exercise more than one of `resolve_attachment_real_path`'s six branches — so a command-level
`Exact` result on `item file`/`item attachments` was structurally incapable of proving the other five
branches correct, no matter how many times it passed. The fix isn't "trust the harness less" — it's
"know exactly what each Exact result is and isn't evidence for," which requires reading the fixture
data the command under test actually touches, not just reading the diff.

## Lessons Learned

- A repeated pattern (two vertical slices now, both green on the first harness attempt) is not
  evidence the verification step can be skipped or shortened next time — if anything it's a reason to
  be more suspicious of the next green run, since it's easy to start trusting it.
- When a plan doc's success criteria still list a superseded requirement, saying so explicitly and
  striking through the specific line is more useful to a future reader than deleting it silently —
  the "why this changed" is exactly the thing that gets lost otherwise.
- An architectural inconsistency between two sibling functions (`get_item` correct, `get_collection`
  narrowed) is easiest to catch and fix cheaply the first time a second caller needs the missing
  behavior — waiting compounds the duplicated-logic cost at every subsequent call site.
- Finding a bug in code adjacent to what you're actually changing is common and doesn't obligate
  fixing it immediately, but it does obligate writing it down somewhere a future session will
  actually read, not just noticing it and moving on.

## Next Steps

- Owner: whoever continues the vertical slice. 76 of 99 harness rows (76 of 96 real commands) remain
  unported.
- `resolve_collection`'s ambiguous-key path (duplicate collection keys across libraries, e.g.
  `DUPCOLL1`) needs the same fixture-or-unit-test treatment `resolve_saved_search` just got.
- Per the user's proposed sequencing, the session-state slice (`session status/use-library/use-item/
  use-collection/clear-*/history/use-selected`, 9 commands) is next — the first slice to introduce
  controlled state *mutation*, and the point where `session_library_id` (regression-tested last
  session but still dead code) becomes load-bearing for the first time.
- `session use-selected` needs Phase 5's connector HTTP client (`getSelectedCollection`) before it can
  land — it's the one Phase 4 command still open, blocked by dependency rather than left undone.
- Windows/Linux CI has not yet run the 10 new tests added this session; they're platform-agnostic pure
  functions so should pass, but "should" isn't "verified" until CI actually runs them.
