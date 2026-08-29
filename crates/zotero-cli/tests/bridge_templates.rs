#[path = "../src/bridge/mod.rs"]
mod bridge;

use bridge::templates::*;
use std::collections::HashMap;

#[test]
fn test_all_templates_include_non_empty_js() {
    // Slice 1b: 10 CRUD fallback templates
    assert!(!T_ITEM_UPDATE.is_empty());
    assert!(!T_ITEM_TAG.is_empty());
    assert!(!T_ITEM_DELETE.is_empty());
    assert!(!T_ITEM_ATTACH.is_empty());
    assert!(!T_ITEM_ADD_TO_COLLECTION.is_empty());
    assert!(!T_ITEM_MOVE_TO_COLLECTION.is_empty());
    assert!(!T_COLLECTION_CREATE.is_empty());
    assert!(!T_COLLECTION_RENAME.is_empty());
    assert!(!T_COLLECTION_DELETE.is_empty());
    assert!(!T_COLLECTION_REMOVE_ITEM.is_empty());

    // Slice 7: 3 confirmed privileged Bridge-only templates
    assert!(!T_FIND_DUPLICATES.is_empty());
    assert!(!T_ITEM_MERGE.is_empty());
    assert!(!T_SYNC.is_empty());
}

#[test]
fn test_render_item_update() {
    let mut fields = HashMap::new();
    fields.insert("title".to_string(), "Deep Learning Advances".to_string());
    fields.insert("date".to_string(), "2026-08".to_string());

    let js = render_item_update(1, "ITEM123", &fields).expect("render succeeds");
    assert!(js.starts_with("const P = JSON.parse("));
    assert!(js.contains("Zotero.Items.getByLibraryAndKey(P.libraryID, P.key)"));
    assert!(js.contains("OK: updated"));
}

#[test]
fn test_render_item_tag() {
    let add_tags = vec!["machine-learning".to_string(), "review".to_string()];
    let remove_tags = vec!["draft".to_string()];

    let js = render_item_tag(1, "ITEM123", &add_tags, &remove_tags).expect("render succeeds");
    assert!(js.starts_with("const P = JSON.parse("));
    assert!(js.contains("item.addTag"));
    assert!(js.contains("item.removeTag"));
}

#[test]
fn test_render_item_delete() {
    let js = render_item_delete(1, "ITEM123").expect("render succeeds");
    assert!(js.starts_with("const P = JSON.parse("));
    assert!(js.contains("DELETED:"));
}

#[test]
fn test_render_item_attach() {
    let js = render_item_attach(1, "ITEM123", "/path/to/paper.pdf").expect("render succeeds");
    assert!(js.starts_with("const P = JSON.parse("));
    assert!(js.contains("Zotero.Attachments.importFromFile"));
}

#[test]
fn test_render_item_add_to_collection() {
    let js = render_item_add_to_collection(1, "ITEM123", "COL456").expect("render succeeds");
    assert!(js.starts_with("const P = JSON.parse("));
    assert!(js.contains("item.addToCollection"));
}

#[test]
fn test_render_item_move_to_collection() {
    let js = render_item_move_to_collection(1, "ITEM123", "COL456", Some("COL789"))
        .expect("render succeeds");
    assert!(js.starts_with("const P = JSON.parse("));
    assert!(js.contains("item.removeFromCollection"));
    assert!(js.contains("item.addToCollection"));
}

#[test]
fn test_render_collection_create() {
    let js =
        render_collection_create(1, "New Collection", Some("PARENT1")).expect("render succeeds");
    assert!(js.starts_with("const P = JSON.parse("));
    assert!(js.contains("new Zotero.Collection()"));
}

#[test]
fn test_render_collection_rename() {
    let js = render_collection_rename(1, "COL456", Some("Renamed Collection"), None)
        .expect("render succeeds");
    assert!(js.starts_with("const P = JSON.parse("));
    assert!(js.contains("col.name = P.name"));
}

#[test]
fn test_render_collection_delete() {
    let js = render_collection_delete(1, "COL456", false).expect("render succeeds");
    assert!(js.starts_with("const P = JSON.parse("));
    assert!(js.contains("DELETED: collection"));
}

#[test]
fn test_render_collection_remove_item() {
    let js = render_collection_remove_item(1, "ITEM123", "COL456").expect("render succeeds");
    assert!(js.starts_with("const P = JSON.parse("));
    assert!(js.contains("item.removeFromCollection"));
}

#[test]
fn test_render_find_duplicates() {
    let js = render_find_duplicates(1, 50).expect("render succeeds");
    assert!(js.starts_with("const P = JSON.parse("));
    assert!(js.contains("new Zotero.Duplicates"));
}

#[test]
fn test_render_item_merge() {
    let others = vec!["ITEM456".to_string(), "ITEM789".to_string()];
    let js = render_item_merge(1, "TARGET1", &others).expect("render succeeds");
    assert!(js.starts_with("const P = JSON.parse("));
    assert!(js.contains("Zotero.Items.merge"));
}

#[test]
fn test_render_sync() {
    let js = render_sync();
    assert!(js.contains("Zotero.Sync.Runner.sync()"));
}
