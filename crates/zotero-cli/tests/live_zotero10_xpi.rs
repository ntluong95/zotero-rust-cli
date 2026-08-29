#[path = "../src/plugin/mod.rs"]
mod plugin;

#[path = "../src/bridge/mod.rs"]
mod bridge;

use bridge::JSBridgeClient;
use plugin::*;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

fn find_default_profile() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let zotero_dir = PathBuf::from(home).join("Library/Application Support/Zotero");
    let profiles_ini = zotero_dir.join("profiles.ini");
    if !profiles_ini.exists() {
        return None;
    }
    let content = std::fs::read_to_string(profiles_ini).ok()?;
    for line in content.lines() {
        if line.starts_with("Path=") {
            let rel = line.trim_start_matches("Path=").trim();
            let path = zotero_dir.join(rel);
            if path.exists() {
                return Some(path);
            }
        }
    }
    None
}

fn restart_zotero() {
    let _ = Command::new("pkill")
        .arg("-TERM")
        .arg("-x")
        .arg("zotero")
        .status();
    thread::sleep(Duration::from_secs(2));

    let app_path = "/Applications/Zotero.app/Contents/MacOS/zotero";
    if std::path::Path::new(app_path).exists() {
        let _ = Command::new(app_path).spawn();
    }

    let client = JSBridgeClient::new(23119);
    for _ in 0..30 {
        thread::sleep(Duration::from_millis(500));
        if client.bridge_endpoint_active() {
            break;
        }
    }
}

#[test]
fn test_live_zotero10_xpi_load_and_ownership() {
    let profile = match find_default_profile() {
        Some(p) => p,
        None => {
            println!("Skipping live test: No Zotero profile found");
            return;
        }
    };

    println!("Targeting test profile: {}", profile.display());

    // 1. Audit manifest before installation
    let manifest_val: serde_json::Value =
        serde_json::from_str(MANIFEST_JSON).expect("manifest.json is valid JSON");
    let app = &manifest_val["applications"]["zotero"];
    assert_eq!(app["id"].as_str().unwrap(), ADDON_ID);
    assert_eq!(app["strict_min_version"].as_str().unwrap(), "6.999");
    assert_eq!(app["strict_max_version"].as_str().unwrap(), "10.0.*");
    assert!(
        app["update_url"]
            .as_str()
            .unwrap()
            .contains("ntluong95/zotero-rust-cli"),
        "update_url must not point to upstream"
    );

    // 2. Audit bootstrap.js invariants
    assert!(BOOTSTRAP_JS.contains("permitBookmarklet: false"));
    assert!(BOOTSTRAP_JS.contains("supportedMethods: [\"POST\"]"));
    assert!(BOOTSTRAP_JS.contains("/cli-bridge/eval"));
    assert!(BOOTSTRAP_JS.contains("/cli-bridge/ownership"));

    // 3. Install XPI into profile
    let xpi_path = install_plugin(&profile).expect("Install XPI into profile");
    assert!(xpi_path.exists());
    assert_eq!(xpi_path.file_name().unwrap(), XPI_FILENAME);

    // 4. Restart Zotero
    restart_zotero();

    // 5. Verify Connector ping is 200 and running Zotero 10
    let resp = ureq::get("http://127.0.0.1:23119/connector/ping")
        .config()
        .timeout_global(Some(Duration::from_secs(5)))
        .http_status_as_error(false)
        .build()
        .call()
        .expect("Connector ping response");
    assert_eq!(resp.status().as_u16(), 200);

    let zotero_version = resp
        .headers()
        .get("X-Zotero-Version")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    println!("Live running Zotero version: {zotero_version}");
    assert!(
        zotero_version.starts_with("10.0") || zotero_version.starts_with("10."),
        "Running Zotero version must be Zotero 10"
    );

    // 6. Verify /cli-bridge/eval responds to ping
    let mut eval_resp = ureq::post("http://127.0.0.1:23119/cli-bridge/eval")
        .header("Content-Type", "text/plain")
        .config()
        .timeout_global(Some(Duration::from_secs(5)))
        .http_status_as_error(false)
        .build()
        .send("return 'ping';".as_bytes())
        .expect("eval ping response");
    assert_eq!(eval_resp.status().as_u16(), 200);

    let eval_bytes = eval_resp.body_mut().read_to_vec().expect("read eval body");
    let eval_json: serde_json::Value =
        serde_json::from_slice(&eval_bytes).expect("eval JSON payload");
    println!("Eval ping JSON response: {eval_json:?}");
    assert_eq!(eval_json["fork"].as_str().unwrap(), "zotero-rust-cli");
    assert_eq!(eval_json["id"].as_str().unwrap(), ADDON_ID);
    assert_eq!(eval_json["version"].as_str().unwrap(), "1.2.1");
    assert_eq!(eval_json["ownership"].as_str().unwrap(), "verified");

    // 7. Verify /cli-bridge/eval executes privileged JavaScript under Zotero 10
    let client = JSBridgeClient::new(23119);
    let js_resp = client.execute_js("return Zotero.version;", 5);
    assert!(js_resp.ok);
    assert_eq!(
        js_resp.data.as_ref().and_then(|v| v.as_str()),
        Some(zotero_version)
    );

    // 8. Verify /cli-bridge/ownership companion endpoint
    let mut own_resp = ureq::get("http://127.0.0.1:23119/cli-bridge/ownership")
        .config()
        .timeout_global(Some(Duration::from_secs(5)))
        .http_status_as_error(false)
        .build()
        .call()
        .expect("ownership response");
    assert_eq!(own_resp.status().as_u16(), 200);

    let own_bytes = own_resp
        .body_mut()
        .read_to_vec()
        .expect("read ownership body");
    let own_json: serde_json::Value = serde_json::from_slice(&own_bytes).expect("ownership JSON");
    println!("Ownership endpoint response: {own_json:?}");
    assert_eq!(own_json["fork"].as_str().unwrap(), "zotero-rust-cli");
    assert_eq!(own_json["id"].as_str().unwrap(), ADDON_ID);
    assert_eq!(own_json["version"].as_str().unwrap(), "1.2.1");
    assert_eq!(own_json["ownership"].as_str().unwrap(), "verified");

    // 9. Verify plugin_status reporting
    let status = plugin_status(Some(&profile), 23119);
    assert!(status.installed_on_disk);
    assert_eq!(
        status.installed_xpi_path.as_deref(),
        Some(xpi_path.to_str().unwrap())
    );
    assert!(!status.upstream_installed_on_disk);
    assert!(status.is_active);
    assert_eq!(
        status.ownership_status,
        OwnershipStatus::ActiveOurFork {
            version: "1.2.1".to_string(),
            id: ADDON_ID.to_string(),
        }
    );

    // 10. Verify that an un-forked / legacy response is not mistaken for our fork
    let foreign_response = OwnershipStatus::ActiveUpstreamPlugin { version: None };
    assert_ne!(status.ownership_status, foreign_response);

    // 11. Clean uninstall
    let uninstalled = uninstall_plugin(&profile).expect("uninstall plugin");
    assert!(uninstalled);
    assert!(!xpi_path.exists());
}
