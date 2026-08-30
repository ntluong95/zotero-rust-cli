var parent = Zotero.Items.getByLibraryAndKey(P.libraryID, P.parentKey);
if (!parent) { return 'ERROR: parent item not found'; }
var note = new Zotero.Item('note');
note.libraryID = P.libraryID;
note.parentItemID = parent.id;
note.setNote(P.noteHtml);
await note.saveTx();
return {key: note.key, itemID: note.id, title: parent.getField('title').substring(0, 60)};
