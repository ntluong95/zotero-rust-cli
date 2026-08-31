# PRE-V1 AGENT DISCOVERY & ONBOARDING — IMPLEMENTATION PLAN

## 1. Current main SHA

```
ca7af316e979630a7715de99e57e1ebdb4ae1380   (Merge PR #28, codex/rc2-ci-test-isolation-fix)
```

Working branch: `pre-v1-agent-discovery`, cut from `ca7af31`. `v1.0.0-rc.2` (`794482a`'s
release) is untouched.

The CI isolation merge added `create_fake_profile` / `create_empty_fake_profile` to
`tests/common/mod.rs` and a default `ZOTERO_PROFILE_DIR` per test, so no test reads the
developer's real Zotero profile. Feature 4's tests build directly on that.

---

## 2. Canonical behaviour of `item find` and `library list`

### `item find` — `core/catalog.py::find_items`

```python
sqlite_path = _require_sqlite(runtime)                     # SQLite is mandatory, always
library_id  = collection["libraryID"] if collection else _default_library(runtime, session)
if not exact_title and runtime.local_api_available:
    payload = local_api_get_json(port, f"{scope}/items/top", q=..., qmode=..., limit=...)
    for record in payload:
        resolved = zotero_sqlite.resolve_item(sqlite_path, record["key"], library_id=library_id)
        ...                                                 # every hit re-read from SQLite
    if results: return results[:limit]
return zotero_sqlite.find_items_by_title(sqlite_path, query, library_id=..., ...)
```

Three facts that matter:

- **Exactly one library**, always: the collection's, or `_default_library` (session
  `current_library`, else the lowest library id). There is no all-libraries mode.
- **SQLite is mandatory on every path.** Even the "Local API" branch is only a *key finder*;
  each key is re-resolved through SQLite to build the returned record.
- The Rust port (`catalog.rs:163-233`) is a faithful copy of this, including the
  Local-API-then-SQLite re-resolution.

`db::find_items_by_title` already accepts `library_id: Option<i64>` and, given `None`, searches
every library with deterministic ordering (exact-title, then prefix, then `INSTR` position, then
`dateModified DESC`, then `itemID DESC`). Cross-library search is therefore already expressible
offline; nothing exposes it.

### `library list` — `core/catalog.py::list_libraries`

```python
return zotero_sqlite.fetch_libraries(_require_sqlite(runtime))
# SELECT libraryID, type, editable, filesEditable, version, storageVersion, lastSync, archived
#   FROM libraries ORDER BY libraryID
```

Eight columns, no name. The Rust port is identical (`db.rs:350-359`). Canonical never surfaces
a library name anywhere.

---

## 3. Proposed `--all-libraries` semantics

Additive flag on `item find` only.

| | without the flag | with `--all-libraries` |
|---|---|---|
| scope | collection's library, else session `current_library`, else default library | every **user + group** library |
| `current_library = null` | still means "default library" — unchanged | irrelevant (flag is explicit) |
| feeds | n/a | excluded, unless `--include-feeds` is also passed |
| `--collection` | as today | **rejected** as a usage error: a collection already fixes one library, so the two are contradictory |
| ordering | unchanged | same relevance ordering, applied across the merged set, then `--limit` |
| session state | not written | **not written** — resolution is in-memory only |

Every returned record already carries `libraryID` and `key`, so two same-titled items in
different libraries stay distinguishable with no schema change.

`--include-feeds` is proposed as a second additive flag. Feeds are excluded by default because a
feed item is an unsaved RSS entry, not a library item — an agent that finds one and tries
`note add` on it gets a confusing failure. The flag exists so the behaviour is explicit rather
than silently lossy.

---

## 4. Live search backend, and why

**The owned Bridge. Not the Local API.**

The deciding constraint is library enumeration. Cross-library search needs the list of
libraries, and:

