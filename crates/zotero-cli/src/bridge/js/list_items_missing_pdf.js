var c = Zotero.Collections.getByLibraryAndKey(P.libraryID, P.key);
if (!c) { return {ok: false, error: 'collection ' + P.key + ' not found'}; }
var ids = c.getChildItems(true);
var items = ids.map(id => Zotero.Items.get(id)).filter(i => i && i.isRegularItem && i.isRegularItem());
var missing = [];
for (var item of items) {
  var aids = item.getAttachments();
  var hasPdf = false;
  for (var aid of aids) {
    var a = Zotero.Items.get(aid);
    if (a && a.attachmentContentType === 'application/pdf') { hasPdf = true; break; }
  }
  if (!hasPdf) {
    missing.push({key: item.key, title: item.getField('title'), DOI: item.getField('DOI') || ''});
  }
}
return {ok: true, total: items.length, missing: missing, missing_count: missing.length};
