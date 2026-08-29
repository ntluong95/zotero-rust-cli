var item = Zotero.Items.getByLibraryAndKey(P.libraryID, P.key);
if (!item) { return 'ERROR: item ' + P.key + ' not found'; }
var title = item.getField('title').substring(0, 60);
await item.eraseTx();
return 'DELETED: ' + title;
