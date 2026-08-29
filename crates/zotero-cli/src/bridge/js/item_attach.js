var item = Zotero.Items.getByLibraryAndKey(P.libraryID, P.key);
if (!item) { return 'ERROR: item ' + P.key + ' not found'; }
var att = await Zotero.Attachments.importFromFile({file: P.filePath, parentItemID: item.id});
return 'OK: ' + att.key + ' attached to ' + item.getField('title').substring(0, 60);
