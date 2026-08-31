---
phase: 12
title: "DOCX Zoterify Chain (Deferred, Gated)"
status: deferred-post-v1
priority: P3
effort: "8-12d (if approved)"
dependencies: [10]
tracking_issue: https://github.com/ntluong95/zotero-rust-cli/issues/30
---

# Phase 12: DOCX Zoterify Chain (Deferred, Gated)

> **DEFERRED TO POST-v1 (2026-08-31).** The go/no-go gate below was **not run**
> for v1.0.0. The maintainer's decision was to ship v1.0.0 on the static OOXML
> DOCX surface rather than block the release on LibreOffice/Java/GUI automation
> work. Tracked in
> [issue #30](https://github.com/ntluong95/zotero-rust-cli/issues/30).
>
> **Supported in v1.0.0** (pure OOXML — no Word, no LibreOffice):
> `docx inspect-citations`, `docx inspect-placeholders`,
> `docx validate-placeholders`, `docx render-citations`.
>
> **Deferred:** `docx cite`, `docx doctor`, `docx insert-citations`,
> `docx prepare-zotero-import`, `docx zoterify`, `docx zoterify-preflight`,
> `docx zoterify-probe`.
>
> The seven commands are absent from the CLI, not stubbed. This deferral changes
> no canonical accounting: they have been classified `Deferred` since the
> original compatibility matrix, and the v1.0.0 totals remain
> Integrated 86 · Missing 0 · Changed 1 · Excluded 1 · Dropped 1 · Deferred 7 ·
> Total 96. Everything below is the still-valid plan for *if* this phase is
> picked up.

## Overview

The seven DOCX commands that require LibreOffice, Java and macOS AppleScript GUI automation.
Excluded from v1 at the challenge gate as the highest-risk area in the port.

**This phase does not start until its go/no-go gate passes.** It may legitimately end in a decision
to deprecate these commands rather than port them.

## Go / No-Go Gate

Evaluate **after Phase 10**, with real data. Proceed only if all of these hold:

| # | Criterion | Why it matters |
|---|---|---|
| 1 | Real users or agent workflows actually invoke these commands | Upstream's own CLI says `prepare-zotero-import` "has failed in Zotero 9 + LibreOffice testing" — this may be dead functionality |
| 2 | The chain works reliably **in Python** on at least two platforms today | Porting a broken workflow reproduces the breakage in a new language |
| 3 | A DOCX corpus with verified expected outputs exists | Phase 9 built the corpus; this needs the dynamic-field expectations too |
| 4 | The maintainer accepts an ongoing LibreOffice/Java/Zotero-version compatibility burden | This chain breaks on every LibreOffice and Zotero major release |
| 5 | No adequate alternative | `docx render-citations` (static, Phase 9) may already satisfy most users |

**If the gate fails:** formally deprecate the seven commands or keep them outside the Rust v1
end-user path with explicit legacy labeling. A Python-only fallback may exist for maintainers or
legacy users during coexistence, but it does **not** satisfy the Rust distribution promise and must
not be counted as a no-Python end-user path in Phase 13.

## Requirements (if approved)

**Functional**
- `docx zoterify` — placeholders → live Zotero LibreOffice fields
- `docx zoterify-preflight`, `docx zoterify-probe`, `docx doctor` — readiness checks
- `docx cite` — one-shot pipeline
- `docx insert-citations`
- `docx prepare-zotero-import` — port last, or drop

**Non-functional**
- Must fail with actionable diagnostics when LibreOffice, Java or the Zotero plugin is missing
- Must never leave a partially-converted document without saying so

## Architecture

```
crates/zotero-cli/src/docx/
  zoterify/
    mod.rs
    probe.rs       # Java, LibreOffice, Zotero plugin detection
    fields.rs      # Zotero field/bookmark/custom-property construction
    csl_json.rs    # csl-citation.json payload construction
    driver.rs      # LibreOffice process + AppleScript automation
  pipeline.rs      # docx cite
```

### External dependency detection

Port `_check_java` and `_check_libreoffice` (`docx.py:507-560`):
- `java` and `javac` on `PATH`
- macOS `/usr/libexec/java_home`
- `soffice` discovery across platform-specific locations
- Zotero LibreOffice plugin presence, checked via the JS bridge

Each check reports `{ok, path, version, ...}` and the aggregate gates conversion.

### The automation problem

`docx_zoterify.py` drives LibreOffice through:
- `subprocess.Popen([soffice, path])` / `open -a LibreOffice` on macOS
- `osascript` with generated AppleScript to click through Zotero's LibreOffice integration

This is macOS-centric GUI automation. It is inherently fragile: it depends on window titles, menu
structure, timing delays, and UI locale.

**If this phase proceeds, treat platform support honestly:** macOS is the only platform where the
AppleScript path exists today. Windows and Linux would need a different mechanism (LibreOffice
Basic macro or UNO bridge) that does not exist in the Python source and would be **new work, not a
port**. Do not present it as parity.

### Field construction

The genuinely portable part: building Zotero field codes, `ZOTERO_BREF_*` bookmarks, custom document
properties, and `csl-citation.json` payloads conforming to
`https://github.com/citation-style-language/schema/raw/master/csl-citation.json`. This is
`zipfile` + XML + JSON work and is testable without LibreOffice.

**Sequence the phase accordingly:** port field construction first and test it standalone; attempt
driver automation only after that is proven.

## Related Code Files

- Create: `src/docx/zoterify/{mod,probe,fields,csl_json,driver}.rs`, `src/docx/pipeline.rs`
- Create: `tests/zoterify_fields.rs`, `tests/zoterify_probe.rs`
- Reference: `core/docx_zoterify.py` (937 LOC), `core/docx_pipeline.py`, `core/docx.py` (lines 153–263, 484–630)

## Implementation Steps (if approved)

1. **Run the gate.** Record the decision and evidence in `PARITY-REPORT.md`. Stop here if it fails.
2. Port `probe.rs` — pure detection, no automation. Delivers `docx doctor` and `zoterify-probe`
   immediately and cheaply.
3. Port `fields.rs` and `csl_json.rs`; test standalone against the Phase 9 corpus with expected
   field structures.
4. Port `zoterify_preflight` on top of `probe` + Phase 9 validation.
5. Attempt `driver.rs` on macOS only. Timebox it.
6. Port `pipeline.rs` (`docx cite`) with its auto/dynamic/static mode selection — falling back to the
   Phase 9 static renderer when the dynamic path is unavailable.
7. Decide on `prepare-zotero-import`: port or drop. Default to drop, given upstream's own assessment.
8. Document per-platform support honestly in `docs/COMPATIBILITY.md`.

## Success Criteria (if approved)

- [ ] Gate decision recorded with evidence
- [ ] `docx doctor` and `docx zoterify-probe` report accurate Java/LibreOffice/Zotero-plugin state on all three platforms
- [ ] Field and `csl-citation.json` construction verified against the corpus **without** LibreOffice
- [ ] `docx zoterify` produces documents Zotero refreshes correctly on macOS
- [ ] `docx cite --mode auto` falls back to the static renderer when the dynamic path is unavailable
- [ ] Failures produce actionable diagnostics naming the missing dependency
- [ ] Partial conversion is never reported as success
- [ ] Platform support stated honestly; Windows/Linux dynamic support is not claimed unless implemented
- [ ] `prepare-zotero-import` explicitly ported or explicitly dropped, with rationale

## Risk Assessment

| Risk | Mitigation |
|---|---|
| GUI automation is inherently unreliable | Timeboxed; probe and field construction deliver value even if the driver is abandoned |
| Upstream's own implementation is known-broken for `prepare-zotero-import` | Gate criterion 2; default to dropping that command |
| Breaks on the next LibreOffice or Zotero release | Gate criterion 4 makes the maintenance burden an explicit, accepted decision |
| Scope creeps into building new Windows/Linux automation | Explicitly out of scope — that is new development, not a port |
| Phase blocks Python retirement | It does not: Phase 13 can proceed with these commands ported, formally deprecated, or labeled legacy-only outside the Rust no-Python support promise |
