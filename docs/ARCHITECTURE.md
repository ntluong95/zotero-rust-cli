# Architecture

How `zotero-cli` actually talks to Zotero, and why each command lands where it
does.

## Four backend surfaces

The CLI is a stateless process. Each invocation resolves an environment, picks a
backend per command, does the work, and exits.

```
                        zotero-cli (native binary, no runtime)
                                      │
        ┌──────────────┬──────────────┼───────────────┬──────────────────┐
        ▼              ▼              ▼               ▼                  ▼
   1. SQLite      2. Connector    3. Local API    4. CLI Bridge     local state
   (read-only)     HTTP 23119      HTTP 23119      /cli-bridge/eval   session.json
                                                                      vectors.sqlite
   zotero.sqlite   /connector/*    /api/*          Zotero plugin      audit JSONL
                                                                      credentials
```

| # | Surface | Used for | Requires |
|---|---|---|---|
| 1 | **Read-only SQLite** | Catalog reads: items, collections, libraries, tags, saved searches, styles | Nothing. Works with Zotero closed. |
| 2 | **Connector HTTP** | Translator-driven ingest (`add *`, `import file/json/doi`), GUI selection (`use-selected`) | Zotero running |
| 3 | **Local API HTTP** | Reads and writes Zotero's Web-API-shaped endpoints can express; citations, bibliographies, exports | Zotero 10+, Local API enabled; writes additionally need human authorization |
| 4 | **CLI Bridge** | Privileged operations only in-app JavaScript can do: merge, attach, full-text/annotation search, `sync`, `collection stats`, `import pmid`, `js` | Zotero running + the Bridge plugin installed |

Local state — session, vector DB, audit log, Local API credentials — is plain
files on disk, in formats shared with the upstream Python implementation.

## Backend routing

Routing is **capability-driven, not version-driven**. The CLI probes what is
actually there rather than assuming a Zotero version.

### Reads

```
read command
  │
  ├─ try read-only SQLite (mode=ro, with a real probe query)
  │     │
  │     ├─ success ──────────────────────────► use it
  │     │
  │     ├─ SQLITE_BUSY and a -wal file exists ─► REFUSE (tagged DatabaseLocked)
  │     │        │
  │     │        └─ item find / library list only:
  │     │             ask an already-running, ownership-verified Bridge
  │     │             to run Zotero's own read-only query
  │     │
  │     └─ SQLITE_BUSY and no -wal file ──────► fall back to immutable=1 (safe)
  │
  └─ commands with no SQLite representation (search items, style rendering, …)
        go straight to the Local API or the Bridge
```

