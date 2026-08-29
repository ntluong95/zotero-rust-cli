# Statement of Changes (Apache License 2.0, §4(b))

This project (`zotero-rust-cli`, binary name `zotero-cli`, alias
`cli-anything-zotero`) is a **behavioural/architectural port** of:

- **Upstream project:** [`PiaoyangGuohai1/cli-anything-zotero`](https://github.com/PiaoyangGuohai1/cli-anything-zotero)
- **Upstream copyright:** Copyright 2026 HKUDS CLI-Anything Team
- **Upstream license:** Apache License, Version 2.0
- **Pinned compatibility commit:** `e42a930e` (v1.2.1)

## Nature of the changes

This is not a line-by-line translation or a copy of upstream source files.
The command surface, JSON output contracts, exit codes, environment
variables, and default paths are reimplemented from scratch in Rust to be
behaviourally compatible with the pinned upstream commit, per the full
analysis in
[`plans/reports/xia-rust-port-analysis.md`](plans/reports/xia-rust-port-analysis.md)
and the command-by-command
[`plans/reports/compatibility-matrix.md`](plans/reports/compatibility-matrix.md).

The one component reused unchanged (not ported) is the Zotero
JavaScript/XPI bridge runtime logic, carried over byte-for-byte except for
`update_url`, the addon id, and any minimal ownership marker required to
distinguish this fork's bridge endpoint from upstream's — see Phase 6 of
the implementation plan.

Two structural defects present in the upstream implementation are fixed
rather than reproduced:

- JS built by string concatenation, escaping only `'` — replaced with
  `serde_json`-serialized parameters passed through `JSON.parse`.
- f-string SQL interpolation in the semantic-search module — replaced with
  bound parameters.

Three behaviours are intentionally changed from upstream; see the "Approved
intentional breaks" table in
[`plans/260829-0840-rust-port-of-cli-anything-zotero/plan.md`](plans/260829-0840-rust-port-of-cli-anything-zotero/plan.md)
for the full rationale of each.

## Attribution

Per Apache-2.0 §4(c)/(d), the upstream copyright and license notice is
reproduced in [`LICENSE`](LICENSE). Third-party Rust dependency licenses are
listed in `THIRD-PARTY-LICENSES.md`, generated per release by
`packaging/generate-third-party-licenses.sh`.