- The **Local API** exposes `/api/users/0` and `/api/groups/{id}` but has **no endpoint that
  enumerates the groups this Zotero has**. Discovering group ids requires reading the `libraries`
  table — SQLite — which is exactly what is unavailable while Zotero holds the WAL lock. The
  Local API therefore cannot satisfy `--all-libraries` at all while Zotero is running.
- The **Bridge** enumerates live (`Zotero.Libraries.getAll()`), searches each library with
  Zotero's own search engine, and returns every field `find_items_by_title` produces — with zero
  SQLite access.

Live-verified against the real Zotero 10.0.1 while its database was WAL-locked, using the exact
scenario from the brief:

```
$ zotero-cli --json js "<cross-library quicksearch for 'Thousands'>"
[ { "libraryID": 7, "libraryType": "group", "libraryName": "INFLUX",
    "key": "A5XSZH5H",
    "title": "Thousands turn out to support science in Italy's stricken olive region" } ]
```

Scope mapping is direct: canonical's `titleCreatorYear` / `fields` / `everything` become
Zotero's `quicksearch-titleCreatorYear` / `quicksearch-fields` / `quicksearch-everything`
search conditions. `exact_title` keeps its SQLite-only meaning (Zotero's quicksearch has no
exact-match mode); an `--exact-title` search while Zotero is running still hits the WAL refusal,
which is honest and unchanged.

Routing:

```
Zotero closed                                  -> SQLite (unchanged, byte-identical)
Zotero running + owned Bridge healthy          -> Bridge search
Zotero running + Bridge unavailable            -> current WAL refusal, unchanged
```

No `immutable=1`, no busy/WAL bypass, no SQLite writes, no raw-JS mutation path. The live path
is read-only by construction and will carry the same structural "no mutation verbs" test the
`target` resolution templates already have.

**Shape parity is achievable.** `find_items_by_title` calls `normalize_item(.., include_related =
false)`, so `fields`, `creators` and `tags` come back empty and the record is thin: ids, key,
libraryID, itemTypeID, typeName, dateAdded, dateModified, version, title, DOI, date, hasPdf,
parent/attachment nulls, note fields. Every one of those is available from the Bridge
(`item.id`, `item.key`, `item.libraryID`, `item.itemTypeID`, `Zotero.ItemTypes.getName()`,
`getField(...)`, `item.parentItemID`, an attachment scan for `hasPdf`). Both paths will be
normalised through **one** constructor so the JSON cannot drift.

---

## 5. Library-name source

**Safe offline SQLite.** No live backend required, no guessing. Zotero's real schema (read from
`~/Zotero/zotero.sqlite.bak`, read-only):

```sql
CREATE TABLE groups (groupID INTEGER PRIMARY KEY, libraryID INT NOT NULL UNIQUE,
                     name TEXT NOT NULL, description TEXT NOT NULL, version INT NOT NULL, ...);
CREATE TABLE feeds  (libraryID INTEGER PRIMARY KEY, name TEXT NOT NULL, url TEXT NOT NULL, ...);
CREATE TABLE libraries (libraryID INTEGER PRIMARY KEY, type TEXT NOT NULL, ...);
```

So:

| library type | name source |
|---|---|
| `user` | the constant `"My Library"` — Zotero's own name for it (confirmed live: `Zotero.Libraries.get(1).name == "My Library"`) |
| `group` | `LEFT JOIN groups ON groups.libraryID = libraries.libraryID` → `groups.name` |
| `feed` | `LEFT JOIN feeds ON feeds.libraryID = libraries.libraryID` → `feeds.name` |
| anything else / row missing | `null` — never fabricated |

`LEFT JOIN` keeps `library list` working on fixture databases that have no `groups`/`feeds`
table, so no existing test loses its fixture.

`name` is an **additive field**; all eight canonical fields stay exactly as they are. Human
output will lead with the name. No live enrichment is needed, so none is added — the offline
answer already agrees with what the Bridge reports (`"INFLUX"` for library 7 from both).

---

## 6. Bridge / XPI onboarding — current vs proposed

