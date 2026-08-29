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

const ADDON_ENTRY_TEMPLATE: &str = r#"{
  "id": "cli-bridge@cli-anything-rust.dev",
  "syncGUID": "{9ae5ac53-3fed-4553-ad27-06f21c9f0d89}",
  "version": "1.2.1",
  "type": "extension",
  "loader": null,
  "updateURL": null,
  "installOrigins": null,
  "manifestVersion": 2,
  "optionsURL": null,
  "optionsType": null,
  "optionsBrowserStyle": true,
  "aboutURL": null,
  "defaultLocale": {
    "name": "CLI Bridge for Zotero (Rust)",
    "description": "Registers /cli-bridge/eval HTTP endpoint for CLI-Anything Zotero integration",
    "creator": null,
    "developers": null,
    "translators": null,
    "contributors": null
  },
  "visible": true,
  "active": true,
  "userDisabled": false,
  "appDisabled": false,
  "embedderDisabled": false,
  "installDate": 0,
  "updateDate": 0,
  "applyBackgroundUpdates": 1,
  "path": "",
  "skinnable": false,
  "sourceURI": null,
  "releaseNotesURI": null,
  "softDisabled": false,
  "foreignInstall": true,
  "strictCompatibility": true,
  "locales": [],
  "targetApplications": [{
    "id": "zotero@zotero.org",
    "minVersion": "7.0",
    "maxVersion": "10.*"
  }],
  "targetPlatforms": [],
  "signedState": 0,
  "signedTypes": [],
  "signedDate": null,
  "seen": true,
  "dependencies": [],
  "incognito": "spanning",
  "userPermissions": { "permissions": [], "origins": [], "data_collection": [] },
  "optionalPermissions": { "permissions": [], "origins": [], "data_collection": [] },
  "requestedPermissions": { "permissions": [], "origins": [], "data_collection": [] },
  "icons": {},
  "iconURL": null,
  "blocklistAttentionDismissed": false,
  "blocklistState": 0,
  "blocklistURL": null,
  "startupData": null,
  "hidden": false,
  "installTelemetryInfo": { "source": "app-profile", "method": "sideload" },
  "recommendationState": null,
  "rootURI": "",
  "location": "app-profile"
}"#;

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
    for pref_file_name in &["user.js", "prefs.js"] {
        let pref_file = profile_dir.join(pref_file_name);
        let current_content = std::fs::read_to_string(&pref_file).unwrap_or_default();
        let pref_line = "user_pref(\"extensions.autoDisableScopes\", 0);\n";
        if !current_content.contains("extensions.autoDisableScopes") {
            let mut new_content = current_content;
            if !new_content.is_empty() && !new_content.ends_with('\n') {
                new_content.push('\n');
            }
            new_content.push_str(pref_line);
            let _ = std::fs::write(&pref_file, new_content);
        }
    }

    // Extract or synchronize UUID if extensions.webextensions.uuids exists in prefs
    let mut sync_guid = "{bbfbe273-ad50-4520-95f0-9a8f1bc55bc6}".to_string();
    let prefs_file = profile_dir.join("prefs.js");
    if prefs_file.exists() {
        if let Ok(prefs_str) = std::fs::read_to_string(&prefs_file) {
            for line in prefs_str.lines() {
                if line.contains("extensions.webextensions.uuids") && line.contains(ADDON_ID) {
                    if let Some(start) = line.find(ADDON_ID) {
                        let rem = &line[start + ADDON_ID.len()..];
                        if let Some(colon) = rem.find(':') {
                            let rem2 = rem[colon + 1..].trim();
                            let clean: String = rem2
                                .chars()
                                .take_while(|c| *c != ',' && *c != '}' && *c != ')')
                                .filter(|c| c.is_ascii_hexdigit() || *c == '-')
                                .collect();
                            if clean.len() >= 32 {
                                sync_guid = format!("{{{clean}}}");
                            }
                        }
                    }
                }
            }
        }
    }

    // If extensions.json exists, update or register the add-on as active and verified
    let ext_json_path = profile_dir.join("extensions.json");
    if ext_json_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&ext_json_path) {
            if let Ok(mut data) = serde_json::from_str::<Value>(&content) {
                if let Some(addons) = data.get_mut("addons").and_then(|v| v.as_array_mut()) {
                    addons.retain(|a| a.get("id").and_then(|v| v.as_str()) != Some(ADDON_ID));
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    let xpi_path_str = xpi_path.to_string_lossy().to_string();
                    let encoded_uri = format!("jar:file://{}!/", xpi_path_str.replace(' ', "%20"));

                    if let Ok(mut entry) = serde_json::from_str::<Value>(ADDON_ENTRY_TEMPLATE) {
                        entry["syncGUID"] = Value::String(sync_guid);
                        entry["path"] = Value::String(xpi_path_str);
                        entry["rootURI"] = Value::String(encoded_uri);
                        entry["installDate"] = Value::Number(serde_json::Number::from(now));
                        entry["updateDate"] = Value::Number(serde_json::Number::from(now));
                        addons.push(entry);
                    }

                    if let Ok(updated_json) = serde_json::to_string_pretty(&data) {
                        let _ = std::fs::write(&ext_json_path, updated_json);
                    }
                }
            }
        }
    }

    // Invalidate stale startup cache so Zotero reloads the add-on on next boot
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
    let ext_json_path = profile_dir.join("extensions.json");
    if ext_json_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&ext_json_path) {
            if let Ok(mut data) = serde_json::from_str::<Value>(&content) {
                if let Some(addons) = data.get_mut("addons").and_then(|v| v.as_array_mut()) {
                    addons.retain(|a| a.get("id").and_then(|v| v.as_str()) != Some(ADDON_ID));
                    if let Ok(updated_json) = serde_json::to_string_pretty(&data) {
                        let _ = std::fs::write(&ext_json_path, updated_json);
                    }
                }
            }
        }
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
