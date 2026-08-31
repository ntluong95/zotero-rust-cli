# Installing zotero-cli

No Python, pip, Rust, or Cargo required. Prebuilt binaries: macOS
(Apple Silicon + Intel), Windows x86_64, Linux x86_64 + ARM64.

## macOS (recommended: Homebrew)

```bash
brew tap ntluong95/zotero-rust-cli
brew install zotero-cli
```

Homebrew strips the macOS quarantine attribute automatically, so this is
the cleanest install path — no Gatekeeper warning, no paid Apple Developer
account required on our side.

### macOS direct download (fallback)

Release binaries are ad-hoc signed (`codesign --sign -`), which satisfies
the Apple Silicon "must be signed to run" requirement but does **not**
clear Gatekeeper's quarantine flag on files downloaded from a browser or
`curl`. After downloading and extracting an archive from
[Releases](https://github.com/ntluong95/zotero-rust-cli/releases), run:

```bash
xattr -d com.apple.quarantine ./zotero-cli
```

or right-click the binary in Finder and choose **Open** the first time.

## Windows (recommended: Scoop)

```powershell
scoop bucket add zotero-rust-cli https://github.com/ntluong95/zotero-rust-cli
scoop install zotero-cli
```

## Linux

Download the archive for your architecture from
[Releases](https://github.com/ntluong95/zotero-rust-cli/releases), extract,
and place `zotero-cli` on your `PATH`:

```bash
tar xzf zotero-cli-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
sudo install -m 755 zotero-cli-vX.Y.Z-x86_64-unknown-linux-gnu/zotero-cli /usr/local/bin/zotero-cli
```

## Verifying a downloaded release

Every release publishes `SHA256SUMS` plus a free, keyless
[GitHub Artifact Attestation](https://docs.github.com/en/actions/security-guides/using-artifact-attestations-to-establish-provenance-for-builds)
(Sigstore-backed — no paid signing account). Verify provenance with the
GitHub CLI:

```bash
gh attestation verify zotero-cli-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz \
  --repo ntluong95/zotero-rust-cli
```

Or verify the checksum only:

```bash
shasum -a 256 -c SHA256SUMS
```

## A note on the `zotero-cli` command name

The upstream Python project (`cli-anything-zotero` on PyPI) also installs a
console script named `zotero-cli`. This project's repository, Homebrew tap,
and Scoop bucket are all named **zotero-rust-cli** specifically to avoid
name collisions in package registries — but the **installed command** is
deliberately still called `zotero-cli` (with `cli-anything-zotero` as an
alias), because existing AI-agent skills and scripts depend on that exact
command name (see the plan's Goal 2).

If you have both the Python and Rust versions installed, whichever was
installed or linked last generally wins on `PATH`. Check with
`which -a zotero-cli` (macOS/Linux) or `where.exe zotero-cli` (Windows), and
uninstall the Python version once you've migrated (see Phase 13 of the
implementation plan for retirement criteria).

## Runtime behaviour: when the CLI starts Zotero, and when it does not

Some commands need a *live* Zotero (the Connector, the Local API, or the CLI
Bridge). When one of those is invoked and nothing is answering Zotero's HTTP
port, the CLI discovers and starts Zotero itself, waits for the specific
backend that command needs, and then continues — no `app launch` → wait →
retry choreography required.

It launches Zotero **at most once**, and only when Zotero appears to be
closed. If Zotero is already answering, no second process is ever started; a
capability that is missing on a running Zotero (Local API disabled, plugin not
loaded) is reported as that specific problem instead.

These never start anything, by design:

- `app doctor`, `app status`, `app ping`, `app version`, `app plugin-status` —
  diagnostics observe state, they do not change it. With Zotero closed they say so.
- Every read that works offline from the local database (`item get/list/find`,
  collection/library/tag reads, `session *`, `docx *`, `audit *`, `export *`).
- `item merge` without `--confirm` — the default preview stays a zero-mutation,
  offline-capable dry run.

To suppress automatic launching entirely (headless machines, shared systems,
CI), set:

```bash
export ZOTERO_CLI_NO_AUTOLAUNCH=1
```

Commands that need a live backend then fail with a message telling you to start
Zotero yourself. `ZOTERO_CLI_LAUNCH_TIMEOUT` (seconds, default 60) bounds how
long the CLI waits for a freshly started Zotero to expose the backend.

### Automatic launch never grants consent

Starting Zotero is not the same as being allowed to write through it. Local API
writes still require the one-time human approval obtained with
`zotero-cli app authorize-local-api`, which shows Zotero's own consent dialog.
If that approval is missing, a write command stops and reports
`authorization_failed` / `needs_human_action` — the CLI never approves on your
behalf, and never prints stored credential material.

### Reading `app doctor`

`write_ready` means *at least one approved write backend is usable right now*.
`write_backends` says which: an authorized Local API, the owned CLI Bridge, or
both. The `bridge.state` field distinguishes the cases that a single boolean
used to blur together — `not_installed`, `installed_zotero_closed`,
`installed_not_loaded`, `ownership_invalid`, `eval_failing`, `healthy` — and
`bridge.port` reports the port Bridge commands in that same invocation will use.

### For AI agents

Prefer the typed, high-level commands. They validate the target, choose an
approved write backend, verify the result, and record an audit entry.

`zotero-cli js` is an expert/debugging escape hatch, **not** a write fallback.
If a supported high-level write command fails while `app doctor` reports the
environment ready, stop and report that contradiction — it is a bug. Do not
perform the mutation with raw JS instead: that bypasses the write routing and
safety checks the typed command exists to provide.
