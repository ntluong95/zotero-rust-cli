# Minimal Python Runtime Audit — v1.0.0

Scope: verify only that a user can **install and run `zotero-cli` without
Python**. Not a cleanup campaign. Python is explicitly allowed to remain for the
parity harness, the frozen Python authority, and development-only oracle tooling.

Base: `ba661c45c3cb0a294094362036c90c79aae4b813` (main), branch `v1-finalization`.

## Method

```bash
grep -rniE "python3?|\bpip\b|venv|virtualenv|setup\.py|pyproject" \
  --exclude-dir=target --exclude-dir=.git --exclude-dir=.claude \
  --exclude-dir=harness --exclude-dir=plans --exclude-dir=reference \
  --exclude-dir=__pycache__ .
```

Development oracle/reference paths (`harness/`, `plans/`, `reference/`,
`target/`) excluded by design. `THIRD-PARTY-LICENSES.md` excluded as generated
license text.

Also checked: every `std::process::Command::new` call site in
`crates/zotero-cli/src/`.

## Findings

| Location | Kind | Verdict |
|---|---|---|
| `.gitignore` (`.venv/`, `venv/`) | Dev-only ignore patterns | **Not a user requirement.** No action. |
| `packaging/generate-third-party-licenses.sh` | Maintainer/CI release-prep script; pipes `cargo about` JSON through `python3` (falls back to `python`) | **Not a user path.** Runs in the release workflow on GitHub runners, which ship Python. Users never invoke it; its *output* (`THIRD-PARTY-LICENSES.md`) ships in every archive. No action. |
| `crates/zotero-cli/**/*.rs` (~40 hits) | Source comments and test names referencing Python-parity semantics (`"matches Python's json.dumps key order"`, `test_..._matches_python`, …) | Comments and identifiers only. No runtime dependency. No action. |
| `crates/zotero-cli/src/app_launch.rs:75` | The only `Command::new` in the crate — launches the **Zotero executable**, never an interpreter | Confirms no runtime shell-out to Python. |

**Zero** production-facing paths require Python.

## Runtime path verification

| Path | Requires Python? |
|---|---|
| Installing the release binary (all 5 targets) | No — single static native binary; archives contain only binaries + license files |
| Library / item reads | No |
| Writes (Local API and CLI Bridge) | No |
| CLI Bridge (XPI staging and install) | No — XPI is embedded in the binary via `include_str!` |
| Local API authorization | No |
| Static DOCX commands | No — pure OOXML via the `zip` + `quick-xml` crates |
| Normal agent use (`--json`) | No |

The release archive staging step in `.github/workflows/release.yml` copies
exactly: `zotero-cli`, `cli-anything-zotero`, `LICENSE`, `NOTICE-CHANGES.md`,
`THIRD-PARTY-LICENSES.md`. No scripts, no interpreter.

## Documentation fixes applied

- `docs/INSTALL.md` — restated the no-runtime promise explicitly as "No Python,
  pip, Node, Rust, or Cargo required — at install time or at run time."
- `docs/MIGRATION.md` §1 — new section enumerating every end-user path and
  confirming none needs Python, and stating plainly that Python survives only as
  maintainer tooling.
- `harness/README.md` — added a "Maintainer tooling only" banner so the one
  Python-requiring README in the repo cannot be mistaken for user instructions.

No user-facing document told users to install Python before this audit; none does
after it.

## Conclusion

**PASS.** Phase 13 criterion 4 ("prebuilt binaries install and run on all five
targets with no Python, pip, Rust or Cargo") is satisfied for v1.0.0 at the
source and packaging level, and confirmed end-to-end against the published
artifact during release smoke testing.

Python remains, correctly and deliberately, in `harness/` and `reference/` as the
parity oracle. Deleting it would remove the ability to verify the port that
replaced it.
