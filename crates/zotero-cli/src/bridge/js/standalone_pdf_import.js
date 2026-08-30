var att = await Zotero.Attachments.importFromFile({file: P.filePath});
if (!att) { return {ok:false, error:'importFromFile returned empty'}; }
att.setField('title', P.title);
await att.saveTx();
if (P.collectionKey) {
  var col = Zotero.Collections.getByLibraryAndKey(P.libraryID, P.collectionKey);
  if (col) { att.addToCollection(col.id); await att.saveTx(); }
}
if (P.tags) {
  for (var t of P.tags) { att.addTag(t); }
  await att.saveTx();
}
return {ok:true, key:att.key, title:att.getField('title')};
