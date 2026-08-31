//! First-run Bridge/XPI onboarding, and the state-aware guidance `app doctor` gives.
//!
//! A new user can install `zotero-cli` with no idea that some live operations need a plugin, no
//! idea that the binary already bundles it, and no idea which file Zotero's install dialog wants.
//! These tests pin the guidance that closes that gap, and the states it must tell apart.
//!
//! The CLI's role stops at *staging*: it writes an `.xpi` into a directory it owns and never
//! touches the Zotero profile, so Zotero's own plugin-consent dialog is not bypassed.

#[path = "common/mod.rs"]
mod common;

use common::{
    build_fixture_sqlite, create_empty_fake_profile, create_fake_profile, run_cli,
    ScriptedResponse, ScriptedServer, TestDir,
};
use serde_json::{json, Value};

fn connector_ping_ok() -> ScriptedResponse {
    ScriptedResponse::json(200, json!({}))
}

fn connector_ping_unavailable() -> ScriptedResponse {
    ScriptedResponse::json(500, json!({}))
}

fn local_api_probe_available() -> ScriptedResponse {
    ScriptedResponse::json_with_headers(
        200,
        vec![("Zotero-Server-ID".to_string(), "TEST-SERVER-1".to_string())],
        json!({}),
    )
}

fn local_api_probe_unavailable() -> ScriptedResponse {
    ScriptedResponse::json(403, json!({"message": "local API disabled"}))
}

fn bridge_ownership_ok() -> ScriptedResponse {
    ScriptedResponse::json(
        200,
        json!({"fork": "zotero-rust-cli", "id": "cli-bridge@cli-anything-rust.dev"}),
    )
}

fn next_steps(value: &Value) -> Vec<String> {
    value["next_steps"]
        .as_array()
        .expect("doctor always emits next_steps")
        .iter()
        .map(|s| s.as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every `next_steps` entry, joined -- for "no step anywhere says X" assertions.
fn all_steps(value: &Value) -> String {
    next_steps(value).join("\n")
}

// ── install-plugin ─────────────────────────────────────────────────────────

#[test]
fn install_plugin_stages_the_bundled_xpi_and_reports_it_structurally() {
    let dir = TestDir::new("install-plugin-structured");
    build_fixture_sqlite(dir.path());
    let profile_dir = create_empty_fake_profile(dir.path());
    let staging = dir.path().join("staging");
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_PROFILE_DIR", profile_dir.to_str().unwrap())],
        &[
            "app",
            "install-plugin",
            "--output-dir",
            staging.to_str().unwrap(),
        ],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");

    // The binary bundles the plugin, so nothing has to be downloaded and no version has to be
    // matched by hand -- but only if the output actually says so.
    let staged_path = value["staged_xpi_path"].as_str().expect("staged path");
    assert!(
        std::path::Path::new(staged_path).is_file(),
        "the staged file must really exist at the reported path: {staged_path}"
    );
    assert!(
        value["bundled_version"].as_str().is_some(),
        "the bundled version must be reported so a caller can compare it: {value}"
    );
    assert_eq!(value["already_installed"], false);
    assert_eq!(value["installed_version"], Value::Null);

    // Ordered, literal steps -- including the exact path to select in Zotero's dialog, which is
    // the step people actually get stuck on.
    let steps: Vec<String> = value["install_steps"]
        .as_array()
        .expect("install_steps is a list")
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    assert!(!steps.is_empty());
    assert!(
        steps.iter().any(|s| s.contains(staged_path)),
        "the steps must name the file to select: {steps:?}"
    );
    assert!(
        steps.iter().any(|s| s.contains("Install Add-on From File")),
        "the steps must name Zotero's actual menu item: {steps:?}"
    );
    assert!(
        steps.iter().any(|s| s.contains("Restart Zotero")),
        "restarting is required for the endpoint to register: {steps:?}"
    );
}

#[test]
fn the_staged_artifact_is_a_real_installable_xpi() {
    let dir = TestDir::new("install-plugin-artifact");
    build_fixture_sqlite(dir.path());
    let staging = dir.path().join("staging");
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[],
        &[
            "app",
            "install-plugin",
            "--output-dir",
            staging.to_str().unwrap(),
        ],
    );
    server.finish();
    assert_eq!(code, 0, "stdout={value}");

    // Staging a file Zotero would reject would be worse than staging nothing: the failure would
    // surface inside Zotero's dialog, far from this command.
    let staged_path = value["staged_xpi_path"].as_str().unwrap();
    let file = std::fs::File::open(staged_path).unwrap();
    let mut archive = zip::ZipArchive::new(file).expect("staged artifact must be a valid zip");
    let mut manifest = String::new();
    {
        use std::io::Read;
        archive
            .by_name("manifest.json")
            .expect("an XPI without manifest.json is not installable")
            .read_to_string(&mut manifest)
            .unwrap();
    }
    let parsed: Value = serde_json::from_str(&manifest).unwrap();
    assert_eq!(parsed["version"], value["bundled_version"]);
}

#[test]
fn install_plugin_reports_an_already_installed_bridge_rather_than_implying_it_is_missing() {
    let dir = TestDir::new("install-plugin-already");
    build_fixture_sqlite(dir.path());
    let profile_dir = create_fake_profile(dir.path(), Some("1.2.1"));
    let staging = dir.path().join("staging");
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_unavailable()]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_PROFILE_DIR", profile_dir.to_str().unwrap())],
        &[
            "app",
            "install-plugin",
            "--output-dir",
            staging.to_str().unwrap(),
        ],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["already_installed"], true);
    assert_eq!(value["installed_version"], "1.2.1");
}

