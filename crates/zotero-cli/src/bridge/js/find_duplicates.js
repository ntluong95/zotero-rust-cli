// Zotero's own duplicate detector (the "Duplicate Items" pane), read-only.
//
// `_findDuplicates()` populates `dup._sets` and returns nothing by design. The set for an item is
// then read with `getSetItemsByItemID(itemID)` -- which *requires* an itemID. The previous
// template (inherited from upstream) called it with no argument, got `[undefined]` back, and
// reported `count: 0` for every library.
//
// An empty result is legitimate: `findAll()` only returns items that were paired with another.
// So the only way to tell "clean library" from "detector silently broken" is to check that the
// internals this relies on are still there and were actually populated by this call; anything
// else is reported as an error, never as a clean library.
try {
  var dup = new Zotero.Duplicates(P.libraryID);
  if (typeof dup._findDuplicates !== 'function' ||
      typeof dup.getSetItemsByItemID !== 'function' ||
      typeof Zotero.DisjointSetForest !== 'function') {
    throw new Error('Zotero duplicate detector internals changed (_findDuplicates/getSetItemsByItemID/DisjointSetForest missing)');
  }
  await dup._findDuplicates();
  if (!(dup._sets instanceof Zotero.DisjointSetForest) || typeof dup._sets.findAll !== 'function') {
    throw new Error('Zotero duplicate detector did not produce its duplicate sets (_sets)');
  }
  var ids = dup._sets.findAll(true);
  var scanned = await Zotero.DB.valueQueryAsync(
    "SELECT COUNT(*) FROM items i JOIN itemTypes t ON t.itemTypeID = i.itemTypeID "
    + "WHERE i.libraryID = ? AND t.typeName NOT IN ('attachment', 'note', 'annotation') "
    + "AND i.itemID NOT IN (SELECT itemID FROM deletedItems)",
    [P.libraryID]
  );
  var seen = {};
  var groups = [];
  for (var i = 0; i < ids.length; i++) {
    if (seen[ids[i]]) { continue; }
    var set = dup.getSetItemsByItemID(ids[i]);
    for (var j = 0; j < set.length; j++) { seen[set[j]] = true; }
    var members = set
      .map(function (id) { return Zotero.Items.get(id); })
      .filter(function (it) { return it && it.isRegularItem() && !it.deleted; });
    if (members.length < 2) { continue; }
    var setID = groups.length + 1;
    groups.push({
      setID: setID,
      count: members.length,
      items: members.map(function (it) {
        return {
          key: it.key,
          title: (it.getField('title') || '').substring(0, 80),
          date: it.getField('date'),
          setID: setID
        };
      })
    });
  }
  var flat = [];
  for (var k = 0; k < groups.length; k++) { flat = flat.concat(groups[k].items); }
  return {
    libraryID: P.libraryID,
    scanned: scanned,
    count: flat.length,
    items: flat.slice(0, P.limit),
    group_count: groups.length,
    groups: groups.slice(0, P.limit)
  };
} catch (e) {
  return {error: e.message, count: 0, items: []};
}
