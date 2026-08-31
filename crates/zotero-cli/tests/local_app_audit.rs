//! Local App / Audit slice CLI integration tests: exercises the actual `zotero-cli` binary
//! against scripted mock servers and isolated environment directories for:
//! `app ping`, `app version`, `app doctor`, `audit path`, and `audit tail`.

#[path = "common/mod.rs"]
mod common;

use common::{
    build_fixture_sqlite, create_empty_fake_profile, create_fake_profile, run_cli,
    ScriptedResponse, ScriptedServer, TestDir,
};
use serde_json::json;
use std::path::Path;
use std::process::Command;

fn connector_ping_ok() -> ScriptedResponse {
    ScriptedResponse::json(200, json!({}))
}

fn connector_ping_unavailable() -> ScriptedResponse {
    ScriptedResponse::Http {
        status: 500,
        headers: Vec::new(),
        body: b"internal error".to_vec(),
    }
}

fn local_api_probe_available() -> ScriptedResponse {
    ScriptedResponse::json(200, json!({}))
}

fn bridge_ownership_ok() -> ScriptedResponse {
    ScriptedResponse::json(
        200,
        json!({"fork": "zotero-rust-cli", "id": "cli-bridge@cli-anything-rust.dev"}),
    )
}

fn run_cli_human(
    data_dir: &Path,
    port: u16,
    extra_env: &[(&str, &str)],
    args: &[&str],
) -> (i32, String, String) {
    let profile_dir = create_empty_fake_profile(data_dir);
    let mut command = Command::new(common::bin_path());
    command
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .env("ZOTERO_HTTP_PORT", port.to_string())
        // Same per-test isolation `common::run_cli` documents: never fall back to the
        // developer's real `~/.config/cli-anything-zotero` session, and never let an automated
        // run reach the lifecycle helper's Zotero-launch path.
        .env("CLI_ANYTHING_ZOTERO_STATE_DIR", data_dir.join("cli-state"))
        .env("ZOTERO_PROFILE_DIR", &profile_dir)
        .env("ZOTERO_CLI_NO_AUTOLAUNCH", "1")
        .env_remove("ZOTERO_LOCAL_API_KEY");
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let output = command.output().expect("failed to run zotero-cli binary");
    let code = output.status.code().unwrap_or(-1);
    (
        code,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn create_fake_zotero_install(dir: &Path) -> std::path::PathBuf {
    let install_dir = dir.join("fake_zotero_install");
    std::fs::create_dir_all(&install_dir).unwrap();
    let executable = install_dir.join(if cfg!(windows) {
        "zotero.exe"
    } else {
        "zotero"
    });
    std::fs::write(&executable, b"").unwrap();
    std::fs::write(
        install_dir.join("application.ini"),
        "[App]\nVersion=7.0.1\n",
    )
    .unwrap();
    executable
}

// ── app ping ─────────────────────────────────────────────────────────────

#[test]
fn app_ping_success_json_mode() {
    let dir = TestDir::new("app-ping-success");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);

    let (code, value) = run_cli(dir.path(), server.port, &[], &["app", "ping"]);
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["connector_available"], true);
    assert_eq!(value["message"], "connector available");
}

#[test]
fn app_ping_unavailable_json_mode_exits_one_with_error() {
    let dir = TestDir::new("app-ping-unavailable-json");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![
        connector_ping_unavailable(),
        local_api_probe_available(),
    ]);

    let (code, value) = run_cli(dir.path(), server.port, &[], &["app", "ping"]);
    server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert!(value["error"]
        .as_str()
        .unwrap_or_default()
        .contains("connector returned HTTP 500"));
}

#[test]
fn app_ping_unavailable_human_mode_prints_stderr_and_exits_one() {
    let dir = TestDir::new("app-ping-unavailable-human");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![
        connector_ping_unavailable(),
        local_api_probe_available(),
    ]);

    let (code, stdout, stderr) = run_cli_human(dir.path(), server.port, &[], &["app", "ping"]);
    server.finish();

    assert_eq!(code, 1);
    assert!(stdout.trim().is_empty());
    assert!(stderr.contains("Error: connector returned HTTP 500"));
}

// ── app version ──────────────────────────────────────────────────────────

