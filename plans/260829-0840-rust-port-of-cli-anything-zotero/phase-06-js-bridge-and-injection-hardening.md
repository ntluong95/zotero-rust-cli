---
phase: 6
title: "Write Backends: Local-API-First"
status: todo
priority: P1
effort: "6-8d"
dependencies: [5, 14]
---

# Phase 6: Write Backends: Local-API-First (formerly "JS Bridge and Injection Hardening")

> ## ⚠️ RECONSTRUCTED 2026-08-29 — MATERIALLY INCOMPLETE, NOT A VERBATIM RECOVERY
>
> This phase file's Zotero-10 redesign (the actual committed diff was **475 changed lines** —
> effectively a full rewrite of this document, per `git diff --stat` captured before the loss) was
> destroyed by an uncoordinated concurrent git operation before it was committed anywhere. It could
> not be read back from any commit, branch, or session-context capture — unlike `plan.md`,
> `phase-05`, and `phase-14`, this file was never read in full after its Zotero-10 update landed.
>
> What follows is rebuilt **only** from what `plan.md` and `phase-14-zotero-10-compatibility-gate.md`
> cite about this phase (both recovered verbatim), plus the original pre-redesign content below,
> which is a real, unmodified recovery of the base file. The result is an honest **skeleton**, not a
> restoration: it correctly states the redesign's *shape* (Local-API-first routing, ~10 privileged
> JS Bridge commands instead of ~33, the C1/C2/C3 concepts named in `plan.md`'s red-team log) but
> does **not** contain whatever detailed command-by-command backend-routing table, JS template diffs,
> or XPI manifest specifics the original 475-line version worked out. **Do not begin Phase 6
> implementation from this file alone** — the missing detail needs fresh design work, informed by
> Phase 14's live-Zotero findings, before this phase can be executed responsibly. Treat every claim
> below as either (a) recovered verbatim and cited, or (b) explicitly marked reconstructed.

## Overview

**Redesigned for Zotero 10** (per `plan.md`'s Phases table and Zotero 10 impact table, both recovered
verbatim): Zotero 10's Local API gained write support (POST/PUT/PATCH/DELETE, tag delete, full-text,
file upload — `plan.md` Finding 3, class OPPORTUNITY). This phase now routes writes **Local-API-first
on Zotero 10+**, falling back to the JS Bridge — shrunk from ~33 commands to **~10 privileged
operations** the Local API cannot express — on Zotero ≤9 or wherever Local API genuinely cannot do
the job. `plan.md`'s red-team adjudication (Finding 17, withdrawn) is explicit that this does **not**
force a parity downgrade: parity is a property of the *observable CLI contract*, not the transport,
so Exact stays achievable behind a compatibility renderer that makes both backends produce the same
JSON shape — see §C2 below.

This phase depends on **Phase 14 passing first** (`dependencies: [5, 14]`): Phase 14's capability
detection (`Zotero-Server-ID` header, `local_api_writes_available` on `RuntimeContext`) is exactly
what this phase's routing decision needs, and Phase 14 also owns the XPI manifest's `strict_max_version`
bump to `10.0.*` that this phase's JS Bridge fallback path depends on to load at all on Zotero 10.

**D1 (JS-injection) still applies, but its scope shrinks with the command count.** Whatever remains
on the JS Bridge path (the ~10 privileged operations) must still fix D1 structurally — see §D1 fix
below, recovered verbatim from the pre-redesign version of this file, which remains valid for those
remaining bridge commands.

### §C1 — Local API authorization and key persistence (gates the whole redesign)

**[RECONSTRUCTED]** `plan.md`'s red-team Finding 18 names this "the only genuine reversal trigger":
Zotero 10's Local API write path requires a user-facing key/consent dialog (`plan.md` Finding 4). If
"Always Allow" does not persist across a Zotero restart — Open Question 9 in Phase 14 — then
unattended agent writes are not viable via Local API at all, and the JS-Bridge-first design (the
*original*, pre-redesign Phase 6 below) would be correct after all. **This phase must not proceed
past its design stage until Phase 14's OQ9 has a live-verified answer.** The exact key-storage
mechanism, where the key lives on disk, and how `zotero-cli` should re-authenticate after a restart
are not specified further here — original detail lost, needs fresh design against Phase 14's
findings.

