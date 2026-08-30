var item = Zotero.Items.getByLibraryAndKey(P.libraryID, P.key);
if (!item) { return 'ERROR: item ' + P.key + ' not found'; }
var aids = item.getAttachments();
for (var id of aids) {
  var a = Zotero.Items.get(id);
  if (a && a.attachmentContentType === 'application/pdf') { return 'FOUND: ' + a.key; }
}
return 'TIMEOUT: PDF lookup timed out after ' + P.timeoutSecs + 's and no PDF attachment found yet. Zotero may still be downloading — retry shortly or check Zotero manually.';
