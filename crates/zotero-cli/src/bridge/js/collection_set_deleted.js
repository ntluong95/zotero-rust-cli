// Moves a collection to Zotero's trash (P.deleted = true) or restores it (false). With
// P.includeItems, the items directly in the collection are trashed/restored with it. Never erases.
var col = Zotero.Collections.getByLibraryAndKey(P.libraryID, P.collectionKey);
if (!col) { return 'ERROR: collection ' + P.collectionKey + ' not found'; }
if (P.includeItems) {
  var items = col.getChildItems(false, true);
  for (var i = 0; i < items.length; i++) {
    items[i].deleted = !!P.deleted;
    await items[i].saveTx();
  }
}
col.deleted = !!P.deleted;
await col.saveTx();
return 'OK: ' + (P.deleted ? 'trashed ' : 'restored ') + col.key;
