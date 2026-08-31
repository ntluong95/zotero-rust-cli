# Using zotero-cli from an AI agent

`zotero-cli` is designed to be driven by a shell-capable AI agent — Claude Code,
Codex, Gemini CLI, OpenCode/OMP, or anything else that can run a subprocess and
parse JSON. This document is the contract those agents should rely on.

## The machine interface

Always use `--json`:

```bash
zotero-cli --json item find "planetary boundaries" --all-libraries
```

`--json` is accepted at **every** level (root, group, and command), so all three
of these are equivalent:

```bash
zotero-cli --json item find X
zotero-cli item --json find X
zotero-cli item find X --json
```

### Output contract

| Property | Contract |
|---|---|
| Success channel | stdout |
| Error channel in `--json` mode | **stdout**, as `{"error": "..."}` — not stderr |
| Error channel without `--json` | stderr |
| Result envelope | `{action, ok, status, code?, error?, ...}` for action-style commands; read commands return their data directly (e.g. `item find` returns a JSON array) |
| Exit code | `1` when `ok` is `false`, or when `status` is one of `partial_success`, `error`, `failed`, `timeout`; otherwise `0` |
| Encoding | UTF-8, 2-space indent, non-ASCII preserved |

Because errors arrive on **stdout** in `--json` mode, parse stdout first and only
treat a non-zero exit as fatal after checking for a structured `error`.

## Rules for agents

### 1. Probe before you act

```bash
zotero-cli --version
zotero-cli --json app doctor
```

`app doctor` answers, in one call: is Zotero installed and where, is it running,
is the Connector reachable, is the Local API available and authorized, is the CLI
Bridge installed/loaded/healthy, is the environment `read_ready`, is it
`write_ready`, and — if not — what `next_steps` would fix it.

Do not infer readiness from a failed command. Ask `app doctor`.

### 2. Search Zotero before searching the internet

When a user asks about a paper, a topic, or "what do I have on X", the local
library is the authoritative, private, zero-latency source. Check it first;
reach for external discovery only when Zotero genuinely has nothing.

### 3. Use `--all-libraries` when the library is unknown

```bash
# Library known (session-scoped or default):
zotero-cli --json item find "query"

# Library unknown — search user and group libraries together:
zotero-cli --json item find "query" --all-libraries

# Explicitly include RSS feed entries (excluded by default):
zotero-cli --json item find "query" --all-libraries --include-feeds
```

`--all-libraries` does not mutate session state. It cannot be combined with
`--collection` (a collection belongs to exactly one library).

### 4. Preserve item keys

Every result carries `libraryID` and `key`. The `key` is Zotero's stable
identifier — carry it verbatim through your reasoning and into follow-up
commands. Never re-derive an item from its title when you already hold its key.

`library list` gives you human-readable names for those `libraryID`s (`name` is
`"My Library"` for the personal library, the group name for groups, the feed name
for feeds, and `null` when Zotero exposes no safe name).

### 5. Pull real context instead of guessing

```bash
zotero-cli --json item context A5XSZH5H --include-notes --include-bibtex --include-csljson
zotero-cli --json item notes A5XSZH5H
zotero-cli --json item attachments A5XSZH5H          # includes resolvedPath
zotero-cli --json item annotations A5XSZH5H          # requires the CLI Bridge
zotero-cli --json item search-fulltext "term"        # requires the CLI Bridge
zotero-cli --json item search-annotations "term"     # requires the CLI Bridge
```

`item context` is the purpose-built LLM context builder: it assembles the item's
metadata plus optional notes, BibTeX, CSL-JSON, and links in a single call rather
than five.

### 6. Use typed high-level commands

There is a typed command for nearly everything. Use it. It validates the target,
picks an approved backend, verifies the result, and writes an audit entry that
`audit tail` can show the user afterwards.

### 7. Resolve exact syntax with `--help`; never guess flags

```bash
zotero-cli item --help
zotero-cli item find --help
```

The help output is the contract. Guessing a flag wastes a turn and can silently
select different behaviour.

## Write safety

**Writes require user intent.** Do not mutate a library because it would make a
task tidier. Mutate because the user asked.

**Preview first where supported.** `item merge` without `--confirm` is a
zero-mutation dry run that resolves the items and reports the merge plan:

```bash
zotero-cli --json item merge KEEPKEY MERGEKEY          # preview — mutates nothing
zotero-cli --json item merge KEEPKEY MERGEKEY --confirm # performs the merge
```

`item delete` and `collection delete` likewise take `--confirm`.

