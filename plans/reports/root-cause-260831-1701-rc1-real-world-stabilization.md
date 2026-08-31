# RC1 REAL-WORLD STABILIZATION — ROOT CAUSE REPORT

Base SHA: `f398d88cc51629b1df3cb314ccac9bc01652efab` (v1.0.0-rc.1)
Host: macOS 15 (Darwin 25.6.0), Zotero 10.0.1 running, CLI Bridge 1.2.1 installed, Local API enabled + authorized.

All three problems were traced in source **and** reproduced live against the real installation
before any code was written. No mutation was performed during diagnosis — only read-only probes
(`/connector/ping`, `/api/`, `POST /cli-bridge/eval` with `return 'ping';`, `item list`).

---

## 0. Live evidence captured

```
Zotero process:        pid 53878, name "zotero" (NOT "Zotero")
profile:               ~/Library/Application Support/Zotero/Profiles/08jcnce6.default
prefs.js:              user_pref("extensions.zotero.httpServer.port", 23120);
                       user_pref("extensions.zotero.httpServer.localAPI.enabled", true);
127.0.0.1:23119/connector/ping   -> connection refused (HTTP 000)
127.0.0.1:23120/connector/ping   -> 200
127.0.0.1:23120/cli-bridge/eval  -> {"pong":true,"fork":"zotero-rust-cli",
                                     "id":"cli-bridge@cli-anything-rust.dev",
                                     "version":"1.2.1","ownership":"verified"}
127.0.0.1:23119/cli-bridge/eval  -> connection refused
zotero.sqlite-wal present (1.1 MB, uncheckpointed)
```

Reproduction, back-to-back, same shell:

```
$ zotero-cli --json app doctor
ready True  write_ready True  read_ready True
bridge {"ok": true, "endpoint_active": true, "js_ok": true,
        "js_result": {"ok": true, "value": "cli-bridge-ok", "version": "10.0.1"}}

$ zotero-cli --json js "return 1+1;"
{"error": "JS Bridge endpoint not available. Install the CLI Bridge plugin: ..."}

$ zotero-cli --json item list --limit 1
{"error": "Zotero appears to be running and holds an exclusive lock on the WAL-mode
           database (...). Reading with immutable=1 would silently skip uncheckpointed
           commits, so this refuses instead of returning stale data. ..."}
```

---

## 1. Root cause — `note add` failure (Problem A)

**Current call path**

```
Commands::Note(NoteCommands::Add)                  lib.rs:926
  -> build_runtime()                               lib.rs:932   (probes connector + local API)
  -> JSBridgeClient::with_default_port()           lib.rs:933   (port 23119 — see §2)
  -> notes::add_note(...)                          lib.rs:936
       -> catalog::get_item(runtime, item_ref, …)  notes.rs:289   <-- FAILS HERE
            -> db::resolve_item(sqlite_path, …)    catalog.rs:157
                 -> db::connect_readonly(…)        db.rs:1288
                      mode=ro open -> SQLITE_BUSY
                      + zotero.sqlite-wal exists
                      -> DomainError (refuse)      db.rs:261-269
       -> bridge.note_add(...)                     notes.rs:310   (never reached)
```

**Incorrect coupling.** Target resolution is unconditionally SQLite-backed and runs *before*
backend selection. `notes::add_note` needs exactly four facts about the parent — `key`,
`libraryID`, `itemType`, `itemID` — every one of which the live Zotero process can answer
through the very Bridge the command is about to write through. It instead asks a database
Zotero holds an exclusive lock on.

**Why `--backend api` did not help.** `--backend` is carried on `RuntimeContext.backend` and is
only reported in `app status` output; no write command branches on it. `note add` is
Bridge-only by design (`notes.rs:274-278`) and its item resolution sits entirely outside the
backend-specific write path, so the flag is inert for this failure.

**The SQLite guard is correct.** `db::connect_readonly` (db.rs:240-276) refusing a
`SQLITE_BUSY` + `-wal`-present open is the intended, live-verified safety behavior and is not
touched by this fix.

**Second, independent SQLite coupling on the Local-API write path.**
`catalog::local_api_scope` (catalog.rs:75-88) calls `db::resolve_library` — i.e. even a pure
Local-API write (`item update`, `item tag`, …) needs SQLite twice: once for the target, once
to learn whether the library is `user` or `group`. Both fail identically while Zotero runs.

