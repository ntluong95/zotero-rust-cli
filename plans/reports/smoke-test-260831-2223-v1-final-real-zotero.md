# Real-Zotero Final Smoke — v1.0.0

Host: macOS 25.6.0, aarch64 (Apple Silicon). Zotero **10.0.1**, real production
library. Binary: `cargo run --release` from branch `v1-finalization`
(`541708d`), version **1.0.0**.

All checks non-destructive. No item was mutated; no write smoke test was needed.

## Version

```
$ cargo run --release --bin zotero-cli -- --version
zotero-cli 1.0.0
```

## Zotero CLOSED

Zotero quit gracefully before this block; `ZOTERO_CLI_NO_AUTOLAUNCH=1` set for the
search so autolaunch could not mask offline behaviour.

### `app doctor`

| Field | Value |
|---|---|
| `status` / `code` | `degraded` / `DEGRADED` |
| `ready` | `false` |
| `read_ready` | **`true`** |
| `write_ready` | `false` |
| `write_backends` | `{bridge: false, local_api: false}` |
| `checks.connector.ok` | `false` |
| `checks.bridge.state` | **`installed_zotero_closed`** |

`next_steps` correctly distinguished the three separate causes rather than
collapsing them into one message:

1. Zotero is not running; commands needing a live backend start it automatically,
   or run `zotero-cli app launch`.
2. The Local API is already enabled in settings; unavailable only because Zotero
   is not running.
3. CLI Bridge is installed but Zotero is not running.

Exit code 0. `read_ready: true` with `write_ready: false` is exactly the intended
offline posture.

### `item find "Thousands" --all-libraries`

```
libraryID: 7
key:       A5XSZH5H
title:     "Thousands turn out to support science in Italy's stricken olive region"
```

**Matches the expected known result.** Exit 0. Served from read-only SQLite with
Zotero closed — no autolaunch, no stale-snapshot fallback.

### `library list`

Human-readable names present on every library, across all three kinds:

| libraryID | type | name |
|---|---|---|
| 1 | user | My Library |
| 2 | group | ASReview public |
| 3 | group | appliedepimanual |
| 4 | group | Doughnut-AMR |
| 5 | group | SRC Health Hub |
| 6 | group | AMR Trend |
| 7 | group | **INFLUX** |
| 8 | group | ML and AI in evidence syntheis |
| 9 | group | AVSE-MedNet |
| 10 | **feed** | ASReview_mentions |
| 11 | group | PhD-ChiMai |

No `null` names in this library set. `libraryID 7` resolves to **INFLUX**, which
is what makes the `item find` result above actionable to a human.

## Relaunch

```
$ zotero-cli --json app launch --wait-timeout 90
{"action":"launch","pid":23233,"connector_ready":true,"local_api_ready":true,...}
```

Single launch, both backends up. Exit 0.

## Zotero RUNNING

### `app doctor`

| Field | Value |
|---|---|
| `status` | **`ready`** |
| `ready` / `read_ready` / `write_ready` | `true` / `true` / `true` |
| `write_backends` | `{bridge: true, local_api: true}` |
| `checks.bridge.state` | **`healthy`** |
| `checks.bridge.port` | `23120` |
| `checks.zotero_app.version` | `10.0.1` |
| `checks.plugin` | installed `1.2.1`, bundled `1.2.1`, `update_available: false` |
| `checks.bridge.js_result` | `{ok: true, value: "cli-bridge-ok", version: "10.0.1"}` |

Privileged eval round-trip confirmed live. `summary`: *"All systems ready for
agent read/write."*

### `item find "Thousands" --all-libraries`

```
libraryID: 7
key:       A5XSZH5H
```

**Same result as the offline run.** Zotero 10 holds the WAL lock while running, so
SQLite refuses with the tagged `DatabaseLocked` error and the search routes to
the owned CLI Bridge — the exact path this feature exists for. Exit 0.

## Result

| Check | Zotero closed | Zotero running |
|---|---|---|
| `--version` → `zotero-cli 1.0.0` | ✅ | ✅ |
| `app doctor` | ✅ `degraded`, `read_ready`, accurate `next_steps` | ✅ `ready`, `bridge healthy` |
| `item find "Thousands" --all-libraries` → 7 / A5XSZH5H | ✅ | ✅ |
| `library list` names | ✅ | ✅ |

**PASS.**

## Finding: `item find` `date` differs by backend

Not a release blocker; recorded for follow-up.

The same item returns a different `date` value depending on which backend served
the read:

| Backend | `date` |
|---|---|
| SQLite (Zotero closed) | `"2019-01-22 22 January 2019"` — Zotero's raw stored multipart value |
| CLI Bridge (Zotero running, WAL lock held) | `"22 January 2019"` — Zotero's rendered field value |

Cause: `crates/zotero-cli/src/search.rs:246` builds the live record with
`it.getField('date')`, while the SQLite path returns the stored `itemData` value
unchanged.

Why it is not a blocker: both are legitimate representations of the same date;
`key`, `libraryID`, and `title` — the fields callers key off — are identical
across backends; the live path only engages when SQLite refuses; and the SQLite
shape is the one pinned byte-identical to the upstream Python implementation by
the golden-fixture harness, so it is the correct anchor and must not change.

Follow-up: normalize the Bridge path to the SQLite representation, or document it
in the compatibility matrix's "Known, accepted divergences" section with a
regression test. The other fields built in the same live record
(`search.rs:241-250`) should be audited the same way.
