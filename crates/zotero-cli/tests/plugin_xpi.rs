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

    // Enforce: update_url must point at this fork's own merged, live update.json --
    // never absent, never HTTP, never an upstream/fake URL.
    const EXPECTED_UPDATE_URL: &str =
        "https://raw.githubusercontent.com/ntluong95/zotero-rust-cli/main/update.json";
    let update_url = app
        .get("update_url")
        .and_then(|v| v.as_str())
        .expect("Phase 6 manifest must declare update_url");
    assert_eq!(
        update_url, EXPECTED_UPDATE_URL,
        "update_url must be the exact repository-owned update.json URL"
    );
    assert!(
        update_url.starts_with("https://"),
        "update_url must use HTTPS"
    );
    assert!(
        update_url.starts_with("https://raw.githubusercontent.com/ntluong95/zotero-rust-cli/"),
        "update_url must be owned by this fork's own repository"
    );
    assert!(
        !manifest_str.contains("cli-anything.dev") && !manifest_str.contains("cli-anything-zotero"),
        "manifest.json must not contain upstream URLs or repo references"
    );
    // update_link is Zotero's binary-update-download field, distinct from update_url --
    // this compatibility-only update.json intentionally omits it (SS3.12/update.json scope),
    // and manifest.json itself never declares it either.
    assert!(
        !manifest_str.contains("update_link"),
        "manifest.json must not declare update_link"
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
fn test_xpi_staging_and_removal_in_neutral_output_dir() {
    let test_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    // Neutral output directory completely unrelated to any Zotero profile
    let temp_output_dir = std::env::temp_dir().join(format!("zotero_cli_artifacts_{test_id}"));
    std::fs::create_dir_all(&temp_output_dir).expect("create temp output dir");

    // Place an unrelated file in output_dir to prove remove_staged_xpi leaves it untouched
    let unrelated_file = temp_output_dir.join("other_artifact.txt");
    std::fs::write(&unrelated_file, b"unrelated artifact data").expect("write unrelated file");

    // 1. Stage XPI into neutral directory
    let staged_path = stage_xpi(&temp_output_dir).expect("staging succeeds");
    assert!(staged_path.exists());
    assert_eq!(staged_path.file_name().unwrap(), XPI_FILENAME);
    assert_eq!(staged_path.parent().unwrap(), temp_output_dir.as_path());
    assert!(std::fs::metadata(&staged_path).unwrap().len() > 0);

    // 2. Status when staged in output directory: file presence alone does NOT claim active runtime registration
    let status = plugin_status(Some(&temp_output_dir), 59999);
    assert_eq!(
        status.staged_xpi_path.as_deref(),
        Some(staged_path.to_str().unwrap())
    );
    assert!(
        !status.is_active,
        "staging XPI in artifact directory must not claim active runtime registration"
    );
    assert_eq!(status.ownership_status, OwnershipStatus::Inactive);

    // 3. Remove staged XPI from neutral directory
    let removed = remove_staged_xpi(&temp_output_dir).expect("removal succeeds");
    assert!(removed);
    assert!(!staged_path.exists());
    // Assert unrelated file was untouched
    assert!(
        unrelated_file.exists(),
        "remove_staged_xpi must leave unrelated files in output directory untouched"
    );

    // 4. Status after removal
    let status_after = plugin_status(Some(&temp_output_dir), 59999);
    assert!(status_after.staged_xpi_path.is_none());
    assert!(!status_after.is_active);

    let _ = std::fs::remove_dir_all(&temp_output_dir);
}
