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
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

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

/// Stages the fork-owned XPI plugin file into the specified Zotero profile's `extensions/` directory.
///
/// NOTE: Staging the XPI on disk writes the `.xpi` file but DOES NOT imply Zotero has activated or
/// installed it in Gecko's AddonManager, and DOES NOT imply `plugin_status` will be active.
/// In modern Zotero (7+ / 10+), active runtime registration requires explicit user installation
/// through Zotero's Add-ons manager UI.
pub fn stage_xpi(profile_dir: &Path) -> Result<PathBuf> {
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

    Ok(xpi_path)
}

/// Removes only the fork-owned staged XPI file from the specified Zotero profile directory.
///
/// NOTE: This only removes the staged `.xpi` file on disk. It does NOT claim to unregister an
/// extension that was installed/activated through Zotero's Add-ons manager UI. Manual uninstall
/// through Zotero's own Add-ons UI remains the standard removal path for an activated add-on.
pub fn remove_staged_xpi(profile_dir: &Path) -> Result<bool> {
    let xpi_path = profile_dir.join("extensions").join(XPI_FILENAME);
    if xpi_path.exists() {
        std::fs::remove_file(&xpi_path)
            .with_context(|| format!("Failed to remove XPI file: {}", xpi_path.display()))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Validates whether a response JSON payload belongs to our forked plugin.
/// Requires BOTH fork == "zotero-rust-cli" AND id == "cli-bridge@cli-anything-rust.dev".
pub fn is_owned_fork_json(val: &Value) -> bool {
    val.get("fork").and_then(|v| v.as_str()) == Some("zotero-rust-cli")
        && val.get("id").and_then(|v| v.as_str()) == Some(ADDON_ID)
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
                    if is_owned_fork_json(&val) {
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
                        if is_owned_fork_json(&val) {
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

/// Comprehensive report on plugin XPI staging on disk and active endpoint ownership status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginStatusReport {
    pub staged_on_disk: bool,
    pub staged_xpi_path: Option<String>,
    pub upstream_staged_on_disk: bool,
    pub ownership_status: OwnershipStatus,
    pub is_active: bool,
    pub message: String,
}

/// Generate a plugin status report checking both filesystem staging and runtime endpoint.
pub fn plugin_status(profile_dir: Option<&Path>, port: u16) -> PluginStatusReport {
    let (staged_on_disk, staged_xpi_path, upstream_staged_on_disk) = match profile_dir {
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
            if staged_on_disk {
                "Plugin XPI staged on disk but bridge endpoint is inactive (install via Tools > Add-ons > Install Add-on From File in Zotero, then restart)".to_string()
            } else {
                "CLI Bridge plugin is not active and XPI is not staged".to_string()
            }
        }
    };

    PluginStatusReport {
        staged_on_disk,
        staged_xpi_path,
        upstream_staged_on_disk,
        ownership_status: ownership,
        is_active,
        message,
    }
}
