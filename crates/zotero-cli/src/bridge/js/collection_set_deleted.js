// Moves a collection to the trash or restores it, exactly as the Zotero UI does. Never erases.
//
// Trash is Zotero's own path (`CollectionTree#deleteSelection`): `deleted = true` plus
// `save({deleteItems})`, whose `Collection#trash()` moves every descendant collection to the
// trash and, with deleteItems, every item in the whole subtree (a library without a trash
// erases them, as in Zotero).
//
// Restore mirrors `ZoteroPane#restoreSelectedItems` for a collection: the collection and every
// trashed descendant collection come back; items are never restored with a collection.
var col = Zotero.Collections.getByLibraryAndKey(P.libraryID, P.collectionKey);
if (!col) { return 'ERROR: collection ' + P.collectionKey + ' not found'; }
await Zotero.DB.executeTransaction(async function () {
  if (P.deleted) {
    col.deleted = true;
    await col.save({ deleteItems: !!P.includeItems });
    return;
  }
  var descendants = col.getDescendents(false, 'collection', true).map(function (d) { return d.id; });
  if (col.deleted) {
    col.deleted = false;
    await col.save();
  }
  var children = Zotero.Collections.get(descendants);
  for (var i = 0; i < children.length; i++) {
    if (children[i].deleted) {
      children[i].deleted = false;
      await children[i].save();
    }
  }
});
return 'OK: ' + (P.deleted ? 'trashed ' : 'restored ') + col.key;
