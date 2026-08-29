---
phase: 2
title: "Distribution Spine and Release Pipeline"
status: todo
priority: P1
effort: "3-5d"
dependencies: []
---

# Phase 2: Distribution Spine and Release Pipeline

## Overview

Prove the entire build-and-ship pipeline on a **trivial binary that only prints its version**,
before 13,500 LOC depend on it. Distribution — not speed — is the justification for this port, so it
is validated first and cheaply.

Runs concurrently with Phase 1.

## Requirements

**Functional**
- Cargo workspace producing a `zotero-cli` binary and a `cli-anything-zotero` alias
- CI matrix building all five targets on every push
- Tagged releases publishing authenticated, checksummed archives to GitHub Releases
- Homebrew tap formula (macOS + Linux) and Scoop manifest (Windows)
- Install instructions requiring **no Rust, Cargo, Python or pip**

**Non-functional**
- Fully static or self-contained binaries — no system libsqlite3, no OpenSSL
- Binary size under 15 MB per target
- Reproducible: same tag builds the same checksums, or deviations are explained by signed provenance metadata

## Architecture

### Target matrix

| Target triple | Runner | Notes |
|---|---|---|
| `aarch64-apple-darwin` | `macos-14` | Primary target |
| `x86_64-apple-darwin` | `macos-15-intel` | Worth shipping: Intel Macs still common in research labs. `macos-13` was the original pick but is being phased down; `macos-15-intel` is GitHub's current dedicated Intel-hosted label. GitHub has announced Intel-hosted runner discontinuation after August 2027 — `ci.yml` carries a non-blocking `cross-compile-validation` job proving ARM64->x86_64 cross-compilation from a `macos-14` runner as the migration path when that happens. |
| `x86_64-pc-windows-msvc` | `windows-latest` | Upstream's own validation platform was Windows |
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` | Build against an old glibc or use `musl` |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` | Worth shipping for ARM servers/Raspberry Pi |

Prefer `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` if glibc compatibility proves
awkward — `rusqlite` with `bundled` and `ureq` with `rustls` have no C dependencies that block musl.

### Dependency choices that make this work

| Concern | Choice | Why it matters here |
|---|---|---|
| SQLite | `rusqlite` + `bundled` | Compiles SQLite from source; zero system dependency on any target |
| TLS | `ureq` + `rustls` | No OpenSSL; removes the single worst cross-compilation problem |
| Cross-compilation | Native runners per target | Simpler and more reliable than `cross` for five targets |

### Release layout

```
zotero-cli-v0.1.0-aarch64-apple-darwin.tar.gz
zotero-cli-v0.1.0-x86_64-apple-darwin.tar.gz
zotero-cli-v0.1.0-x86_64-pc-windows-msvc.zip
zotero-cli-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
zotero-cli-v0.1.0-aarch64-unknown-linux-gnu.tar.gz
SHA256SUMS
```

Each archive contains: the binary, the alias binary (or a documented symlink/shim), `LICENSE`,
`NOTICE-CHANGES.md` (Apache-2.0 §4b), and `THIRD-PARTY-LICENSES.md`.

### Release authentication

Unsigned checksums beside unsigned binaries do not authenticate a native binary. This phase must
choose one shippable path before release automation is considered complete:

| Option | Requirement |
|---|---|
| Sigstore/GitHub provenance | Generate and publish bundle/provenance; document `gh attestation verify` or `cosign verify-blob` |
| GPG/minisign | Publish public key, detached signatures, and verification commands |
| Platform package-manager trust only | Explicitly mark direct GitHub downloads unauthenticated and not recommended |

Success requires a user-verifiable authentication path. If signing credentials are unavailable,
direct-download releases are not production-ready.

**Decided:** Sigstore/GitHub provenance, via `actions/attest-build-provenance` — free, keyless,
requires no signing credentials to manage. This is a GitHub-hosted attestation service, not a
locally-generated file: it registers provenance against GitHub's transparency log rather than
emitting a `release-provenance.json` or `SHA256SUMS.sig` artifact (the release-layout diagram
above no longer lists either, since neither exists in this implementation). Verification is
`gh attestation verify <file> --repo <owner>/<repo>`, documented in `docs/INSTALL.md`.

### macOS signing and notarization

Unsigned binaries downloaded from GitHub are quarantined by Gatekeeper. Three options, decided in
this phase:

