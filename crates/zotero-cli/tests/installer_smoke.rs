#[cfg(unix)]
#[path = "common/mod.rs"]
mod common;

#[cfg(unix)]
use common::{ScriptedResponse, ScriptedServer, TestDir};
#[cfg(unix)]
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn script_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts")
        .join(name)
        .canonicalize()
        .expect("script file must exist")
}

#[test]
fn install_sh_passes_posix_syntax_check() {
    let script = script_path("install.sh");
    let status = Command::new("sh")
        .arg("-n")
        .arg(&script)
        .status()
        .expect("failed to run sh -n on scripts/install.sh");
    assert!(
        status.success(),
        "scripts/install.sh failed sh -n syntax check"
    );
}

#[test]
fn install_ps1_contains_required_windows_targets_and_security_invariants() {
    let script = script_path("install.ps1");
    let content = fs::read_to_string(&script).expect("failed to read scripts/install.ps1");

    // Must map AMD64/x86_64 to x86_64-pc-windows-msvc
    assert!(content.contains("x86_64-pc-windows-msvc"));
    assert!(content.contains("PROCESSOR_ARCHITECTURE"));

    // Must use SHA256 file hashing
    assert!(content.contains("Get-FileHash"));
    assert!(content.contains("SHA256"));

    // Must verify against SHA256SUMS before copying
    assert!(content.contains("SHA256SUMS"));
    assert!(content.contains("Checksum verification failed"));

    // Must install to user-local Programs directory by default
    assert!(content.contains(r"Programs\zotero-cli"));

    // Must verify installed binary with --version
    assert!(content.contains("--version"));

    // Must instruct user on app doctor
    assert!(content.contains("app doctor"));

    // Must NOT contain security bypass commands
    assert!(!content.contains("Set-ExecutionPolicy"));
    assert!(!content.contains("-ExecutionPolicy Bypass"));
    assert!(!content.contains("Unblock-File"));
}

#[test]
fn install_sh_contains_required_unix_targets_and_security_invariants() {
    let script = script_path("install.sh");
    let content = fs::read_to_string(&script).expect("failed to read scripts/install.sh");

    // All four Unix target triples
    assert!(content.contains("aarch64-apple-darwin"));
    assert!(content.contains("x86_64-apple-darwin"));
    assert!(content.contains("x86_64-unknown-linux-gnu"));
    assert!(content.contains("aarch64-unknown-linux-gnu"));

    // Must check uname -s and uname -m
    assert!(content.contains("uname -s"));
    assert!(content.contains("uname -m"));

    // Must download SHA256SUMS and verify
    assert!(content.contains("SHA256SUMS"));
    assert!(content.contains("Checksum verification failed"));

    // Must default to ~/.local/bin
    assert!(content.contains(".local/bin"));

    // Must verify installed binary with --version
    assert!(content.contains("--version"));

    // Must instruct user on app doctor
    assert!(content.contains("app doctor"));

    // Must NOT contain automatic quarantine removal or sudo
    assert!(!content.contains("xattr -d"));
    assert!(!content.contains("sudo "));
}

#[cfg(unix)]
#[test]
fn install_sh_successfully_installs_from_mock_release_server() {
    let script = script_path("install.sh");
    let test_dir = TestDir::new("installer-smoke-success");
    let install_dest = test_dir.path().join("dest_bin");

    let target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        _ => return,
    };

    let archive_bytes = create_mock_tar_gz(&test_dir, "zotero-cli 1.0.0-test\n");
    let mut hasher = Sha256::new();
    hasher.update(&archive_bytes);
    let hash_hex = format!("{:x}", hasher.finalize());

    let sha256sums_content = format!("{}  zotero-cli-{}.tar.gz\n", hash_hex, target);

    let server = ScriptedServer::start(vec![
        // 1. Download SHA256SUMS
        ScriptedResponse::Http {
            status: 200,
            headers: vec![("Content-Type".to_string(), "text/plain".to_string())],
            body: sha256sums_content.into_bytes(),
        },
        // 2. Download archive
        ScriptedResponse::Http {
            status: 200,
            headers: vec![("Content-Type".to_string(), "application/gzip".to_string())],
            body: archive_bytes,
        },
    ]);

    let base_url = format!("http://127.0.0.1:{}", server.port);

    let output = Command::new("sh")
        .arg(&script)
        .arg("--dir")
        .arg(&install_dest)
        .env("ZOTERO_CLI_BASE_URL", &base_url)
        .output()
        .expect("failed to execute install.sh");

    server.finish();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "install.sh failed: stdout={stdout}, stderr={stderr}"
    );

    assert!(stdout.contains("zotero-cli 1.0.0-test installed successfully"));
    assert!(stdout.contains("app doctor"));

    let installed_bin = install_dest.join("zotero-cli");
    assert!(installed_bin.exists());

    let run_out = Command::new(&installed_bin)
        .arg("--version")
        .output()
        .expect("failed to run installed binary");
    assert_eq!(
        String::from_utf8_lossy(&run_out.stdout).trim(),
        "zotero-cli 1.0.0-test"
    );
}

