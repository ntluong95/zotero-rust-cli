---
phase: 6
title: "JS Bridge and Injection Hardening"
status: todo
priority: P1
effort: "6-8d"
dependencies: [5]
---

# Phase 6: JS Bridge and Injection Hardening

## Overview

Port the JavaScript **generation** layer of `core/jsbridge.py` and ship the XPI plugin. Delivers the
33 bridge-backed commands — the largest single block of functionality.

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

- [ ] All 33 bridge commands reach their target compatibility class
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
