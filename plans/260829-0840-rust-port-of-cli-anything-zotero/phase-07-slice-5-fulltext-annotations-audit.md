---
phase: 7
slice: 5
title: "Full-Text / PDF / Annotation Search — Read-Only Audit + Frozen Live Evidence"
status: audit-complete
priority: P1
dependencies: [7-slice-3, 7-slice-4]
---

# Phase 7 Slice 5: Full-Text / Annotations — Implementation Spec

> **Recovery note (2026-08-30):** this file did not exist during the original audit — the audit was
> produced as chat-only output across two assistant turns and never written to disk, then lost to
> context compaction. This document was reconstructed verbatim from the raw session transcript
> (`~/.claude/projects/.../90382e2a-f7da-4ed4-b98f-4b0d31702de6.jsonl`, turns at lines 2252, 2280,
> 2286, 2682) by streaming/grepping for the audit's own section markers and concatenating the
> matching assistant text in chronological order. All ~23 numbered sections plus the corrected §13
> and the frozen live-evidence finding were recovered in full — nothing here is invented or
> paraphrased from the orchestrator's own summary; it is the original audit text, reassembled.

**Evidence basis (as stated by the original audit):** Fetched the actual pinned upstream source
(`PiaoyangGuohai1/cli-anything-zotero@e42a930e`) via `gh api` — `zotero_cli.py` (all 3 command
definitions + `emit_js`), `core/jsbridge.py` (`get_annotations`, `search_fulltext`,
`search_annotations`, complete), `core/catalog.py` and `utils/zotero_sqlite.py` (confirmed zero
involvement), `core/notes.py` (confirmed zero overlap with Slice 4). No code modified, no commits
made during the audit.

---

## 1. Exact Python source map

| Function | File | Lines (approx) |
|---|---|---|
| `item_search_fulltext_command` | `zotero_cli.py` | ~1387-1394 |
| `item_search_annotations_command` | `zotero_cli.py` | ~1175-1183 |
| `item_annotations_command` | `zotero_cli.py` | ~1397-1403 |
| `emit_js` (shared output/exit-code classifier) | `zotero_cli.py` | ~317-349 |
| `JSBridgeClient.search_fulltext` | `core/jsbridge.py` | 565-575 |
| `JSBridgeClient.search_annotations` | `core/jsbridge.py` | 577-603 |
| `JSBridgeClient.get_annotations` | `core/jsbridge.py` | 419-443 |

**Confirmed zero involvement:** `core/catalog.py` (nothing annotation/fulltext-related at all),
`utils/zotero_sqlite.py` (only the already-ported generic
`annotationText`/`annotationComment`/`isAnnotation` base-item fields used by unrelated read
commands — never read by these 3 commands), `core/notes.py` (standalone-notes-only, zero shared
code with annotations; see corrected §13 below).

## 2. Canonical command inventory

All three are **B. search/retrieval, zero mutation** (see §11). None fall into category C.

### `item search-fulltext <QUERY> --limit=10`
- `QUERY`: **required** positional.
- `--limit`: `int`, default **10**.
- No `--collection`, no `--library`, no session integration at all — hardcoded `library_id=1` (see §3).
- JSON output: **bare top-level array** (not wrapped in an object) — `[{"key","title","date"}, ...]` or `[]`.
- Human output: `emit_js` falls through to `emit()`'s list-printing path (one `json_text()` line per dict item; `[]` prints literally as `[]`).
- Exit: 0 for a successful search (including zero matches); 1 only on a genuine Bridge/transport failure.

### `item search-annotations [QUERY=""] --color <repeatable> --limit=20`
- `QUERY`: **optional** positional, Click default `""` (empty string is a real, valid, distinct mode — see §4/§8).
- `--color`: repeatable string, exact-match filter (no normalization).
- `--limit`: `int`, default **20**.
- No `--library`/`--collection`/session integration — hardcoded `library_id=1`.
- JSON output: bare top-level array — `[{"type","text","comment","color","page","parentTitle"}, ...]` or `[]`.
- Exit: 0 for any search outcome; 1 only on transport failure.

### `item annotations <ITEM_KEY>`
- `ITEM_KEY`: required positional. **Raw string passed straight to the Bridge — no SQLite/`catalog` pre-resolution, no numeric-id support, no session fallback** (see §9).
- No flags.
- JSON output: `{"count": N, "annotations": [{"type","text","comment","color","page"}, ...]}` **on success**, but a **bare JSON string** `"ERROR: item <key> not found"` / `"ERROR: attachment has no parent item"` on the two JS-level failure paths — and critically, **both still exit 0** (see §14/§15).
- Exit: 0 in every case above; 1 only on transport failure.

## 3. Full-text search architecture (exact execution flow)

```
item_search_fulltext_command (zotero_cli.py)
  -> current_bridge(ctx).search_fulltext(query, limit=limit)   [library_id defaults to 1, never overridden]
       -> JS: new Zotero.Search(); s.libraryID = 1;
              s.addCondition('fulltextContent', 'contains', query);
              ids = await s.search();
              items = await Zotero.Items.getAsync(ids);
              return items.slice(0, limit).map(i => ({key, title, date}));
  -> emit_js(ctx, result)   [no require_data]
```

