# zotero-cli

A native Rust CLI for working with a **local Zotero library** — built for AI
agents first, and usable by hand.

No Python. No Node. No runtime to install. One static binary, five platforms,
a stable `--json` contract on every command.

```bash
zotero-cli app doctor                                   # is everything wired up?
zotero-cli --json item find "olive region" --all-libraries
zotero-cli --json item context A5XSZH5H --include-notes
```

- **Install:** [`docs/INSTALL.md`](docs/INSTALL.md)
- **Using it from an AI agent:** [`docs/AGENTS.md`](docs/AGENTS.md)
- **Coming from the Python `cli-anything-zotero`:** [`docs/MIGRATION.md`](docs/MIGRATION.md)
- **How it talks to Zotero:** [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)
- **Security model:** [`docs/SECURITY.md`](docs/SECURITY.md)
- **Zotero version support:** [`docs/ZOTERO-COMPATIBILITY.md`](docs/ZOTERO-COMPATIBILITY.md)

## Quick start

1. Install the binary (see [`docs/INSTALL.md`](docs/INSTALL.md)) and confirm it runs:

   ```bash
   zotero-cli --version
   ```

2. Run the first diagnostic. This is always the right first command — it tells
   you what works right now and what, if anything, needs setting up:

   ```bash
   zotero-cli app doctor
   ```

3. Find something:

   ```bash
   zotero-cli --json item find "Thousands" --all-libraries
   ```

   Every result carries a `libraryID` and a `key`. Those two values are how you
   address the item in every follow-up command.

## What it does

| Area | Commands |
|---|---|
| Discovery | `item find` (incl. `--all-libraries`), `item list/get`, `collection find/list/tree/items`, `library list`, `tag list/items`, `search list/get/items`, `style list` |
| Item context | `item context`, `item children/notes/attachments/file`, `item metrics`, `item analyze` |
| Notes | `note get`, `note add` |
| PDFs, full text, annotations | `item find-pdf/fetch-pdf`, `collection find-pdfs/fetch-pdfs`, `item search-fulltext`, `item search-annotations`, `item annotations` |
| Citations & export | `item citation/bibliography/export`, `export bib`, `style list` |
| Ingest | `add doi/arxiv/file/bibtex/url`, `import file/json/doi/pmid` |
| Semantic search | `item build-index`, `item semantic-search`, `item similar` |
| Writes | `item update/tag/delete/attach/add-to-collection/move-to-collection/merge`, `collection create/rename/delete/remove-item`, `note add`, `sync` |
| Hygiene | `item duplicates`, `item merge` (preview by default) |
| DOCX (static) | `docx inspect-citations`, `docx inspect-placeholders`, `docx validate-placeholders`, `docx render-citations` |
| Session | `session use-library/use-collection/use-item/clear-*/status/history/use-selected` |
| App & audit | `app status/version/ping/doctor/launch/install-plugin/plugin-status/uninstall-plugin/authorize-local-api`, `audit path/tail` |
| Escape hatch | `js` (privileged raw Zotero JavaScript — expert/debugging only) |

`--json` is accepted at **every** level: `zotero-cli --json item find X`,
`zotero-cli item --json find X`, and `zotero-cli item find X --json` are all
equivalent.

Run `zotero-cli <group> <command> --help` for exact flags. Do not guess flags —
the help output is the contract.

## Live vs. offline

The CLI picks a backend per command based on what is actually available:

- **Zotero closed** → safe read-only SQLite, where the command supports it. Most
  reads work fine with Zotero shut.
- **Zotero running** → the live backend (Local API, Connector, or the CLI Bridge),
  where the command supports it.

When Zotero 10+ holds its database in WAL mode and the CLI cannot get a
*consistent* read, it **refuses loudly** rather than returning a silently stale
or partial answer. `item find` and `library list` will instead use an
already-running, fork-owned CLI Bridge to run Zotero's own read. The CLI never
writes to `zotero.sqlite` directly and never skips uncheckpointed WAL data.

Details and the underlying evidence: [`docs/ZOTERO-COMPATIBILITY.md`](docs/ZOTERO-COMPATIBILITY.md).

## The CLI Bridge (optional)

`zotero-cli` is a standalone binary. Many read operations need nothing else.

Some advanced live operations — privileged writes, `item merge --confirm`,
full-text/annotation search, `sync`, and the `js` escape hatch — go through the
**CLI Bridge**, a small Zotero plugin. The compatible XPI is bundled inside the
binary; you never need to hunt for a matching build on GitHub.

```bash
zotero-cli app doctor          # tells you if the Bridge is needed and missing
zotero-cli app install-plugin  # stages the bundled XPI and prints the install steps
```

Then in Zotero: **Tools → Plugins → gear icon → Install Add-on From File…** →
select the staged `.xpi` → restart Zotero. Full walkthrough in
[`docs/INSTALL.md`](docs/INSTALL.md#the-cli-bridge-plugin).

## Write safety

- Prefer **typed commands** (`item update`, `item tag`, `collection create`, …).
  They validate the target, pick an approved backend, verify the result, and
  write an audit entry.
- Writes require **user intent**. Local API writes additionally require a
  one-time human consent dialog inside Zotero, obtained with
  `zotero-cli app authorize-local-api`. The CLI never approves on your behalf.
- Destructive operations preview first where supported. `item merge` without
  `--confirm` is a zero-mutation dry run; `--confirm` performs the merge.
- `zotero-cli js` is an expert/debugging escape hatch, **not** a write fallback.
  If a typed write fails while `app doctor` reports the environment ready, that
  is a bug worth reporting — not a reason to mutate through raw JS.

## What is not in v1

Seven dynamic Zotero/DOCX commands (`docx cite`, `docx doctor`,
`docx insert-citations`, `docx prepare-zotero-import`, `docx zoterify`,
`docx zoterify-preflight`, `docx zoterify-probe`) are **deferred to post-v1** and
tracked in [issue #30](https://github.com/ntluong95/zotero-rust-cli/issues/30).
They need LibreOffice/Java/GUI automation that this release deliberately does not
ship. The four static DOCX commands above cover placeholder inspection,
validation, and static citation rendering without Word or LibreOffice.

`repl` is not ported: a blocking stdin read is the worst possible failure mode
for a non-interactive agent, so a bare `zotero-cli` prints help and exits 0.

## Credits and license

This is a Rust port of
[`PiaoyangGuohai1/cli-anything-zotero`](https://github.com/PiaoyangGuohai1/cli-anything-zotero)
(Apache-2.0), pinned to upstream commit `e42a930e` (v1.2.1). It is an
independent fork and is **not endorsed by** the upstream project or by Zotero.

- License: [`LICENSE`](LICENSE) (Apache-2.0)
- Statement of changes (Apache-2.0 §4(b)): [`NOTICE-CHANGES.md`](NOTICE-CHANGES.md)
- Dependency licenses: [`THIRD-PARTY-LICENSES.md`](THIRD-PARTY-LICENSES.md)