// ── doctor: Bridge states ──────────────────────────────────────────────────

#[test]
fn bridge_missing_makes_doctor_recommend_install_plugin_and_say_why() {
    let dir = TestDir::new("doctor-bridge-missing");
    build_fixture_sqlite(dir.path());
    let profile_dir = create_empty_fake_profile(dir.path());
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);

    let (_code, value) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_PROFILE_DIR", profile_dir.to_str().unwrap())],
        &["app", "doctor"],
    );
    server.finish();

    assert_eq!(value["checks"]["bridge"]["state"], "not_installed");
    let steps = all_steps(&value);
    assert!(
        steps.contains("CLI Bridge is not installed"),
        "must name what is missing: {steps}"
    );
    assert!(
        steps.contains("zotero-cli app install-plugin"),
        "must give the exact command: {steps}"
    );
    assert!(
        steps.contains("require it"),
        "must say why it matters, or a new user has no reason to run it: {steps}"
    );
}

#[test]
fn a_staged_but_uninstalled_bridge_is_a_distinct_state_with_its_own_instruction() {
    let dir = TestDir::new("doctor-bridge-staged");
    build_fixture_sqlite(dir.path());
    let profile_dir = create_empty_fake_profile(dir.path());
    let state_dir = dir.path().join("cli-state");

    // Stage into the default location doctor inspects, exactly as `app install-plugin` does.
    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let (code, staged) = run_cli(
        dir.path(),
        server.port,
        &[
            ("ZOTERO_PROFILE_DIR", profile_dir.to_str().unwrap()),
            ("CLI_ANYTHING_ZOTERO_STATE_DIR", state_dir.to_str().unwrap()),
        ],
        &["app", "install-plugin"],
    );
    server.finish();
    assert_eq!(code, 0, "stdout={staged}");
    let staged_path = staged["staged_xpi_path"].as_str().unwrap().to_string();

    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);
    let (_code, value) = run_cli(
        dir.path(),
        server.port,
        &[
            ("ZOTERO_PROFILE_DIR", profile_dir.to_str().unwrap()),
            ("CLI_ANYTHING_ZOTERO_STATE_DIR", state_dir.to_str().unwrap()),
        ],
        &["app", "doctor"],
    );
    server.finish();

    // "Staged, waiting for you to finish Zotero's dialog" is where a first-run user actually
    // gets stuck, and it is not the same as "nothing has happened".
    assert_eq!(value["checks"]["bridge"]["state"], "staged_not_installed");
    assert_eq!(value["checks"]["bridge"]["staged_xpi_path"], staged_path);
    let steps = all_steps(&value);
    assert!(
        steps.contains("staged but not installed"),
        "must distinguish staged from absent: {steps}"
    );
    assert!(
        steps.contains(&staged_path),
        "must name the file to select: {steps}"
    );
    assert!(
        !steps.contains("app install-plugin"),
        "re-running the command the user already ran is not the next step: {steps}"
    );
}

