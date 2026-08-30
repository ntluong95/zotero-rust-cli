try {
  var translate = new Zotero.Translate.Search();
  translate.setIdentifier({DOI: P.doi});
  var translators = await translate.getTranslators();
  if (!translators || !translators.length) {
    return {ok:false, code:'NO_TRANSLATOR', error:'No DOI translators available for ' + P.doi, DOI:P.doi};
  }
  translate.setTranslator(translators);
  var items = await translate.translate({libraryID: P.libraryID});
  if (!items || !items.length) {
    return {ok:false, code:'TRANSLATOR_EMPTY', error:'No items returned from any translator for DOI ' + P.doi, DOI:P.doi};
  }
  var item = items[0];
  if (P.collectionKey) {
    var col = Zotero.Collections.getByLibraryAndKey(P.libraryID, P.collectionKey);
    if (col) { item.addToCollection(col.id); }
  }
  if (P.tags) {
    for (var t of P.tags) { item.addTag(t); }
  }
  if (P.collectionKey || (P.tags && P.tags.length)) {
    await item.saveTx();
  }
  return {ok:true, code:'IMPORTED', key:item.key, title:item.getField('title'), DOI:item.getField('DOI') || P.doi, source:'zotero-translator'};
} catch (e) {
  return {ok:false, code:'TRANSLATOR_ERROR', error:(e && (e.message || e.toString && e.toString()) || String(e)), name:e && e.name || null, stack:e && e.stack ? String(e.stack).slice(0,500) : null};
}
