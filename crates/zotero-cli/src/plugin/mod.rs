#![allow(dead_code, unused_imports)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

pub const ADDON_ID: &str = "cli-bridge@cli-anything-rust.dev";
pub const UPSTREAM_ADDON_ID: &str = "cli-bridge@cli-anything.dev";
pub const XPI_FILENAME: &str = "cli-bridge@cli-anything-rust.dev.xpi";
pub const UPSTREAM_XPI_FILENAME: &str = "cli-bridge@cli-anything.dev.xpi";

pub const MANIFEST_JSON: &str = include_str!("assets/manifest.json");
pub const BOOTSTRAP_JS: &str = include_str!("assets/bootstrap.js");

/// Builds the XPI plugin zip archive in memory.
pub fn build_xpi() -> Result<Vec<u8>> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buffer);
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);

        zip.start_file("manifest.json", options)
            .context("Failed to add manifest.json to XPI")?;
        zip.write_all(MANIFEST_JSON.as_bytes())?;

        zip.start_file("bootstrap.js", options)
            .context("Failed to add bootstrap.js to XPI")?;
        zip.write_all(BOOTSTRAP_JS.as_bytes())?;

        zip.finish().context("Failed to finalize XPI zip archive")?;
    }
    Ok(buffer.into_inner())
}

/// Installs the XPI plugin into the specified Zotero profile directory.
pub fn install_plugin(profile_dir: &Path) -> Result<PathBuf> {
    let extensions_dir = profile_dir.join("extensions");
    if !extensions_dir.exists() {
        std::fs::create_dir_all(&extensions_dir).with_context(|| {
            format!(
                "Failed to create extensions directory: {}",
                extensions_dir.display()
            )
        })?;
    }

    let xpi_path = extensions_dir.join(XPI_FILENAME);
    let xpi_bytes = build_xpi()?;
    std::fs::write(&xpi_path, xpi_bytes)
        .with_context(|| format!("Failed to write XPI file to: {}", xpi_path.display()))?;

    // In Zotero 7+ through 10+, ensure sideloaded extension is not auto-disabled on startup
    let user_js = profile_dir.join("user.js");
    let pref_line = "user_pref(\"extensions.autoDisableScopes\", 0);\n";
    let current_user_js = std::fs::read_to_string(&user_js).unwrap_or_default();
    if !current_user_js.contains("extensions.autoDisableScopes") {
        let mut new_content = current_user_js;
        if !new_content.is_empty() && !new_content.ends_with('\n') {
            new_content.push('\n');
        }
        new_content.push_str(pref_line);
        let _ = std::fs::write(&user_js, new_content);
    }

    // Invalidate stale startup cache so Zotero detects and loads the new XPI on next boot
    let startup_cache = profile_dir.join("addonStartup.json.lz4");
    if startup_cache.exists() {
        let _ = std::fs::remove_file(&startup_cache);
    }

    Ok(xpi_path)
}