#[test]
fn app_version_json_mode_offline() {
    let dir = TestDir::new("app-version-json");
    build_fixture_sqlite(dir.path());

    let (code, value) = run_cli(dir.path(), 23119, &[], &["app", "version"]);

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["package_version"], env!("CARGO_PKG_VERSION"));
    assert!(value.as_object().unwrap().contains_key("zotero_version"));
}

#[test]
fn app_version_human_mode_offline() {
    let dir = TestDir::new("app-version-human");
    build_fixture_sqlite(dir.path());

    let (code, stdout, stderr) = run_cli_human(dir.path(), 23119, &[], &["app", "version"]);

    assert_eq!(code, 0);
    assert!(stderr.trim().is_empty());
    assert!(!stdout.trim().is_empty());
}

// ── app doctor ───────────────────────────────────────────────────────────

#[test]
fn app_doctor_healthy_fixture_all_checks_pass() {
    let dir = TestDir::new("app-doctor-healthy");
    build_fixture_sqlite(dir.path());
    let profile_dir = create_fake_profile(dir.path(), Some("1.2.1"));
    let executable = create_fake_zotero_install(dir.path());

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({"ok": true, "value": "cli-bridge-ok", "version": "7.0.1"}),
        ),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[
            ("ZOTERO_PROFILE_DIR", profile_dir.to_str().unwrap()),
            ("ZOTERO_EXECUTABLE", executable.to_str().unwrap()),
        ],
        &["app", "doctor"],
    );
    server.finish();

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["action"], "app_doctor");
    assert_eq!(value["ok"], true);
    assert_eq!(value["status"], "ready");
    assert_eq!(value["code"], "READY");
    assert_eq!(value["ready"], true);
    assert_eq!(value["write_ready"], true);
    assert_eq!(value["checks"]["package"]["ok"], true);
    assert_eq!(value["checks"]["connector"]["ok"], true);
    assert_eq!(value["checks"]["local_api"]["ok"], true);
    assert_eq!(value["checks"]["plugin"]["ok"], true);
    assert_eq!(value["checks"]["plugin"]["update_available"], false);
    assert_eq!(value["checks"]["bridge"]["ok"], true);
    assert_eq!(value["checks"]["bridge"]["js_ok"], true);
    assert_eq!(value["checks"]["bridge"]["zotero_js_version"], "7.0.1");
    assert_eq!(
        value["next_steps"],
        json!(["CLI Bridge and local surfaces look healthy."])
    );
}

/// `next_steps` as plain strings, for readable assertions.
fn next_steps(value: &serde_json::Value) -> Vec<String> {
    value["next_steps"]
        .as_array()
        .expect("doctor always emits next_steps")
        .iter()
        .map(|s| s.as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn app_doctor_degraded_when_connector_unavailable() {
    let dir = TestDir::new("app-doctor-connector-down");
    build_fixture_sqlite(dir.path());
    let profile_dir = create_fake_profile(dir.path(), Some("1.2.1"));
    let executable = create_fake_zotero_install(dir.path());

    let server = ScriptedServer::start(vec![
        connector_ping_unavailable(),
        local_api_probe_available(),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[
            ("ZOTERO_PROFILE_DIR", profile_dir.to_str().unwrap()),
            ("ZOTERO_EXECUTABLE", executable.to_str().unwrap()),
        ],
        &["app", "doctor"],
    );
    server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert_eq!(value["ok"], false);
    assert_eq!(value["status"], "degraded");
    assert_eq!(value["code"], "DEGRADED");
    assert_eq!(value["ready"], false);
    assert_eq!(value["checks"]["connector"]["ok"], false);
    // The Local API is answering here, so Zotero is demonstrably running: the guidance must
    // describe *that* -- a connector that is not responding on a live Zotero -- rather than
    // telling the user to start an application that is already open.
    let steps = next_steps(&value);
    assert!(
        steps
            .iter()
            .any(|s| s.contains("connector is not answering")),
        "expected a connector-specific step, got {steps:?}"
    );
    assert!(
        !steps.iter().any(|s| s.contains("Zotero is not running")),
        "must not claim Zotero is closed while its Local API answers: {steps:?}"
    );
}

#[test]
fn app_doctor_degraded_when_plugin_update_available() {
    let dir = TestDir::new("app-doctor-plugin-update");
    build_fixture_sqlite(dir.path());
    let profile_dir = create_fake_profile(dir.path(), Some("1.0.0"));
    let executable = create_fake_zotero_install(dir.path());

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({"ok": true, "value": "cli-bridge-ok", "version": "7.0.1"}),
        ),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[
            ("ZOTERO_PROFILE_DIR", profile_dir.to_str().unwrap()),
            ("ZOTERO_EXECUTABLE", executable.to_str().unwrap()),
        ],
        &["app", "doctor"],
    );
    server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert_eq!(value["ok"], false);
    assert_eq!(value["checks"]["plugin"]["ok"], false);
    assert_eq!(value["checks"]["plugin"]["update_available"], true);
    assert_eq!(value["checks"]["plugin"]["installed_version"], "1.0.0");
    assert_eq!(value["checks"]["plugin"]["bundled_version"], "1.2.1");
    assert!(value["next_steps"].as_array().unwrap().iter().any(|s| s
        .as_str()
        .unwrap()
        .contains("Upgrade CLI Bridge 1.0.0 → 1.2.1")));
}

#[test]
fn app_ping_human_mode_success() {
    let dir = TestDir::new("app-ping-human-ok");
    build_fixture_sqlite(dir.path());

    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);

    let (code, stdout, stderr) = run_cli_human(dir.path(), server.port, &[], &["app", "ping"]);
    server.finish();

    assert_eq!(code, 0);
    assert!(stderr.trim().is_empty());
    assert!(stdout.contains("\"connector_available\": true"));
}

