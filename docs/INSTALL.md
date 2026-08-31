# Installing zotero-cli

No Python, pip, Node, Rust, or Cargo required — at install time or at run time.
`zotero-cli` is a single self-contained native binary.

Prebuilt binaries are published for five targets:

| Platform | Release target | Archive |
|---|---|---|
| macOS Apple Silicon | `aarch64-apple-darwin` | `.tar.gz` |
| macOS Intel | `x86_64-apple-darwin` | `.tar.gz` |
| Windows x86_64 | `x86_64-pc-windows-msvc` | `.zip` |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | `.tar.gz` |
| Linux arm64 | `aarch64-unknown-linux-gnu` | `.tar.gz` |

Every archive contains the `zotero-cli` binary, the `cli-anything-zotero` alias
binary, `LICENSE`, `NOTICE-CHANGES.md`, and `THIRD-PARTY-LICENSES.md`.

## After installing, run this first

```bash
zotero-cli --version
zotero-cli app doctor
```

`app doctor` is the recommended first diagnostic on any machine. It reports the
detected Zotero installation, data and profile directories, Connector and Local
API reachability, CLI Bridge state, and whether reads and writes are ready right
now — plus concrete next steps when something is missing.

---

## macOS

### Homebrew (recommended)

```bash
brew tap ntluong95/zotero-rust-cli
brew install zotero-cli
```

Homebrew strips the macOS quarantine attribute automatically, so this is the
cleanest install path — no Gatekeeper warning, and no paid Apple Developer
account required on our side.

### Direct download — Apple Silicon (`aarch64-apple-darwin`)

```bash
curl -LO https://github.com/ntluong95/zotero-rust-cli/releases/download/v1.0.0/zotero-cli-v1.0.0-aarch64-apple-darwin.tar.gz
tar xzf zotero-cli-v1.0.0-aarch64-apple-darwin.tar.gz
xattr -d com.apple.quarantine zotero-cli-v1.0.0-aarch64-apple-darwin/zotero-cli
sudo install -m 755 zotero-cli-v1.0.0-aarch64-apple-darwin/zotero-cli /usr/local/bin/zotero-cli
zotero-cli --version
```

### Direct download — Intel (`x86_64-apple-darwin`)

Identical, with `x86_64-apple-darwin` in place of `aarch64-apple-darwin`:

```bash
curl -LO https://github.com/ntluong95/zotero-rust-cli/releases/download/v1.0.0/zotero-cli-v1.0.0-x86_64-apple-darwin.tar.gz
tar xzf zotero-cli-v1.0.0-x86_64-apple-darwin.tar.gz
xattr -d com.apple.quarantine zotero-cli-v1.0.0-x86_64-apple-darwin/zotero-cli
sudo install -m 755 zotero-cli-v1.0.0-x86_64-apple-darwin/zotero-cli /usr/local/bin/zotero-cli
```

### macOS quarantine, explained

Release binaries are ad-hoc signed (`codesign --sign -`), which satisfies the
Apple Silicon "must be signed to run" requirement but does **not** clear
Gatekeeper's quarantine flag on files downloaded via a browser or `curl`. Without
clearing it you get *"cannot be opened because the developer cannot be verified"*.

Either run the `xattr -d com.apple.quarantine` line above, or right-click the
binary in Finder and choose **Open** once. Homebrew installs are unaffected.

---

## Windows x86_64 (`x86_64-pc-windows-msvc`)

### Scoop (recommended)

```powershell
scoop bucket add zotero-rust-cli https://github.com/ntluong95/zotero-rust-cli
scoop install zotero-cli
```

### Direct download

```powershell
Invoke-WebRequest -Uri https://github.com/ntluong95/zotero-rust-cli/releases/download/v1.0.0/zotero-cli-v1.0.0-x86_64-pc-windows-msvc.zip -OutFile zotero-cli.zip
Expand-Archive -Path zotero-cli.zip -DestinationPath .
```

