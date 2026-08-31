---
title: "RC2 CI Test Isolation Fix"
description: "Restore native CI by making the bridge orchestration test profile-explicit and host-independent."
status: pending
priority: P1
effort: 1.5h
branch: main
tags: [bugfix, ci, tests, rust]
blockedBy: []
blocks: []
created: 2026-08-31
---

# RC2 CI Test Isolation Fix

## Scope
- Outcome: fix post-RC2 native CI failure without changing product runtime behavior.
- In scope: test harness/profile fixture only; targeted regression proof; full local gates where possible.
- Out of scope: RC2 tag/release/history edits, release workflow changes, broad refactor, CI/test weakening.
- Related plan: `plans/260829-0840-rust-port-of-cli-anything-zotero/plan.md`; no blocking dependency.

## Evidence
- CI required gate runs fmt, clippy, build, release test per target: `.github/workflows/ci.yml:55`, `:58`, `:61`, `:64`.
- Release only builds/packages; no tests before publish: `.github/workflows/release.yml:43`, `:66`.
- Failing test creates data fixture only, then expects bridge healthy: `crates/zotero-cli/tests/runtime_orchestration.rs:485`, `:488`, `:500`, `:504`.
- `run_cli` isolates state/autolaunch but inherits host profile unless caller overrides: `crates/zotero-cli/tests/common/mod.rs:346`, `:360`, `:361`, `:362`, `:364`.
- Profile discovery honors `ZOTERO_PROFILE_DIR`, else probes real OS locations: `crates/zotero-cli/src/paths.rs:55`, `:104`, `:108`.
- `doctor` correctly returns `not_installed` when no XPI exists: `crates/zotero-cli/src/doctor.rs:13`, `:65`.
- Installed plugin check needs XPI under profile extensions: `crates/zotero-cli/src/paths.rs:457`, `:461`, `:467`.
- Reusable fake profile writes `prefs.js` and valid XPI manifest: `crates/zotero-cli/tests/local_app_audit.rs:66`, `:70`, `:73`, `:78`, `:90`.
- Docs say `app doctor` is diagnostic and bridge states include `not_installed` and `healthy`: `docs/INSTALL.md:96`, `:125`, `:129`.

## Chosen Fix
- Keep runtime logic unchanged. `doctor` behavior is correct.
- Move/create shared test helper in `crates/zotero-cli/tests/common/mod.rs`: fake Zotero profile with optional bridge XPI.
- In `common::run_cli`, set default `ZOTERO_PROFILE_DIR` to per-test empty profile root under `data_dir`; apply `extra_env` after default so explicit fake profiles still win.
- In `case_c_doctor_bridge_probe_success_is_followed_by_a_working_js_command`, create fake profile with installed bridge XPI and pass it to both `app doctor` and `js` calls.
- Optionally update `local_app_audit.rs` to use shared helper, deleting duplicate private helper; no behavior change.

## Data Flow
- Test fixture enters through `TestDir` -> SQLite data dir + fake profile dir.
- `run_cli` transforms test inputs into child env: `--data-dir`, `ZOTERO_HTTP_PORT`, isolated state, no autolaunch, isolated `ZOTERO_PROFILE_DIR`.
- Runtime builds profile/env from child env -> `paths::plugin_installed` sees fake XPI -> `doctor::run_doctor` probes scripted bridge -> emits `bridge.state="healthy"`.
- Follow-up `js` command receives same fake profile + port -> probes same scripted endpoint -> JSON command output.

## Phases
| Phase | Owner files | Depends | Done |
|---|---|---|---|
| 1. Harness isolation | `tests/common/mod.rs`, optional `tests/local_app_audit.rs` | none | shared fake-profile helper; default empty profile env |
| 2. Case C fixture | `tests/runtime_orchestration.rs` | phase 1 | explicit installed fake profile for doctor + js |
| 3. Validation | none | phases 1-2 | targeted fail/pass proof; full gates |

## TODO
- [ ] Reproduce fail before fix with empty profile env and targeted release test.
- [ ] Add/share fake profile helper; keep XPI manifest valid.
- [ ] Default `run_cli` profile to per-test empty root; ensure `extra_env` override order preserved.
- [ ] Update `case_c` to pass installed fake profile to both subprocesses.
- [ ] Run targeted release test with empty profile env.
- [ ] Run fmt, clippy, workspace debug tests, workspace release tests where local target support allows.

## Acceptance Criteria
- [ ] Before fix: `ZOTERO_PROFILE_DIR=/tmp/zotero-cli-empty-profile-for-ci-repro cargo test -p zotero-cli --test runtime_orchestration --release case_c_doctor_bridge_probe_success_is_followed_by_a_working_js_command -- --exact` fails with `not_installed`.
- [ ] After fix: same command passes without relying on host Zotero profile.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo test --workspace --release` passes.
- [ ] Native GitHub Actions matrix expected green; no release workflow change.

## Risks / Rollback
- High: default fake profile changes hidden assumptions in tests. Mitigate by allowing `extra_env` override and running `local_app_audit` plus workspace tests.
- Medium: helper extraction breaks audit tests. Mitigate by moving exact logic and preserving XPI filename/manifest.
- Low: Windows path behavior. Mitigate by using `PathBuf` and existing zip helper pattern.
- Rollback: revert changes in `tests/common/mod.rs`, `tests/local_app_audit.rs`, `tests/runtime_orchestration.rs`; CI returns to known failing state only.

## Unresolved Questions
None.