**Current.** `app install-plugin` already stages the bundled XPI (`plugin::build_xpi` writes
`manifest.json` + `bootstrap.js` from compiled-in assets), so the binary genuinely carries the
plugin and nothing needs downloading. What it returns is thin:

```json
{ "staged_xpi_path": "...", "message": "XPI staged. Install manually via Zotero: Tools > Plugins/Add-ons > Install Add-on From File, then restart Zotero." }
```

No bundled version, no structured steps, and `doctor` never tells a new user this command exists
in a way that connects to *why* they would want it.

**Proposed.** Keep the staging model exactly as-is — the CLI stages, the human installs through
Zotero's own consent UI, `doctor` verifies. No profile writes, no bypass of Zotero's plugin
consent. Only the *reporting* improves:

```json
{
  "action": "app_install_plugin",
  "staged_xpi_path": "/…/cli-bridge@cli-anything-rust.dev.xpi",
  "bundled_version": "1.2.1",
  "installed_version": null,
  "already_installed": false,
  "install_steps": [
    "Open Zotero.",
    "Tools → Plugins.",
    "Click the gear icon → Install Add-on From File…",
    "Select: /…/cli-bridge@cli-anything-rust.dev.xpi",
    "Restart Zotero.",
    "Verify with: zotero-cli app doctor"
  ],
  "message": "…"
}
```

Human mode prints the numbered steps with the real path. `doctor` gains, in the missing case, a
next step that says the Bridge is not installed, that live operations need it, and to run
`zotero-cli app install-plugin`.

**Bridge diagnostic states.** RC2 already reports `not_installed`, `installed_zotero_closed`,
`installed_not_loaded`, `ownership_invalid`, `eval_failing`, `healthy` in `checks.bridge.state`.
This package adds one the brief asks for: **`staged_not_installed`** — the bundled XPI has been
written to the staging directory but is not present in the profile's `extensions/`. That is
reliably detectable (both paths are known) and is precisely the state a user is in between
running `install-plugin` and completing the Zotero dialog, which is where they are most likely to
ask the CLI what to do next.

---

## 7. `doctor` next_steps root cause

`doctor.rs:133-140`:

```rust
if !runtime.local_api_available {
    next_steps.push("Enable Local API: zotero-cli app enable-local-api --launch (or Zotero
                     Settings → Advanced → allow other apps).");
}
```

Two independent defects:

1. **It keys off the wrong fact.** `local_api_available` is *reachability*.
   `environment.local_api_enabled_configured` is *configuration* (the
   `extensions.zotero.httpServer.localAPI.enabled` pref). With Zotero closed the first is false
   while the second is true, and the user is told to enable something they already enabled. The
   real condition is "Zotero is closed".
2. **It recommends a command that does not exist.** `app enable-local-api` is the intentionally
   **Excluded** canonical behaviour — the Rust CLI ships `app authorize-local-api` instead.
   `zotero-cli app --help` lists no `enable-local-api`. So the current advice is not merely
   misleading, it is unrunnable, and following it would push a user toward the unsafe canonical
   workflow the exclusion exists to prevent.

Proposed state table (each row emits exactly one step):

| configured | reachable | Zotero up | next step |
|---|---|---|---|
| no | no | no | start Zotero, then enable the Local API in Zotero Settings → Advanced |
| no | no | yes | enable it in Zotero Settings → Advanced (no CLI command — deliberate) |
| yes | no | no | "Local API is configured but unavailable because Zotero is not running." → start Zotero, or `zotero-cli app launch`; note live commands auto-launch |
| yes | no | yes | Local API is configured but the running Zotero is not serving it — recheck Settings → Advanced, restart Zotero |
| yes | yes | yes | nothing about configuration; if writes are unauthorised → `zotero-cli app authorize-local-api` |

`enable-local-api` is removed from the string entirely.

---

## 8. Files expected to change

