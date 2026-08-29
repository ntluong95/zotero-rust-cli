var col = new Zotero.Collection();
col.name = P.name;
col.libraryID = P.libraryID;
if (P.parentKey) {
  var parent = Zotero.Collections.getByLibraryAndKey(P.libraryID, P.parentKey);
  if (parent) { col.parentID = parent.id; }
}
await col.saveTx();
return {key: col.key, id: col.id, name: col.name, libraryID: P.libraryID};
