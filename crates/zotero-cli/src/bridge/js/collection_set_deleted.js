// Moves a collection and its descendants to/from trash. With P.includeItems, also changes
// the items directly in the requested collection. Never erases.
var col = Zotero.Collections.getByLibraryAndKey(P.libraryID, P.collectionKey);
if (!col) { return 'ERROR: collection ' + P.collectionKey + ' not found'; }
await Zotero.DB.executeTransaction(async function () {
  // Zotero's collection trash operation cascades to descendants, but restore does not.
  var descendants = P.deleted ? [] : col.getDescendents(false, 'collection', true);
  if (P.includeItems) {
    var items = col.getChildItems(false, true);
    for (var i = 0; i < items.length; i++) {
      items[i].deleted = !!P.deleted;
      await items[i].save();
    }
  }
  col.deleted = !!P.deleted;
  await col.save();
  for (var j = 0; j < descendants.length; j++) {
    var child = Zotero.Collections.get(descendants[j].id);
    child.deleted = false;
    await child.save();
  }
});
return 'OK: ' + (P.deleted ? 'trashed ' : 'restored ') + col.key;