```
crates/zotero-cli/src/cli.rs                    --all-libraries / --include-feeds flags; help text
crates/zotero-cli/src/catalog.rs                find_items scope resolution + live routing seam
crates/zotero-cli/src/search.rs                 NEW — shared search abstraction (SQLite | Bridge),
                                                one normaliser both paths return through
crates/zotero-cli/src/db.rs                     Library.name via LEFT JOIN groups/feeds;
                                                find_items_by_title multi-library scoping
crates/zotero-cli/src/bridge/js/search_items.js NEW — cross-library quicksearch template
crates/zotero-cli/src/bridge/js/list_libraries.js NEW — live library enumeration (only if needed)
crates/zotero-cli/src/bridge/templates.rs       render_search_items
crates/zotero-cli/src/bridge/client.rs          search_items()
crates/zotero-cli/src/doctor.rs                 state-aware next_steps; staged_not_installed
crates/zotero-cli/src/plugin/mod.rs             staged-XPI detection helper
crates/zotero-cli/src/lib.rs                    dispatch wiring for the new flags/fields
crates/zotero-cli/src/output.rs (or lib.rs)     human rendering for library names / install steps
crates/zotero-cli/tests/agent_discovery.rs      NEW — tests 1-13
crates/zotero-cli/tests/bridge_onboarding.rs    NEW — tests 14-20
docs/INSTALL.md                                 onboarding + discovery section
Cargo.toml                                      version -> 1.0.0-rc.3 (last step only)
```

---

## 9. Compatibility classification of every public change

| # | Change | Class | Notes |
|---|---|---|---|
| 1 | `item find --all-libraries` | **Additive Rust extension** | New optional flag. Absent flag ⇒ byte-identical canonical behaviour. |
| 2 | `item find --include-feeds` | **Additive Rust extension** | Only meaningful with `--all-libraries`. |
| 3 | `item find --all-libraries --collection` rejected | **Additive Rust extension** | New flag combination only; canonical `--collection` alone unchanged. |
| 4 | `library list` gains `name` | **Additive Rust extension** | Additive JSON field; all 8 canonical fields preserved in place. |
| 5 | `item find` live Bridge routing when Zotero is running | **Additive Rust extension (safety-divergence remediation)** | See the note below — **flagged for your decision.** |
| 6 | `app install-plugin` gains `bundled_version`, `installed_version`, `already_installed`, `install_steps`, `action` | **Additive Rust extension** | Existing `staged_xpi_path` / `message` unchanged. |
| 7 | `doctor` next_steps rewritten; `enable-local-api` removed | **Bug fix** | Current text names a command this CLI does not have (Excluded canonical behaviour). `next_steps` is prose, not a stable contract. |
| 8 | `doctor` `checks.bridge.state` gains `staged_not_installed` | **Additive Rust extension** | New enum member on an RC2-additive field. |

**Canonical accounting is unchanged: Integrated 86 / Missing 0 / Changed 1 / Excluded 1 /
Dropped 1 / Deferred 7 / Total 96.** Everything above is either an additive flag/field outside
the canonical 96 or a fix to Rust-side prose.

**Row 5 needs your explicit sign-off, and I will not decide it silently.** Canonical `find_items`
also fails while Zotero is running — but for a different reason: canonical opens SQLite with an
unconditional `mode=ro&immutable=1`, which *succeeds* and silently returns a stale, possibly
incomplete snapshot. Rust deliberately diverged (`db.rs:240-276`) to refuse instead. So the error
being removed here is an artifact of **our** safety divergence, not of canonical behaviour, and
routing around it restores canonical's *intent* (search works while Zotero runs) through a safe
mechanism rather than an unsafe one. On that reading it is additive remediation and adds no
canonical `Changed` entry — the same reasoning RC2 used when `note add` stopped failing in this
exact situation. If you would rather record it as a second `Changed` entry, say so and I will
classify it that way instead; it does not affect the implementation.

---

## 10. Test matrix

Cross-library (`tests/agent_discovery.rs`):