**Confirmed: `Zotero.Search` + `fulltextContent`/`contains` — Zotero's own live full-text search
API, never a direct SQLite FTS query.** This is the *only* mechanism; there is no SQLite fallback,
no filesystem read, no Connector/Local API involvement anywhere in this path. The project's "use
Zotero's live API, not raw FTS tables" rule is not just a preference here — it is literally what
upstream already does.

## 4. Search query semantics (exact, as delegated to Zotero)

- **Match operator:** `'contains'` on `fulltextContent` — a Zotero-native condition; this port has
  no visibility into (and must not reverse-engineer) Zotero's internal tokenization/stemming/case-folding.
  Whatever Zotero's FTS layer does is authoritative and un-testable offline beyond mocking the
  Bridge response.
- **Phrase vs plain text, AND/OR, stemming:** entirely Zotero's own behavior; Python passes the raw
  query string through with only a naive `'`-escape (a real D1-class risk if ported literally — see §10).
- **Field restriction:** none beyond `fulltextContent` itself — no per-field targeting.
- **Attachment-only / full-text-only restriction:** implicit in the condition itself (only
  attachments carry `fulltextContent`), but the *returned* records come from
  `Zotero.Items.getAsync(ids)` on whatever `s.search()` yields — whether Zotero's search rolls a
  fulltext match up to the parent bibliographic item or returns the attachment item directly was an
  **unresolved parity question at audit time**; **resolved by live verification, see the Frozen Live
  Evidence section at the end of this document.**
- **Library/collection filtering:** library is hardcoded to `1`; **no collection filtering exists in Python at all.**
- **Limit:** applied via `.slice(0, limit)` **after** the full unbounded Zotero result set returns —
  pure client-side truncation, no pagination.
- **Ordering:** whatever `s.search()` + `Items.getAsync()` yield natively — **not independently
  sorted or guaranteed by Python.** Do not invent a sort.
- **Duplicate suppression:** none — Python does not dedupe.

## 5. Indexing state

**Confirmed: Python does not wait, retry, poll, or trigger indexing anywhere in this code.** The
search runs once against whatever Zotero has already indexed at that instant. A PDF that exists but
hasn't finished background indexing simply produces fewer/no matches — indistinguishable from "no
matches for another reason." No indexing-state field is exposed. **Do not invent synchronization,
retry, or an indexing-status field — none exists in the reference.**

## 6. PDF text search results — exact fields

`search_fulltext` returns **only**: `key`, `title`, `date`. That is the complete, exhaustive field
set. **Not present, despite being plausible:** attachment key, `libraryID`, filename, path, page
number, matched text/snippet, score/rank, annotation linkage. None of these exist in the Python
output — do not add them; a later slice can extend this only as a documented, intentional
"Changed"-class enhancement, not silently.

## 7. Annotation retrieval — exact behavior

```
item_annotations_command (zotero_cli.py)
  -> current_bridge(ctx).get_annotations(item_key)   [library_id defaults to 1]
       -> JS: item = Zotero.Items.getByLibraryAndKey(1, item_key);
              if (!item) return 'ERROR: item <key> not found';
              if (item.isAttachment()) {
                parent = Zotero.Items.get(item.parentItemID);
                if (!parent) return 'ERROR: attachment has no parent item';
                item = parent;
              }
              // item is now guaranteed to be the top-level bibliographic item
              for each PDF attachment of item:
                try { annots = att.getAnnotations(); collect } catch(e) {}  // silently skipped on error
              return {count, annotations: [{type, text(≤200 chars), comment, color, page}, ...]}
```

