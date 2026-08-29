use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::templates;
use super::types::{BridgeResponse, WriteOutcome};

pub const DEFAULT_PORT: u16 = 23119;

static POSITIVE_PROBES: LazyLock<Mutex<HashSet<u16>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Clears positive probe cache. Primarily for testing.
pub fn clear_probe_cache() {
    if let Ok(mut guard) = POSITIVE_PROBES.lock() {
        guard.clear();
    }
}

/// Formats a bridge or plugin error payload into a normalized, non-empty error string,
/// matching Python's `_format_bridge_error`.
pub fn format_bridge_error(err: &Value) -> String {
    if err.is_null() {
        return "unknown bridge error".to_string();
    }
    if let Some(s) = err.as_str() {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return "unknown bridge error".to_string();
        }
        return trimmed.to_string();
    }
    if let Some(obj) = err.as_object() {
        for key in &["error", "message", "raw", "name"] {
            if let Some(val) = obj.get(*key) {
                if let Some(s) = val.as_str() {
                    if !s.trim().is_empty() {
                        return s.trim().to_string();
                    }
                } else if !val.is_null() {
                    let s = val.to_string();
                    if !s.trim().is_empty() {
                        return s.trim().to_string();
                    }
                }
            }
        }
        let dumped = err.to_string();
        if !dumped.is_empty() && dumped != "{}" {
            return dumped;
        }
        return "unknown bridge error".to_string();
    }
    err.to_string()
}

/// JSBridgeClient handles communication with Zotero via the `/cli-bridge/eval` HTTP endpoint.
#[derive(Debug, Clone)]
pub struct JSBridgeClient {
    pub port: u16,
}

impl JSBridgeClient {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    pub fn with_default_port() -> Self {
        let port = std::env::var("ZOTERO_HTTP_PORT")
            .ok()
            .and_then(|p| p.trim().parse::<u16>().ok())
            .unwrap_or(DEFAULT_PORT);
        Self::new(port)
    }

    pub fn bridge_url(&self) -> String {
        format!("http://127.0.0.1:{}/cli-bridge/eval", self.port)
    }

    /// Check if the JS Bridge endpoint is active and verified to be our forked plugin (`zotero-rust-cli`).
    /// Uses positive probe caching (D3 fix): caches successful, verified probes for process lifetime,
    /// but never caches negative or unverified probes so retries after installation work immediately.
    /// An HTTP 200 response alone is NOT sufficient; fork ownership verification must succeed.
    pub fn bridge_endpoint_active(&self) -> bool {
        {
            if let Ok(guard) = POSITIVE_PROBES.lock() {
                if guard.contains(&self.port) {
                    return true;
                }
            }
        }

        let resp = ureq::post(&self.bridge_url())
            .header("Content-Type", "text/plain")
            .config()
            .timeout_global(Some(Duration::from_secs(3)))
            .http_status_as_error(false)
            .build()
            .send("return 'ping';".as_bytes());

        match resp {
            Ok(mut response) => {
                if response.status().as_u16() == 200 {
                    if let Ok(bytes) = response.body_mut().read_to_vec() {
                        if let Ok(val) = serde_json::from_slice::<Value>(&bytes) {
                            if val.get("fork").and_then(|v| v.as_str()) == Some("zotero-rust-cli")
                                && val.get("id").and_then(|v| v.as_str())
                                    == Some("cli-bridge@cli-anything-rust.dev")
                            {
                                if let Ok(mut guard) = POSITIVE_PROBES.lock() {
                                    guard.insert(self.port);
                                }
                                return true;
                            }
                        }
                    }
                    false
                } else {
                    false
                }
            }
            Err(_) => false,
        }
    }