1. personal-library match — found, `libraryID` 1
2. group-library match — found, correct `libraryID`
3. identical title in two libraries — two results, distinguishable by `libraryID`/`key`
4. no match — `[]`, exit 0
5. feeds excluded by default; included with `--include-feeds`
6. persistent session file byte-identical before/after `--all-libraries`
7. `--all-libraries` with `--collection` — usage error, no search performed
8. ordering is deterministic across libraries (fixed fixture ⇒ fixed order)

Live search:

9. Zotero closed → SQLite path, no Bridge request issued
10. Zotero running + WAL-locked + Bridge healthy → live search succeeds, **zero** SQLite access
11. Zotero running + WAL-locked + Bridge unavailable → WAL refusal preserved verbatim
12. no unsafe SQLite fallback: structural assertion that no live path opens `immutable=1`
13. live and SQLite paths produce the same JSON key set for the same logical item
14. live search template contains no mutation verbs (`saveTx`/`eraseTx`/`merge`/`trash`)

Library names:

15. user library → `"My Library"`
16. group library → `groups.name`
17. feed → `feeds.name`
18. `groups` row missing / table absent → `name: null`, never fabricated, command still succeeds

Bridge onboarding (`tests/bridge_onboarding.rs`):

19. Bridge missing → doctor recommends `app install-plugin`
20. `install-plugin` → structured `staged_xpi_path` + `bundled_version` + `install_steps`, and
    the staged file is a valid XPI containing `manifest.json`
21. staged but not installed → `state: "staged_not_installed"` with the right instruction
22. installed + Zotero closed → `installed_zotero_closed`, no install recommendation
23. Bridge healthy → no install recommendation at all

Doctor next_steps:

24. Local API configured + Zotero closed → next_steps must **not** contain "Enable Local API"
    and must mention Zotero not running
25. Local API unconfigured → configuration guidance, pointing at Zotero Settings
26. authorization missing → recommends `app authorize-local-api`
27. **no next_step ever names `enable-local-api`** (regression guard on the Excluded command)

All tests use the existing mock-server + fake-profile harness. None launches or mutates a real
Zotero.

---

## 11. Is `app setup` worth implementing?

**No — recommend skipping it.**

Once Features 4 and 5 land, `app doctor` already emits every fact the proposed `app setup` would
print (installation, offline read capability, Bridge state, Local API configuration, write
readiness) plus an ordered, state-aware next step. `app setup` would be a second command,
outside the canonical 96, rendering the same data in a different shape — new surface to
document, test, and keep consistent, for no information a caller cannot already get.

The brief's own escape clause applies: *"If doctor + install-plugin already provide the same UX
cleanly, skip `app setup`."* They will. I will make `doctor`'s human output lead with a compact
status block so the first-run experience reads like the mock-up, without adding a command.

---

## 12. Recommended RC3 scope

In:

- Features 1, 2, 3, 4, 5 as specified above.
- The tests in §10 and a real-Zotero dogfood checklist covering "Thousands" / `A5XSZH5H`, run
  once with Zotero closed and once with it running.
- `docs/INSTALL.md` onboarding + discovery section.
- Version → `1.0.0-rc.3`, tagged only after automated gates, the full native CI matrix, and the
  dogfood test are green.

Out:

- `app setup` (§11).
- Phase 11 Agent Skill — deferred by design until this interface stabilises, which is the point
  of this package.
- The known RC2 `item merge --dry-run` + `current_library` issue. The new shared search/target
  infrastructure may well make a clean fix obvious; if so I will **report it separately** rather
  than fold it in, exactly as instructed.
- Python retirement, dynamic DOCX, LibreOffice/Word, CLI redesign, any database-safety change.

---

## Unresolved questions

1. **Row 5 classification** (§9): additive safety-divergence remediation, or a second canonical
   `Changed` entry? My recommendation is the former; it changes only bookkeeping, not code.
2. `--include-feeds` — worth shipping, or should feeds simply always be excluded from
   `--all-libraries` with no flag at all? I lean toward shipping it (explicit beats silent), but
   it is one more flag on the public surface.