- **Accepted references:** the top-level bibliographic item key **or** any of its PDF attachment
  keys — both resolve to the same parent-level annotation set. **The annotation's own key is never
  accepted as input** (there's no "get one annotation by its own key" mode).
- **Annotation types:** `a.annotationType` passed through raw, unvalidated/unenumerated — whatever
  Zotero itself uses (`highlight`, `note`, `image`, `ink`, `underline`, etc. are Zotero's own type
  strings; Python does not enumerate or restrict them).
- **Fields returned per annotation:** `type`, `text` (annotationText, truncated to 200 chars, `''`
  default), `comment` (annotationComment, **not truncated**, `''` default), `color` (`''` default),
  `page` (annotationPageLabel, `''` default).
- **Fields NOT returned, despite being plausible:** the annotation's own `key`, `sortIndex`,
  `position` (JSON), `isExternal`, the parent attachment's key, the parent bibliographic item's key.
  **None of these exist in Python's output.**
- **Silent per-attachment failure:** a malformed annotation / `getAnnotations()` exception on one
  PDF attachment is swallowed by an empty `catch(e) {}` — that attachment simply contributes zero
  annotations, with **no error signal, no partial-failure flag**. Preserve this exactly; do not
  surface a warning Python doesn't.
- **Mutation:** none — no `saveTx`/`eraseTx` anywhere.

## 8. Annotation search — exact behavior

```
item_search_annotations_command (zotero_cli.py)
  -> current_bridge(ctx).search_annotations(query, colors=[...] or None, limit=limit)
       -> JS:
          if query: s.addCondition('annotationText', 'contains', query)   // highlight text ONLY
          else:     s.addCondition('itemType', 'is', 'annotation')        // browse-all mode
          ids = await s.search(); annots = await Items.getAsync(ids);
          filtered = annots.filter(a => colors ? colors.includes(a.annotationColor) : true);  // exact match, client-side
          return filtered.slice(0, limit).map(a => {
            parent = Items.get(a.parentItemID);            // the PDF attachment
            grandparent = parent ? Items.get(parent.parentItemID) : null;  // the bibliographic item
            title = grandparent?.title ?? parent?.title ?? '';
            return {type, text(≤200), comment, color, page, parentTitle: title};
          });
```

- **Text searched:** `annotationText` (the highlighted/underlined text) **only** —
  `annotationComment` is **never** searched. This is an exact, deliberate distinction; do not add
  comment-searching.
- **Case handling / matching mode:** delegated entirely to Zotero's `'contains'` condition — same
  caveat as §4.
- **Empty query behavior:** switches condition to `itemType == 'annotation'` — returns **every
  annotation in the library** (subject to color filter + limit), not an error and not `[]` by
  default.
- **Color filter:** applied **client-side, after** `s.search()` returns, via exact string membership
  (`colors.includes(a.annotationColor)`) — no case-insensitivity, no partial matching, no hex-name
  normalization. **`limit` is applied after color filtering**, so the pre-filter Zotero result count
  is not itself bounded by `limit`.
- **`parentTitle` is actually the grandparent's (bibliographic item's) title** when the chain
  resolves that far, falling back to the immediate parent's (attachment's) title otherwise — a
  slightly misleading field name worth documenting exactly as-is.
- **No annotation key returned here either** — same gap as `get_annotations`.
- **Library/collection scope:** hardcoded `library_id=1`; no collection scoping at all.
- **Ordering:** unsorted, whatever Zotero returns.

## 9. Read-only SQLite role

**None, beyond what's already ported.** Zero of these three commands touch `zotero_sqlite`/`catalog`
for item/attachment resolution — `item_key`/`query` strings are passed **raw** straight into the
Bridge JS (`Zotero.Items.getByLibraryAndKey`), with no numeric-id support and no session/current-item
fallback. The existing `db.rs` `Item.annotation_text`/`annotation_comment`/`is_annotation` fields
(already ported, Phase 4) are used only by unrelated generic item-listing commands (`item children`,
`item list`) — **not** by anything in this slice. **Do not add new SQLite reads for this feature; do
not use Zotero's internal FTS tables as a substitute for `fulltextContent`/`annotationText` search —
Python itself never does.**

**Open design tension, not resolved here:** every other already-ported write/mutation command in
this project resolves refs via `catalog::get_item` (numeric ID + session-fallback support) before
touching the Bridge. Python's own `get_annotations` does not do this — it accepts only a raw Zotero
key. Whether the Rust port should (a) match Python exactly (raw key, no numeric-id/session support)
or (b) extend it for this project's own established UX convention is a genuine open question for
whoever wires the CLI layer — flagged, not decided.

## 10. Proposed Bridge primitives