---

## 2. Root cause — `app doctor` vs `js` contradiction (Problem B)

**Two different ports. One authoritative resolution path; the other is hard-coded.**

| | client construction | port resolution |
|---|---|---|
| `app doctor` | `JSBridgeClient::new(runtime.environment.port)` (lib.rs:146) | `paths::get_http_port` — `ZOTERO_HTTP_PORT` env → `extensions.zotero.httpServer.port` in `user.js`/`prefs.js` → 23119 |
| `js` and ~30 others | `JSBridgeClient::with_default_port()` (bridge/client.rs:205-211) | `ZOTERO_HTTP_PORT` env → **23119**, profile pref never consulted |

On this machine the profile pref is **23120**, so `doctor` probes the live Bridge and passes,
while `js` probes a dead port and reports "endpoint not available". Identical
`bridge_endpoint_active()` code, different socket.

`app plugin-status` is the only other caller that uses the runtime port (lib.rs:2573) — which is
why its `endpoint_active` also disagreed with `js`.

**Blast radius: every Bridge-routed command on a non-default-port profile.**
`with_default_port()` call sites: lib.rs 624, 638, 659, 669, 677, 772, 846, 869, 910, 933, 959,
990, 1017, 1064, 1147, 1173, 1642, 1820, 1869, 1911, 1973, 2061, 2188, 2303, 2376, 2425, 2507,
2530, 2537; hygiene.rs 325. That is `js`, `sync`, `note add`, every Bridge-fallback CRUD write,
every `add`/`import` composition, PDF cascade, merge preview and merge `--confirm`.

**Why every RC1 test missed it.** `tests/common/mod.rs::run_cli` sets
`ZOTERO_HTTP_PORT=<mock port>` for every subprocess. That env var is the *one* input both
resolution paths share, so the two paths were forced into agreement in every test. The
divergence is only observable when the port comes from the profile pref — which no test
exercises.

---

## 3. Root cause / design gap — no auto-launch (Problem C)

There is no gap in the *launch* code: `app_launch::launch_zotero` (app_launch.rs:93-143) already
does cross-platform discovery, `open`-vs-exec selection, spawning through an injectable
`ProcessSpawner`, and readiness polling via `http::wait_for_endpoint`.

The gap is that it has exactly **one** caller — the `app launch` command arm (lib.rs:157-163).
No command that *requires* a live backend can reach it, so a closed Zotero surfaces as a raw
"endpoint not available" / "connector is not available" and the agent must hand-orchestrate
`app launch` → wait → retry.

Three further design gaps make a naive "just call launch" wrong:

1. **`launch_zotero` waits for the wrong things.** It always waits for the Connector and
   (conditionally) the Local API, and *never* for the Bridge eval endpoint. A `js` command that
   auto-launched through it would return before its own backend was usable.
2. **Readiness is too coarse.** `doctor`'s `write_ready = connector.ok && plugin.ok && bridge.ok`
   (doctor.rs:129-131) omits Local API reachability *and* omits write authorization entirely, so
   `write_ready: true` can coexist with a Local API write that fails `AuthorizationRequired`.
   Conversely it demands the Bridge even for a command that only needs the Local API.
3. **No "is Zotero actually closed?" signal.** Nothing distinguishes "Zotero closed" from
   "Zotero running but this one capability is unavailable" — launching in the second case would
   spawn a redundant process.

`doctor`'s `plugin.xpi_installed` is also a pure filesystem check (`paths::plugin_installed`,
paths.rs:457-471) with no bearing on whether the plugin is *loaded*, so the four states the task
asks to distinguish (not installed / installed-but-Zotero-closed / loaded-but-unreachable /
ownership-invalid / healthy) are currently collapsed into two booleans.

---

## 4. Affected command inventory

### 4a. Commands broken by SQLite-before-backend target resolution (Problem A)

Reached only when Zotero is running (which is a precondition for the live write itself):

