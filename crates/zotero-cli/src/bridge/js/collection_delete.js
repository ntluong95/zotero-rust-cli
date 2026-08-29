var col = Zotero.Collections.getByLibraryAndKey(P.libraryID, P.collectionKey);
if (!col) { return 'ERROR: collection ' + P.collectionKey + ' not found'; }
var name = col.name;
if (P.deleteItems) {
  await col.eraseTx();
} else {
  await col.eraseTx({deleteItems: false});
}
return 'DELETED: collection ' + name;