#[cfg(unix)]
#[test]
fn install_sh_creates_nested_destination_directory() {
    let script = script_path("install.sh");
    let test_dir = TestDir::new("installer-nested-dir");
    let install_dest = test_dir.path().join("deeply").join("nested").join("bin");

    let target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        _ => return,
    };

    let archive_bytes = create_mock_tar_gz(&test_dir, "zotero-cli 1.0.0-nested\n");
    let mut hasher = Sha256::new();
    hasher.update(&archive_bytes);
    let hash_hex = format!("{:x}", hasher.finalize());

    let sha256sums_content = format!("{}  zotero-cli-{}.tar.gz\n", hash_hex, target);

    let server = ScriptedServer::start(vec![
        ScriptedResponse::Http {
            status: 200,
            headers: vec![("Content-Type".to_string(), "text/plain".to_string())],
            body: sha256sums_content.into_bytes(),
        },
        ScriptedResponse::Http {
            status: 200,
            headers: vec![("Content-Type".to_string(), "application/gzip".to_string())],
            body: archive_bytes,
        },
    ]);

    let base_url = format!("http://127.0.0.1:{}", server.port);

    let output = Command::new("sh")
        .arg(&script)
        .arg("--dir")
        .arg(&install_dest)
        .env("ZOTERO_CLI_BASE_URL", &base_url)
        .output()
        .expect("failed to execute install.sh");

    server.finish();

    assert!(output.status.success());
    assert!(install_dest.join("zotero-cli").exists());
}

#[cfg(unix)]
#[test]
fn install_sh_atomically_overwrites_existing_installation() {
    let script = script_path("install.sh");
    let test_dir = TestDir::new("installer-overwrite");
    let install_dest = test_dir.path().join("dest_bin");
    fs::create_dir_all(&install_dest).unwrap();

    // Create existing pre-installed binary
    let existing_bin = install_dest.join("zotero-cli");
    fs::write(&existing_bin, "#!/bin/sh\necho old-v0.1\n").unwrap();

    let target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        _ => return,
    };

    let archive_bytes = create_mock_tar_gz(&test_dir, "zotero-cli 1.0.0-new\n");
    let mut hasher = Sha256::new();
    hasher.update(&archive_bytes);
    let hash_hex = format!("{:x}", hasher.finalize());

    let sha256sums_content = format!("{}  zotero-cli-{}.tar.gz\n", hash_hex, target);

    let server = ScriptedServer::start(vec![
        ScriptedResponse::Http {
            status: 200,
            headers: vec![("Content-Type".to_string(), "text/plain".to_string())],
            body: sha256sums_content.into_bytes(),
        },
        ScriptedResponse::Http {
            status: 200,
            headers: vec![("Content-Type".to_string(), "application/gzip".to_string())],
            body: archive_bytes,
        },
    ]);

    let base_url = format!("http://127.0.0.1:{}", server.port);

    let output = Command::new("sh")
        .arg(&script)
        .arg("--dir")
        .arg(&install_dest)
        .env("ZOTERO_CLI_BASE_URL", &base_url)
        .output()
        .expect("failed to execute install.sh");

    server.finish();

    assert!(output.status.success());
    let run_out = Command::new(&existing_bin)
        .arg("--version")
        .output()
        .expect("failed to run installed binary");
    assert_eq!(
        String::from_utf8_lossy(&run_out.stdout).trim(),
        "zotero-cli 1.0.0-new"
    );
}