#[test]
fn bridge_installed_but_zotero_closed_says_so_instead_of_suggesting_a_reinstall() {
    let dir = TestDir::new("doctor-bridge-closed");
    build_fixture_sqlite(dir.path());
    let profile_dir = create_fake_profile(dir.path(), Some("1.2.1"));
    // Port 1 answers nothing: Zotero is closed.
    let (_code, value) = run_cli(
        dir.path(),
        1,
        &[("ZOTERO_PROFILE_DIR", profile_dir.to_str().unwrap())],
        &["app", "doctor"],
    );

    assert_eq!(
        value["checks"]["bridge"]["state"],
        "installed_zotero_closed"
    );
    let steps = all_steps(&value);
    assert!(
        !steps.contains("install-plugin"),
        "the plugin is installed; recommending installation would be wrong: {steps}"
    );
    assert!(
        steps.contains("Zotero is not running"),
        "the actual condition is a closed Zotero: {steps}"
    );
}

#[test]
fn a_healthy_bridge_produces_no_install_recommendation_at_all() {
    let dir = TestDir::new("doctor-bridge-healthy");
    build_fixture_sqlite(dir.path());
    let profile_dir = create_fake_profile(dir.path(), Some("1.2.1"));
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({"ok": true, "value": "cli-bridge-ok", "version": "10.0.1"}),
        ),
    ]);

    let (_code, value) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_PROFILE_DIR", profile_dir.to_str().unwrap())],
        &["app", "doctor"],
    );
    server.finish();

    assert_eq!(value["checks"]["bridge"]["state"], "healthy");
    let steps = all_steps(&value);
    assert!(
        !steps.contains("install-plugin"),
        "a healthy install must not be told to reinstall: {steps}"
    );
}

// ── doctor: Local API guidance ─────────────────────────────────────────────

/// A profile with the Local API pref enabled, so "configured" and "reachable" differ.
fn create_local_api_configured_profile(dir: &std::path::Path) -> std::path::PathBuf {
    let profile_dir = dir.join("local_api_profile");
    std::fs::create_dir_all(profile_dir.join("extensions")).unwrap();
    std::fs::write(
        profile_dir.join("prefs.js"),
        "user_pref(\"extensions.zotero.httpServer.localAPI.enabled\", true);\n",
    )
    .unwrap();
    profile_dir
}

#[test]
fn local_api_configured_but_zotero_closed_never_says_enable_it() {
    let dir = TestDir::new("doctor-local-api-closed");
    build_fixture_sqlite(dir.path());
    let profile_dir = create_local_api_configured_profile(dir.path());

    // Port 1 answers nothing: configured, unreachable, Zotero closed.
    let (_code, value) = run_cli(
        dir.path(),
        1,
        &[("ZOTERO_PROFILE_DIR", profile_dir.to_str().unwrap())],
        &["app", "doctor"],
    );

    assert_eq!(value["checks"]["local_api"]["ok"], false);
    assert_eq!(value["checks"]["local_api"]["configured"], true);
    let steps = all_steps(&value);
    // The reported defect: being told to enable what is already enabled, when the real cause is
    // simply that Zotero is not running.
    assert!(
        !steps.contains("Enable the Local API"),
        "must not tell the user to enable an already-enabled Local API: {steps}"
    );
    assert!(
        steps.contains("already enabled"),
        "must state that it is configured: {steps}"
    );
    assert!(
        steps.contains("Zotero is not running"),
        "must name the real cause: {steps}"
    );
}

