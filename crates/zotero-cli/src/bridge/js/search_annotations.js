var s = new Zotero.Search();
s.libraryID = P.libraryID;
if (P.query) {
  s.addCondition('annotationText', 'contains', P.query);
} else {
  s.addCondition('itemType', 'is', 'annotation');
}
var ids = await s.search();
var annots = await Zotero.Items.getAsync(ids);
var filtered = annots;
if (P.colors && P.colors.length) {
  filtered = annots.filter(a => P.colors.includes(a.annotationColor));
}
return filtered.slice(0, P.limit).map(a => {
  var parent = Zotero.Items.get(a.parentItemID);
  var grandparent = parent ? Zotero.Items.get(parent.parentItemID) : null;
  var title = grandparent ? grandparent.getField('title').substring(0, 60) : (parent ? parent.getField('title').substring(0, 60) : '');
  return {
    type: a.annotationType,
    text: (a.annotationText || '').substring(0, 200),
    comment: a.annotationComment || '',
    color: a.annotationColor || '',
    page: a.annotationPageLabel || '',
    parentTitle: title
  };
});
