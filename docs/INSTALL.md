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