#[test]
fn local_api_unconfigured_gets_setup_guidance_pointing_at_zotero_settings() {
    let dir = TestDir::new("doctor-local-api-unconfigured");
    build_fixture_sqlite(dir.path());
    let profile_dir = create_empty_fake_profile(dir.path());
    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        ScriptedResponse::json(403, json!({"message": "local API disabled"})),
    ]);

    let (_code, value) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_PROFILE_DIR", profile_dir.to_str().unwrap())],
        &["app", "doctor"],
    );
    server.finish();

    assert_eq!(value["checks"]["local_api"]["configured"], false);
    let steps = all_steps(&value);
    assert!(
        steps.contains("Enable the Local API in Zotero"),
        "this is the one case where enabling is the right advice: {steps}"
    );
    assert!(
        steps.contains("Settings"),
        "the setting lives in Zotero, not in this CLI: {steps}"
    );
}

/// The regression guard that matters most here.
///
/// `app enable-local-api` is canonical behavior this fork deliberately **Excludes** on safety
/// grounds -- `app authorize-local-api` is the approved consent path. Recommending it was not
/// merely misleading: the command does not exist, so the advice was impossible to follow and
/// pointed at the very workflow the exclusion prevents.
#[test]
fn no_doctor_guidance_ever_recommends_the_excluded_enable_local_api_command() {
    let dir = TestDir::new("doctor-never-enable-local-api");
    build_fixture_sqlite(dir.path());

    let scenarios: Vec<(&str, Option<&str>)> =
        vec![("unconfigured", None), ("configured", Some("configured"))];
    for (label, configured) in scenarios {
        let profile_dir = match configured {
            Some(_) => create_local_api_configured_profile(dir.path()),
            None => create_empty_fake_profile(dir.path()),
        };
        // Both with Zotero closed and with it running.
        let (_code, closed) = run_cli(
            dir.path(),
            1,
            &[("ZOTERO_PROFILE_DIR", profile_dir.to_str().unwrap())],
            &["app", "doctor"],
        );
        let server = ScriptedServer::start(vec![
            connector_ping_ok(),
            ScriptedResponse::json(403, json!({"message": "local API disabled"})),
        ]);
        let (_code, running) = run_cli(
            dir.path(),
            server.port,
            &[("ZOTERO_PROFILE_DIR", profile_dir.to_str().unwrap())],
            &["app", "doctor"],
        );
        server.finish();

        for (state, value) in [("closed", &closed), ("running", &running)] {
            let steps = all_steps(value);
            assert!(
                !steps.contains("enable-local-api"),
                "{label}/{state} recommended a command this CLI does not implement: {steps}"
            );
        }
    }
}

#[test]
fn a_running_zotero_with_a_dead_connector_is_not_reported_as_a_closed_zotero() {
    let dir = TestDir::new("doctor-connector-only-down");
    build_fixture_sqlite(dir.path());
    let profile_dir = create_fake_profile(dir.path(), Some("1.2.1"));
    // The Local API answers, so Zotero is demonstrably up; only the connector is not.
    let server = ScriptedServer::start(vec![
        connector_ping_unavailable(),
        local_api_probe_available(),
    ]);

    let (_code, value) = run_cli(
        dir.path(),
        server.port,
        &[("ZOTERO_PROFILE_DIR", profile_dir.to_str().unwrap())],
        &["app", "doctor"],
    );
    server.finish();

    let steps = all_steps(&value);
    assert!(
        !steps.contains("Zotero is not running"),
        "Zotero is answering; claiming otherwise sends the user down the wrong path: {steps}"
    );
    assert!(
        steps.contains("connector is not answering"),
        "must describe the actual condition: {steps}"
    );
}
