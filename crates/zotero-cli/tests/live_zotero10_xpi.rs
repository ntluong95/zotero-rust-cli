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

fn get_explicit_test_profile() -> Option<PathBuf> {
    let raw = std::env::var("ZOTERO_CLI_LIVE_TEST_PROFILE").ok()?;
    let path = PathBuf::from(raw.trim());
    if path.as_os_str().is_empty() || !path.exists() {
        eprintln!(
            "ZOTERO_CLI_LIVE_TEST_PROFILE is set to '{}' but the path does not exist",
            path.display()
        );
        return None;
    }
    Some(path)
}

fn stop_zotero() {
    bridge::clear_probe_cache();
    for _ in 0..10 {
        let _ = Command::new("pkill")
            .arg("-9")
            .arg("-x")
            .arg("zotero")
            .status();
        let _ = Command::new("pkill")
            .arg("-9")
            .arg("-x")
            .arg("plugin-container")
            .status();
        let out = Command::new("pgrep").arg("-x").arg("zotero").output();
        if let Ok(o) = out {
            if o.stdout.is_empty() {
                break;
            }
        }
        thread::sleep(Duration::from_millis(300));
    }
    thread::sleep(Duration::from_millis(2500));
}

fn start_zotero(profile_dir: &std::path::Path) {
    bridge::clear_probe_cache();
    let app_path = "/Applications/Zotero.app/Contents/MacOS/zotero";
    if std::path::Path::new(app_path).exists() {
        let _ = Command::new(app_path)
            .arg("-profile")
            .arg(profile_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    let client = JSBridgeClient::new(23119);
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(500));
        if client.bridge_endpoint_active() {
            return;
        }
    }
}

#[test]
#[ignore = "requires running desktop Zotero instance with ZOTERO_CLI_LIVE_TEST_PROFILE set"]
fn test_live_zotero10_xpi_load_and_ownership() {
    let profile = match get_explicit_test_profile() {
        Some(p) => p,
        None => {
            println!(
                "SKIPPING live test: ZOTERO_CLI_LIVE_TEST_PROFILE environment variable is not set \
                 to an explicit disposable test profile. The default user profile will never be targeted."
            );
            return;
        }
    };

    println!("Targeting explicit test profile: {}", profile.display());

    // 1. Audit manifest before installation
    let manifest_val: serde_json::Value =
        serde_json::from_str(MANIFEST_JSON).expect("manifest.json is valid JSON");
    let app = &manifest_val["applications"]["zotero"];
    assert_eq!(app["id"].as_str().unwrap(), ADDON_ID);
    assert_eq!(app["strict_min_version"].as_str().unwrap(), "6.999");
    assert_eq!(app["strict_max_version"].as_str().unwrap(), "10.0.*");
    assert!(
        app.get("update_url").is_none(),
        "Phase 6 manifest must not declare update_url"
    );
    assert!(
        !MANIFEST_JSON.contains("update_url"),
        "manifest.json must not contain update_url"
    );

    // 2. Audit bootstrap.js invariants
    assert!(BOOTSTRAP_JS.contains("permitBookmarklet: false"));
    assert!(BOOTSTRAP_JS.contains("supportedMethods: [\"POST\"]"));
    assert!(BOOTSTRAP_JS.contains("/cli-bridge/eval"));
    assert!(BOOTSTRAP_JS.contains("/cli-bridge/ownership"));

    // 3. Stop running Zotero before touching extensions on disk
    stop_zotero();

    // 4. Install XPI into profile
    let xpi_path = install_plugin(&profile).expect("Install XPI into profile");
    assert!(xpi_path.exists());
    assert_eq!(xpi_path.file_name().unwrap(), XPI_FILENAME);

    // 5. Start Zotero with the disposable test profile
    start_zotero(&profile);

    // 6. Verify Connector ping is 200 and running Zotero 10
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

    // 7. Verify /cli-bridge/eval responds to ping
    let mut eval_json: Option<serde_json::Value> = None;
    for _ in 0..20 {
        if let Ok(mut eval_resp) = ureq::post("http://127.0.0.1:23119/cli-bridge/eval")
            .header("Content-Type", "text/plain")
            .config()
            .timeout_global(Some(Duration::from_secs(2)))
            .http_status_as_error(false)
            .build()
            .send("return 'ping';".as_bytes())
        {
            if eval_resp.status().as_u16() == 200 {
                if let Ok(eval_bytes) = eval_resp.body_mut().read_to_vec() {
                    if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&eval_bytes) {
                        eval_json = Some(json);
                        break;
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(500));
    }
    let eval_json = eval_json.expect("eval ping should respond with 200 within 10s");
    println!("Eval ping JSON response: {eval_json:?}");
    assert_eq!(eval_json["fork"].as_str().unwrap(), "zotero-rust-cli");
    assert_eq!(eval_json["id"].as_str().unwrap(), ADDON_ID);
    assert_eq!(eval_json["version"].as_str().unwrap(), "1.2.1");
    assert_eq!(eval_json["ownership"].as_str().unwrap(), "verified");

    // 8. Verify /cli-bridge/eval executes privileged JavaScript under Zotero 10
    let client = JSBridgeClient::new(23119);
    let js_resp = client.execute_js("return Zotero.version;", 5);
    assert!(js_resp.ok);
    assert_eq!(
        js_resp.data.as_ref().and_then(|v| v.as_str()),
        Some(zotero_version)
    );

    // 9. Verify /cli-bridge/ownership companion endpoint
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

    // 10. Verify plugin_status reporting
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

    // 11. Verify that an un-forked / legacy response is not mistaken for our fork
    let foreign_response = OwnershipStatus::ActiveUpstreamPlugin { version: None };
    assert_ne!(status.ownership_status, foreign_response);

    // 12. Clean uninstall
    stop_zotero();
    let uninstalled = uninstall_plugin(&profile).expect("uninstall plugin");
    assert!(uninstalled);
    assert!(!xpi_path.exists());

    // 13. Verify that after uninstall and restart, the endpoint is inactive and uninstalled
    start_zotero(&profile);
    let post_status = plugin_status(Some(&profile), 23119);
    assert!(!post_status.installed_on_disk);
    assert!(!post_status.is_active);
    stop_zotero();
}