/// Uninstalls the XPI plugin from the specified Zotero profile directory.
pub fn uninstall_plugin(profile_dir: &Path) -> Result<bool> {
    let xpi_path = profile_dir.join("extensions").join(XPI_FILENAME);
    let startup_cache = profile_dir.join("addonStartup.json.lz4");
    if startup_cache.exists() {
        let _ = std::fs::remove_file(&startup_cache);
    }
    if xpi_path.exists() {
        std::fs::remove_file(&xpi_path)
            .with_context(|| format!("Failed to remove XPI file: {}", xpi_path.display()))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Runtime ownership verification status of the `/cli-bridge/eval` endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OwnershipStatus {
    /// Active and owned by our forked plugin (`cli-bridge@cli-anything-rust.dev`).
    ActiveOurFork { version: String, id: String },
    /// Active, but owned by upstream or un-forked plugin (`cli-bridge@cli-anything.dev`).
    ActiveUpstreamPlugin { version: Option<String> },
    /// Endpoint is inactive / unreachable.
    Inactive,
}

/// Verifies whether `/cli-bridge/eval` is active and whether it belongs to our fork.
pub fn verify_ownership(port: u16, timeout: Duration) -> OwnershipStatus {
    // 1. Probe dedicated ownership endpoint
    let ownership_url = format!("http://127.0.0.1:{port}/cli-bridge/ownership");
    if let Ok(mut resp) = ureq::get(&ownership_url)
        .config()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .call()
    {
        if resp.status().as_u16() == 200 {
            if let Ok(bytes) = resp.body_mut().read_to_vec() {
                if let Ok(val) = serde_json::from_slice::<Value>(&bytes) {
                    if val.get("fork").and_then(|v| v.as_str()) == Some("zotero-rust-cli") {
                        let id = val
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or(ADDON_ID)
                            .to_string();
                        let version = val
                            .get("version")
                            .and_then(|v| v.as_str())
                            .unwrap_or("1.2.1")
                            .to_string();
                        return OwnershipStatus::ActiveOurFork { version, id };
                    }
                }
            }
        }
    }

    // 2. Probe eval endpoint with ping
    let eval_url = format!("http://127.0.0.1:{port}/cli-bridge/eval");
    match ureq::post(&eval_url)
        .header("Content-Type", "text/plain")
        .config()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .send("return 'ping';".as_bytes())
    {
        Ok(mut resp) => {
            if resp.status().as_u16() == 200 {
                if let Ok(bytes) = resp.body_mut().read_to_vec() {
                    if let Ok(val) = serde_json::from_slice::<Value>(&bytes) {
                        if val.get("fork").and_then(|v| v.as_str()) == Some("zotero-rust-cli") {
                            let id = val
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or(ADDON_ID)
                                .to_string();
                            let version = val
                                .get("version")
                                .and_then(|v| v.as_str())
                                .unwrap_or("1.2.1")
                                .to_string();
                            return OwnershipStatus::ActiveOurFork { version, id };
                        }
                    }
                }
                OwnershipStatus::ActiveUpstreamPlugin { version: None }
            } else {
                OwnershipStatus::Inactive
            }
        }
        Err(_) => OwnershipStatus::Inactive,
    }
}

/// Comprehensive report on plugin installation and active endpoint ownership status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginStatusReport {
    pub installed_on_disk: bool,
    pub installed_xpi_path: Option<String>,
    pub upstream_installed_on_disk: bool,
    pub ownership_status: OwnershipStatus,
    pub is_active: bool,
    pub message: String,
}

/// Generate a plugin status report checking both filesystem and runtime endpoint.
pub fn plugin_status(profile_dir: Option<&Path>, port: u16) -> PluginStatusReport {
    let (installed_on_disk, installed_xpi_path, upstream_installed_on_disk) = match profile_dir {
        Some(p) => {
            let our_xpi = p.join("extensions").join(XPI_FILENAME);
            let up_xpi = p.join("extensions").join(UPSTREAM_XPI_FILENAME);
            (
                our_xpi.exists(),
                if our_xpi.exists() {
                    Some(our_xpi.to_string_lossy().into_owned())
                } else {
                    None
                },
                up_xpi.exists(),
            )
        }
        None => (false, None, false),
    };

    let ownership = verify_ownership(port, Duration::from_secs(3));
    let is_active = !matches!(ownership, OwnershipStatus::Inactive);

    let message = match &ownership {
        OwnershipStatus::ActiveOurFork { version, id } => {
            format!("CLI Bridge active ({id} v{version})")
        }
        OwnershipStatus::ActiveUpstreamPlugin { .. } => {
            "CLI Bridge active (upstream plugin detected)".to_string()
        }
        OwnershipStatus::Inactive => {
            if installed_on_disk {
                "Plugin installed on disk but bridge endpoint is inactive (restart Zotero to activate)".to_string()
            } else {
                "CLI Bridge plugin is not installed".to_string()
            }
        }
    };

    PluginStatusReport {
        installed_on_disk,
        installed_xpi_path,
        upstream_installed_on_disk,
        ownership_status: ownership,
        is_active,
        message,
    }
}
