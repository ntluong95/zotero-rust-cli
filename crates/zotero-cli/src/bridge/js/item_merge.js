var target = Zotero.Items.getByLibraryAndKey(P.libraryID, P.targetKey);
if (!target) { return 'ERROR: target item ' + P.targetKey + ' not found'; }
var others = [];
for (var i = 0; i < P.otherKeys.length; i++) {
  var k = P.otherKeys[i];
  var it = Zotero.Items.getByLibraryAndKey(P.libraryID, k);
  if (!it) { return 'ERROR: item ' + k + ' not found'; }
  others.push(it);
}
await Zotero.Items.merge(target, others);
return 'OK: merged ' + others.length + ' items into ' + target.getField('title').substring(0, 60);
