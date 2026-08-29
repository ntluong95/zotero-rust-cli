var col = Zotero.Collections.getByLibraryAndKey(P.libraryID, P.collectionKey);
if (!col) { return 'ERROR: collection ' + P.collectionKey + ' not found'; }
if (P.name) { col.name = P.name; }
if (typeof P.parentKey !== 'undefined') {
  if (P.parentKey === null || P.parentKey === '') {
    col.parentID = false;
  } else {
    var parent = Zotero.Collections.getByLibraryAndKey(P.libraryID, P.parentKey);
    if (parent) { col.parentID = parent.id; }
  }
}
await col.saveTx();
return 'OK: updated collection ' + col.name;
