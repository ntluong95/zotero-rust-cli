---
phase: 3
title: "CLI Skeleton, Result Contract and Config"
status: in-progress
priority: P1
effort: "5-7d"
dependencies: [1, 2]
---

# Phase 3: CLI Skeleton, Result Contract and Config

## Overview

Build the complete command tree (all 96 paths as stubs), the result envelope, exit-code mapping,
output/encoding rules, error routing, environment/config resolution, and Zotero path discovery.

**This phase defines the contract every later phase must satisfy.** It blocks everything downstream.

### Sequencing deviation: vertical slice, not stub-everything-first

Decided after Phase 2 shipped: rather than stubbing all 96 command paths before any real behavior
lands (this phase's original plan), Phases 3-5 are being built as a **vertical slice** — the runtime
foundation plus a minimal read-only command set, each command proven against the Python oracle via
the Phase 1 harness before being marked migrated. Rationale: surfaces integration risk (path
discovery, SQLite queries, JSON key order, HTTP client behavior) against real, verified end-to-end
behavior immediately, instead of discovering it late across 96 simultaneously-stubbed commands.

**Landed so far** (verified **Exact** via `harness/compare.py` — see
`plans/reports/compatibility-matrix.md`'s "Migration Progress" section for the authoritative,
per-command tracker): `app status`, `item list`, `item get`, `item find`, `collection list`.

Runtime foundation landed as flat modules in `crates/zotero-cli/src/` (not the `cli/mod.rs` +
`cli/global.rs` + `cli/emit.rs` submodule split originally sketched below — flat files were sufficient
at this scope and were simpler to keep exactly traceable to the Python source file each one ports):
`paths.rs`, `session.rs` (read path only — no `save`/`append` yet), `http.rs`, `db.rs`, `catalog.rs`,
`runtime.rs`, `output.rs` (the `emit.rs` equivalent), `error.rs`, `cli.rs`.

**Not yet done** (still applies to the rest of this phase's scope): `result.rs`/`ResultPayload`
(none of the 5 landed commands use `result_payload` — all are raw-output Exact commands), the full
17-environment-variable `config.rs` inventory (only the 4 already needed —
`ZOTERO_DATA_DIR`/`ZOTERO_PROFILE_DIR`/`ZOTERO_EXECUTABLE`/`ZOTERO_HTTP_PORT`/
`CLI_ANYTHING_ZOTERO_STATE_DIR` — are wired), `NOT_IMPLEMENTED` stubs for the other 91 v1 leaves,
the deferred/dropped-command visibility inventory, and the Windows console-encoding fallback (see
`output.rs`'s own doc comment: Rust's stdout has no `UnicodeEncodeError`-equivalent failure mode,
so the fallback branch may not be portable/needed as originally scoped — revisit once Windows CI
runs the real binary rather than assuming this is still required as designed below).

## Requirements

**Functional**
- All v1 command paths parse and produce correct help text; deferred/dropped paths are either absent
  from the generated agent skill or emit explicit `DEFERRED` / `DROPPED` diagnostics that cannot be
  mistaken for available functionality
- `--json` accepted at root, group **and** command level
- Root flags: `--backend {auto,sqlite,api}`, `--data-dir`, `--profile-dir`, `--executable`, `--version`
- Result payload helper `{action, ok, status, code?, error?, ...}` with Python-identical key order
  only for commands that use `core.results.result_payload`; raw-list/raw-object commands stay raw
- Exit-code mapping identical to `results.exit_code_for`
- Full environment-variable resolution
- Zotero path discovery: profile root, active profile, data dir, executable, version, HTTP port,
  Local API pref
- Bare invocation prints help and exits 0 (approved intentional break)

**Non-functional**
- Cold start under 10 ms for `--help` and any stub command
- stdout carries only command output; all diagnostics go to stderr

## Architecture

```
crates/zotero-cli/src/
  main.rs            # entry, dispatch, top-level error → exit code
  cli/
    mod.rs           # clap derive tree: 14 groups + 3 root commands
    global.rs        # --json at any level, root flags, RootCliConfig
    emit.rs          # emit / emit_js equivalents, encoding fallback
  result.rs          # ResultPayload, exit_code_for, normalize_if_exists
  error.rs           # CliError (thiserror) → result envelope + exit code
  config.rs          # env vars, defaults, RootCliConfig
  paths.rs           # port of utils/zotero_paths.py
```

### `--json` at any level

Python implements this with a custom `_JsonAwareGroup` that strips `--json` from sub-level args and
pushes it to the root context (`zotero_cli.py:63-143`). In `clap` the equivalent is a **global
argument**:

```rust
#[arg(long, global = true, help = "Emit machine-readable JSON.")]
json: bool,
```

`global = true` makes `--json` valid at every level and hoists it to the root matches — the same
observable behaviour with none of the custom parser machinery.

> The other four root flags are **not** global in Python — they are root-only. Do not mark them
> `global`; that would accept `item find X --data-dir /tmp`, which Python rejects.

### Result payloads, raw outputs and key order

Python does **not** use one universal result envelope. Many Exact read commands call `emit(...)` on
raw catalog arrays or objects, including collection list/find/tree/get/items and item list/find/get.
Wrapping those outputs would fail parity. Use a command-output contract table from the Phase 1
golden capture:

| Output shape | Rule |
|---|---|
| Raw list/object/string | Emit exactly the Python shape; no synthetic `action`, `ok`, or `status` |
| `result_payload` object | Preserve helper key order and `exit_code_for` semantics |
| JS bridge transport | Preserve `emit_js` transport/nested-failure behavior |
| Usage/runtime error in `--json` mode | Emit `{"error": "..."}` to stdout |

```rust
#[derive(Serialize)]
pub struct ResultPayload {
    pub action: String,
    pub ok: bool,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")] pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub error: Option<String>,
    #[serde(flatten)] pub extra: serde_json::Map<String, Value>,
}
```

For result-payload commands, field order `action, ok, status, code, error, ...extra` reproduces
`results.result_payload` exactly. `serde_json` must be built with the `preserve_order` feature so
`extra` retains insertion order.

Exit-code mapping (`results.py:40-47`):

```
ok == false                                             → 1
status ∈ {partial_success, error, failed, timeout}      → 1
otherwise                                               → 0
```

### Error routing — preserve the quirk

| Mode | Destination | Format |
|---|---|---|
| `--json` | **stdout** | `{"error": "<message>"}` |
| human | stderr | Click-style `Error: <message>` / usage errors |

This is D6. It looks wrong and it is deliberate — agents parse stdout. Document it; do not "fix" it.

### JSON encoding fallback

Python: `json.dumps(data, ensure_ascii=False, indent=2)`, falling back to `ensure_ascii=True` when
stdout's encoding cannot represent the text (`zotero_cli.py:173-177`). Rust equivalent: serialize
with 2-space pretty printing and non-escaped Unicode; on Windows, detect the console code page and
fall back to `\uXXXX` escaping when the text is not encodable.

### Path discovery (`paths.rs`)

Direct port of `utils/zotero_paths.py`. Precedence must match exactly:

| Setting | Precedence |
|---|---|
| Profile root | `--profile-dir` → `ZOTERO_PROFILE_DIR` → platform candidates (`%APPDATA%/Zotero/Zotero`, `~/AppData/Roaming/Zotero/Zotero`, `~/Library/Application Support/Zotero`, `~/.zotero/zotero`) |
| Data dir | `--data-dir` → `ZOTERO_DATA_DIR` → `prefs.js` `extensions.zotero.dataDir` (only when `useDataDir=true` **and** the path exists) → `~/Zotero` |
| Executable | `--executable` → `ZOTERO_EXECUTABLE` → `PATH` lookup (`zotero`, `zotero.exe`) → hardcoded candidates |
| HTTP port | `ZOTERO_HTTP_PORT` → `prefs.js` `httpServer.port` → `23119` |

`profiles.ini` parsing must reproduce `find_active_profile`: prefer the section with `Default=1`,
otherwise the first parseable `Profile*` section; honour `IsRelative`.

Pref file reading tries `utf-8`, `utf-8-sig`, then `latin-1` (`_read_pref_file`) — replicate the
fallback chain, and replicate `_decode_pref_string` (`\\`→`\`, `\"`→`"`).

> **Config directory:** Python hardcodes `Path.home() / ".config" / "cli-anything-zotero"` on **all**
> platforms, including Windows (`session.py:14-18`). Do **not** use a platform-native config dir — it
> would move the session file on Windows and break existing users.

### Deferred and dropped command visibility

Do not make deferred/dropped commands look usable to agents. Phase 3 may keep parser entries only
when they are needed for compatibility diagnostics, but Phase 11's generated `SKILL.md` must exclude
them from normal command examples. If retained in the binary, they must return one of:

| Class | Code | Message requirement |
|---|---|---|
| Deferred | `DEFERRED` | Name the Phase 12 gate and the Python fallback/deprecation decision |
| Dropped | `DROPPED` | Name the replacement behavior, e.g. bare invocation help instead of REPL |

`NOT_IMPLEMENTED` is for a porting gap inside the accepted v1 surface, not for intentionally
deferred or dropped functionality.

## Related Code Files

- Create: `crates/zotero-cli/src/main.rs`, `cli/mod.rs`, `cli/global.rs`, `cli/emit.rs`
- Create: `crates/zotero-cli/src/result.rs`, `error.rs`, `config.rs`, `paths.rs`
- Create: `crates/zotero-cli/tests/cli_surface.rs`, `tests/paths.rs`, `tests/result_contract.rs`
- Reference: `reference/.../zotero_cli.py` (lines 26–415, 2657–2681), `core/results.py`, `utils/zotero_paths.py`

## Implementation Steps

1. Define the `clap` derive tree for all 14 groups and 96 leaves. Command names, argument names,
   flag spellings, defaults and `Choice` value sets come from `compatibility-matrix.md` — transcribe,
   do not improvise.
2. Implement `--json` as a global arg; keep the four root flags root-only.
3. Implement `result.rs` with `ResultPayload` and `exit_code_for`; unit-test the exit-code table
   exhaustively.
4. Implement `error.rs` mapping every error class to `(payload, exit_code, stream)`.
5. Implement `emit.rs`: JSON mode, human mode, list/dict/string handling, encoding fallback.
6. Implement `config.rs` covering all 17 environment variables.
7. Port `paths.rs` with the precedence tables above.
8. Implement bare-invocation → help + exit 0.
9. Every unimplemented v1 leaf returns
   `{"action": "<cmd>", "ok": false, "status": "error", "code": "NOT_IMPLEMENTED"}` with exit 1, so
   the parity harness can distinguish "not yet ported" from "ported and wrong".
10. Add an inventory test that classifies each command as v1, Deferred, Dropped, or Changed, and
    asserts the generated agent skill includes only the intended public v1 surface plus warnings for
    documented exceptions.
11. Wire the Phase 1 harness to run against the Rust binary; expect `NOT_IMPLEMENTED` for v1 leaves and
    **Exact** on `--help` structure and path/config resolution.
12. Benchmark cold start; assert under 10 ms in CI.

## Success Criteria

- [ ] All v1 command paths parse; deferred/dropped command visibility is explicitly classified
- [ ] `item find X --json`, `item --json find X`, and `--json item find X` all behave identically
- [ ] Root-only flags are rejected at sub-levels, matching Python
- [ ] Raw-output commands stay raw; result-payload commands preserve `result_payload` key order
- [ ] `exit_code_for` table matches `results.py` for every result-payload status value, verified by unit test
- [ ] JSON-mode errors go to stdout; human-mode errors go to stderr — asserted in tests
- [ ] Path discovery matches Python on all four Phase 1 fixture states, on all three OSes
- [ ] Session file resolves to `~/.config/cli-anything-zotero/session.json` on Windows too
- [ ] Bare invocation prints help and exits 0
- [ ] Cold start under 10 ms
- [ ] Harness runs end-to-end against the Rust binary and reports a clean `NOT_IMPLEMENTED` baseline for accepted v1 leaves

## Risk Assessment

| Risk | Mitigation |
|---|---|
| Transcription errors across 96 commands and ~250 flags | Generate the clap tree skeleton from `compatibility-matrix.md` programmatically, then hand-review; add a test asserting the Rust command inventory equals the matrix |
| `clap` help output differs from Click's | Help text is **not** a compatibility contract — only structure is. Assert that every command exists, not that help renders identically |
| `serde_json` key order drift | Enable `preserve_order`; add a golden test on envelope ordering |
| Raw read commands accidentally wrapped in the result payload | Generate per-command output-shape tests from Phase 1 golden files |
| Windows console encoding fallback wrong | Dedicated Windows CI test writing CJK to a cp1252 console |
| Platform-native config helpers silently changing the config path | Explicitly forbidden above; add a test asserting the literal `~/.config/cli-anything-zotero` path on Windows |
| Deferred/dropped commands appear in generated agent docs as usable commands | Phase 11 generator filters by compatibility class and includes warning-only entries for exclusions |