### §C2 — Compatibility renderer (keeps Exact achievable across two backends)

**[RECONSTRUCTED]** Per `plan.md`'s Finding 17b (the surviving, inverted risk): a renderer that makes
the JS-Bridge and Local-API write paths emit byte-identical CLI JSON is necessary for correctness,
not just cosmetics — the risk is a renderer that achieves output parity while leaving Zotero in
**different states** across the two backends. Concretely, Zotero 10's Local API object `version`
field means something different from the Web API's / the JS Bridge's notion of `version` (per
`plan.md`'s Finding 17 adjudication note), so a naive pass-through would leak a backend-specific
value into the CLI's output. Success requires **post-write state parity** (re-reading the item after
the write and confirming the same logical result), not merely matching JSON shapes. The specific
field-by-field mapping between the Local API's write-response shape and the JS Bridge's
`OK: <title>`-style return strings is not recoverable from session context and needs fresh design.

### §C3 — Command routing (Local API vs. JS Bridge)

**[RECONSTRUCTED — HIGH-LEVEL ONLY, NOT A COMMAND TABLE]** The original file almost certainly
contained a per-command backend-routing table (which of the ~33 original bridge commands move to
Local API vs. which ~10 stay on the JS Bridge, and why). That table is not recoverable. What can be
stated with confidence from `plan.md`'s citations: routing should **attempt Exact first, downgrade
only on demonstrated evidence** (the corrected rule that replaced withdrawn Finding 17); the ~10
JS-Bridge-only operations are described only as "privileged ops" the Local API cannot express, which
plausibly includes `js` (the raw privileged escape hatch — never a Local API candidate by
definition), `item move-to-collection` (needs the transactional add+remove-in-one-operation
semantics the original design already required), and other operations Zotero's Local API simply
does not expose. **This table must be rebuilt from scratch during Phase 6 planning**, cross-checked
against Zotero 10's actual documented Local API surface, before implementation starts.

---

## Pre-redesign content (recovered verbatim from the last committed version)

Everything below this line is the phase's original, pre-Zotero-10-redesign content, recovered from
`origin/main` (unmodified by the loss — this base version was never uncommitted). It describes the
**JS-Bridge-first** design the redesign above supersedes as the *primary* routing decision, but it
remains materially relevant: the D1/D3 fixes, the AppleScript removal, the XPI packaging/fork
mechanics, and the `item move-to-collection` composition design still apply to whichever commands end
up on the JS Bridge path after §C3's routing table is rebuilt. **Do not delete this section when
filling in the redesign** — it is real, verified content, not a placeholder.

Port the JavaScript **generation** layer of `core/jsbridge.py` and ship the XPI plugin. Delivers the
33 bridge-backed commands — the largest single block of functionality *under the original,
pre-Zotero-10 design*; the redesign above shrinks this to ~10.

The XPI itself is reused byte-for-byte. What is ported is the ~500 lines of JS source templates that
Python builds as strings and POSTs to `/cli-bridge/eval`.

**This phase also fixes D1**, the JS-injection defect, structurally rather than by patching escape
functions.

## Requirements

**Functional**
- `execute_js` transport with the `{ok, data, error}` envelope, including `error_name`, `error_stack`, `error_raw` passthrough
- `execute_js_http_required` strict variant
- All bridge operations: item update/tag/delete/attach/find-pdf/annotations, collection CRUD/stats,
  duplicates, fulltext + annotation search, DOI/PMID import, sync, raw `js`
- XPI build, install, uninstall, version detection, `plugin-status`
- `emit_js` semantics including `require_data` and nested application-level failure detection
- `item move-to-collection` implemented as one generated JS transaction-equivalent operation, not a chain of separate bridge saves

**Non-functional**
- Parameters containing `\`, `'`, `"`, newlines, `</script>` or CJK must not corrupt the generated JS
- One endpoint probe per process, not per call

## Architecture

