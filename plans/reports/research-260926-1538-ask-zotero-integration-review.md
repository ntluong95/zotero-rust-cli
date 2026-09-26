# ASK × zotero-cli integration review

**Conducted:** 2026-09-26, Europe/Stockholm
**CLI:** `zotero-cli 1.0.0` (`~/.local/bin`, arm64; source `f92a769`)
**ASK surface:** `~/.claude/skills/ask-zotero` (contract revision 1.4, wrapper `scripts/zotero.mjs` v1.1.0), Node v24.13.1
**Zotero:** 10.0.4, which upgraded from 10.0.3 during the session. Tested both closed and running.
Local API enabled on port 23120. CLI Bridge 1.2.1 healthy. Better BibTeX present.

## Verdict

The integration does **not** work well yet on Zotero 10. Installation, provenance, diagnostics,
library listing, and search all work. However, **while Zotero is running, most reads that ASK
depends on fail**, and while Zotero is closed, bibliography rendering fails. The result is that
three ASK features cannot work end to end on this machine today:

- ASK's verified export transaction, used by `ask-litreview-scoping --export`
- bibliography export
- per-library lookup (`find --library`)

The root cause is in the CLI, not in ASK. The Zotero Local API can serve every one of the refused
reads live (verified: HTTP 200), but the CLI still sends them to SQLite. Separately, ASK's wrapper
has one bug of its own: it checks for existing records by DOI without the search scope that
matches DOIs, so its duplicate check never finds anything.

## Contents