The refusal in the middle branch is the point. See [the `immutable=1`
trade-off](#the-immutable1-trade-off) below.

### Writes

```
write command
  │
  ├─ Local API available AND authorized? ──► route there
  │                                            (compatibility renderer maps the
  │                                             Web-API-shaped response back to
  │                                             this CLI's flat output contract)
  │
  ├─ else Bridge healthy and owned? ───────► route there
  │
  └─ else ─────────────────────────────────► fail with authorization_failed /
                                             needs_human_action — never a silent
                                             direct-SQLite write
```

`app doctor` exposes exactly this: `write_ready` (is at least one approved
backend usable now) and `write_backends` (`{bridge, local_api}`).

Some writes are Bridge-only regardless of Local API availability, because Zotero
exposes them only to in-app JavaScript: `item merge --confirm`, `item attach`,
`item find-pdf`, `item search-fulltext`, `item search-annotations`,
`item annotations`, `collection stats`, `sync`, `import pmid`, `js`.

## The `immutable=1` trade-off

The single most consequential decision in the port.

Zotero holds its database connection in SQLite's exclusive locking mode on
**every** version — a plain `mode=ro` open fails with `SQLITE_BUSY` on the first
statement while Zotero runs. Upstream's answer was `immutable=1`, which lets the
file open regardless.

On Zotero ≤9 (rollback journal) that is harmless. On Zotero 10+ (WAL) it is not:
`immutable=1` tells SQLite the file can never change, so SQLite never attaches the
`-wal` sidecar at all. Every committed-but-uncheckpointed row silently vanishes
— exit code 0, no error, wrong answer.

The port's policy:

1. Try `mode=ro`, with a real probe query (the lock only surfaces at
   statement-prepare time, not at connection-open time).
2. Success → use it. This is the only path when Zotero is closed, or on any
   database with no `-wal` sidecar.
3. `SQLITE_BUSY` **and** a `-wal` file exists → refuse, with an actionable error.
   No silent `immutable=1` fallback on a WAL database, and no bypass flag.
4. `SQLITE_BUSY` and no `-wal` file → fall back to `immutable=1`. Unconditionally
   safe: a rollback-journal database has nothing in a WAL to miss.

Correctness is preferred over availability here, deliberately. A refusal is
recoverable; a silently incomplete answer is not.

Evidence, including the live reproduction and the four regression tests that pin
both the bug and the fix: [`ZOTERO-COMPATIBILITY.md`](ZOTERO-COMPATIBILITY.md).

## Process lifecycle

Commands needing a live backend start Zotero themselves — **at most once**, only
when nothing is answering the HTTP port — then wait for the specific backend that
command needs before continuing.

Never autolaunch: diagnostics (`app doctor/status/ping/version/plugin-status`),
every offline-capable read, and `item merge` without `--confirm`.

`ZOTERO_CLI_NO_AUTOLAUNCH=1` disables it; `ZOTERO_CLI_LAUNCH_TIMEOUT` bounds the
wait (default 60s).

Launching is not consent. Local API writes still require
`app authorize-local-api`.

## The CLI Bridge plugin

A two-file Zotero bootstrap extension (`manifest.json` + `bootstrap.js`),
**embedded in the binary** and staged on demand by `app install-plugin`. It
registers two endpoints on Zotero's own HTTP server:

- `POST /cli-bridge/eval` — run privileged JavaScript, return JSON
- `POST /cli-bridge/ownership` — identify which fork owns the endpoint

Ownership matters: upstream's Python project ships a similar bridge. An HTTP 200
on `/cli-bridge/eval` is **not** accepted as proof; the CLI verifies the
responding plugin identifies as this fork
(`cli-bridge@cli-anything-rust.dev`, `fork: "zotero-rust-cli"`) before trusting
it. `app plugin-status` surfaces that answer directly.

Staging writes only to a caller-selected output directory, never into Zotero's
profile — an unregistered XPI dropped into `<profile>/extensions/` is purged
during Zotero's startup reconciliation, so profile-writing would not even work,
let alone respect plugin consent.

JavaScript sent over the Bridge is built by serializing parameters with
`serde_json` and parsing them with `JSON.parse` on the Zotero side — never by
string concatenation.

## Output contract

One rendering layer serves every command, in two modes:

- `--json` — machine mode. Accepted at root, group, and command level. Errors go
  to **stdout** as `{"error": "..."}`. Action commands return
  `{action, ok, status, code?, error?, ...}`; read commands return their data
  directly.
- default — human mode, deliberately *not* valid JSON, matching upstream.

Exit code is derived from the payload, not from control flow: `1` when `ok` is
`false` or `status` ∈ {`partial_success`, `error`, `failed`, `timeout`},
otherwise `0`.

A standing denylist test asserts that backend identity (`backend`, `server_id`)
and internal Local API versioning never leak into stdout JSON — which backend
served a write is an implementation detail, not part of the contract.

## Module map

| Module | Responsibility |
|---|---|
| `cli` | The `clap` surface; `--json` global at every level |
| `lib` | Command dispatch, result envelopes, exit codes |
| `paths` | Zotero install/profile/data discovery, env overrides |
| `runtime` | `RuntimeContext`: environment + capability probe, built lazily |
| `db` | Read-only SQLite layer and the connection policy above |
| `catalog` | Library/collection/item domain reads |
| `search` | Cross-library discovery, live-read routing |
| `http`, `http/connector`, `http/local_write` | Connector and Local API clients |
| `bridge` | Bridge client, JS templates, ownership probe |
| `plugin` | Embedded XPI assets and staging |
| `write`, `write_router`, `target` | Write outcome contract, backend selection, target resolution |
| `credentials` | Local API credential resolution and storage |
| `session` | Session state (`session.json`), advisory-locked |
| `notes`, `rendering`, `csl`, `analysis`, `hygiene`, `metrics` | Domain features |
| `add_import`, `import_*`, `pdf_fetch`, `pdf_cascade` | Ingest and PDF acquisition |
| `semantic` | Embedding index and similarity search |
| `docx` | Pure-OOXML static DOCX commands |
| `audit` | JSONL write audit log |
| `lifecycle`, `app_launch` | Autolaunch and backend readiness waiting |
| `output`, `error` | Rendering modes and the domain-error → exit-code contract |

## Verification

- **Golden-fixture parity harness** (`harness/`): captures the frozen upstream
  Python implementation's output for 101 fixtures and compares Rust against it.
  This is why `reference/cli-anything-zotero/` is retained — it is the parity
  oracle, and Python is required to run it. It is maintainer tooling and is not
  on any user install path.
- **Integration tests** (`crates/zotero-cli/tests/`): ~35 test binaries covering
  each functional area, including live-Zotero conformance regressions, SQL
  injection, write-backend routing, and the output denylist.
- **Command accounting**: all 96 upstream leaf commands are classified in
  [`../plans/reports/compatibility-matrix.md`](../plans/reports/compatibility-matrix.md).
