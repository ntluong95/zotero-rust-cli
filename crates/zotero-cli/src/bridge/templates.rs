use serde_json::{json, Value};
use std::collections::HashMap;

// In-scope JS templates for Phase 6:
// Slice 1b: CRUD Fallback set (≤9 templates)
pub const T_ITEM_UPDATE: &str = include_str!("js/item_update.js");
pub const T_ITEM_TAG: &str = include_str!("js/item_tag.js");
pub const T_ITEM_DELETE: &str = include_str!("js/item_delete.js");
pub const T_ITEM_ATTACH: &str = include_str!("js/item_attach.js");
pub const T_ITEM_ADD_TO_COLLECTION: &str = include_str!("js/item_add_to_collection.js");
pub const T_ITEM_MOVE_TO_COLLECTION: &str = include_str!("js/item_move_to_collection.js");
pub const T_COLLECTION_CREATE: &str = include_str!("js/collection_create.js");
pub const T_COLLECTION_RENAME: &str = include_str!("js/collection_rename.js");
pub const T_COLLECTION_DELETE: &str = include_str!("js/collection_delete.js");
pub const T_COLLECTION_REMOVE_ITEM: &str = include_str!("js/collection_remove_item.js");

// Slice 7: Confirmed privileged Bridge-only operations
pub const T_FIND_DUPLICATES: &str = include_str!("js/find_duplicates.js");
pub const T_ITEM_MERGE: &str = include_str!("js/item_merge.js");
pub const T_SYNC: &str = include_str!("js/sync.js");

// Phase 7 Slice 3: PDF cascade discovery primitives (`core/jsbridge.py`'s
// `find_pdf`/`list_items_missing_pdf` -- Zotero's own "Find Available PDF", not the
// open-access cascade, which lives entirely outside the Bridge in `pdf_fetch.rs`).
pub const T_FIND_PDF: &str = include_str!("js/find_pdf.js");
pub const T_FIND_PDF_VERIFY: &str = include_str!("js/find_pdf_verify.js");
pub const T_LIST_ITEMS_MISSING_PDF: &str = include_str!("js/list_items_missing_pdf.js");

// Phase 7 Slice 4: Note creation (`core/notes.py::add_note`'s inline JS block, factored through
// the shared `render`/`JSON.parse` mechanism so the note's normalized HTML -- arbitrary user
// content -- never gets string-interpolated into JS source).
pub const T_NOTE_ADD: &str = include_str!("js/note_add.js");

// Phase 7 Slice 5: Full-text/annotation search + annotation retrieval
// (`core/jsbridge.py`'s `search_fulltext`/`search_annotations`/`get_annotations`). All three
// delegate entirely to Zotero's live `Zotero.Search`/`Item.getAnnotations` APIs -- never Zotero's
// FTS SQLite tables -- and are read-only.
pub const T_SEARCH_FULLTEXT: &str = include_str!("js/search_fulltext.js");
pub const T_SEARCH_ANNOTATIONS: &str = include_str!("js/search_annotations.js");
pub const T_GET_ANNOTATIONS: &str = include_str!("js/get_annotations.js");

// Add/import composition Bridge primitives.
pub const T_FIND_ITEMS_BY_DOI: &str = include_str!("js/find_items_by_doi.js");
pub const T_IMPORT_FROM_DOI: &str = include_str!("js/import_from_doi.js");
pub const T_IMPORT_FROM_PMID: &str = include_str!("js/import_from_pmid.js");
pub const T_STANDALONE_PDF_IMPORT: &str = include_str!("js/standalone_pdf_import.js");

/// Renders a JavaScript snippet by binding `params` safely via `JSON.parse`
/// into the constant `P`.
///
/// This eliminates the D1 injection vulnerability entirely because
/// parameters are never interpolated into code or raw string literals;
/// instead `serde_json` serializes the payload to a valid JSON string,
/// which is then JSON-encoded into a JS string literal argument for `JSON.parse(...)`.
pub fn render(template: &str, params: &Value) -> Result<String, serde_json::Error> {
    let json_str = serde_json::to_string(params)?;
    let js_literal = serde_json::to_string(&json_str)?;
    Ok(format!(
        "const P = JSON.parse({});\n{}",
        js_literal, template
    ))
}

pub fn render_item_update(
    library_id: u32,
    key: &str,
    fields: &HashMap<String, String>,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "key": key,
        "fields": fields,
    });
    render(T_ITEM_UPDATE, &params)
}

pub fn render_item_tag(
    library_id: u32,
    key: &str,
    add_tags: &[String],
    remove_tags: &[String],
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "key": key,
        "addTags": add_tags,
        "removeTags": remove_tags,
    });
    render(T_ITEM_TAG, &params)
}

pub fn render_item_delete(library_id: u32, key: &str) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "key": key,
    });
    render(T_ITEM_DELETE, &params)
}

pub fn render_item_attach(
    library_id: u32,
    key: &str,
    file_path: &str,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "key": key,
        "filePath": file_path,
    });
    render(T_ITEM_ATTACH, &params)
}