1. [How ASK reaches Zotero](#1-how-ask-reaches-zotero)
2. [What works](#2-what-works)
3. [What fails](#3-what-fails)
4. [Next steps](#4-next-steps)
5. [Unresolved questions](#5-unresolved-questions)

## 1. How ASK reaches Zotero

Only `ask-zotero` talks to Zotero. `ask-litreview-scoping`, `ask-litreview-deep`, and
`ask-setup` delegate to it, and no other ASK skill calls `zotero-cli` directly.

```text
ask-litreview-* / ask-setup
        │
        ▼
ask-zotero/scripts/zotero.mjs  ──spawn──►  zotero-cli --json …
  (isolated session via CLI_ANYTHING_ZOTERO_STATE_DIR)
        │                                   ├─ SQLite (refuses when Zotero 10 is running)
        │                                   ├─ Local API (:23120)
        └─ fetch BBT JSON-RPC               └─ CLI Bridge (/cli-bridge/eval)
```

## 2. What works

| Check | Result |
|---|---|
| Provenance (`ask-setup` §5.1) | Both `zotero-cli` and `cli-anything-zotero` in `~/.local/bin` are the Rust Mach-O binary. No Python package is installed, so there is no command collision. |
| Bootstrap reference | The `scripts/install.sh` and `install.ps1` files cited by `ask-zotero/references/installation.md` exist in the repo. |
| `probe` | Every field `doctorSummary()` reads exists: `ready`, `read_ready`, `write_ready`, `write_backends`, `checks.bridge.{state,port,ok}`, `checks.plugin.*`, `checks.local_api.*`. The Bridge reports `healthy` on 10.0.4, so the `10.0.*` cap still holds. |
| `libraries` | Works with Zotero open or closed; lists 1 user library and several group libraries. |
| `find --all-libraries` | Uses the CLI's native `--all-libraries` support; works open or closed. |
| `find` (current library) | Works open or closed, because `item find` tries the Local API first. |
| Isolated session | `CLI_ANYTHING_ZOTERO_STATE_DIR` is honoured, so the user's persistent session is not touched. |
| Write target resolution | `item tag`, `note add`, and `item add-to-collection` resolve their target through the live backend. Tested with nonexistent keys, which returned "Item not found" rather than a lock error, and nothing was written. |
| `item merge` preview | Dry run with zero mutation works while Zotero is running. |
| Error contract | `--json` errors arrive on stdout with exit 1, which `cliError()` parses correctly. |
| `add doi` duplicate backstop | The CLI checks for an existing DOI through the Bridge (`if_exists=file` by default) and files the existing item instead of re-importing it (`add_import.rs:635-675`). |

## 3. What fails

### 3.1 CRITICAL (CLI): reads refuse while Zotero 10 is running

With Zotero 10 running, every command below failed with the WAL-lock refusal ("Zotero appears to
be running and holds an exclusive lock…"):

`session use-library`, `collection list/find/get/items`, `item get/attachments/children/notes`,
`note get`, `tag list`, `item context`, `item export`, `item citation`, `export bib`.

With Zotero closed, `item citation`, `item bibliography`, and `item export` fail the other way:
"Zotero Local API is not available". They did not auto-launch Zotero. **Citation and export
rendering therefore work in neither state on Zotero 10.**

Impact on ASK operations:

| ASK operation | Zotero running | Zotero closed |
|---|---|---|
| `collection-items`, `attachments`, `note-get` | ❌ lock refusal | ✅ |
| `find --library N` | ❌ its isolated `session use-library` fails | ✅ |
| `export` plan (read-only) | ❌ fails at the first step | ✅ plan produced |
| `export --confirmed` | ❌ | ❌ (inferred, see below) |
| Bibliography export (ASK contract: "Stable") | ❌ | ❌ |

The `export --confirmed` failure with Zotero closed is inferred, not observed. The write steps
need a live backend, and once Zotero is running, the transaction's own
`collection items` membership check hits the lock. This was not run, because it would write to
your library.

**Why it is fixable:** with Zotero running, the Local API returned HTTP 200 for
`collections/<key>`, `collections/<key>/items`, `items/<key>`, `items/<key>/children`, `tags`, and
`items/<key>?format=bibtex`. `docs/ZOTERO-COMPATIBILITY.md` already marks these reads as
"Local API confirmed sufficient (LIVE VERIFIED)". It also records that the routing to them
("Layer B") was specified but never wired into `catalog.rs`. The v1 smoke test checked `item find`
while Zotero was running but not the other reads, which is why this shipped.

### 3.2 HIGH (ASK wrapper): the duplicate lookup never matches a DOI

`findExistingRecord()` (`zotero.mjs:1151`) runs `item find <identifier>` with the default
`--scope titleCreatorYear`. Quick search in that mode does not search the DOI field. Live checks
against a DOI that is in library 1:

| Command | Matches |
|---|---|
| `item find "10.1016/s2542-5196(24)00273-0"` | 0 |
| `… --scope fields` | 1 (`8CGMWH6E`) |
| `… --scope everything --all-libraries` | 1 |

The consequences differ by identifier type:

- **DOI records:** the CLI's own `add doi` duplicate check catches them, so no duplicate item is
  created. ASK still reports them as `imported` instead of `reused`.
- **DOI records when the Bridge call fails:** `existing_transport.ok == false` makes `add doi` skip
  its duplicate check, so it imports a duplicate.
- **PMID, arXiv, and URL records:** there is no CLI backstop, so re-running an export creates
  duplicates. This breaks ASK's "convergent/idempotent" export contract.

**Fix:** add `"--scope", "fields"` to the `item find` call in `findExistingRecord()`.

### 3.3 LOW (ASK wrapper): `bbt-probe` checks the wrong port

Run on its own, `bbt-probe` defaults to port 23119 and reports `available: false`. This Zotero
listens on 23120, and `probe`, which takes the port from `doctor`, correctly reports Better BibTeX
as present. Separately, BBT has no `ping` method, so "available" really means "the endpoint
answered".

**Fix:** derive the port from `zotero-cli --json app status` (`.port`) when `ZOTERO_HTTP_PORT` is
unset.

### 3.4 Carried over from the parity review

`search list` and `search get` fail on Zotero 10 because of the dropped `required` column (see
`research-260926-1538-upstream-parity-and-zotero-10-review.md`). ASK does not call these commands,
so this does not affect ASK directly.

## 4. Next steps

1. **CLI, v1.0.1 (blocks ASK):** when SQLite refuses on a locked WAL database, route these reads to
   the Local API instead:
   - `collection list/find/get/items`, `item get/children/attachments/notes`, `note get`, `tag list`
   - `session use-library`: validate the library through the same live path that `library list`
     already uses
   - the item-resolution step of `item citation/bibliography/export`, `export bib`, and
   `item context`

   Reuse `item find`'s existing Local-API-first pattern. Group libraries need a
   `libraryID → groupID` mapping for `/api/groups/<id>/…`. Ship this together with the
   `search list` fix.
2. **CLI:** make `item citation/bibliography/export` auto-launch Zotero when it is closed, as the
   docs promise for live-backend commands. Otherwise, return an explicit "start Zotero" status that
   ASK can act on.
3. **ASK wrapper:** add `--scope fields` in `findExistingRecord()`, and derive the BBT port from
   `app status`.
4. **Verify the fix:** rerun this review's matrix with Zotero open and closed, then run one
   approved `export --confirmed` into a throwaway collection and check that re-running it is
   idempotent.
5. **Until then:** close Zotero for ASK reads (collection items, attachments, notes, per-library
   find). ASK's export and bibliography flows should be treated as unavailable on Zotero 10.

## 5. Unresolved questions

- Does the Local API expose group libraries under `/api/groups/<groupID>/…` with the IDs the CLI
  can map from `libraryID`? This was not verified; the only group URL probed was malformed by the
  test script.
- Should the export membership check tolerate eventual consistency? It re-reads the collection
  immediately after writing, and the Local API's read-after-write behaviour has not been verified
  for that.
- Should ASK's `ask-zotero/references/api-surface.md` stop describing the CLI as "RC2" now that
  v1.0.0 is installed?
