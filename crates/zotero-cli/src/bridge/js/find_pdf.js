var item = Zotero.Items.getByLibraryAndKey(P.libraryID, P.key);
if (!item) { return 'ERROR: item ' + P.key + ' not found'; }
var att = await Zotero.Attachments.addAvailablePDF(item);
return att ? 'FOUND: ' + att.key : 'NOT_FOUND: no PDF available for ' + item.getField('title').substring(0, 60);
