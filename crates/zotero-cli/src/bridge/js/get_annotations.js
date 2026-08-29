var item = Zotero.Items.getByLibraryAndKey(P.libraryID, P.key);
if (!item) { return 'ERROR: item ' + P.key + ' not found'; }
if (item.isAttachment && item.isAttachment()) {
  var parent = Zotero.Items.get(item.parentItemID);
  if (!parent) { return 'ERROR: attachment has no parent item'; }
  item = parent;
}
var attIDs = item.getAttachments();
var allAnnots = [];
for (var aid of attIDs) {
  var att = Zotero.Items.get(aid);
  if (att && att.isPDFAttachment && att.isPDFAttachment()) {
    try {
      var annots = att.getAnnotations();
      allAnnots = allAnnots.concat(annots.map(a => ({
        type: a.annotationType,
        text: (a.annotationText || '').substring(0, 200),
        comment: a.annotationComment || '',
        color: a.annotationColor || '',
        page: a.annotationPageLabel || ''
      })));
    } catch (e) {}
  }
}
return {count: allAnnots.length, annotations: allAnnots};