All three follow the exact existing `bridge/client.rs` + `bridge/templates.rs` + `bridge/js/*.js`
pattern (same as Slice 3's `find_pdf`/`list_items_missing_pdf`). **Critical: Python's own JS uses
naive `query.replace("'", "\\'")` string interpolation for `query`/`item_key` — a live D1 injection
vector if ported literally. All three must use the established `bridge::templates::render`/
`JSON.parse` safe-binding mechanism instead, like every other bridge primitive in this codebase.**

| Primitive | JS API called | Inputs | Output | Privileged? | Timeout | Read-only? |
|---|---|---|---|---|---|---|
| `search_fulltext` | `new Zotero.Search()` + `addCondition('fulltextContent','contains',q)` | `libraryID`, `query`, `limit` | raw array `[{key,title,date}]` | No (but still ownership-gated like all bridge calls) | 8s (Python's own default) | Yes |
| `search_annotations` | `new Zotero.Search()` + `addCondition('annotationText'\|'itemType',...)`, client-side color filter | `libraryID`, `query`, `colors[]`, `limit` | raw array `[{type,text,comment,color,page,parentTitle}]` | No | 8s | Yes |
| `get_annotations` | `Zotero.Items.getByLibraryAndKey` + `Attachments`/`getAnnotations()` | `libraryID`, `key` | `{count,annotations[]}` or bare `"ERROR: ..."` string | No | 5s | Yes |

Ownership gate unchanged: `fork == "zotero-rust-cli" && id == "cli-bridge@cli-anything-rust.dev"`,
already enforced by the shared `bridge_endpoint_active()`/`execute_js` machinery every existing
primitive goes through — nothing new needed here, just reuse.

## 11. Mutation check

**Confirmed 100% read-only from Zotero's perspective** — no `saveTx()`, `eraseTx()`,
`Zotero.Attachments.*`, or any transaction call anywhere in
`get_annotations`/`search_fulltext`/`search_annotations`'s JS bodies. Do not classify anything here
as needing write-outcome/confirm-flag handling.

## 12. Slice 3 interaction

No coupling exists in Python, and none should be invented: `search_fulltext`/`get_annotations` run
against whatever Zotero has already indexed/annotated at call time, with zero synchronization to
Slice 3's `find-pdf`/`fetch-pdf`. A PDF attached moments earlier by Slice 3 may not yet be
full-text-indexed, and will legitimately produce no matches — this is expected, matches Python, and
must not be "fixed" with a wait/retry loop.

## 13. Slice 4 interaction (corrected)

**Corrected sentence (original said "Connector-mediated" — this was wrong):** `core/notes.py`
(standalone Zotero notes) — `get_note`/`get_item_notes` are read-only SQLite
(`zotero_sqlite.resolve_item`/`fetch_item_notes`); `add_note` creates the note via the **JS Bridge**
(`bridge.execute_js` running `new Zotero.Item('note'); note.saveTx();`), **not Connector-mediated**.
The file defines a `_require_connector(runtime)` helper, but it is never called by any function
shown in the module — vestigial/unused, not evidence of real Connector involvement. The only textual
overlap with the annotation-retrieval JS in `jsbridge.py` is a type-check guard (`typeName in
{"note","attachment","annotation"}`) in `add_note` — not reusable logic. **Confirmed zero shared
code. Slice 5 has no semantic dependency on Slice 4** (but see §22 for a real *file-collision* risk
on shared Bridge plumbing files, which is a sequencing concern, not a semantic one).

## 14. Error semantics (exact, do not collapse)

| Scenario | Transport `ok` | `data` shape | Exit code | Notes |
|---|---|---|---|---|
| Item not found (`get_annotations`) | `true` | bare string `"ERROR: item <key> not found"` | **0** | Not a dict, `emit_js` treats it as ordinary success data |
| Attachment has no parent (`get_annotations`) | `true` | bare string `"ERROR: attachment has no parent item"` | **0** | Same mechanism as above |
| No annotations found | `true` | `{"count":0,"annotations":[]}` | 0 | Real structured success, not the string-error shape |
| No full-text/annotation-search matches | `true` | `[]` (bare array) | 0 | Success with empty results |
| Malformed annotation on one attachment | `true` (per-attachment) | that attachment silently contributes 0 annotations | 0 | No partial-failure signal anywhere |
| Bridge unavailable / wrong ownership / genuine timeout | `false` | `null`, with `error` set | **1** | The one real hard-error path; full `{ok,data,error}` envelope printed |

**Do not normalize these into one generic result** — the "success but the payload happens to be an
error-shaped string, exit 0" case is real, exact Python behavior and must be preserved for
Semantic-class parity, not silently upgraded to a proper error/non-zero exit.

## 15. Output parity — exact shapes and classification

| Command | Success shape | Class | Deviation risk |
|---|---|---|---|
| `item search-fulltext` | bare array `[{key,title,date}]` | **Semantic** (already so classified in `compatibility-matrix.md` row 71) | Values are Zotero-search-dependent, non-deterministic; schema (3 fields, no more) must match exactly |
| `item search-annotations` | bare array `[{type,text,comment,color,page,parentTitle}]` | **Semantic** (matrix row 70) | Same — schema exact, values Zotero-dependent |
| `item annotations` | `{count,annotations[]}` **or** bare error string | **Semantic** (matrix row 49) | The dual success-shape (object vs bare string) must be preserved, not unified |

No field is ever `null` vs omitted ambiguity in these three — Python's JS always supplies a default
(`''`, `0`, `[]`) rather than omitting keys.

## 16. Proposed Rust files

```
src/fulltext.rs           # search_fulltext orchestration + response classification
src/annotations.rs        # get_annotations + search_annotations orchestration + classification
src/bridge/js/search_fulltext.js
src/bridge/js/search_annotations.js
src/bridge/js/get_annotations.js
tests/fulltext.rs
tests/annotations.rs
```

Extend (not replace) the existing shared files, matching Slice 3's precedent exactly:
- `src/bridge/client.rs` — add `search_fulltext`, `search_annotations`, `get_annotations` methods
  (return raw `BridgeResponse`, mirroring Python's own thin layering — no `find_pdf`-style outcome
  enum needed here since Python itself does no such classification at this layer).
- `src/bridge/templates.rs` — add `T_SEARCH_FULLTEXT`/`render_search_fulltext`, etc., 3 new template
  constants + render functions.

A single shared "classify like `emit_js`" helper (exit-code-and-shape decision per §14) belongs in
`bridge/client.rs` or one of the two new files — recommend `bridge/client.rs` since it's the one
place already responsible for response-shape conventions, and it's reusable by any future "raw
emit_js-style" command.

**`cli.rs`/`lib.rs` explicitly out of scope**, per instruction — no module registration in this
slice either (matches Slice 1-3's own `#[path]`-test-inclusion convention pending a future
CLI-integration slice).

## 17. Implementation-level test matrix

**FULL-TEXT**
- `search_fulltext(library_id, query, limit)` renders via `bridge::templates::render` (JSON.parse
  binding) — assert the emitted code is `const P = JSON.parse("...");` + template body, never raw
  string concatenation of `query` into JS.
- Injection set applied to `query`: `'`, `"`, `\`, newline, Unicode/CJK, `</script>`, `${...}` —
  assert each round-trips as inert JSON-encoded data (mirrors `tests/bridge_injection.rs`'s existing
  vector set) and never breaks JS parsing / never gets executed as a script fragment.
- Orchestration default: `library_id` defaults to `1` when unset by the (not-yet-built) CLI layer —
  test the *function*, not a CLI flag, since no CLI exists yet: call with `library_id=1` explicitly
  and assert the rendered JS contains `s.libraryID = 1;`.
- Orchestration default: `limit` defaults to `10` — same, test at the function-parameter level.
- Exact JS body assertion (golden-template test, like `tests/bridge_templates.rs`): contains `new
  Zotero.Search()`, `s.libraryID = P.libraryID`, `addCondition('fulltextContent', 'contains', ...)`,
  `await s.search()`, `Zotero.Items.getAsync(ids)`, `.slice(0, P.limit)` — in that structural order.
- Output projection: mock a Bridge response with extra JS-side fields (e.g.
  `{key,title,date,extra:"x"}`) and assert the Rust classifier only surfaces `key`/`title`/`date` if
  it does any reshaping at all — or, if it passes the raw array straight through unmodified (matching
  Python's own zero-reshaping behavior), assert *that* instead. **Decide and test exactly one
  behavior — do not add fields Python doesn't emit.**
- Zero results: mock `[]` → classifier returns success, empty list, no error.
- Multiple results: mock an out-of-alphabetical/out-of-key-order array from the Bridge → assert the
  classifier preserves that exact order (no sort call anywhere in the Rust path).
- No dedupe: mock two identical `{key,title,date}` entries → assert both survive unchanged.
- Bridge unavailable: reuse the `ScriptedServer` ownership-rejection pattern from Slice 3 → assert
  transport failure, not a JS-level "no results" outcome.
- Genuine timeout: reuse the `ScriptedResponse::Stall` pattern (past the `execute_http` 10s floor) →
  assert transport failure classification, and assert **no second request was made** (no retry).
- No indexing wait/poll: assert the function makes **exactly one** Bridge call per invocation
  regardless of the mocked response content — there is no code path that could retry on an
  "unindexed" signal because no such signal exists to key off.

**ANNOTATION SEARCH**
- Empty query (`""` or omitted) → rendered JS uses `addCondition('itemType', 'is', 'annotation')`,
  **not** `annotationText`.
- Non-empty query → rendered JS uses `addCondition('annotationText', 'contains', query)`.
- Assert `annotationComment` never appears as a search-condition field anywhere in the rendered
  template (a static/golden-template assertion, not just a runtime behavior test).
- `--color` repeated values → all passed through to the client-side filter array; test 0, 1, and 3 colors.
- Color filtering is **exact-match, case-sensitive**: mock annotations with `color: "Yellow"` and a
  filter of `"yellow"` → assert no match (proves no case-folding is invented).
- Filtering-before-limit: mock 10 annotations where only 3 match a color filter, with `limit=2` →
  assert exactly 2 results are returned **from the color-matching set**, not 2 taken pre-filter then
  filtered down to fewer.
- Default `limit=20` at the orchestration-function level.
- `parentTitle`: three fixture cases — (a) grandparent resolvable → its title used; (b) grandparent
  missing but parent resolvable → parent's title used; (c) neither resolvable → `""`. All three
  driven by mocked Bridge JSON, since the actual resolution happens inside the JS itself — these are
  tests of the *classifier* correctly passing through whatever `parentTitle` value the mock supplies,
  not tests of Zotero's own parent-walk (which is untestable offline).
- Output fields exactly: `type`, `text`, `comment`, `color`, `page`, `parentTitle` — a golden-shape
  test failing on any extra or missing key.
- `annotationText` truncation: mock a >200-char string in the Bridge response and assert the Rust
  side does **not** re-truncate it (truncation already happened in JS/Zotero before the response
  arrived) — the classifier must pass through whatever length the mock provides unchanged. A
  companion **golden-template test** asserts the JS body contains `.substring(0, 200)` verbatim.
- Zero results → `[]`.
- Order preserved, no dedupe — same style as full-text.
- Bridge errors → transport failure, no retry.

**GET ANNOTATIONS**
- Regular bibliographic item key → mock response `{count:1,annotations:[...]}`, assert pass-through.
- PDF attachment key case: since the parent-walk happens inside JS, this is a **golden-template
  test** (assert the rendered JS contains the `item.isAttachment()` →
  `Zotero.Items.get(item.parentItemID)` walk), not a runtime-behavior test — the *actual* resolution
  can't be exercised without live Zotero.
- Missing item: mock the bare string `"ERROR: item ITEM_KEY not found"` as the Bridge `data` →
  assert the Rust classifier treats this as **success** (transport `ok:true`) and surfaces the
  string as-is, **not** as a Rust `Err`/failure variant.
- Orphan attachment: mock `"ERROR: attachment has no parent item"` → same treatment.
- Both above: assert whatever "exit code" classification helper Slice 5 builds reports **0**,
  matching `emit_js`'s exact behavior — this is the single most important regression to lock down
  with a test, since it's the most counter-intuitive finding in this audit.
- No annotations: mock `{count:0,annotations:[]}` → assert this is treated identically in
  success-shape to the "found some" case, distinct in *shape* (object, not string) from the
  not-found cases above.
- Multiple PDF attachments contributing annotations: mock-level concern only insofar as the JS
  aggregates across attachments — a golden-template assertion that the loop iterates
  `item.getAttachments()`, not a runtime test (Zotero-side behavior).
- Non-PDF attachments ignored: golden-template assertion that the JS filters on
  `att.isPDFAttachment()` before calling `getAnnotations()`.
- Per-attachment exception swallowed: golden-template assertion that the `getAnnotations()` call
  sits inside its own `try {} catch(e) {}` block with an empty catch body — this is a structural
  JS-text assertion, not independently runtime-testable in Rust.
- Raw `annotationType` preserved: mock an unusual/unenumerated type string (e.g. `"ink"`) and assert
  the classifier does not validate/reject/remap it.
- `annotationText` truncated to Python's exact 200-char point: same golden-template `.substring(0,
  200)` assertion as above.
- `comment` **not** truncated: golden-template assertion that no `.substring(...)` call wraps
  `annotationComment`.
- Empty/default values: mock an annotation with only `type` set (no text/comment/color/page in the
  raw JS return, though in practice the JS always supplies `''` defaults) → assert Rust doesn't
  introduce `null` where JS would have supplied `''`.
- Output fields exactly `type,text,comment,color,page` for `get_annotations` (no `parentTitle` here
  — that's `search_annotations`-only).
- No annotation key, no `position`/`sortIndex`, no attachment/parent keys anywhere in the output —
  negative-assertion test (`assert_no_forbidden_keys`-style, reusing the Phase 6 denylist-test
  pattern) confirming these fields are never accidentally introduced by a future refactor.

**SECURITY (all three new templates)**
Reuse the exact vector set from `tests/bridge_injection.rs`: single quotes, double quotes,
backslashes, newline, CJK/Unicode, `</script>`-style content, JS-fragment-shaped strings (e.g. `';
alert(1); //`). For each: render the template via `bridge::templates::render`, assert the parameter
appears **only** inside the `JSON.parse("...")` payload string (never spliced into the executable JS
body directly), and assert the rendered code still parses as syntactically valid JS wrapping
(structural sanity, not a full JS parser — matching how `tests/bridge_templates.rs` already
validates other templates).

## 18. Live Zotero verification requirement — SATISFIED

The original audit required a minimal disposable-profile acceptance check before this slice is
review-complete. **This has since been performed and its finding frozen as authoritative evidence —
see the "Frozen Live Evidence" section at the end of this document.** The requirement text is
preserved here for record:

1. **Setup:** disposable Zotero profile (never the user's real library), one bibliographic item with
   one attached, Zotero-indexed PDF containing a unique, unlikely-to-collide phrase.
2. **Action:** call `JSBridgeClient::search_fulltext` — or, before implementation exists, hand-execute
   the exact JS body via the existing `js`/`execute_raw_js` primitive already merged — with a query
   matching that unique phrase.
3. **Record:** for every object in the raw result array — `key`, resolved `itemType`, `title`, and
   whether that key matches the bibliographic item's own key or the PDF attachment's key.
4. **Compare:** against Python's own live behavior if available; otherwise record as LIVE VERIFIED
   evidence.
5. **Explicit non-goal:** whatever this finds must be documented, not used to justify adding
   parent-mapping/normalization logic beyond what Python's JS already does.

## 19. Slice split decision

**Recommendation: OPTION A — one Slice 5**, confirming the maintainer's stated preference; the
completed audit found no substantive reason to split.

| Criterion | Finding | Favors split? |
|---|---|---|
| Transport | Identical for all three: JS Bridge + `Zotero.Search`/`Zotero.Items`, no Local API, no Connector, no divergent surface | No |
| Shared Bridge plumbing | All three go through the same `execute_js`/ownership-gate/`bridge::templates::render` machinery already merged | No — favors keeping together |
| Implementation size | 3 small JS templates (~10-20 lines each) + 2 thin Rust files; smaller in aggregate than Slice 3's single PDF-cascade module | No |
| Reviewability | Small diff, no cross-cutting state (no resume files, no external HTTP, no multi-step orchestration) — nothing like Slice 3's complexity | No |
| Test surface | Sizable (§17) but mechanically uniform across all three (golden-template + mock-response classifier tests) — reviewing similar tests in one PR is not harder than reviewing them split across two | No |
| Architectural coupling | `search_annotations`/`get_annotations` are tightly related (search is `get_annotations`'s generalization); `search_fulltext` is a clean, separate module (`fulltext.rs`) with zero cross-imports needed | Weak, already addressed by the file split, not a slice split |

**No criterion favors 5A/5B.** The `fulltext.rs`/`annotations.rs` file boundary already captures the
one real internal seam; splitting the *slice* on top of that would add PR/branch/review overhead
without a corresponding risk or size justification.

## 20. Exact/Semantic classification — confirmed

All three **remain Semantic**, unchanged from `compatibility-matrix.md` (rows 49, 70, 71). Not
reclassifying to Exact merely because the JS structure itself can be matched precisely:

- **Deterministic schema parity (achievable, must be enforced by tests):** field names, field set,
  field defaults (`''`/`0`/`[]`), the dual-shape success case for `item annotations` (object vs.
  bare string), the bare-array top-level shape for the two search commands, exit-code rules from
  §14.
- **Zotero-runtime-dependent value parity (not achievable, not required):** which items/annotations
  actually match a query, result ordering, whether a fulltext hit resolves to a parent or attachment
  record (now resolved, see Frozen Live Evidence), annotation counts, timing of index availability.
- **Known observable deviations to watch for, not yet confirmed either way:** none identified as
  *necessary* deviations in this audit — the goal is byte-for-byte schema parity. Any deviation found
  during implementation must be reported explicitly, not silently absorbed into "Semantic allows it."

## 21. CLI-integration contract for later (documented now, not implemented)

**`item search-fulltext`**
- Required `QUERY` positional; `--limit` default `10`; `library_id` hardcoded `1` (no `--library`,
  no session read); output is a bare JSON array; `[]` is success/exit 0; only a Bridge/transport
  failure exits 1.

**`item search-annotations`**
- Optional `QUERY` positional, default `""` (empty-string mode = browse all annotations, not an
  error); repeatable `--color`; `--limit` default `20`; `library_id` hardcoded `1`; bare JSON array
  output; `[]` success/exit 0; only transport failure exits 1.

**`item annotations`**
- Required raw `ITEM_KEY` — **no `catalog`/SQLite pre-resolution, no numeric-id support, no
  session-current-item fallback**, passed byte-for-byte to the Bridge; success payload is **either**
  `{count,annotations[]}` **or** a bare JSON string starting with `"ERROR: "`; the bare-string case
  still exits **0**; only a genuine Bridge/transport failure exits 1.

## 22. Dependencies / collision analysis

- **Dependency on Slice 3:** none semantically — no shared functions, no shared JS templates, no
  data coupling. Slice 3's `find_pdf`/`item_attach` primitives are untouched by this slice's design.
- **Dependency on Slice 4:** none semantically — confirmed in §13 (corrected): zero shared
  normalization, zero shared JS, zero shared Python functions between `core/notes.py` and the
  annotation/full-text code in `core/jsbridge.py`.
- **File-collision risk:** real, even without semantic coupling. Slice 4 (Notes Core) is very likely
  to touch `src/bridge/client.rs` and `src/bridge/templates.rs` too — Python's `add_note` calls
  `bridge.execute_js` with an inline JS body (not yet a named `JSBridgeClient` method in this port),
  so a faithful Slice 4 port would plausibly add its own `add_note`/`get_note` primitive to those
  same two shared files, exactly the files this Slice 5 spec also proposes extending.
- **Sequencing requirement, per instruction:** **implementation must not begin from a
  worktree/branch cut before Slice 4 merges to `origin/main`.** Start Slice 5's actual
  implementation from a fresh `origin/main` *after* Slice 4 lands, to avoid a
  `bridge/client.rs`/`bridge/templates.rs` merge conflict or wasted rebase.
  **Status: satisfied — Slice 4 (Notes Core) merged via PR #15 (commit `7c5bbb1`) prior to this
  implementation slice starting.**

## 23. Unresolved parity questions

1. **Fulltext result item type (attachment vs. parent bibliographic item):** ~~cannot be determined
   from source alone; requires the live acceptance check~~ **RESOLVED — see Frozen Live Evidence.**
2. **`annotationText` truncation and UTF-16 surrogate splitting:** truncation (`.substring(0, 200)`)
   happens **entirely inside Zotero's own JS**, using JS's UTF-16-code-unit semantics, before any
   response reaches Rust. Rust does not need to (and must not) re-implement truncation — it only
   needs to deserialize whatever the Bridge sends, including the rare edge case of a split surrogate
   pair mid-string. Flagged as a test case (mock a pre-split string and confirm `serde_json`
   deserializes it without panicking), not an implementation change.
3. **Negative/zero `--limit` and JS `Array.prototype.slice` semantics:** Python does no input
   validation beyond `type=int`. `limit=0` → `slice(0,0)` → `[]` (empty, not an error).
   **`limit` negative (e.g. `-1`) → JS's `slice(0, -1)` means "all elements except the last 1"** —
   not empty, not an error, a genuinely surprising exact behavior if ported naively. This must be
   preserved by passing the limit value through to the JS `slice()` call unchanged, not clamped or
   rejected on the Rust side, if true parity is desired — flagged as a deliberate decision point for
   the implementer, not silently "fixed" with input validation Python lacks.
4. **Hardcoded `library_id=1`:** whether the eventual CLI-integration slice should extend this to
   honor `session use-library` (a `Changed`-class enhancement, explicit product decision) or preserve
   the hardcoded value exactly (true parity) — unresolved, not decided by this audit.
5. **Raw-key-only `item annotations` input:** whether to preserve Python's no-numeric-id/no-session-
   fallback behavior exactly, or extend it to match this project's established
   `catalog::get_item`-based ref-resolution convention used everywhere else — unresolved, flagged for
   the CLI-integration slice, not decided here.
6. **JS-level `"ERROR: ..."` strings exiting 0:** confirmed exact and reproducible (§14), but flagged
   here as the single most likely point of accidental "improvement" during implementation — an
   implementer instinctively wanting to turn this into a real Rust error must be stopped; this is a
   deliberate parity requirement, not an oversight to fix.

---

## Frozen Live Evidence — PHASE 7 SLICE 5 LIVE FULLTEXT ACCEPTANCE (VERIFIED, AUTHORITATIVE)

Performed against a disposable Zotero profile/data directory (never the user's real library),
accepted by the user as authoritative evidence for this slice.

**Setup**
- Disposable profile: `/tmp/zotero_slice5_live_profile`; Zotero version `10.0.1`.
- Bridge ownership confirmed immediately before test data creation:
  `fork=="zotero-rust-cli"`, `id=="cli-bridge@cli-anything-rust.dev"`, `ownership:"verified"`.
- Query marker: a UUID embedded in a real text document, converted to a genuine PDF via
  `cupsfilter` (texttopdf), verified extractable with `pdftotext` before import (real PDF text
  stream, not just metadata).
- Test item created: bibliographic item **key=`EMI3S3GJ`** (itemID `5938`, itemType `document`,
  title "Zotero Rust CLI Slice 5 Live Test Item"), with the marker PDF attached via
  `Zotero.Attachments.importFromFile` → attachment **key=`RZ694UHL`** (itemID `5939`, itemType
  `attachment`).
- Indexing sanity: `Zotero.Fulltext.getIndexedState(attachment)` returned `3` (`INDEXED`,
  diagnostic-only call, not product code) before searching — the result reflects a genuinely
  indexed match, not an indexing-race artifact.

**Raw live search** — executed the verbatim JS body from `core/jsbridge.py::search_fulltext`:

```json
[{
  "id": 5939,
  "key": "RZ694UHL",
  "itemType": "attachment",
  "title": "PDF",
  "isAttachment": true,
  "parentItemID": 5938,
  "parentKey": "EMI3S3GJ",
  "pythonProjection": {"key": "RZ694UHL", "title": "PDF", "date": ""}
}]
```

**Resolution:** **Zotero returns the PDF ATTACHMENT item — definitively.** Not the parent
bibliographic item, and not context-dependent in any way observed. Exact Python-style projected
result (`{key, title, date}`):

```json
{"key": "RZ694UHL", "title": "PDF", "date": ""}
```

This is the attachment's own key, its generic Zotero-assigned title (`"PDF"`, not the paper's real
title), and an empty date (attachments don't carry a populated `date` field the way bibliographic
items do). An agent calling `item search-fulltext` and expecting the matched paper's
title/DOI/date would get none of that — only an attachment key and the literal string `"PDF"`.

**Binding constraint on implementation.** Because of this finding, the Slice 5 implementation
**MUST NOT**:
- resolve full-text hits to bibliographic parents;
- substitute parent titles;
- add DOI/date/metadata from the parent;
- add snippets/pages/scores;
- wait for indexing;
- retry because an item is not indexed yet.

Preserve the exact attachment-level Zotero result shape (`key`, `title`, `date` — nothing else),
matching §6 and §20 above.

**Python cross-check:** not performed (no local Python reference environment available) — not
needed for confidence, since the JS executed was the exact, unmodified body from
`core/jsbridge.py::search_fulltext` (same conditions, same call sequence, same projection), so this
live result **is** Python's own behavior, not an inference from it.

---

READ-ONLY AUDIT + FROZEN EVIDENCE. This document reflects the state of the design at the start of
implementation. `cli.rs`/`lib.rs` remain untouched by this slice per §16/§21.
