# Parity Harness

Phase 1 harness for comparing the Python reference CLI against the Rust port.

## Layout

- `commands.tsv` lists all 96 leaf commands, expected compatibility class, fixture state, and representative arguments.
- `fixtures/build_fixture.py` builds deterministic Zotero profile/data fixtures from the upstream test helpers.
- `capture.py` runs an implementation against fixture-safe commands and stores normalized stdout, stderr, exit code, and HTTP calls.
- `normalize.py` strips machine-specific paths and volatile values from captures.
- `compare.py` compares two capture directories and reports `Exact`, `Semantic`, `Skipped`, `Mismatch`, or `Missing`.
- `golden/python/` stores the Python baseline.

The `group-library` fixture uses the upstream helper's built-in user and group libraries. The
`unicode-cjk` fixture adds titles, tags, and collections with quotes, slashes, newlines, script-like
text, and CJK characters.

## Usage

```bash
PYTHONPATH=reference/cli-anything-zotero python3 harness/capture.py --impl python --output harness/golden/python --clean
python3 harness/compare.py harness/golden/python harness/golden/python
```

The Python self-compare should report captured commands as `Exact` and live-only placeholders as
`Skipped`. A separate repeat capture may report `Semantic` for commands whose compatibility class
allows shape-preserving nondeterminism, but any exit-code change, traceback, invalid JSON stdout, or
metadata mismatch is a failure.

To compare a Rust binary later:

```bash
./target/release/zotero-cli --version
python3 harness/capture.py --impl ./target/release/zotero-cli --output harness/current/rust --clean
python3 harness/compare.py harness/golden/python harness/current/rust
```

Commands marked `live-only` require real Zotero, LibreOffice, Java, or network state and are not captured by default. Phase 10 owns live certification.
