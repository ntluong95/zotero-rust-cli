function summarize(item) {
  if (!item) return null;
  var tags = (item.getTags() || []).map(t => (t && (t.tag || t.name)) || String(t));
  var cols = (item.getCollections() || []).map(id => {
    var c = Zotero.Collections.get(id);
    return c ? {id: c.id, key: c.key, name: c.name} : {id: id};
  });
  var attachments = (item.getAttachments() || []).map(id => {
    var a = Zotero.Items.get(id);
    if (!a) return {id: id};
    return {key: a.key, title: a.getField('title'), contentType: a.attachmentContentType || '',
            filename: a.attachmentFilename || '', linkMode: a.attachmentLinkMode};
  });
  var notes = ((item.getNotes && item.getNotes()) || []).map(id => {
    var n = Zotero.Items.get(id);
    if (!n) return {id: id};
    var title = ''; try { title = n.getNoteTitle ? n.getNoteTitle() : n.getField('title'); } catch (e) {}
    return {key: n.key, title: (title || '').substring(0, 80)};
  });
  return {key: item.key, title: item.getField('title'), DOI: item.getField('DOI') || '',
          date: item.getField('date') || '', itemType: item.itemType,
          tags: tags, collections: cols, attachments: attachments, notes: notes,
          nAttachments: attachments.length, nNotes: notes.length, nTags: tags.length, nCollections: cols.length};
}
var keys = [P.keepKey].concat(P.mergeKeys);
var keep = Zotero.Items.getByLibraryAndKey(P.libraryID, keys[0]);
if (!keep) { return {ok:false, error:'keep item not found', keep: keys[0]}; }
var keepSum = summarize(keep);
var others = []; var missing = [];
for (var i = 1; i < keys.length; i++) {
  var it = Zotero.Items.getByLibraryAndKey(P.libraryID, keys[i]);
  if (!it) { missing.push(keys[i]); continue; }
  others.push(summarize(it));
}
var keepTagSet = new Set(keepSum.tags || []);
var keepColSet = new Set((keepSum.collections || []).map(c => c.key || String(c.id)));
var tagsToAdd = []; var colsToAdd = []; var attachmentsToMove = 0; var notesToMove = 0;
for (var o of others) {
  attachmentsToMove += (o.nAttachments || 0);
  notesToMove += (o.nNotes || 0);
  for (var t of (o.tags || [])) { if (!keepTagSet.has(t)) { tagsToAdd.push(t); keepTagSet.add(t); } }
  for (var c of (o.collections || [])) {
    var ck = c.key || String(c.id);
    if (!keepColSet.has(ck)) { colsToAdd.push(c); keepColSet.add(ck); }
  }
}
return {ok:true, keep: keepSum, others: others, missing: missing,
        will: {move_attachments: attachmentsToMove, move_notes: notesToMove,
               add_tags: tagsToAdd, add_collections: colsToAdd, trash_items: others.map(o => o.key)}};
