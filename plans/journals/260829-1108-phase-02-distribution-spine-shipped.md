---
date: 2026-08-29 11:08
severity: medium
component: distribution-spine (Phase 2 of Rust port)
status: in-progress
---

# Phase 2 shipped: v0.1.0 released, attestation blocked us until the repo went public

**Date**: 2026-08-29 11:08
**Severity**: Medium
**Component**: `crates/zotero-cli` scaffold, `.github/workflows/ci.yml`, `.github/workflows/release.yml`, packaging manifests
**Status**: Ongoing (Phase 2 in-progress, not complete)

## What Happened

Entered via `/ak-plan continue the implementation`. `ak-plan`'s charter is plan-only — no code — so
it correctly handed off to `/ak:cook` to actually build Phase 2 ("Distribution Spine and Release
Pipeline") of the 13-phase `cli-anything-zotero` -> Rust port
(`plans/260829-0840-rust-port-of-cli-anything-zotero/`). Repo had no git history at all before this
session; initialized it and created `ntluong95/zotero-rust-cli` on GitHub.

Scaffolded a Cargo workspace (`crates/zotero-cli`, two `[[bin]]` targets — `zotero-cli` and the
`cli-anything-zotero` alias, matching upstream's two `console_scripts` pointing at one entrypoint),
wired in `rusqlite` (bundled) and `ureq` (rustls) immediately per the plan's own stated rationale:
surface cross-compilation pain on a binary that only prints its version, not after 500 call sites
depend on it. Built a 5-target CI matrix (build/test/fmt/clippy + binary-size and dynamic-library
gates) and a tag-triggered release workflow (build, ad-hoc macOS codesign, archive, checksum, GitHub
Artifact Attestation, `gh release create`).

A code-review subagent, run before any tag was pushed, caught five real bugs that local macOS-only
testing would never have surfaced: a Windows job assuming `python3` is on PATH, a missing Homebrew
Linux block, toolchain-pin drift between `rust-toolchain.toml` and the `dtolnay/rust-toolchain@stable`
action, an unreliable `7z` invocation under Windows Git Bash, and an over-granted `GITHUB_TOKEN` on
the build job. All five fixed and re-verified pre-tag (commit `21eca17`).

Then live infrastructure kicked us in the shins: the planned `macos-13` CI runner sat queued for
16-18 minutes on two separate runs (`33242456826`, `33243167458`) and both had to be cancelled —
GitHub is phasing down Intel-hosted runner capacity in favor of Apple Silicon. User decision: switch
to `macos-15-intel` and add a non-blocking `cross-compile-validation` job on `macos-14` that proves
ARM64->x86_64 ad-hoc cross-compilation works, as the documented fallback for whenever GitHub fully
retires Intel-hosted runners (their own announcement targets after August 2027).

Tagged and shipped `v0.1.0`. First release attempt failed outright: GitHub Artifact Attestation
returned "Feature not available for user-owned private repositories" — attestation is gated off for
private repos on personal accounts, full stop, no workaround in-workflow. User decision: make the
repo public rather than switch to GPG/minisign or ship archives unauthenticated. Re-ran; this time it
went clean — 5 target archives, `SHA256SUMS`, one valid SLSA v1 attestation
(published `2026-08-29T08:59:47Z`). Verified for real, not just "CI went green": downloaded the
release, `shasum -a 256 -c SHA256SUMS` (all OK), `gh attestation verify` (1 valid attestation),
extracted, ran both binaries, confirmed ad-hoc codesign held.

Also upgraded `THIRD-PARTY-LICENSES.md` from `cargo-license` (SPDX-id summary) to `cargo-about` (full
license text per dependency, `about.toml` as the accepted-license allowlist) per explicit user
decision, and replaced placeholder checksums in the Homebrew formula / Scoop manifest with the real
`v0.1.0` values, fixing actual `brew style` findings (desc length, file permissions) along the way.

Closed by syncing `plan.md` and `phase-02-*.md` status to `in-progress` — deliberately not
`completed`, because Homebrew/Scoop are unverified (no tap/bucket repo exists yet, that's a separate
repo-creation decision nobody has made) and there is no literal clean-machine (no Rust/Cargo/Python)
install verification yet.

## The Brutal Truth

The Intel-runner stall and the attestation-on-private-repo rejection were both things nobody could
have caught by reading the plan carefully — they only show up when you actually push a tag against
real GitHub infrastructure. Eighteen minutes of a CI run sitting queued, twice, before anyone admits
the runner pool is the problem, is a genuinely annoying way to lose half an hour. And "make the repo
public" as the fix for an attestation error is the kind of decision that feels small in a commit log
but is not small — it changes the project's default visibility permanently, and it happened because
a security feature (attestation) silently doesn't exist for the tier of account we're on, with no
error until you actually try to ship. That's a bad failure mode: it should have been a docs warning,
not a release-time surprise.

The good part: the code-review pass before tagging genuinely earned its keep. Five bugs — a
Windows-only `python3` assumption, an unreliable `7z` call, over-granted token perms — none of them
would have shown up on a Mac laptop running `cargo build`. They'd have shown up as a broken release
for exactly the platforms nobody on the team tests locally. Catching that before the tag instead of
after is the whole point of the review gate, and it worked as designed.

## Technical Details

- Error that forced the public-repo decision: `Feature not available for user-owned private
  repositories` from GitHub Artifact Attestation, on the first `v0.1.0` release attempt.
- Runner stalls: CI runs `33242456826` and `33243167458` both cancelled after 16-18 min queued on
  `macos-13`; GitHub Actions status confirmed Intel macOS runner capacity is being phased down.
- Fix: `.github/workflows/ci.yml` and `.github/workflows/release.yml` moved primary macOS Intel build
  to `runner: macos-15-intel`; added `cross-compile-validation` job pinned to `macos-14`, marked
  non-blocking, that cross-builds `x86_64-apple-darwin` from ARM64 as the documented fallback path.
- Release verified end-to-end: `v0.1.0` published `2026-08-29T08:59:47Z`, 5 archives +
  `SHA256SUMS`, `shasum -a 256 -c SHA256SUMS` all OK, `gh attestation verify` reports 1 valid SLSA v1
  attestation, both `zotero-cli` and `cli-anything-zotero` binaries run post-extraction.
- Commits this session: `8315615` (scaffold) -> `21eca17` (harden against Windows/Linux failure
  modes) -> `7e6945d` (Intel runner + license bundling follow-ups) -> `394d9ef` (real checksums,
  `brew style` clean) -> `b05f7cf` (status sync).

## What We Tried

- **`macos-13` runner** for the Intel build leg — rejected after two 16-18 min stalled/cancelled runs;
  replaced with `macos-15-intel`.
- **`cargo-license` for third-party notices** — rejected in favor of `cargo-about` because it only
  produced an SPDX-id summary, not the full license text upstream's Apache-2.0 §4b obligation
  arguably wants; user decision, not silently overridden.
- **Shipping `v0.1.0` from a private repo** — failed hard on the attestation step; considered
  GPG/minisign as an alternative signing path but user chose to make the repo public instead, since
  the project's actual objective is a public distribution pipeline anyway.

## Root Cause Analysis

Two separate root causes, both "the plan couldn't know this until we touched real infrastructure":

1. GitHub's Intel-hosted macOS runner pool is being drawn down. `macos-13` is the deprecated tier;
   `macos-15-intel` is what's actually schedulable today. The plan's original target matrix
   (written before this session, presumably from stale docs) named the wrong runner.
2. GitHub Artifact Attestation is a private-repo-tier feature gate that isn't documented anywhere
   prominent — it just returns a plain error at attestation time. Nothing in `gh` CLI output or the
   Actions UI warns you in advance that a personal-account private repo can't use it.

Neither is a coding mistake. Both are "infrastructure changed / was never fully documented, and the
plan's target matrix and the personal-account assumption were both stale by the time we shipped."

## Lessons Learned

- Don't trust a CI target matrix written before the session as gospel — a queued run sitting at 15+
  minutes on a `macos-*` runner is a signal to check GitHub's runner-image deprecation notices, not
  to keep waiting.
- If a release pipeline depends on GitHub Artifact Attestation (or any GitHub-tier-gated feature),
  verify the account/repo visibility tier supports it *before* writing the workflow step, not after
  the first failed tag.
- Running a code-review subagent before pushing a release tag caught cross-platform bugs invisible on
  local macOS dev — Windows `python3` PATH assumption, `7z` reliability, token over-grant. This is
  now the standard gate for any release-pipeline change, not optional polish.
- Changing repo visibility to solve an infra error is a real, user-facing decision (this repo is now
  permanently public) — it was surfaced to and made by the user, not silently applied. That's the
  right call every time this trade-off appears again.
- "Status: in-progress" in `phase-02-*.md` is honest and should stay that way until Homebrew tap +
  Scoop bucket + a literal clean-machine install are actually verified — don't let a green release
  tag get mistaken for "phase complete."

## Next Steps

- Owner: whoever picks up Phase 2 completion. Create `homebrew-zotero-rust-cli` tap repo and a Scoop
  bucket repo, then verify `brew install`/`scoop install` against them for real (currently only
  `brew style` lint and manifest checksums are verified, not an actual install).
- Run a literal clean-machine verification (no Rust, Cargo, or Python installed) of the release
  archives before marking Phase 2 `completed` — this is an explicit unmet success criterion.
- No `README.md` exists at the repo root and the repo is now public with no landing page. Flagged,
  not created — README creation requires an explicit user request per policy. Someone should ask for
  one soon given the repo just went public.
- Phase 1's remaining success criteria (cross-platform byte-identical fixture builds, parity harness
  running in CI) were not touched this session — still open, blocking real progress into Phase 3+.
- Phases 3-13 (the actual 96-command CLI port) have not started. Phase 2 proved the pipeline; nothing
  has been ported yet.
