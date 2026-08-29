use serde_json::{json, Value};
use std::collections::HashMap;

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
pub const T_COLLECTION_STATS: &str = include_str!("js/collection_stats.js");
pub const T_FIND_PDF: &str = include_str!("js/find_pdf.js");
pub const T_FIND_PDF_FALLBACK: &str = include_str!("js/find_pdf_fallback.js");
pub const T_FIND_DUPLICATES: &str = include_str!("js/find_duplicates.js");
pub const T_ITEM_MERGE: &str = include_str!("js/item_merge.js");
pub const T_GET_ANNOTATIONS: &str = include_str!("js/get_annotations.js");
pub const T_SEARCH_FULLTEXT: &str = include_str!("js/search_fulltext.js");
pub const T_SEARCH_ANNOTATIONS: &str = include_str!("js/search_annotations.js");
pub const T_SYNC: &str = include_str!("js/sync.js");

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

pub fn render_collection_stats(
    library_id: u32,
    collection_key: &str,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "collectionKey": collection_key,
    });
    render(T_COLLECTION_STATS, &params)
}

pub fn render_find_pdf(library_id: u32, key: &str) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "key": key,
    });
    render(T_FIND_PDF, &params)
}

pub fn render_find_pdf_fallback(
    library_id: u32,
    key: &str,
    timeout: u64,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "key": key,
        "timeout": timeout,
    });
    render(T_FIND_PDF_FALLBACK, &params)
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

pub fn render_get_annotations(library_id: u32, key: &str) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "key": key,
    });
    render(T_GET_ANNOTATIONS, &params)
}

pub fn render_search_fulltext(
    library_id: u32,
    query: &str,
    limit: usize,
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
    query: Option<&str>,
    colors: Option<&[String]>,
    limit: usize,
) -> Result<String, serde_json::Error> {
    let params = json!({
        "libraryID": library_id,
        "query": query,
        "colors": colors,
        "limit": limit,
    });
    render(T_SEARCH_ANNOTATIONS, &params)
}

pub fn render_sync() -> &'static str {
    T_SYNC
}
