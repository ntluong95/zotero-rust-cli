#[path = "../src/plugin/mod.rs"]
mod plugin;

use plugin::*;
use std::io::Read;
use zip::ZipArchive;

#[test]
fn test_build_xpi_contains_valid_files() {
    let bytes = build_xpi().expect("XPI build succeeds");
    assert!(!bytes.is_empty());

    let cursor = std::io::Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor).expect("ZIP archive is valid");

    assert!(archive.len() >= 2);

    let manifest_str = {
        let mut manifest_entry = archive
            .by_name("manifest.json")
            .expect("manifest.json present");
        let mut s = String::new();
        manifest_entry
            .read_to_string(&mut s)
            .expect("manifest.json is readable");
        s
    };

    let manifest_val: serde_json::Value =
        serde_json::from_str(&manifest_str).expect("manifest.json is valid JSON");
    let app = &manifest_val["applications"]["zotero"];
    assert_eq!(app["id"].as_str().unwrap(), ADDON_ID);
    assert_eq!(app["strict_min_version"].as_str().unwrap(), "6.999");
    assert_eq!(app["strict_max_version"].as_str().unwrap(), "10.0.*");

    // Enforce: NO update_url in Phase 6 manifest (must not point to nonexistent or upstream update.json)
    assert!(
        app.get("update_url").is_none(),
        "Phase 6 manifest must not declare update_url"
    );
    assert!(
        !manifest_str.contains("update_url"),
        "manifest.json must not contain update_url key"
    );
    assert!(
        !manifest_str.contains("update.json"),
        "manifest.json must not reference update.json"
    );
    assert!(
        !manifest_str.contains("cli-anything-zotero"),
        "manifest.json must not contain upstream URLs or repo references"
    );

    let bootstrap_str = {
        let mut bootstrap_entry = archive
            .by_name("bootstrap.js")
            .expect("bootstrap.js present");
        let mut s = String::new();
        bootstrap_entry
            .read_to_string(&mut s)
            .expect("bootstrap.js is readable");
        s
    };

    assert!(bootstrap_str.contains("/cli-bridge/eval"));
    assert!(bootstrap_str.contains("/cli-bridge/ownership"));
    assert!(bootstrap_str.contains("permitBookmarklet: false"));
    assert!(bootstrap_str.contains("zotero-rust-cli"));
}

#[test]
fn test_plugin_install_and_uninstall_in_temp_dir() {
    let test_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_profile = std::env::temp_dir().join(format!("zotero_profile_test_{test_id}"));
    std::fs::create_dir_all(&temp_profile).expect("create temp profile dir");

    let profile_path = &temp_profile;

    // 1. Install
    let installed_path = install_plugin(profile_path).expect("installation succeeds");
    assert!(installed_path.exists());
    assert_eq!(installed_path.file_name().unwrap(), XPI_FILENAME);

    // 2. Status when installed but offline
    let status = plugin_status(Some(profile_path), 59999);
    assert!(status.installed_on_disk);
    assert!(status.installed_xpi_path.is_some());
    assert!(!status.upstream_installed_on_disk);
    assert!(!status.is_active);
    assert_eq!(status.ownership_status, OwnershipStatus::Inactive);

    // 3. Uninstall
    let removed = uninstall_plugin(profile_path).expect("uninstall succeeds");
    assert!(removed);
    assert!(!installed_path.exists());

    // 4. Status after uninstall
    let status_after = plugin_status(Some(profile_path), 59999);
    assert!(!status_after.installed_on_disk);
    assert!(status_after.installed_xpi_path.is_none());

    let _ = std::fs::remove_dir_all(&temp_profile);
}