| Command | Resolution calls before backend selection | Site |
|---|---|---|
| `note add` | `catalog::get_item` | notes.rs:289 |
| `item update` | `get_item`, `local_api_scope` | lib.rs:1609, 1616 |
| `item tag` | `get_item`, `local_api_scope` | lib.rs:1791, 1794 |
| `item delete` | `get_item`, `local_api_scope` | lib.rs:1847, 1850 |
| `item attach` | `get_item` | lib.rs:1909 |
| `item add-to-collection` | `get_item`, `get_collection`, `local_api_scope` | lib.rs:1933-1942 |
| `item move-to-collection` | `get_item`, `get_collection`, `resolve_from_keys`, `local_api_scope` | lib.rs:2002-2011, 2089 |
| `item merge --confirm` | `get_item` × (1 + n) | lib.rs:2174, 2177 |
| `collection create` | `default_library`, `get_collection`, `list_collections`, `local_api_scope` | lib.rs:2244-2279 |
| `collection rename` | `get_collection` × 2, `local_api_scope` | lib.rs:2338-2355 |
| `collection delete` | `get_collection`, `local_api_scope` | lib.rs:2401-2406 |
| `collection remove-item` | `get_collection`, `get_item`, `local_api_scope` | lib.rs:2467-2476 |

Not affected (already live-first, or SQLite is the correct source):
`item merge` bare/`--dry-run` (bridge-first with SQLite fallback, hygiene.rs:278-302),
`add doi/arxiv/url/file`, `import doi/pmid` (no pre-resolution of an existing target),
all read commands.

### 4b. Commands broken by the Bridge port divergence (Problem B)

Every `with_default_port()` call site listed in §2 — i.e. all of 4a's Bridge paths plus `js`,
`sync`, `pdf fetch`/cascade, `collection stats`, `search fulltext`/`annotations`,
`item find-pdf`, `add`/`import`, and merge preview.

### 4c. Commands that should auto-launch (Problem C, live-backend-requiring)

`js`, `sync`, `note add`, all of 4a, `add *`, `import doi/pmid`, `pdf fetch`,
`collection use-selected` / `session use-selected`, `search items`, `item find-pdf`,
`collection stats`, `app authorize-local-api`.

### 4d. Commands that must NOT auto-launch

`app doctor`, `app status`, `app ping`, `app version`, `app plugin-status`,
`app install-plugin`/`uninstall-plugin`, all `item`/`collection`/`library`/`tag`/`note` reads,
`session *`, `docx *`, `audit *`, `export *`, semantic search/similar.

---

## 5. Proposed architecture (smallest shared correction)

Three seams, no speculative refactor.

**Seam 1 — one authoritative Bridge port.** Replace `JSBridgeClient::with_default_port()` with
`JSBridgeClient::for_runtime(&RuntimeContext)` (= `new(runtime.environment.port)`) at every call
site. `with_default_port()` is removed so the hard-coded path cannot be reintroduced. The three
sites with no runtime today (`js`, `sync`, `import pmid`) build one — they are live commands and
need it for Seam 3 regardless.

**Seam 2 — live-first target resolution for write commands.** New `crate::target` module:

```
resolve_item_target(runtime, bridge, item_ref, session)       -> TargetItem
resolve_collection_target(runtime, bridge, coll_ref, session) -> TargetCollection
```

Order, mirroring the accepted `hygiene::merge_preview` bridge-first pattern:

1. owned Bridge eval (`Zotero.Items.get` / `getByLibraryAndKey` across libraries, honoring a
   session library scope) → `{key, libraryID, libraryType, itemType, itemID}`;
2. Local API (`GET {scope}/items/{key}`) when the Bridge is unavailable but the Local API is;
3. SQLite (`catalog::get_item`) — unchanged code path, used offline and as the last resort.

`local_api_scope` gains a live equivalent derived from the resolver's `libraryType`, removing the
second SQLite dependency from the Local-API write path. Read commands keep calling
`catalog::*` directly; nothing about the SQLite guard changes.

**Seam 3 — one lifecycle helper.** New `crate::lifecycle`:

```
enum Backend { Connector, LocalApi, LocalApiWrite, Bridge }
ensure_backend(runtime, Backend, &mut dyn ProcessSpawner) -> Result<RuntimeContext>
```

- already available → return immediately, **no spawn**;
- unavailable *and* Zotero appears closed (no port answers at all) → `app_launch::launch_zotero`
  infrastructure, then wait for the **specific** backend (`/connector/ping`, `/api/`, or an
  owned `POST /cli-bridge/eval` probe), then re-probe and return a fresh runtime;
