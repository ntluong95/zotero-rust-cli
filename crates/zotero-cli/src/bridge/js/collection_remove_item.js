var item = Zotero.Items.getByLibraryAndKey(P.libraryID, P.itemKey);
if (!item) { return 'ERROR: item ' + P.itemKey + ' not found'; }
var col = Zotero.Collections.getByLibraryAndKey(P.libraryID, P.collectionKey);
if (!col) { return 'ERROR: collection ' + P.collectionKey + ' not found'; }
item.removeFromCollection(col.id);
await item.saveTx();
return 'OK: removed ' + item.getField('title').substring(0, 50) + ' from ' + col.name;
