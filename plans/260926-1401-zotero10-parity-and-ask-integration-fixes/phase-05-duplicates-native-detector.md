---
phase: 5
title: "item duplicates --by zotero fix"
status: pending
priority: P2
effort: "4h"
dependencies: []
---

# Phase 5: `item duplicates --by zotero` fix

## Goal

Make `item duplicates --by zotero` return Zotero's real duplicate sets for the session library,
and make it fail loudly instead of reporting a clean library. Also harden the DOI normalizer used
by `--by doi`.

## Context

- `bridge/js/find_duplicates.js` calls `dup.getSetItemsByItemID()` with no argument. Zotero's
  implementation (`xpcom/duplicates.js:89`) requires an `itemID`, so the call returns
  `[undefined]`, which the script turns into `count: 0` and exit 0 every time.
- The Rust side hard-codes library 1 for this path: `render_find_duplicates(1, limit)` in
  `hygiene.rs:216`.
- `hygiene.rs:62-68` strips only one resolver prefix. It misses `doi:`, `doi.acm.org`, and doubled
  prefixes such as `https://doi.org/https://doi.org/10.x/y`.

## Files

- Modify: `crates/zotero-cli/src/bridge/js/find_duplicates.js`.
- Modify: `crates/zotero-cli/src/hygiene.rs` (`find_duplicates_zotero`, the DOI normalizer).
- Modify: `crates/zotero-cli/src/lib.rs:825` to pass `session::session_library_id(&session, 1)?`.
- Modify: `crates/zotero-cli/src/bridge/templates.rs` (`render_find_duplicates`).

## Steps

1. Rewrite the template to use Zotero's own detector:

   ```js
   var dup = new Zotero.Duplicates(P.libraryID);
   await dup._findDuplicates();            // populates dup._sets; returns nothing by design
   var ids = dup._sets.findAll(true);
   var seen = new Set(), groups = [];
   for (var id of ids) {
     if (seen.has(id)) continue;
     var set = dup.getSetItemsByItemID(id);  // public; requires an itemID
     set.forEach(x => seen.add(x));
     var items = set.map(x => Zotero.Items.get(x)).filter(i => i && i.isRegularItem());
     if (items.length > 1) groups.push(items);
   }
   return { scanned: ids.length, groups: /* key,title,date per item */, ... };
   ```

   This keeps the private `_findDuplicates` call, as upstream does. It avoids `getSearchObject()`,
   which leaks a temporary table per call.
2. Keep the legacy output fields `count`, and `items[]` with `key`, `title`, `date`, and `setID`.
   Add `groups` and `scanned`. The field names match upstream PR #10's additive fields.
3. Return a non-zero exit with an `error` payload in three cases: the template returns `error`, the
   payload is malformed, or `scanned == 0` while the library holds regular items. Get the item
   count from the same template.
4. Pass the session library through. Keep default `--by doi`, as upstream v1.2.1 does.
5. Make the DOI normalizer extract the DOI by shape, using the regex `10\.\d{4,9}/\S+`. Apply it
   after URL-decoding, and strip trailing punctuation. Unit-test `doi:` prefixes, `dx.doi.org`,
   `doi.acm.org`, doubled prefixes, and mixed case.

## Verification

- `cargo test -p zotero-cli hygiene duplicates`
- Live on Zotero 10.0.4, with approval: create two copies of one throwaway item in a scratch
  collection. Confirm `item duplicates --by zotero --json` reports them as one group, then trash
  both copies (Phase 6 command).

## Risk

`_findDuplicates` is a private Zotero API and could change. Mitigation: the loud-failure rule turns
any future breakage into a visible error instead of a false "clean library".