**Local API writes need one-time human consent.** If a write reports
`authorization_failed` / `needs_human_action`, tell the user to run:

```bash
zotero-cli app authorize-local-api
```

…and approve the dialog in Zotero. The CLI cannot and will not approve on the
user's behalf.

**Never fall back to raw JS.** `zotero-cli js` executes arbitrary privileged
JavaScript inside Zotero. It is an expert/debugging tool. If a typed write
command fails while `app doctor` reports the environment ready, that is a
contradiction worth reporting to the user — it is a bug. Reaching for `js`
instead bypasses write routing, target validation, result verification, and the
audit log. Do not do it silently, and do not do it at all without explicit user
instruction.

## Live vs. offline behaviour

| Zotero state | What happens |
|---|---|
| Closed | Safe read-only SQLite where the command supports it. Most reads work. |
| Running | Live backend (Local API / Connector / CLI Bridge) where the command supports it. |
| Running, WAL lock held, no Bridge | The CLI **refuses** the read rather than returning stale or partial data. |

That refusal is a safety property, not a bug. Do not work around it, do not
suggest the user work around it, and do not substitute a stale snapshot. `item
find` and `library list` already route around it correctly by using an
already-running, fork-owned CLI Bridge.

The CLI never writes to `zotero.sqlite` directly, never silently skips
uncheckpointed WAL data, and never requires an immutable stale snapshot.

### Autolaunch

Commands that need a live backend will start Zotero themselves (at most once,
only when it appears closed) and wait for the specific backend they need.
Diagnostics and offline-capable reads never start anything. Set
`ZOTERO_CLI_NO_AUTOLAUNCH=1` to disable this on headless or shared machines.

Starting Zotero does not grant write consent — those are separate gates.

## Bridge onboarding, for agents

If `app doctor` reports a `bridge.state` other than `healthy` and the task needs
a Bridge-only command, walk the user through it rather than improvising:

```bash
zotero-cli app install-plugin
```

Then tell them: **Zotero → Tools → Plugins → gear icon → Install Add-on From
File… → select the staged `.xpi` → restart Zotero.**

Never tell a user to search GitHub for a compatible XPI — the correct one is
bundled in the binary they already have.

## Known limitations

- **Seven dynamic DOCX commands are not in v1.** `docx cite`, `docx doctor`,
  `docx insert-citations`, `docx prepare-zotero-import`, `docx zoterify`,
  `docx zoterify-preflight`, `docx zoterify-probe` are deferred post-v1
  ([issue #30](https://github.com/ntluong95/zotero-rust-cli/issues/30)). They are
  not hidden or stubbed — they simply do not exist as commands. Do not suggest
  them.
- **Static DOCX support is available and sufficient for most work:**
  `docx inspect-citations`, `docx inspect-placeholders`,
  `docx validate-placeholders`, `docx render-citations`. These are pure OOXML and
  need neither Word nor LibreOffice.
- **No REPL.** A bare `zotero-cli` prints help and exits 0.
- **`app check-update` does not exist.** Updates come from the package manager or
  the GitHub releases page.
- **Bridge-only commands** (`item merge --confirm`, `item attach`,
  `item search-fulltext`, `item search-annotations`, `item annotations`,
  `item find-pdf`, `collection stats`, `sync`, `import pmid`, `js`) fail cleanly
  with an actionable message when the Bridge is absent. Read that message; do not
  retry blindly.
- **Semantic search requires an index.** Run `item build-index` before
  `item semantic-search` / `item similar`, and note it can send item text to a
  configured embedding endpoint — see [`SECURITY.md`](SECURITY.md).
- **`item analyze` sends item context to a configured LLM endpoint.** Treat it as
  an outbound data flow and say so to the user.

## A worked example

```bash
# 1. Confirm the environment.
zotero-cli --json app doctor

# 2. Find the item without knowing which library it lives in.
zotero-cli --json item find "Thousands turn out to support science" --all-libraries
#    -> [{ "libraryID": 7, "key": "A5XSZH5H", "title": "...", ... }]

# 3. Name that library for the user.
zotero-cli --json library list

# 4. Pull everything needed to reason about it.
zotero-cli --json item context A5XSZH5H --include-notes --include-csljson

# 5. Cite it.
zotero-cli --json item citation A5XSZH5H --style apa

# 6. Only if the user asked for it: write.
zotero-cli --json note add A5XSZH5H --text "Reviewed 2026-08-31." --format markdown

# 7. Show the user what was changed.
zotero-cli --json audit tail --limit 5
```