- unavailable while Zotero *is* answering → no spawn, capability-specific domain error;
- spawn failure → domain error, exit 1; readiness timeout → distinct timeout error, exit 1;
- `LocalApiWrite` waits for the Local API and then reports `AuthorizationRequired` through the
  existing `WriteOutcome` path — it never calls `authorize_interactive`;
- opt-out: `ZOTERO_CLI_NO_AUTOLAUNCH=1` (also set by the integration harness so no test can spawn);
- diagnostics (`app doctor`/`status`/`ping`) never call it.

**Readiness truthfulness.** `doctor`'s public schema is preserved; `write_ready` is corrected to
mean "at least one write backend is actually usable" and the `bridge` check gains explicit
`plugin_loaded` / `ownership_verified` discrimination so the five plugin states are
distinguishable.

---

## 6. Files likely to change

```
crates/zotero-cli/src/bridge/client.rs     for_runtime(); remove with_default_port()
crates/zotero-cli/src/bridge/templates.rs  live target-resolution template
crates/zotero-cli/src/target.rs            NEW — live-first resolver
crates/zotero-cli/src/lifecycle.rs         NEW — ensure_backend()
crates/zotero-cli/src/app_launch.rs        backend-specific readiness wait
crates/zotero-cli/src/catalog.rs           live scope helper alongside local_api_scope
crates/zotero-cli/src/notes.rs             add_note uses the resolver
crates/zotero-cli/src/lib.rs               call-site swaps in write commands + live commands
crates/zotero-cli/src/doctor.rs            truthful readiness / plugin-state discrimination
crates/zotero-cli/src/hygiene.rs           bridge client construction
crates/zotero-cli/tests/common/mod.rs      autolaunch opt-out + profile-pref fixture support
crates/zotero-cli/tests/runtime_orchestration.rs  NEW — cases A-J
Cargo.toml / crates/zotero-cli/Cargo.toml  version -> 1.0.0-rc.2
```

---

## 7. Test plan

New deterministic integration binary covering cases A–J from the task, plus two regression tests
the RC1 suite structurally could not catch:

- **port divergence**: a fixture profile whose `prefs.js` sets a port *different* from
  `ZOTERO_HTTP_PORT`'s absence, asserting `doctor` and `js` resolve the same socket;
- **SQLite-hostile live write**: a fixture data dir with a `zotero.sqlite` + `-wal` sidecar that
  is locked by a writer connection, asserting `note add` still succeeds through the mock Bridge.

No automated test may spawn a real Zotero: the launch path is exercised only through the existing
`ProcessSpawner` fake, and `ZOTERO_CLI_NO_AUTOLAUNCH=1` is set for every non-launch test.

---

## 8. Safety analysis

| Invariant | Impact |
|---|---|
| No direct SQLite writes | Unchanged — no write path gains a SQLite connection. |
| Busy + WAL refusal | Unchanged — `connect_readonly` is not modified; live writes simply stop *needing* it. |
| Mutation only via Connector / Local API / owned Bridge | Unchanged; the resolver is read-only and adds no mutation surface. |
| Bridge ownership checks | Strengthened — resolution now goes through the same `bridge_endpoint_active()` fork+id gate, and the previously-wrong port made ownership checks moot on non-default profiles. |
| Local API human authorization | Unchanged — `ensure_backend` never calls `authorize_interactive`; `AuthorizationRequired` still stops the command. |
| No credentials in logs/audit | Unchanged — no new logging of credential material. |
| No raw-JS write fallback | Unchanged, and reinforced: typed commands now work, removing the pressure that made an agent reach for `js`. |
| `item merge` safe-by-default | Unchanged — the `dry_run` branch is untouched; `--confirm` only swaps its resolver. |
| Deferred dynamic DOCX | Out of scope, untouched. |

**New risk introduced:** auto-launch can start a GUI process the user did not explicitly ask for.
Mitigated by: never launching when any Zotero port answers; never launching from diagnostic or
offline-read commands; a documented `ZOTERO_CLI_NO_AUTOLAUNCH=1` opt-out; and a hard rule that
tests use the injectable spawner only.