#[cfg(unix)]
#[test]
fn install_sh_aborts_and_never_installs_on_checksum_mismatch() {
    let script = script_path("install.sh");
    let test_dir = TestDir::new("installer-smoke-mismatch");
    let install_dest = test_dir.path().join("dest_bin");

    let target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        _ => return,
    };

    let archive_bytes = create_mock_tar_gz(&test_dir, "zotero-cli 1.0.0-malicious\n");
    let bad_hash_hex = "0000000000000000000000000000000000000000000000000000000000000000";

    let sha256sums_content = format!("{}  zotero-cli-{}.tar.gz\n", bad_hash_hex, target);

    let server = ScriptedServer::start(vec![
        ScriptedResponse::Http {
            status: 200,
            headers: vec![("Content-Type".to_string(), "text/plain".to_string())],
            body: sha256sums_content.into_bytes(),
        },
        ScriptedResponse::Http {
            status: 200,
            headers: vec![("Content-Type".to_string(), "application/gzip".to_string())],
            body: archive_bytes,
        },
    ]);

    let base_url = format!("http://127.0.0.1:{}", server.port);

    let output = Command::new("sh")
        .arg(&script)
        .arg("--dir")
        .arg(&install_dest)
        .env("ZOTERO_CLI_BASE_URL", &base_url)
        .output()
        .expect("failed to execute install.sh");

    server.finish();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "install.sh should have failed on checksum mismatch"
    );
    assert!(stderr.contains("Checksum verification failed"));

    let installed_bin = install_dest.join("zotero-cli");
    assert!(
        !installed_bin.exists(),
        "binary must NOT be installed when checksum fails"
    );
}

#[cfg(unix)]
#[test]
fn install_sh_aborts_when_target_checksum_is_missing_from_sha256sums() {
    let script = script_path("install.sh");
    let test_dir = TestDir::new("installer-missing-checksum");
    let install_dest = test_dir.path().join("dest_bin");

    let archive_bytes = create_mock_tar_gz(&test_dir, "zotero-cli 1.0.0-unrecorded\n");
    let sha256sums_content = "1111111111111111111111111111111111111111111111111111111111111111  some-other-file.tar.gz\n";

    let server = ScriptedServer::start(vec![
        ScriptedResponse::Http {
            status: 200,
            headers: vec![("Content-Type".to_string(), "text/plain".to_string())],
            body: sha256sums_content.as_bytes().to_vec(),
        },
        ScriptedResponse::Http {
            status: 200,
            headers: vec![("Content-Type".to_string(), "application/gzip".to_string())],
            body: archive_bytes,
        },
    ]);

    let base_url = format!("http://127.0.0.1:{}", server.port);

    let output = Command::new("sh")
        .arg(&script)
        .arg("--dir")
        .arg(&install_dest)
        .env("ZOTERO_CLI_BASE_URL", &base_url)
        .output()
        .expect("failed to execute install.sh");

    server.finish();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("not found in SHA256SUMS"));
    assert!(!install_dest.join("zotero-cli").exists());
}

#[cfg(unix)]
fn create_mock_tar_gz(test_dir: &TestDir, version_string: &str) -> Vec<u8> {
    use std::os::unix::fs::PermissionsExt;

    let bin_dir = test_dir.path().join("mock_bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let bin_path = bin_dir.join("zotero-cli");

    let script_content = format!(
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"{}\"; exit 0; fi\n",
        version_string.trim()
    );
    fs::write(&bin_path, script_content).unwrap();
    let mut perms = fs::metadata(&bin_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&bin_path, perms).unwrap();

    let tar_gz_path = test_dir.path().join("mock.tar.gz");
    let status = Command::new("tar")
        .arg("-czf")
        .arg(&tar_gz_path)
        .arg("-C")
        .arg(&bin_dir)
        .arg("zotero-cli")
        .status()
        .expect("failed to execute tar command");
    assert!(status.success());

    fs::read(&tar_gz_path).unwrap()
}
