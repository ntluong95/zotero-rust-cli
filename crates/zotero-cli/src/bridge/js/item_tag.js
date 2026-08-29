var item = Zotero.Items.getByLibraryAndKey(P.libraryID, P.key);
if (!item) { return 'ERROR: item ' + P.key + ' not found'; }
if (P.addTags && P.addTags.length) {
  for (var i = 0; i < P.addTags.length; i++) {
    item.addTag(P.addTags[i]);
  }
}
if (P.removeTags && P.removeTags.length) {
  for (var j = 0; j < P.removeTags.length; j++) {
    item.removeTag(P.removeTags[j]);
  }
}
await item.saveTx();
return 'OK: tags updated for ' + item.getField('title').substring(0, 60);