| Option | Cost | User experience |
|---|---|---|
| Developer ID signing + notarization | Paid Apple Developer account | Clean — no warning |
| Ad-hoc signing only | Free | Gatekeeper warning; user must right-click → Open or run `xattr -d` |
| Homebrew tap only | Free | Homebrew strips quarantine automatically — clean for `brew` users |

**Recommendation:** ship via Homebrew as the primary macOS channel (free, clean UX), with ad-hoc
signed direct downloads plus documented `xattr` instructions as the fallback. Revisit notarization
only if direct-download demand is real.

## Related Code Files

- Create: `Cargo.toml` (workspace), `crates/zotero-cli/Cargo.toml`
- Create: `crates/zotero-cli/src/main.rs` (version-only stub)
- Create: `.github/workflows/ci.yml` (build + test matrix)
- Create: `.github/workflows/release.yml` (tag-triggered)
- Create: `packaging/homebrew/zotero-cli.rb`
- Create: `packaging/scoop/zotero-cli.json`
- Create: `packaging/generate-third-party-licenses.sh` (`cargo-about`, full license-text bundling — decided over `cargo-license`'s SPDX-id-only summary) and `about.toml` (accepted-license allowlist)
- Create: `rust-toolchain.toml` (pinned stable)
- Create: `NOTICE-CHANGES.md`
- Create: `docs/INSTALL.md`

## Implementation Steps

1. Initialize the Cargo workspace with a single binary crate whose `main` prints
   `zotero-cli <version>` and exits 0.
2. Add `rusqlite` (bundled) and `ureq` (rustls) as dependencies **immediately**, even though
   unused — they are the two crates most likely to break cross-compilation, and this phase exists to
   discover that now rather than at Phase 4.
3. Write `ci.yml`: build and test on all five targets on push and PR.
4. Write `release.yml`: on tag, build all targets, strip, archive, generate `SHA256SUMS`, attach
   signatures or Sigstore provenance, and create a GitHub Release.
5. Verify binary size and that `ldd` / `otool -L` show no unexpected dynamic dependencies.
6. Produce the alias binary. Prefer a second `[[bin]]` target over a symlink — symlinks do not
   survive Windows zip extraction reliably.
7. Write and test the Homebrew formula against a real tap.
8. Write and test the Scoop manifest.
9. Decide and implement the macOS signing approach; document the outcome in `docs/INSTALL.md`.
10. Generate `THIRD-PARTY-LICENSES.md` and wire it into the release archives.
11. Write `NOTICE-CHANGES.md` stating this is a modified derivative of `cli-anything-zotero`
    (Apache-2.0), citing the upstream repo and commit `e42a930e`.
12. **End-to-end validation:** on a machine with no Rust, no Cargo, no Python — verify artifact
    authentication, download each artifact, install it, and run `zotero-cli --version`.

## Success Criteria

- [ ] All five targets build green in CI
- [ ] A tagged release produces five archives plus `SHA256SUMS` and signatures/provenance
- [ ] Verification instructions succeed for at least one direct-download artifact
- [ ] `brew install <tap>/zotero-cli` works on macOS ARM64 and Intel
- [ ] `scoop install zotero-cli` works on Windows
- [ ] Linux tarball extracts and runs on a clean container with no toolchain
- [ ] Verified on a machine with **no Rust, Cargo, Python or pip**: install succeeds and `--version` prints
- [ ] Binary under 15 MB per target
- [ ] No unexpected dynamic library dependencies (`otool -L` / `ldd` clean)
- [ ] macOS install path produces no Gatekeeper dead-end; the chosen approach is documented
- [ ] `LICENSE`, `NOTICE-CHANGES.md` and `THIRD-PARTY-LICENSES.md` present in every archive

## Risk Assessment

| Risk | Mitigation |
|---|---|
| `rusqlite bundled` fails to cross-compile on some target | Discovered in this phase by design; fall back to musl targets or per-target native runners |
| glibc version mismatch breaks older Linux distros | Build against oldest supported Ubuntu, or switch to musl |
| macOS notarization requires a paid account not available | Homebrew as primary channel makes this non-blocking; documented in Open Questions |
| Name collision with the upstream PyPI `zotero-cli` console script | Resolve in this phase — see plan Open Question 1; a user with both installed must get a deterministic result |
| Windows alias binary handling | Use a real second `[[bin]]` target, not a symlink |
| Release workflow leaks secrets into logs | Use GitHub OIDC / encrypted secrets; never echo signing material |
| Unsigned checksums create false confidence | Require detached signatures or Sigstore provenance, and document verification |