pub fn render_item_add_to_collection(
    library_id: u32,
    item_key: &str,
    collection_key: &str,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "itemKey": item_key,
        "collectionKey": collection_key,
    });
    render(T_ITEM_ADD_TO_COLLECTION, &params)
}

pub fn render_item_move_to_collection(
    library_id: u32,
    item_key: &str,
    to_collection_key: &str,
    from_collection_key: Option<&str>,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "itemKey": item_key,
        "toCollectionKey": to_collection_key,
        "fromCollectionKey": from_collection_key,
    });
    render(T_ITEM_MOVE_TO_COLLECTION, &params)
}

pub fn render_collection_create(
    library_id: u32,
    name: &str,
    parent_key: Option<&str>,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "name": name,
        "parentKey": parent_key,
    });
    render(T_COLLECTION_CREATE, &params)
}

pub fn render_collection_rename(
    library_id: u32,
    collection_key: &str,
    name: Option<&str>,
    parent_key: Option<&str>,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "collectionKey": collection_key,
        "name": name,
        "parentKey": parent_key,
    });
    render(T_COLLECTION_RENAME, &params)
}

pub fn render_collection_delete(
    library_id: u32,
    collection_key: &str,
    delete_items: bool,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "collectionKey": collection_key,
        "deleteItems": delete_items,
    });
    render(T_COLLECTION_DELETE, &params)
}

pub fn render_collection_remove_item(
    library_id: u32,
    item_key: &str,
    collection_key: &str,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "itemKey": item_key,
        "collectionKey": collection_key,
    });
    render(T_COLLECTION_REMOVE_ITEM, &params)
}

pub fn render_find_duplicates(library_id: u32, limit: usize) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "limit": limit,
    });
    render(T_FIND_DUPLICATES, &params)
}

pub fn render_item_merge(
    library_id: u32,
    target_key: &str,
    other_keys: &[String],
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "targetKey": target_key,
        "otherKeys": other_keys,
    });
    render(T_ITEM_MERGE, &params)
}

pub fn render_sync() -> &'static str {
    T_SYNC
}

pub fn render_find_pdf(library_id: u32, key: &str) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "key": key,
    });
    render(T_FIND_PDF, &params)
}

pub fn render_find_pdf_verify(
    library_id: u32,
    key: &str,
    timeout_secs: u64,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "key": key,
        "timeoutSecs": timeout_secs,
    });
    render(T_FIND_PDF_VERIFY, &params)
}

pub fn render_list_items_missing_pdf(
    library_id: u32,
    key: &str,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "key": key,
    });
    render(T_LIST_ITEMS_MISSING_PDF, &params)
}

pub fn render_note_add(
    library_id: u32,
    parent_key: &str,
    note_html: &str,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "parentKey": parent_key,
        "noteHtml": note_html,
    });
    render(T_NOTE_ADD, &params)
}

/// `limit` is `i64`, not `usize`/`u32`: Python's `items.slice(0, limit)` (and this port's
/// `items.slice(0, P.limit)`) inherits JS's `Array.slice` semantics unclamped, where `0` yields
/// an empty result and a negative value counts back from the end. Do not validate or clamp this
/// value here -- Zotero's own JS engine is the authority on what it means.
pub fn render_search_fulltext(
    library_id: u32,
    query: &str,
    limit: i64,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "query": query,
        "limit": limit,
    });
    render(T_SEARCH_FULLTEXT, &params)
}

pub fn render_search_annotations(
    library_id: u32,
    query: &str,
    colors: Option<&[String]>,
    limit: i64,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "query": query,
        "colors": colors,
        "limit": limit,
    });
    render(T_SEARCH_ANNOTATIONS, &params)
}

pub fn render_get_annotations(library_id: u32, key: &str) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "key": key,
    });
    render(T_GET_ANNOTATIONS, &params)
}

pub fn render_find_items_by_doi(
    library_id: u32,
    doi: &str,
    limit: i64,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "doi": doi,
        "limit": limit,
    });
    render(T_FIND_ITEMS_BY_DOI, &params)
}

pub fn render_import_from_doi(
    library_id: u32,
    doi: &str,
    collection_key: Option<&str>,
    tags: Option<&[String]>,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "doi": doi,
        "collectionKey": collection_key,
        "tags": tags,
    });
    render(T_IMPORT_FROM_DOI, &params)
}

pub fn render_import_from_pmid(
    library_id: u32,
    pmid: &str,
    collection_key: Option<&str>,
    tags: Option<&[String]>,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "pmid": pmid,
        "collectionKey": collection_key,
        "tags": tags,
    });
    render(T_IMPORT_FROM_PMID, &params)
}

pub fn render_standalone_pdf_import(
    library_id: u32,
    file_path: &str,
    title: &str,
    collection_key: Option<&str>,
    tags: &[String],
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "filePath": file_path,
        "title": title,
        "collectionKey": collection_key,
        "tags": tags,
    });
    render(T_STANDALONE_PDF_IMPORT, &params)
}
