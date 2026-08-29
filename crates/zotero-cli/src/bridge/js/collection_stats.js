var c = Zotero.Collections.getByLibraryAndKey(P.libraryID, P.collectionKey);
if (!c) { return 'ERROR: collection ' + P.collectionKey + ' not found'; }
var ids = c.getChildItems(true);
var items = ids.map(id => Zotero.Items.get(id)).filter(i => i && !i.isAttachment() && !i.isNote());
var total = items.length;
var withPDF = items.filter(i => i.getAttachments().some(aid => {
  var a = Zotero.Items.get(aid); return a && a.attachmentContentType === 'application/pdf';
})).length;
var years = {};
var journals = {};
items.forEach(i => {
  var y = (i.getField('date') || '').substring(0, 4);
  if (y) years[y] = (years[y] || 0) + 1;
  var j = i.getField('publicationTitle') || '';
  if (j) journals[j] = (journals[j] || 0) + 1;
});
return {
  total: total,
  withPDF: withPDF,
  noPDF: total - withPDF,
  byYear: years,
  topJournals: Object.entries(journals).sort((a, b) => b[1] - a[1]).slice(0, 10).map(e => ({journal: e[0], count: e[1]}))
};
