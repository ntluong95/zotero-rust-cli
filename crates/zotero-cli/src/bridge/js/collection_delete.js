// Permanently erases a collection and its descendant collections. With P.deleteItems, every
// item in the subtree is erased as well: Zotero's own erase only moves them to the trash
// (`Collection#_eraseData` -> `trash({deleteItems})`), so they are erased explicitly after it.
var col = Zotero.Collections.getByLibraryAndKey(P.libraryID, P.collectionKey);
if (!col) { return 'ERROR: collection ' + P.collectionKey + ' not found'; }
var name = col.name;
var itemIDs = P.deleteItems
  ? col.getDescendents(false, 'item', true).map(function (d) { return d.id; })
  : [];
await col.eraseTx({ deleteItems: !!P.deleteItems });
if (itemIDs.length) {
  await Zotero.Items.erase(itemIDs);
}
return 'DELETED: collection ' + name;
