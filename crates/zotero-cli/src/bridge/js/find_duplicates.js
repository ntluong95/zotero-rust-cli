try {
  var dup = new Zotero.Duplicates(P.libraryID);
  await dup._findDuplicates();
  var map = dup.getSetItemsByItemID();
  var itemIDs = Object.keys(map).map(Number).filter(Boolean);
  var items = itemIDs.map(id => Zotero.Items.get(id)).filter(i => i && !i.isAttachment() && !i.isNote());
  return {
    count: items.length,
    items: items.slice(0, P.limit).map(i => ({
      key: i.key,
      title: i.getField('title').substring(0, 80),
      date: i.getField('date'),
      setID: map[i.id]
    }))
  };
} catch (e) {
  return {error: e.message, count: 0, items: []};
}
