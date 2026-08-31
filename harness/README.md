# Parity Harness

Phase 1 harness for comparing the Python reference CLI against the Rust port.

> **Maintainer tooling only.** This directory (and `reference/`) is the parity
> oracle used to verify Rust behaviour against the frozen upstream Python
> implementation. It requires Python; **`zotero-cli` itself does not.** Nothing
> here is on any user install or runtime path — see
> [`../docs/INSTALL.md`](../docs/INSTALL.md).

## Layout

- `commands.tsv` lists all 96 leaf commands plus 3 rows dedicated to covering a specific branch a
  command's default fixture doesn't reach (see below), expected compatibility class, fixture state,
  and representative arguments.
- `fixtures/build_fixture.py` builds deterministic Zotero profile/data fixtures from the upstream test helpers.
- `capture.py` runs an implementation against fixture-safe commands and stores normalized stdout, stderr, exit code, and HTTP calls.
- `normalize.py` strips machine-specific paths and volatile values from captures.
- `compare.py` compares two capture directories and reports `Exact`, `Semantic`, `Skipped`, `Mismatch`, or `Missing`.
- `golden/python/` stores the Python baseline.

The `group-library` fixture uses the upstream helper's built-in user and group libraries. The
`unicode-cjk` fixture adds titles, tags, and collections with quotes, slashes, newlines, script-like
text, and CJK characters. The `zotero-unreachable` fixture starts **no** fake HTTP server at all —
the CLI under test is pointed at a real closed TCP port, producing a genuine connection refusal
instead of a mocked HTTP status; see `normalize.py`'s narrowly-scoped `connector_message`/
`local_api_message` normalization for why this fixture state is special-cased.

### Branch-coverage rows beyond the 96 commands

A command classifying **Exact** against its default fixture proves that fixture's code path is
correct — it says nothing about branches the fixture doesn't exercise. Three rows exist specifically
to close gaps found by auditing what a green run actually covers, not by guessing:

| Row | Command label | What it covers that the default fixture doesn't |
|---|---|---|
| 97 | `app status (unreachable)` | Real connection-refused transport failure (see `zotero-unreachable` above) |
| 98 | `item find (sql-fallback)` | `--exact-title` forces the SQLite path (`find_items_by_title`); the default `item find` fixture only exercises the Local-API-hit path |
| 99 | `search get (ambiguous)` | A saved-search key duplicated across libraries; exercises `resolve_saved_search`'s ambiguous-reference error, which no other fixture reaches |

When porting a new command, check what its existing golden fixture(s) actually exercise before
declaring it done — a first-attempt Exact result is a necessary signal, not a sufficient one. If a
branch is real but genuinely hard to reach through the CLI-level harness (e.g. a pure function with
many input-shape branches), a direct unit test in the relevant Rust module is an acceptable
substitute for a new fixture row — see `resolve_attachment_real_path`'s and `build_collection_tree`'s
tests in `db.rs` for examples.

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