    fn execute_http(&self, code: &str, timeout_secs: u64) -> BridgeResponse {
        let timeout = Duration::from_secs(timeout_secs.max(10));
        let response_result = ureq::post(&self.bridge_url())
            .header("Content-Type", "text/plain")
            .config()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build()
            .send(code.as_bytes());

        let mut response = match response_result {
            Ok(resp) => resp,
            Err(err) => {
                let msg = err.to_string();
                let lower = msg.to_lowercase();
                let formatted = if lower.contains("timed out") || lower.contains("timeout") {
                    format!("timed out: {msg}")
                } else {
                    msg
                };
                return BridgeResponse::failure(formatted);
            }
        };

        let status = response.status().as_u16();
        let raw = match response.body_mut().read_to_vec() {
            Ok(bytes) => bytes,
            Err(err) => {
                return BridgeResponse::failure(format!("Failed to read response body: {err}"));
            }
        };
        let body = String::from_utf8_lossy(&raw).into_owned();

        if status == 200 {
            let data: Value = match serde_json::from_str(&body) {
                Ok(val) => val,
                Err(_) => Value::String(body),
            };
            BridgeResponse::success(data)
        } else {
            let err_val: Option<Value> = serde_json::from_str(&body).ok();
            if let Some(err_json) = err_val {
                let message = format_bridge_error(&err_json);
                let mut resp = BridgeResponse::failure(message.clone());
                if let Some(obj) = err_json.as_object() {
                    if let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
                        resp.error_name = Some(name.to_string());
                    }
                    if let Some(stack) = obj.get("stack").and_then(|v| v.as_str()) {
                        resp.error_stack = Some(stack.to_string());
                    }
                    if let Some(raw_val) = obj.get("raw").and_then(|v| v.as_str()) {
                        if raw_val != message {
                            resp.error_raw = Some(raw_val.to_string());
                        }
                    }
                }
                resp
            } else {
                let msg = if body.trim().is_empty() {
                    format!("HTTP {status}")
                } else {
                    body
                };
                BridgeResponse::failure(msg)
            }
        }
    }

    /// Execute JavaScript via HTTP bridge. If the bridge is not active,
    /// returns an actionable error message without AppleScript fallback.
    pub fn execute_js(&self, code: &str, timeout_secs: u64) -> BridgeResponse {
        if !self.bridge_endpoint_active() {
            return BridgeResponse::failure(
                "JS Bridge endpoint not available. Install the CLI Bridge plugin: zotero-cli app install-plugin, then restart Zotero.".to_string(),
            );
        }
        self.execute_http(code, timeout_secs)
    }

    /// Execute JavaScript requiring the installed HTTP bridge plugin.
    pub fn execute_js_http_required(&self, code: &str, timeout_secs: u64) -> BridgeResponse {
        if !self.bridge_endpoint_active() {
            return BridgeResponse::failure(
                "CLI Bridge endpoint is not active. Run: zotero-cli app install-plugin, restart Zotero, then verify with: zotero-cli app plugin-status".to_string(),
            );
        }
        self.execute_http(code, timeout_secs)
    }

    /// Execute raw JavaScript and return the parsed result (Command 76 `js`).
    pub fn execute_raw_js(&self, code: &str, timeout_secs: u64) -> Result<Value> {
        let resp = self.execute_js(code, timeout_secs);
        let data = resp.require_data()?;
        Ok(data.clone())
    }

    // ── Slice 1b: JS-Bridge CRUD Fallback Operations ───────────────────────

    pub fn item_update(
        &self,
        library_id: u32,
        key: &str,
        fields: &HashMap<String, String>,
    ) -> Result<WriteOutcome> {
        if fields.is_empty() {
            bail!("No fields provided");
        }
        let code = templates::render_item_update(library_id, key, fields)?;
        let resp = self.execute_js(&code, 10);
        let data = resp.require_data()?;
        let text = data.as_str().unwrap_or("");
        if text.starts_with("OK:") {
            Ok(WriteOutcome::Applied {
                affected_key: key.to_string(),
            })
        } else if text.starts_with("ERROR:") {
            Ok(WriteOutcome::TransportError {
                detail: text.to_string(),
            })
        } else {
            Ok(WriteOutcome::Applied {
                affected_key: key.to_string(),
            })
        }
    }

    pub fn item_tag(
        &self,
        library_id: u32,
        key: &str,
        add_tags: &[String],
        remove_tags: &[String],
    ) -> Result<WriteOutcome> {
        if add_tags.is_empty() && remove_tags.is_empty() {
            bail!("No tags to add or remove");
        }
        let code = templates::render_item_tag(library_id, key, add_tags, remove_tags)?;
        let resp = self.execute_js(&code, 10);
        let data = resp.require_data()?;
        let text = data.as_str().unwrap_or("");
        if text.starts_with("OK:") {
            Ok(WriteOutcome::Applied {
                affected_key: key.to_string(),
            })
        } else if text.starts_with("ERROR:") {
            Ok(WriteOutcome::TransportError {
                detail: text.to_string(),
            })
        } else {
            Ok(WriteOutcome::Applied {
                affected_key: key.to_string(),
            })
        }
    }

    pub fn item_delete(&self, library_id: u32, key: &str) -> Result<WriteOutcome> {
        let code = templates::render_item_delete(library_id, key)?;
        let resp = self.execute_js(&code, 10);
        let data = resp.require_data()?;
        let text = data.as_str().unwrap_or("");
        if text.starts_with("DELETED:") {
            Ok(WriteOutcome::Applied {
                affected_key: key.to_string(),
            })
        } else if text.starts_with("ERROR:") {
            Ok(WriteOutcome::TransportError {
                detail: text.to_string(),
            })
        } else {
            Ok(WriteOutcome::Applied {
                affected_key: key.to_string(),
            })
        }
    }

    pub fn item_attach(
        &self,
        library_id: u32,
        key: &str,
        file_path: &Path,
    ) -> Result<WriteOutcome> {
        let abs_path = std::fs::canonicalize(file_path)
            .or_else(|_| {
                if file_path.is_absolute() {
                    Ok(file_path.to_path_buf())
                } else {
                    std::env::current_dir().map(|cwd| cwd.join(file_path))
                }
            })
            .with_context(|| format!("File not found: {}", file_path.display()))?;

        if !abs_path.is_file() {
            bail!("File not found: {}", abs_path.display());
        }

        let abs_str = abs_path.to_string_lossy();
        let code = templates::render_item_attach(library_id, key, &abs_str)?;
        let resp = self.execute_js(&code, 15);
        let data = resp.require_data()?;
        let text = data.as_str().unwrap_or("");
        if text.starts_with("OK:") {
            Ok(WriteOutcome::Applied {
                affected_key: key.to_string(),
            })
        } else if text.starts_with("ERROR:") {
            Ok(WriteOutcome::TransportError {
                detail: text.to_string(),
            })
        } else {
            Ok(WriteOutcome::Applied {
                affected_key: key.to_string(),
            })
        }
    }

    pub fn item_add_to_collection(
        &self,
        library_id: u32,
        item_key: &str,
        collection_key: &str,
    ) -> Result<WriteOutcome> {
        let code = templates::render_item_add_to_collection(library_id, item_key, collection_key)?;
        let resp = self.execute_js(&code, 10);
        let data = resp.require_data()?;
        let text = data.as_str().unwrap_or("");
        if text.starts_with("OK:") {
            Ok(WriteOutcome::Applied {
                affected_key: item_key.to_string(),
            })
        } else if text.starts_with("ERROR:") {
            Ok(WriteOutcome::TransportError {
                detail: text.to_string(),
            })
        } else {
            Ok(WriteOutcome::Applied {
                affected_key: item_key.to_string(),
            })
        }
    }

    pub fn item_move_to_collection(
        &self,
        library_id: u32,
        item_key: &str,
        to_collection_key: &str,
        from_collection_key: Option<&str>,
    ) -> Result<WriteOutcome> {
        let code = templates::render_item_move_to_collection(
            library_id,
            item_key,
            to_collection_key,
            from_collection_key,
        )?;
        let resp = self.execute_js(&code, 10);
        let data = resp.require_data()?;
        let text = data.as_str().unwrap_or("");
        if text.starts_with("OK:") {
            Ok(WriteOutcome::Applied {
                affected_key: item_key.to_string(),
            })
        } else if text.starts_with("ERROR:") {
            Ok(WriteOutcome::TransportError {
                detail: text.to_string(),
            })
        } else {
            Ok(WriteOutcome::Applied {
                affected_key: item_key.to_string(),
            })
        }
    }

    pub fn collection_create(
        &self,
        library_id: u32,
        name: &str,
        parent_key: Option<&str>,
    ) -> Result<WriteOutcome> {
        let code = templates::render_collection_create(library_id, name, parent_key)?;
        let resp = self.execute_js(&code, 10);
        let data = resp.require_data()?;
        if let Some(key) = data.get("key").and_then(|v| v.as_str()) {
            Ok(WriteOutcome::Applied {
                affected_key: key.to_string(),
            })
        } else if let Some(err) = data.get("error").and_then(|v| v.as_str()) {
            Ok(WriteOutcome::TransportError {
                detail: err.to_string(),
            })
        } else {
            bail!("Unexpected response from collection create: {data:?}");
        }
    }

    pub fn collection_rename(
        &self,
        library_id: u32,
        collection_key: &str,
        name: Option<&str>,
        parent_key: Option<&str>,
    ) -> Result<WriteOutcome> {
        if name.is_none() && parent_key.is_none() {
            bail!("No changes specified (use --name or --parent)");
        }
        let code =
            templates::render_collection_rename(library_id, collection_key, name, parent_key)?;
        let resp = self.execute_js(&code, 10);
        let data = resp.require_data()?;
        let text = data.as_str().unwrap_or("");
        if text.starts_with("OK:") {
            Ok(WriteOutcome::Applied {
                affected_key: collection_key.to_string(),
            })
        } else if text.starts_with("ERROR:") {
            Ok(WriteOutcome::TransportError {
                detail: text.to_string(),
            })
        } else {
            Ok(WriteOutcome::Applied {
                affected_key: collection_key.to_string(),
            })
        }
    }

    pub fn collection_delete(
        &self,
        library_id: u32,
        collection_key: &str,
        delete_items: bool,
    ) -> Result<WriteOutcome> {
        let code = templates::render_collection_delete(library_id, collection_key, delete_items)?;
        let resp = self.execute_js(&code, 10);
        let data = resp.require_data()?;
        let text = data.as_str().unwrap_or("");
        if text.starts_with("DELETED:") {
            Ok(WriteOutcome::Applied {
                affected_key: collection_key.to_string(),
            })
        } else if text.starts_with("ERROR:") {
            Ok(WriteOutcome::TransportError {
                detail: text.to_string(),
            })
        } else {
            Ok(WriteOutcome::Applied {
                affected_key: collection_key.to_string(),
            })
        }
    }

    pub fn collection_remove_item(
        &self,
        library_id: u32,
        item_key: &str,
        collection_key: &str,
    ) -> Result<WriteOutcome> {
        let code = templates::render_collection_remove_item(library_id, item_key, collection_key)?;
        let resp = self.execute_js(&code, 10);
        let data = resp.require_data()?;
        let text = data.as_str().unwrap_or("");
        if text.starts_with("OK:") {
            Ok(WriteOutcome::Applied {
                affected_key: item_key.to_string(),
            })
        } else if text.starts_with("ERROR:") {
            Ok(WriteOutcome::TransportError {
                detail: text.to_string(),
            })
        } else {
            Ok(WriteOutcome::Applied {
                affected_key: item_key.to_string(),
            })
        }
    }

    // ── Slice 7: Confirmed Independent Privileged JS Bridge Operations ────

    pub fn trigger_sync(&self) -> Result<String> {
        let code = templates::render_sync();
        let resp = self.execute_js(code, 30);
        let data = resp.require_data()?;
        Ok(data.as_str().unwrap_or("Sync completed").to_string())
    }

    pub fn find_duplicates(&self, library_id: u32, limit: usize) -> Result<Value> {
        let code = templates::render_find_duplicates(library_id, limit)?;
        let resp = self.execute_js(&code, 15);
        let data = resp.require_data()?;
        Ok(data.clone())
    }

    pub fn item_merge(
        &self,
        library_id: u32,
        target_key: &str,
        other_keys: &[String],
    ) -> Result<WriteOutcome> {
        if other_keys.is_empty() {
            bail!("No items to merge into target");
        }
        let code = templates::render_item_merge(library_id, target_key, other_keys)?;
        let resp = self.execute_js(&code, 15);
        let data = resp.require_data()?;
        let text = data.as_str().unwrap_or("");
        if text.starts_with("OK:") {
            Ok(WriteOutcome::Applied {
                affected_key: target_key.to_string(),
            })
        } else if text.starts_with("ERROR:") {
            Ok(WriteOutcome::TransportError {
                detail: text.to_string(),
            })
        } else {
            Ok(WriteOutcome::Applied {
                affected_key: target_key.to_string(),
            })
        }
    }
}
