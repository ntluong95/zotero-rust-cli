# Migrating from the Python `cli-anything-zotero` to the native Rust `zotero-cli`

This guide is for anyone already using
[`PiaoyangGuohai1/cli-anything-zotero`](https://github.com/PiaoyangGuohai1/cli-anything-zotero)
(the Python implementation, pinned here at `e42a930e` / v1.2.1) who wants to move
to the native Rust binary.

**Short version:** the command surface is almost entirely the same, your session
state and audit log carry over untouched, and you no longer need Python.

---

## 1. No production Python runtime is needed

The Rust CLI is a single statically-linked native binary. Nothing in a supported
end-user path requires Python, pip, a virtualenv, Node, Rust, or Cargo:

- installing the release binary — no
- library and item reads — no
- writes — no
- the CLI Bridge — no
- Local API authorization — no
- the four static DOCX commands — no
- normal agent use — no

Python survives in this repository **only** as maintainer tooling: the parity
harness in `harness/` and the frozen upstream Python authority in `reference/`,
which together are how Rust behaviour is verified against the original. Neither
ships to users, and neither is on any install path.

## 2. Installation is now binary-based

Uninstall or ignore the Python package and install the binary instead:

| Before | After |
|---|---|
| `pip install cli-anything-zotero` | `brew install zotero-cli` (macOS), `scoop install zotero-cli` (Windows), or download a release archive |
| Requires a Python 3 interpreter and its dependency tree | Requires nothing |
| `python -m cli_anything.zotero ...` | `zotero-cli ...` |

Full per-platform instructions: [`INSTALL.md`](INSTALL.md).

### Both can coexist during the transition

The Rust binary installs as `zotero-cli` **and** `cli-anything-zotero` — the same
two names the Python package uses — so whichever was installed or linked last
wins on `PATH`. Check which one you are actually running:

```bash
which -a zotero-cli     # macOS/Linux
where.exe zotero-cli    # Windows
zotero-cli --version    # the Rust build prints "zotero-cli 1.0.0"
```

If you want both available at once, keep the Python one reachable through
`python -m cli_anything.zotero` and let `zotero-cli` mean the Rust binary.

## 3. The command families are largely unchanged

All 96 leaf commands of the pinned Python CLI were classified command-by-command
in [`../plans/reports/compatibility-matrix.md`](../plans/reports/compatibility-matrix.md).
The authoritative accounting for v1.0.0:

| Disposition | Count | Meaning |
|---|---|---|
| Integrated | 86 | Ported and certified in v1 |
| Changed | 1 | Deliberate behaviour change (`app check-update`) |
| Excluded | 1 | Implemented, but not certifiable against golden fixtures (`app enable-local-api` → `app authorize-local-api`) |
| Dropped | 1 | Not ported (`repl`) |
| Deferred | 7 | Post-v1 (the dynamic DOCX/zoterify chain) |
| Missing | 0 | Nothing is unaccounted for |
| **Total** | **96** | |

86 + 0 + 1 + 1 + 1 + 7 = 96.

Everything preserved verbatim from the Python contract:

| Contract | Detail |
|---|---|
| `--json` position | Accepted at root, group **and** command level |
| JSON error channel | In `--json` mode, errors print to **stdout** as `{"error": "..."}` |
| Human error channel | Without `--json`, errors go to stderr |
| Exit codes | `ok: false` → 1; `status` ∈ {`partial_success`, `error`, `failed`, `timeout`} → 1; else 0 |
| Result envelope | `{action, ok, status, code?, error?, ...}` |
| Session state | `~/.config/cli-anything-zotero/session.json` on **all** platforms, including Windows |
| Vector DB | `~/Zotero/cli-anything-vectors.sqlite` |
| Audit log | Same JSONL file and format — entries from both implementations interleave harmlessly |
| Binary names | `zotero-cli` (primary), `cli-anything-zotero` (alias) |
| Default port | `23119` fallback |

Because the session file, the vector database, the PDF resume state, and the
audit log all use the same on-disk formats, you can genuinely switch mid-workflow
— start something under Python and finish it under Rust.

## 4. Known intentional differences

These are deliberate, and none of them is a defect:

1. **`repl` is gone.** Bare `zotero-cli` prints help and exits 0 instead of
   entering an interactive shell. A blocking stdin read is the worst possible
   failure mode for a non-interactive agent.
2. **The macOS AppleScript bridge fallback is gone.** Install the CLI Bridge XPI
   instead — it is bundled in the binary and staged by
   `zotero-cli app install-plugin`. See
   [`INSTALL.md`](INSTALL.md#the-cli-bridge-plugin).
3. **The `--experimental` direct-SQLite write path is gone.** `collection create`,
   `item add-to-collection`, and `item move-to-collection` now go through the
   Local API or the JS Bridge. The CLI never writes to `zotero.sqlite` directly.
   `item move-to-collection` in particular now works with Zotero **running**
   (Python required it closed) and takes no `--experimental` flag.
4. **Seven DOCX commands are not ported yet** — see §6.
5. **`app check-update` no longer polls upstream.** A fork must not poll the
   upstream project's own version endpoint on your behalf. Use your package
   manager, or the
   [releases page](https://github.com/ntluong95/zotero-rust-cli/releases).
6. **`app enable-local-api` is now `app authorize-local-api`** and performs the
   real `POST /api/local/authorize` handshake, blocking on Zotero's own human
   consent dialog. See §5.
7. **`library list` gained a `name` field.** Additive; all eight original fields
   are unchanged and in place. This is the one command whose output is no longer
   byte-identical to Python's, and it is byte-identical-or-the-feature — not both.
8. **`item find` gained `--all-libraries` and `--include-feeds`.** Additive; with
   the flags absent, behaviour is unchanged.
9. **Cleaner failures.** A missing `zotero.sqlite`, or a numeric ref that
   overflows a 64-bit integer, returns a structured `{"error": ...}` with exit 1
   instead of Python's raw traceback. Both still exit 1.

A small number of further nuances (HTML entity decoding in legacy note content;
transport error prose in `app status` when nothing is listening at all) are
documented in the compatibility matrix's "Known, accepted divergences" section.

## 5. Bridge, Local API, and consent

Two things changed shape here, and both are worth understanding before you
migrate a script.

**The CLI Bridge XPI is bundled.** You do not download it separately, and you do
not look for a version-compatible build on GitHub:

```bash
zotero-cli app doctor          # reports bridge.state and what to do
zotero-cli app install-plugin  # stages the bundled XPI + prints install steps
zotero-cli app plugin-status   # is the endpoint live, and who owns it
```

Install through Zotero itself: **Tools → Plugins → gear icon → Install Add-on
From File… → select the staged `.xpi` → restart Zotero.**

The fork's Bridge uses its own addon id (`cli-bridge@cli-anything-rust.dev`) and
an ownership marker on the endpoint, so it cannot be confused with the upstream
Python project's bridge. `app plugin-status` reports ownership explicitly. Keep
only one active.

**Local API writes need one-time human authorization** on Zotero 10+:

```bash
zotero-cli app authorize-local-api
```

Approve the dialog inside Zotero. The credential is then stored in a
restrictive-permission local file beside your session state and is never printed.
Without it, writes stop with `authorization_failed` / `needs_human_action`.

On Zotero ≤9 there is no Local API at all — the CLI Bridge is the write path.

## 6. The seven deferred DOCX commands

Not in v1.0.0, deferred post-v1, tracked in
[issue #30](https://github.com/ntluong95/zotero-rust-cli/issues/30):

`docx cite` · `docx doctor` · `docx insert-citations` ·
`docx prepare-zotero-import` · `docx zoterify` · `docx zoterify-preflight` ·
`docx zoterify-probe`

They depend on LibreOffice, Java, and macOS GUI automation. These are **not
implemented** in the Rust CLI — they are absent, not stubbed.

What v1.0.0 *does* ship, as pure OOXML with no Word and no LibreOffice:

`docx inspect-citations` · `docx inspect-placeholders` ·
`docx validate-placeholders` · `docx render-citations`

For most placeholder-based workflows, `docx render-citations` is the answer: it
converts `{{zotero:ITEMKEY}}` placeholders into static citation and bibliography
text. If you specifically need live, editable Zotero citation fields, keep the
Python implementation for that one step during the transition — but note that a
Python fallback is explicitly **not** part of the Rust distribution's
no-Python promise.

## 7. JSON output is built for automation

The Rust CLI keeps Python's JSON contract byte-for-byte where it can, and it is
the interface to build on. If you are driving the CLI from a script or an AI
agent, read [`AGENTS.md`](AGENTS.md) — it covers the output contract, the
exit-code rules, cross-library discovery, key preservation, and write safety in
one place.

## 8. Environment variables

Implemented and behaving as in Python:

`ZOTERO_DATA_DIR` · `ZOTERO_PROFILE_DIR` · `ZOTERO_EXECUTABLE` ·
`ZOTERO_HTTP_PORT` · `CLI_ANYTHING_ZOTERO_STATE_DIR` · `ZOTERO_CLI_AUDIT_DIR` ·
`ZOTERO_EMBED_API` · `ZOTERO_EMBED_MODEL` · `ZOTERO_EMBED_KEY` ·
`ZOTERO_VECTOR_DB` · `OPENAI_API_KEY` · `CLI_ANYTHING_ZOTERO_OPENAI_URL`

New in the Rust CLI:

| Variable | Effect |
|---|---|
| `ZOTERO_CLI_NO_AUTOLAUNCH=1` | Never start Zotero automatically |
| `ZOTERO_CLI_LAUNCH_TIMEOUT` | Seconds to wait for a freshly launched Zotero (default 60) |
| `ZOTERO_LOCAL_API_KEY` | Operator-supplied Local API credential; the CLI reads it and never writes, modifies, or deletes it |

No longer meaningful, because their only consumer was dropped: `ZOTERO_LOCALE`
(AppleScript bridge), `NO_COLOR` and `CLI_ANYTHING_NO_COLOR` (REPL skin).

The full 17-variable audit, including two that turned out to be Python
test-harness-only rather than part of the CLI contract, is in the
[compatibility matrix](../plans/reports/compatibility-matrix.md#environment-variable-status-17-variable-inventory).

## 9. Migration checklist

```bash
# 1. Install the Rust binary and confirm you are running it.
zotero-cli --version                 # expect: zotero-cli 1.0.0

# 2. Check the environment end to end.
zotero-cli app doctor

# 3. If bridge.state is not "healthy" and you need Bridge-only commands:
zotero-cli app install-plugin
#    Zotero -> Tools -> Plugins -> gear -> Install Add-on From File... -> restart

# 4. If you write through the Local API on Zotero 10+:
zotero-cli app authorize-local-api

# 5. Confirm your existing session state carried over.
zotero-cli --json session status

# 6. Re-point scripts at `zotero-cli --json ...`; audit any use of
#    --experimental, `repl`, `app check-update`, or the 7 deferred docx commands.
```
