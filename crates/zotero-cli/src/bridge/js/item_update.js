var item = Zotero.Items.getByLibraryAndKey(P.libraryID, P.key);
if (!item) { return 'ERROR: item ' + P.key + ' not found'; }
for (var k in P.fields) {
  if (Object.prototype.hasOwnProperty.call(P.fields, k)) {
    item.setField(k, P.fields[k]);
  }
}
await item.saveTx();
return 'OK: updated ' + item.getField('title').substring(0, 60);
