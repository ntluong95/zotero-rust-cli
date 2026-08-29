var s = new Zotero.Search();
s.libraryID = P.libraryID;
s.addCondition('fulltextContent', 'contains', P.query);
var ids = await s.search();
var items = await Zotero.Items.getAsync(ids);
return items.slice(0, P.limit).map(i => ({
  key: i.key,
  title: i.getField('title'),
  date: i.getField('date')
}));
