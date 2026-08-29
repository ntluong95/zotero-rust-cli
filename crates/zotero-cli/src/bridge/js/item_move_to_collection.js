var item = Zotero.Items.getByLibraryAndKey(P.libraryID, P.itemKey);
if (!item) { return 'ERROR: item ' + P.itemKey + ' not found'; }
var toCol = Zotero.Collections.getByLibraryAndKey(P.libraryID, P.toCollectionKey);
if (!toCol) { return 'ERROR: collection ' + P.toCollectionKey + ' not found'; }

if (P.fromCollectionKey) {
  var fromCol = Zotero.Collections.getByLibraryAndKey(P.libraryID, P.fromCollectionKey);
  if (!fromCol) { return 'ERROR: source collection ' + P.fromCollectionKey + ' not found'; }
  item.removeFromCollection(fromCol.id);
} else {
  var currentCols = item.getCollections();
  for (var i = 0; i < currentCols.length; i++) {
    item.removeFromCollection(currentCols[i]);
  }
}
item.addToCollection(toCol.id);
await item.saveTx();
return 'OK: moved ' + item.getField('title').substring(0, 60) + ' to ' + toCol.name;
