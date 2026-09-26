---
phase: 4
title: "Bridge version cap, appDisabled, coexistence"
status: pending
priority: P1
effort: "4h"
dependencies: []
---

# Phase 4: Bridge version cap, appDisabled, and coexistence

## Goal

Keep the Bridge loading across all Zotero 10 releases. Make `app doctor` diagnose the two failure
modes it currently misreports as "Restart Zotero": a plugin disabled for version incompatibility,
and a conflicting upstream plugin.

## Context

- The manifest and `update.json` cap the plugin at `10.0.*`. The user decided on `10.*`.
- Zotero writes `appDisabled: true` to `<profile>/extensions.json` when it rejects the plugin. The
  doctor then reports `installed_not_loaded` and advises restarting Zotero, which loops forever.
  Upstream PR #9 fixes the same problem.
- Upstream `cli-bridge@cli-anything.dev` registers the same `/cli-bridge/eval` key. Its
  `shutdown()` deletes that key regardless of which plugin owns it. The fork's client safely
  refuses the foreign endpoint, but the doctor advises reinstalling this fork's plugin, which does
  not help.

## Files

- Modify: `crates/zotero-cli/src/plugin/assets/manifest.json`, setting `"strict_max_version": "10.*"`
  and bumping `version` to `1.2.2`.
- Modify: `update.json` at the repo root, with the same cap and version.
- Modify: `crates/zotero-cli/src/paths.rs` to add `plugin_app_disabled(profile_dir) -> Option<bool>`
  and `upstream_plugin_state(profile_dir) -> Option<{active, appDisabled}>`. Both parse
  `extensions.json` read-only.
- Modify: `crates/zotero-cli/src/doctor.rs` to add Bridge states `app_disabled` and
  `upstream_plugin_conflict`, add `checks.plugin.app_disabled` (same field name as upstream PR #9),
  and give each state its own `next_steps` text.
- Modify: `crates/zotero-cli/src/plugin/mod.rs` (`plugin_status` message).
- Modify: `tests/plugin_xpi.rs` and `tests/live_zotero10_xpi.rs`, where they assert the manifest
  cap.

## Steps

1. Update the cap and version in the manifest and `update.json`. Update the tests that assert
   `10.0.*`.
2. Add the read-only `extensions.json` parsing. Treat a missing or malformed file as `None`, never
   as an error.
3. Order the doctor states: `app_disabled` is checked before `installed_zotero_closed` and
   `installed_not_loaded`, and `upstream_plugin_conflict` is reported when the upstream id is
   `active`.
   - **`app_disabled` next step:** "Zotero disabled the CLI Bridge as incompatible with Zotero X.
     Upgrade zotero-cli, then `zotero-cli app install-plugin` and install it from Tools → Plugins."
   - **`upstream_plugin_conflict` next step:** "Disable or remove 'CLI Bridge for Zotero'
     (cli-bridge@cli-anything.dev) in Tools → Plugins, then restart Zotero."
4. Add unit tests with fixture `extensions.json` files for four cases: disabled, active-upstream,
   both installed, and a missing file.
5. Document the coexistence caveat in `docs/MIGRATION.md`: remove the Python Bridge plugin after
   migrating.

## Verification

- `cargo test -p zotero-cli doctor plugin`
- Live: the doctor stays `healthy` on 10.0.4. Rebuild the XPI and confirm that
  `extensions.json` shows `maxVersion: 10.*` after reinstalling it.

## Risk

`10.*` still blocks Zotero 11, which is intentional because 11 is untested. The new
`app_disabled` state makes that failure mode self-explaining.