```
crates/zotero-cli/src/
  bridge/
    mod.rs           # BridgeClient, transport, envelope
    js/              # JS templates as include_str! assets
      item_update.js
      item_tag.js
      collection_create.js
      import_doi.js
      ...
    templates.rs     # parameter binding
  plugin/
    mod.rs           # XPI build/install/uninstall/version
    assets/
      manifest.json  # forked: update_url + addon id changed
      bootstrap.js   # upstream runtime logic, plus optional ownership marker
```

### D1 fix — parameters via `JSON.parse`, not string concatenation

Python builds JS by interpolation, escaping only `'`:

```python
# core/jsbridge.py:360-374  — BROKEN for backslashes, newlines, quotes
set_lines = " ".join(
    f"item.setField('{k}', '{v.replace(chr(39), chr(92) + chr(39))}');"
    for k, v in fields_dict.items()
)
```

A title containing `C:\Users\x` or a newline produces malformed or injected JavaScript. Upstream
patched exactly one instance of this for Windows paths in `attach_pdf` (issue #4); the class of bug
remains everywhere else.

**The port passes all parameters as a single JSON-encoded argument:**

```rust
let params = serde_json::json!({ "libraryID": library_id, "key": item_key, "fields": fields });
let code = format!(
    "const P = JSON.parse({});\n{}",
    serde_json::to_string(&serde_json::to_string(&params)?)?,  // JSON string literal
    include_str!("js/item_update.js")
);
```

```js
// js/item_update.js — no interpolation anywhere
var item = Zotero.Items.getByLibraryAndKey(P.libraryID, P.key);
if (!item) { return 'ERROR: item ' + P.key + ' not found'; }
for (const [k, v] of Object.entries(P.fields)) { item.setField(k, v); }
await item.saveTx();
return 'OK: updated ' + item.getField('title').substring(0, 60);
```

`serde_json` produces a correctly-escaped JSON string literal for **any** input, so the injection
class disappears rather than being narrowed. JS templates become static assets that can be linted
and reviewed.

> **Return values must stay byte-identical.** Strings like `'OK: updated '`, `'ERROR: item {key} not found'`,
> `'FOUND: '`, `'NOT_FOUND: '`, `'TIMEOUT: '`, `'DELETED: '` are parsed by Python callers and by the
> compatibility harness. Change the parameter passing; do not change the output strings.

### D3 fix — cache positive probes only

`execute_js` calls `bridge_endpoint_active()` before every call (`jsbridge.py:289`), doubling round
trips across all 33 commands. Cache only a **successful** probe in a `OnceCell` for the process
lifetime. Do not permanently cache a negative result: install, launch, or registration flows can make
the endpoint appear later in the same process.

### AppleScript fallback — dropped

`_execute_applescript` (`jsbridge.py:153-212`) drives Zotero's "Run JavaScript" dialog through
`osascript`, keyed on **localized menu names** (`_MENU_PATHS` has only `en` and `zh`). It is
macOS-only, deprecated upstream, and superseded by the XPI.

When the endpoint is inactive, return Python's existing non-macOS error verbatim:

```
JS Bridge endpoint not available. Install the CLI Bridge plugin: zotero-cli app install-plugin, then restart Zotero.
```

This is a behaviour change on macOS only, in a deprecated path, and it fails **loudly** rather than
silently automating the GUI. Document it in the migration guide.

### XPI packaging and the fork problem

`manifest.json` currently declares:

```json
"id": "cli-bridge@cli-anything.dev",
"update_url": "https://raw.githubusercontent.com/PiaoyangGuohai1/cli-anything-zotero/main/update.json"
```

**[RECONSTRUCTED note]** `strict_max_version` is currently `9.0.*`, which makes this manifest
unloadable on Zotero 10 (`plan.md` Finding 2, CRITICAL) — bumping it to `10.0.*` is owned by
**Phase 14** (it must happen before this phase's XPI work is meaningful on Zotero 10 at all), not
duplicated here; this phase owns the id/`update_url` fork changes below.

Shipping this unchanged would let **upstream** push plugin updates to this fork's users. Both fields
must change. The addon id also determines the installed XPI filename
(`<profile>/extensions/<id>.xpi`), so changing it means a user can have both plugins installed —
which is safer than silent clobbering, but `plugin-status` must then report clearly which one is
active. Resolve alongside plan Open Question 2.

`bootstrap.js` itself contains no Zotero domain logic and is a generic `eval` endpoint. If both the
upstream and forked plugins may be installed, byte-identical bootstrap code cannot prove which plugin
owns `/cli-bridge/eval`: both extensions register and delete the same `Zotero.Server.Endpoints` key.
Phase 6 must choose one of these before shipping:

| Option | Requirement |
|---|---|
| Single-plugin policy | Detect upstream XPI and require uninstall before installing the fork |
| Ownership marker | Add a minimal endpoint/version marker in bootstrap and update the byte-identity claim accordingly |

`plugin-status` must verify active endpoint ownership through the endpoint itself, not only by
checking XPI files on disk.

### Security note

The XPI grants arbitrary privileged code execution inside Zotero to any local process that can reach
`127.0.0.1:23119`. That is upstream's existing posture and the port inherits it. Do **not** silently
widen it: keep the endpoint bound to loopback, keep `permitBookmarklet: false`, and document the
exposure in `docs/SECURITY.md`.

The raw `js` command is the intentional escape hatch and cannot use parameter binding. It must stay
out of normal agent examples, be marked privileged in generated skill docs, and require explicit user
intent in any higher-level agent workflow.

## Related Code Files

- Create: `src/bridge/mod.rs`, `templates.rs`, `src/bridge/js/*.js`
- Create: `src/plugin/mod.rs`, `src/plugin/assets/{manifest.json,bootstrap.js}`
- Create: `tests/bridge_templates.rs`, `tests/bridge_injection.rs`, `tests/plugin_xpi.rs`
- Reference: `core/jsbridge.py`, `core/hygiene.py`, `utils/zotero_paths.py` (lines 292–383)

## Implementation Steps

1. Implement the transport: POST `text/plain` to `/cli-bridge/eval`, map 200 → `{ok:true,data}`,
   500 → structured error with `error_name`/`error_stack`/`error_raw`, timeouts → `"timed out: ..."`.
2. Implement `_format_bridge_error` equivalent — the `error`/`message`/`raw`/`name` fallback chain
   producing `"unknown bridge error"` rather than `"{}"`.
3. Extract every JS template from `jsbridge.py` into `src/bridge/js/*.js` as static assets,
   converting interpolation to `P.*` references. Preserve every return string exactly.
4. Implement `templates.rs` binding params via the `JSON.parse` prologue.
5. Implement `emit_js`: transport failure → emit result, exit 1; `require_data` with null data →
   `EMPTY_RESULT`; nested `data.ok == false` → emit `data`, exit 1.
6. Cache successful endpoint probes only (D3); retest after install/launch/register actions before
   returning endpoint-unavailable errors.
7. Port `find_pdf`'s two-stage retry: on timeout, issue the secondary attachment-check JS.
8. Port `find_pdfs_in_collection` per-item loop with its progress summary shape
   (`total_in_collection`, `checked`, `found`, `not_found`, `timeouts`, `errors`, `details`, `strategy`, `timeout_per_item`).
9. Port `core/hygiene.py` duplicate detection and merge preview/execute.
10. Implement XPI build (`zip` crate: `manifest.json` + `bootstrap.js` at archive root), install to
    `<profile>/extensions/<addon-id>.xpi`, uninstall, and version read from the installed XPI.
11. Implement `app install-plugin` including the programmatic `AddonManager` install path — and note
    that this path itself embeds a file path in JS, so it must use the `JSON.parse` mechanism too.
12. Handle the three `--experimental` commands. **These are not uniform — verify before coding:**
    - `collection create` and `item add-to-collection` already default to the bridge upstream
      (`zotero_cli.py:810`, `:1352`). v1 simply omits the `--experimental` flag; the default path is
      unchanged.
    - `item move-to-collection` has **no bridge path**: `_require_experimental_flag` fires
      unconditionally (`zotero_cli.py:1371`) and it is implemented only as a direct SQLite write.
      v1 must implement one dedicated JS template that adds the target and removes source
      memberships inside one Zotero-side operation, with rollback/compensation if any save fails.
      It may reuse logic from the existing primitives but must not call them as separate bridge
      round trips. Honour `--from` (repeatable) and `--all-other-collections`.
      This is new work and an approved behaviour change: it gains the ability to run while Zotero is
      running and no longer requires `--experimental`. Classify as **Changed**, not Semantic.
13. Replace fake-bridge-only validation with two layers: render/lint every static JS template offline,
    then smoke-test all bridge templates against a real Zotero or a JS harness that exposes the
    required Zotero object methods. The existing fake server is not enough because it pattern-matches
    only a subset of code strings.
14. Add the injection regression suite using the Phase 1 `unicode-cjk` fixture.

## Success Criteria

- [ ] **[RECONSTRUCTED]** Phase 14 has passed (its success criteria are all checked) before any
      implementation in this phase starts — the dependency graph in `plan.md` makes this a hard gate
- [ ] **[RECONSTRUCTED]** Local API write routing implemented for Zotero 10+ (detected via
      `RuntimeContext.local_api_writes_available` from Phase 14), JS Bridge routing preserved for
      Zotero ≤9 and the ~10 privileged operations Local API cannot express (§C3 table, rebuilt fresh)
- [ ] **[RECONSTRUCTED]** Compatibility renderer (§C2) achieves output parity **and** verified
      post-write state parity across both backends for every command that supports both
- [ ] **[RECONSTRUCTED]** Open Question 9 (does "Always Allow" persist across a Zotero restart)
      answered before unattended-agent-write claims are made for the Local API path (§C1)
- [ ] All ~10 remaining JS-Bridge-routed commands (down from 33 under the original design) reach their target compatibility class
- [ ] **Injection suite passes**: titles, tags and collection names containing `\`, `'`, `"`, newline, `</script>`, `${}` and CJK round-trip correctly through every write command
- [ ] The same adversarial inputs demonstrably **break** the Python implementation — recorded as evidence that D1 is fixed, not merely avoided
- [ ] Bridge return strings byte-identical to Python (`OK: `, `ERROR: `, `FOUND: `, `NOT_FOUND: `, `TIMEOUT: `, `DELETED: `)
- [ ] Successful endpoint probed at most once per process; negative probes are retried after install/launch/register actions
- [ ] `emit_js` `require_data` and nested-failure semantics match Python
- [ ] XPI builds and installs; `plugin-status` reports installed/active/version/update-available and active endpoint ownership correctly
- [ ] `bootstrap.js` domain logic is byte-identical to upstream, or a minimal endpoint ownership marker is documented and hash-tested
- [ ] `manifest.json` `update_url` and addon id changed; no path resolves to upstream's update feed; duplicate-plugin behavior is deterministic
- [ ] macOS AppleScript removal produces the documented error, not a silent failure
- [ ] `item move-to-collection` works via one Zotero-side operation with Zotero **running**, honouring `--from` and `--all-other-collections`, and no longer requires `--experimental`
- [ ] `collection create` and `item add-to-collection` behave identically to Python's default (non-`--experimental`) path
- [ ] `docs/SECURITY.md` documents the eval-endpoint exposure

## Risk Assessment

| Risk | Mitigation |
|---|---|
| A JS template is transcribed with subtly different semantics | Templates are extracted as whole units, not retyped; diff each against the Python f-string with interpolation removed |
| `JSON.parse` prologue breaks Zotero's `eval` wrapper (`(async () => {...})()`) | The prologue is plain `const` + `JSON.parse`, valid inside an async IIFE; covered by a live smoke test |
| Return-string drift breaks callers | Golden tests assert exact prefixes |
| Dropping AppleScript regresses a real macOS user | Deprecated upstream; fails loudly with actionable instructions; documented in the migration guide |
| Addon-id change creates ambiguous endpoint ownership | Either enforce a single-plugin policy or add an endpoint ownership marker; `plugin-status` verifies active ownership via HTTP |
| Two-stage `find_pdf` retry timing differs | Timeouts transcribed as named constants; behaviour asserted against the fake server |
| Fake bridge passes broken JS templates | Add offline JS rendering/linting plus real Zotero or Zotero-object-harness smoke tests |
