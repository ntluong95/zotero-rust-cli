// Moves an item to Zotero's trash (P.deleted = true) or restores it (false). Recoverable by
// design: this sets the same `deleted` flag the Zotero UI's "Move to Trash" / "Restore to
// Library" do, and never erases.
var item = Zotero.Items.getByLibraryAndKey(P.libraryID, P.key);
if (!item) { return 'ERROR: item ' + P.key + ' not found'; }
item.deleted = !!P.deleted;
await item.saveTx();
return 'OK: ' + (P.deleted ? 'trashed ' : 'restored ') + item.key;