#[test]
fn app_doctor_degraded_when_local_api_unavailable() {
    let dir = TestDir::new("app-doctor-local-api-down");
    build_fixture_sqlite(dir.path());
    let profile_dir = create_fake_profile(dir.path(), Some("1.2.1"));
    let executable = create_fake_zotero_install(dir.path());

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        ScriptedResponse::Http {
            status: 403,
            headers: Vec::new(),
            body: b"forbidden".to_vec(),
        },
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({"ok": true, "value": "cli-bridge-ok", "version": "7.0.1"}),
        ),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[
            ("ZOTERO_PROFILE_DIR", profile_dir.to_str().unwrap()),
            ("ZOTERO_EXECUTABLE", executable.to_str().unwrap()),
        ],
        &["app", "doctor"],
    );
    server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert_eq!(value["ok"], false);
    assert_eq!(value["status"], "degraded");
    assert_eq!(value["checks"]["local_api"]["ok"], false);
    let steps = next_steps(&value);
    // This fixture's profile does not enable the Local API, so enabling it *is* the right
    // advice -- and it must point at Zotero's own settings, never at `app enable-local-api`,
    // which this CLI deliberately does not implement (canonical behavior, Excluded on safety
    // grounds).
    assert!(
        steps
            .iter()
            .any(|s| s.contains("Enable the Local API in Zotero")),
        "expected setup guidance, got {steps:?}"
    );
    assert!(
        !steps.iter().any(|s| s.contains("enable-local-api")),
        "must never recommend the Excluded `app enable-local-api` command: {steps:?}"
    );
}

#[test]
fn app_doctor_degraded_when_bridge_eval_fails() {
    let dir = TestDir::new("app-doctor-bridge-eval-fail");
    build_fixture_sqlite(dir.path());
    let profile_dir = create_fake_profile(dir.path(), Some("1.2.1"));
    let executable = create_fake_zotero_install(dir.path());

    let server = ScriptedServer::start(vec![
        connector_ping_ok(),
        local_api_probe_available(),
        bridge_ownership_ok(),
        ScriptedResponse::json(
            200,
            json!({"ok": false, "error": "Evaluation error in Zotero"}),
        ),
    ]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[
            ("ZOTERO_PROFILE_DIR", profile_dir.to_str().unwrap()),
            ("ZOTERO_EXECUTABLE", executable.to_str().unwrap()),
        ],
        &["app", "doctor"],
    );
    server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert_eq!(value["ok"], false);
    assert_eq!(value["checks"]["bridge"]["ok"], false);
    assert_eq!(value["checks"]["bridge"]["js_ok"], false);
    assert!(value["next_steps"].as_array().unwrap().iter().any(|s| s
        .as_str()
        .unwrap()
        .contains("Bridge endpoint is up but eval failed")));
}

