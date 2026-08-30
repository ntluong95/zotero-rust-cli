var s = new Zotero.Search();
s.libraryID = P.libraryID;
s.addCondition('DOI', 'is', P.doi);
var ids = await s.search();
var items = await Zotero.Items.getAsync(ids);
return items.slice(0, P.limit).filter(i => i.isRegularItem()).map(i => ({
  key: i.key,
  title: i.getField('title'),
  DOI: i.getField('DOI'),
  date: i.getField('date')
}));