Move `zotero-cli-v1.0.0-x86_64-pc-windows-msvc\zotero-cli.exe` somewhere
permanent (for example `%LOCALAPPDATA%\Programs\zotero-cli\`) and add that
directory to your user `PATH`:

```powershell
$dir = "$env:LOCALAPPDATA\Programs\zotero-cli"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
Copy-Item .\zotero-cli-v1.0.0-x86_64-pc-windows-msvc\*.exe $dir
[Environment]::SetEnvironmentVariable("Path", "$([Environment]::GetEnvironmentVariable('Path','User'));$dir", "User")
```

Open a new terminal, then:

```powershell
zotero-cli --version
```

---

## Linux

### x86_64 (`x86_64-unknown-linux-gnu`)

```bash
curl -LO https://github.com/ntluong95/zotero-rust-cli/releases/download/v1.0.0/zotero-cli-v1.0.0-x86_64-unknown-linux-gnu.tar.gz
tar xzf zotero-cli-v1.0.0-x86_64-unknown-linux-gnu.tar.gz
sudo install -m 755 zotero-cli-v1.0.0-x86_64-unknown-linux-gnu/zotero-cli /usr/local/bin/zotero-cli
zotero-cli --version
```

### arm64 (`aarch64-unknown-linux-gnu`)

```bash
curl -LO https://github.com/ntluong95/zotero-rust-cli/releases/download/v1.0.0/zotero-cli-v1.0.0-aarch64-unknown-linux-gnu.tar.gz
tar xzf zotero-cli-v1.0.0-aarch64-unknown-linux-gnu.tar.gz
sudo install -m 755 zotero-cli-v1.0.0-aarch64-unknown-linux-gnu/zotero-cli /usr/local/bin/zotero-cli
```

Without `sudo`, install to `~/.local/bin` instead and make sure it is on `PATH`:

```bash
install -m 755 zotero-cli-v1.0.0-x86_64-unknown-linux-gnu/zotero-cli ~/.local/bin/zotero-cli
```

---

## Verifying a downloaded release

Every release publishes `SHA256SUMS` plus a free, keyless
[GitHub Artifact Attestation](https://docs.github.com/en/actions/security-guides/using-artifact-attestations-to-establish-provenance-for-builds)
(Sigstore-backed — no paid signing account). Verify provenance with the GitHub
CLI:

```bash
gh attestation verify zotero-cli-v1.0.0-x86_64-unknown-linux-gnu.tar.gz --repo ntluong95/zotero-rust-cli
```

Or verify the checksum only:

```bash
shasum -a 256 -c SHA256SUMS
```

---

## The CLI Bridge plugin

**`zotero-cli` is a standalone native binary.** It is not a Zotero plugin, and
many operations need no plugin at all:

- Reads that work from the local Zotero database (`item get/list/find`,
  `collection *` reads, `library list`, `tag *`, `search list/get`, `session *`,
  `docx *`, `audit *`) work with Zotero closed and no Bridge installed.
- Reads and writes that Zotero's **Local API** can express work over HTTP against
  a running Zotero 10+, once authorized — again with no Bridge.

Some advanced live operations do require the **CLI Bridge**, a small Zotero
plugin that registers a `/cli-bridge/eval` endpoint on Zotero's own local HTTP
server. The Bridge is needed for privileged operations Zotero exposes only to
in-app JavaScript: `item merge --confirm`, `item attach`, `item search-fulltext`,
`item search-annotations`, `item annotations`, `item find-pdf`,
`collection stats`, `sync`, `import pmid`, `js`, and as the write fallback on
Zotero ≤9 where no Local API exists.

### The compatible XPI is bundled

You do **not** need to find a matching XPI on GitHub. The binary embeds the
version-matched CLI Bridge XPI and stages it for you.

### Onboarding flow

1. Ask the CLI what is missing:

   ```bash
   zotero-cli app doctor
   ```

   Read the `bridge.state` field. `healthy` means nothing to do. Anything else —
   `not_installed`, `staged_not_installed`, `installed_zotero_closed`,
   `installed_not_loaded`, `ownership_invalid`, `eval_failing` — names the exact
   problem, and `next_steps` tells you what to do about it.

2. If the Bridge is missing, stage the bundled XPI:

   ```bash
   zotero-cli app install-plugin
   ```

   This reports the staged `.xpi` path, the bundled version, the installed
   version when present, whether the Bridge is already installed, and ordered
   install steps. It writes **only** to the staging directory — it never writes
   into the Zotero profile and never bypasses Zotero's plugin consent.

3. Install it in Zotero, using the path `app install-plugin` printed:

   ```
   Zotero
     → Tools
     → Plugins
     → gear icon (top right of the Plugins window)
     → Install Add-on From File...
     → select the staged .xpi
     → restart Zotero
   ```

4. Confirm:

   ```bash
   zotero-cli app plugin-status
   zotero-cli app doctor
   ```

   `app plugin-status` reports whether the `/cli-bridge` endpoint is active and
   **who owns it** — this fork's Bridge, or the upstream Python project's. That
   ownership check is why two similarly-named plugins cannot silently shadow each
   other.

### Related app commands

| Command | What it does |
|---|---|
| `app doctor` | Full readiness diagnosis: Zotero, Connector, Local API, Bridge, read/write readiness, next steps |
| `app plugin-status` | Is the `/cli-bridge` endpoint active, and who owns it |
| `app install-plugin` | Stage the bundled XPI and print ordered install steps (`--output-dir` to choose where) |
| `app uninstall-plugin` | Remove the *staged* XPI artifact (does not uninstall an installed extension — do that in Zotero) |
| `app authorize-local-api` | Run the one-time Local API write-authorization handshake; blocks on Zotero's own consent dialog |
| `app launch` | Start the local Zotero desktop app and wait for the connector (`--wait-timeout`, default 30s) |

---

## Authorizing Local API writes

On Zotero 10+, writes can go through Zotero's Local API instead of the Bridge —
but only after a **human** approves it inside Zotero:

```bash
zotero-cli app authorize-local-api
```

This performs the real `POST /api/local/authorize` handshake and blocks on
Zotero's consent dialog. Approve it in the Zotero window. The resulting
credential is stored in a restrictive-permission local file beside your session
state (same threat model as `~/.netrc` or an SSH key); the CLI never prints it.

If authorization is missing, write commands stop with
`authorization_failed` / `needs_human_action`. The CLI never approves on your
behalf. See [`SECURITY.md`](SECURITY.md).

---

## Runtime behaviour: when the CLI starts Zotero, and when it does not

Some commands need a *live* Zotero (the Connector, the Local API, or the CLI
Bridge). When one of those is invoked and nothing is answering Zotero's HTTP
port, the CLI discovers and starts Zotero itself, waits for the specific backend
that command needs, and then continues — no `app launch` → wait → retry
choreography required.

It launches Zotero **at most once**, and only when Zotero appears to be closed.
If Zotero is already answering, no second process is ever started; a capability
that is missing on a running Zotero (Local API disabled, plugin not loaded) is
reported as that specific problem instead.

These never start anything, by design:

- `app doctor`, `app status`, `app ping`, `app version`, `app plugin-status` —
  diagnostics observe state, they do not change it. With Zotero closed they say so.
- Every read that works offline from the local database (`item get/list/find`,
  collection/library/tag reads, `session *`, `docx *`, `audit *`, `export *`).
  If Zotero already holds a WAL-mode database lock, `item find` and `library list`
  may use an already-running owned CLI Bridge instead; they still never autolaunch
  Zotero and never use stale SQLite reads.
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

---

## Finding items when you do not know the library

When you know which library you are working in, a plain search is enough:

```bash
zotero-cli --json item find "query"
```

When you do **not** know which library holds the item, search them all:

```bash
zotero-cli --json item find "query" --all-libraries
```

`--all-libraries` searches user and group libraries together, leaves your session
state untouched, and returns `libraryID` plus `key` for every match so follow-up
commands can target the right item.

Feed libraries are excluded by default, because feed entries are unsaved RSS
items rather than normal library items. Include them explicitly when you want
them:

```bash
zotero-cli --json item find "query" --all-libraries --include-feeds
```

`--all-libraries` cannot be combined with `--collection`: a collection already
belongs to exactly one library.

To turn a `libraryID` into something human-readable, use `library list`. It
includes a `name` field wherever Zotero stores or exposes one: `"My Library"` for
the personal library, group names from Zotero's `groups` table, feed names from
`feeds`, and `null` when no safe name source exists.

---

## Reading `app doctor`

`write_ready` means *at least one approved write backend is usable right now*.
`write_backends` says which: an authorized Local API, the owned CLI Bridge, or
both. The `bridge.state` field distinguishes the cases that a single boolean used
to blur together — `not_installed`, `staged_not_installed`,
`installed_zotero_closed`, `installed_not_loaded`, `ownership_invalid`,
`eval_failing`, `healthy` — and `bridge.port` reports the port Bridge commands in
that same invocation will use.

---

## A note on the `zotero-cli` command name

The upstream Python project (`cli-anything-zotero` on PyPI) also installs a
console script named `zotero-cli`. This project's repository, Homebrew tap, and
Scoop bucket are all named **zotero-rust-cli** specifically to avoid name
collisions in package registries — but the **installed command** is deliberately
still called `zotero-cli` (with `cli-anything-zotero` as an alias), because
existing AI-agent skills and scripts depend on that exact command name.

If you have both the Python and Rust versions installed, whichever was installed
or linked last generally wins on `PATH`. Check with `which -a zotero-cli`
(macOS/Linux) or `where.exe zotero-cli` (Windows). See
[`MIGRATION.md`](MIGRATION.md) for moving off the Python implementation.

---

## For AI agents

Prefer the typed, high-level commands. They validate the target, choose an
approved write backend, verify the result, and record an audit entry.

`zotero-cli js` is an expert/debugging escape hatch, **not** a write fallback.
If a supported high-level write command fails while `app doctor` reports the
environment ready, stop and report that contradiction — it is a bug. Do not
perform the mutation with raw JS instead: that bypasses the write routing and
safety checks the typed command exists to provide.

Full agent guidance: [`AGENTS.md`](AGENTS.md).