#[test]
fn app_doctor_plugin_missing_diagnostic() {
    let dir = TestDir::new("app-doctor-plugin-missing");
    build_fixture_sqlite(dir.path());
    // Create profile directory with NO plugin installed
    let profile_dir = create_fake_profile(dir.path(), None);
    let executable = create_fake_zotero_install(dir.path());

    let server = ScriptedServer::start(vec![connector_ping_ok(), local_api_probe_available()]);

    let (code, value) = run_cli(
        dir.path(),
        server.port,
        &[
            ("ZOTERO_PROFILE_DIR", profile_dir.to_str().unwrap()),
            ("ZOTERO_EXECUTABLE", executable.to_str().unwrap()),
        ],
        &["app", "doctor"],
    );
    server.finish();

    assert_eq!(code, 1, "stdout={value}");
    assert_eq!(value["ok"], false);
    assert_eq!(value["checks"]["plugin"]["ok"], false);
    assert_eq!(value["checks"]["plugin"]["xpi_installed"], false);
    assert_eq!(value["checks"]["bridge"]["state"], "not_installed");
    let steps = next_steps(&value);
    // A new user has no way to know the Bridge exists, so the step has to say what is missing,
    // why it matters, and the exact command -- not just "not available".
    assert!(
        steps
            .iter()
            .any(|s| s.contains("CLI Bridge is not installed")
                && s.contains("zotero-cli app install-plugin")),
        "expected actionable Bridge onboarding, got {steps:?}"
    );
}

// ── audit path ───────────────────────────────────────────────────────────

#[test]
fn audit_path_default_and_env_override() {
    let dir = TestDir::new("audit-path-env");
    let audit_dir = dir.path().join("my_audit_dir");

    let (code, value) = run_cli(
        dir.path(),
        23119,
        &[("ZOTERO_CLI_AUDIT_DIR", audit_dir.to_str().unwrap())],
        &["audit", "path"],
    );

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["action"], "audit_path");
    assert_eq!(value["ok"], true);
    assert_eq!(value["status"], "success");
    assert_eq!(
        value["path"],
        audit_dir.join("audit.jsonl").to_str().unwrap()
    );
    assert!(audit_dir.exists(), "audit_dir should be created if missing");

    // Human mode prints bare path
    let (h_code, stdout, stderr) = run_cli_human(
        dir.path(),
        23119,
        &[("ZOTERO_CLI_AUDIT_DIR", audit_dir.to_str().unwrap())],
        &["audit", "path"],
    );
    assert_eq!(h_code, 0);
    assert!(stderr.trim().is_empty());
    assert_eq!(
        stdout.trim(),
        audit_dir.join("audit.jsonl").to_str().unwrap()
    );

    // Calling `audit path` must not create any audit record in audit.jsonl
    assert!(!audit_dir.join("audit.jsonl").exists());
}

#[test]
fn audit_path_spaces_and_unicode() {
    let dir = TestDir::new("audit-path-unicode");
    let audit_dir = dir.path().join("Audit Dir 📚 2026");

    let (code, value) = run_cli(
        dir.path(),
        23119,
        &[("ZOTERO_CLI_AUDIT_DIR", audit_dir.to_str().unwrap())],
        &["audit", "path"],
    );

    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(
        value["path"],
        audit_dir.join("audit.jsonl").to_str().unwrap()
    );
}

// ── audit tail ───────────────────────────────────────────────────────────

#[test]
fn audit_tail_missing_and_empty_file() {
    let dir = TestDir::new("audit-tail-empty");
    let audit_dir = dir.path().join("audit_empty");
    std::fs::create_dir_all(&audit_dir).unwrap();

    // 1. Missing audit.jsonl
    let (code, value) = run_cli(
        dir.path(),
        23119,
        &[("ZOTERO_CLI_AUDIT_DIR", audit_dir.to_str().unwrap())],
        &["audit", "tail"],
    );
    assert_eq!(code, 0);
    assert_eq!(value["action"], "audit_tail");
    assert_eq!(value["count"], 0);
    assert_eq!(value["entries"], json!([]));

    let (h_code, stdout, _) = run_cli_human(
        dir.path(),
        23119,
        &[("ZOTERO_CLI_AUDIT_DIR", audit_dir.to_str().unwrap())],
        &["audit", "tail"],
    );
    assert_eq!(h_code, 0);
    assert_eq!(stdout.trim(), "(empty audit log)");

    // 2. Empty audit.jsonl
    std::fs::write(audit_dir.join("audit.jsonl"), "").unwrap();
    let (code2, value2) = run_cli(
        dir.path(),
        23119,
        &[("ZOTERO_CLI_AUDIT_DIR", audit_dir.to_str().unwrap())],
        &["audit", "tail"],
    );
    assert_eq!(code2, 0);
    assert_eq!(value2["count"], 0);
    assert_eq!(value2["entries"], json!([]));
}

#[test]
fn audit_tail_records_limit_and_malformed_lines() {
    let dir = TestDir::new("audit-tail-records");
    let audit_dir = dir.path().join("audit_records");
    std::fs::create_dir_all(&audit_dir).unwrap();
    let audit_file = audit_dir.join("audit.jsonl");

    let line1 =
        json!({"ts": "2026-08-30T10:00:00Z", "action": "item_merge", "ok": true, "key": "K1"});
    let line2 = "invalid json line here";
    let line3 =
        json!({"ts": "2026-08-30T10:05:00Z", "action": "import_file", "ok": true, "key": "K2"});
    let line4 = json!({"ts": "2026-08-30T10:10:00Z", "action": "add_url", "ok": true, "key": "K3"});

    let content = format!("{}\n{}\n{}\n{}\n", line1, line2, line3, line4,);
    std::fs::write(&audit_file, content).unwrap();

    // Default tail (returns all 4 lines)
    let (code, value) = run_cli(
        dir.path(),
        23119,
        &[("ZOTERO_CLI_AUDIT_DIR", audit_dir.to_str().unwrap())],
        &["audit", "tail"],
    );
    assert_eq!(code, 0, "stdout={value}");
    assert_eq!(value["count"], 4);
    let entries = value["entries"].as_array().unwrap();
    assert_eq!(entries[0]["action"], "item_merge");
    assert_eq!(entries[1]["ok"], false);
    assert_eq!(entries[1]["error"], "invalid json line");
    assert_eq!(entries[2]["action"], "import_file");
    assert_eq!(entries[3]["action"], "add_url");

    // Tail with --limit 2 (returns last 2 lines)
    let (code2, value2) = run_cli(
        dir.path(),
        23119,
        &[("ZOTERO_CLI_AUDIT_DIR", audit_dir.to_str().unwrap())],
        &["audit", "tail", "--limit", "2"],
    );
    assert_eq!(code2, 0);
    assert_eq!(value2["count"], 2);
    let entries2 = value2["entries"].as_array().unwrap();
    assert_eq!(entries2[0]["action"], "import_file");
    assert_eq!(entries2[1]["action"], "add_url");

    // Human mode output prints per-line JSON
    let (h_code, stdout, _) = run_cli_human(
        dir.path(),
        23119,
        &[("ZOTERO_CLI_AUDIT_DIR", audit_dir.to_str().unwrap())],
        &["audit", "tail", "--limit", "1"],
    );
    assert_eq!(h_code, 0);
    assert!(stdout.contains("\"action\": \"add_url\""));
}

#[test]
fn centralized_auditing_writes_entry_on_writeish_command() {
    let dir = TestDir::new("centralized-audit");
    let audit_dir = dir.path().join("audit_test");
    let audit_file = audit_dir.join("audit.jsonl");

    // Emitting a writeish payload via output::emit must write to audit log
    let payload = json!({
        "action": "item_merge",
        "ok": true,
        "status": "success",
        "key": "SURVIVOR1",
        "path": "/some/path"
    });

    std::env::set_var("ZOTERO_CLI_AUDIT_DIR", audit_dir.to_str().unwrap());
    zotero_cli::output::emit(true, &payload);

    assert!(audit_file.exists());
    let entries = zotero_cli::audit::tail(10);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["action"], "item_merge");
    assert_eq!(entries[0]["key"], "SURVIVOR1");
    assert_eq!(entries[0]["ok"], true);
    assert_eq!(entries[0]["status"], "success");
    assert!(entries[0].get("ts").is_some());
}
