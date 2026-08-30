use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::templates;
use super::types::BridgeResponse;
// The canonical write-outcome contract, shared with the Local API write path -- re-exported by
// `super` (`bridge/mod.rs`) from `crate::write`, not imported directly from there, so this file
// compiles identically whether `bridge` is a real child module of this crate (Slice 6) or
// included via `#[path]` into a test binary (today) -- see `bridge/mod.rs`'s own note.
use super::WriteOutcome;

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

/// Maps a Bridge write call's raw response onto the canonical `WriteOutcome`, at the
/// write-operation boundary (not the transport layer -- `execute_http`/`execute_js` are
/// untouched). Every JS-Bridge write template in this crate returns one of two shapes: a
/// prefixed status string (`"<success_prefix>: ..."` / `"ERROR: ..."`) or, for
/// `collection_create`, a JSON object -- see [`map_object_outcome`] for that one.
///
/// The Bridge has no consent/authorization semantics of its own to distinguish from a generic
/// failure: `execute_js`/`execute_js_http_required` already gate every call on
/// `bridge_endpoint_active()`'s fork+id ownership verification *before* this function is ever
/// reached, so nothing here can legitimately become `WriteOutcome::AuthorizationFailed` --
/// doing so would misrepresent an ordinary script failure as a consent-dialog-shaped problem
/// that doesn't exist at this layer.
///
/// Three failure paths, all folded into `TransportError` (preserving the pre-existing meaning of
/// the Bridge's own `"ERROR:"` convention -- it never distinguished precondition-vs-conflict
/// failures, so this does not invent that distinction):
/// - the bridge call itself failed (endpoint unavailable, non-200, or `ok: false`) --
///   previously escaped as an untyped `anyhow::Error` via `resp.require_data()?`, now folded in
///   here so every reachable outcome is a `WriteOutcome`, matching the Local API write path's
///   own transport-safety convention (`write_router.rs`);
/// - the script itself reported `"ERROR: ..."`;
/// - the response matched neither the success prefix nor `"ERROR:"` -- previously fell through
///   to `Applied` by mistake (a real bug this convergence fixes), now correctly treated as an
///   ambiguous/unrecognized response, per the same "never silently claim success" rule the Local
///   API path already follows.
fn map_text_outcome(
    resp: &BridgeResponse,
    success_prefix: &str,
    affected_key: &str,
) -> WriteOutcome {
    let data = match resp.require_data() {
        Ok(data) => data,
        Err(err) => {
            return WriteOutcome::TransportError {
                detail: err.to_string(),
            };
        }
    };
    let text = data.as_str().unwrap_or("");
    if text.starts_with(success_prefix) {
        WriteOutcome::Applied {
            affected_key: affected_key.to_string(),
        }
    } else if text.starts_with("ERROR:") {
        WriteOutcome::TransportError {
            detail: text.to_string(),
        }
    } else {
        WriteOutcome::TransportError {
            detail: format!(
                "bridge returned an unrecognized response (expected a \"{success_prefix}\" or \"ERROR:\" prefix): {text:?}"
            ),
        }
    }
}

/// `type(data).__name__` for the JSON shapes `serde_json::Value` can hold, matching Python's
/// runtime type names for `core/notes.py::add_note`'s own error message
/// (`f"...got {type(data).__name__}): {data}"`).
fn python_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "int"
            } else {
                "float"
            }
        }
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Renders `data` the way Python's f-string `f"...: {data}"` would for a non-dict value --
/// unquoted for a string (the common case here: the Bridge's own `'ERROR: ...'` string when the
/// parent item vanished between resolution and the write attempt), `None` for null, and each
/// other JSON shape's compact form otherwise (a documented minor divergence from Python's `repr`
/// for `list`/`bool`/`float`, since those shapes never occur from this crate's own JS templates).
fn format_bridge_data_for_error(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "None".to_string(),
        other => other.to_string(),
    }
}

/// Same idea as [`map_text_outcome`] but for `collection_create`'s JSON-object response shape
/// (`{"key": "..."}` on success, `{"error": "..."}` on failure) rather than a prefixed string.
fn map_object_outcome(resp: &BridgeResponse) -> WriteOutcome {
    let data = match resp.require_data() {
        Ok(data) => data,
        Err(err) => {
            return WriteOutcome::TransportError {
                detail: err.to_string(),
            };
        }
    };
    // "key" and "error" must be mutually exclusive for this to be an unambiguous success --
    // a response carrying both (e.g. `{"key":"ABC123","error":"failed"}`) is malformed/
    // self-contradictory, not a success with an ignorable stray field, so it must not become
    // `Applied` (the same "never silently claim success on an ambiguous response" rule
    // `map_text_outcome` follows). An empty-string key is likewise not a usable affected key.
    let key = data
        .get("key")
        .and_then(|v| v.as_str())
        .filter(|k| !k.is_empty());
    let error = data.get("error").and_then(|v| v.as_str());
    match (key, error) {
        (Some(key), None) => WriteOutcome::Applied {
            affected_key: key.to_string(),
        },
        (None, Some(err)) => WriteOutcome::TransportError {
            detail: err.to_string(),
        },
        (Some(_), Some(_)) => WriteOutcome::TransportError {
            detail: format!("bridge returned both \"key\" and \"error\" (ambiguous/malformed): {data:?}"),
        },
        (None, None) => WriteOutcome::TransportError {
            detail: format!(
                "bridge returned an unrecognized response (expected a non-empty \"key\" or \"error\"): {data:?}"
            ),
        },
    }
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
        Ok(map_text_outcome(&resp, "OK:", key))
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
        Ok(map_text_outcome(&resp, "OK:", key))
    }

    pub fn item_delete(&self, library_id: u32, key: &str) -> Result<WriteOutcome> {
        let code = templates::render_item_delete(library_id, key)?;
        let resp = self.execute_js(&code, 10);
        Ok(map_text_outcome(&resp, "DELETED:", key))
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
        Ok(map_text_outcome(&resp, "OK:", key))
    }

    pub fn item_add_to_collection(
        &self,
        library_id: u32,
        item_key: &str,
        collection_key: &str,
    ) -> Result<WriteOutcome> {
        let code = templates::render_item_add_to_collection(library_id, item_key, collection_key)?;
        let resp = self.execute_js(&code, 10);
        Ok(map_text_outcome(&resp, "OK:", item_key))
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
        Ok(map_text_outcome(&resp, "OK:", item_key))
    }

    pub fn collection_create(
        &self,
        library_id: u32,
        name: &str,
        parent_key: Option<&str>,
    ) -> Result<WriteOutcome> {
        let code = templates::render_collection_create(library_id, name, parent_key)?;
        let resp = self.execute_js(&code, 10);
        Ok(map_object_outcome(&resp))
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
        Ok(map_text_outcome(&resp, "OK:", collection_key))
    }

    pub fn collection_delete(
        &self,
        library_id: u32,
        collection_key: &str,
        delete_items: bool,
    ) -> Result<WriteOutcome> {
        let code = templates::render_collection_delete(library_id, collection_key, delete_items)?;
        let resp = self.execute_js(&code, 10);
        Ok(map_text_outcome(&resp, "DELETED:", collection_key))
    }

    pub fn collection_remove_item(
        &self,
        library_id: u32,
        item_key: &str,
        collection_key: &str,
    ) -> Result<WriteOutcome> {
        let code = templates::render_collection_remove_item(library_id, item_key, collection_key)?;
        let resp = self.execute_js(&code, 10);
        Ok(map_text_outcome(&resp, "OK:", item_key))
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
        Ok(map_text_outcome(&resp, "OK:", target_key))
    }

    // ── Phase 7 Slice 3: PDF cascade discovery primitives ──────────────────

    /// Triggers Zotero's own "Find Available PDF" for one item
    /// (`Zotero.Attachments.addAvailablePDF`). Returns the raw transport response --
    /// `"FOUND: <key>"` / `"NOT_FOUND: ..."` / `"ERROR: ..."` -- for the caller to interpret,
    /// matching `core/jsbridge.py::find_pdf`'s own layering (this method never parses the
    /// prefix itself).
    ///
    /// On an ambiguous timeout (the addAvailablePDF call's own error message contains
    /// "timed out"), this does **not** retry `addAvailablePDF` -- a second, unrelated
    /// download could already be in flight server-side. Instead it issues one cheap
    /// read-only verification call that inspects the item's current attachments directly,
    /// so an ambiguous transport failure never risks a duplicate download.
    pub fn find_pdf(&self, library_id: u32, key: &str, timeout_secs: u64) -> BridgeResponse {
        let code = match templates::render_find_pdf(library_id, key) {
            Ok(code) => code,
            Err(err) => return BridgeResponse::failure(err.to_string()),
        };
        let resp = self.execute_js(&code, timeout_secs.max(10));
        let is_timeout = resp
            .error_message()
            .map(|msg| msg.to_lowercase().contains("timed out"))
            .unwrap_or(false);
        if resp.is_ok() || !is_timeout {
            return resp;
        }
        let verify_code = match templates::render_find_pdf_verify(library_id, key, timeout_secs) {
            Ok(code) => code,
            Err(err) => return BridgeResponse::failure(err.to_string()),
        };
        self.execute_js(&verify_code, 10)
    }

    /// Lists regular items in a collection that have no `application/pdf`-typed attachment
    /// yet. Returns the raw transport response (`data` is `{ok, total, missing, missing_count}`
    /// on success) for the caller to interpret -- matches `core/jsbridge.py::list_items_missing_pdf`.
    pub fn list_items_missing_pdf(&self, library_id: u32, collection_key: &str) -> BridgeResponse {
        let code = match templates::render_list_items_missing_pdf(library_id, collection_key) {
            Ok(code) => code,
            Err(err) => return BridgeResponse::failure(err.to_string()),
        };
        self.execute_js(&code, 15)
    }

    /// `core/jsbridge.py::collection_stats` (hardcoded `library_id = 1` at the CLI layer --
    /// `collection stats` takes no `--library` option in Python, so a group-library collection
    /// can never be targeted through this command). Read-only: counts regular (non-attachment,
    /// non-note) items, PDF-attached vs. not, a publication-year histogram, and the top 10
    /// journals by item count.
    ///
    /// A missing collection returns the bare string `"ERROR: collection <key> not found"` as
    /// `data` (not a `{ok: false, ...}` object) -- matching Python's `emit_js`, this is a
    /// *transport success* (exit code `0`, not `1`), the same `"ERROR: ..."`-string-is-still-
    /// success quirk `get_annotations` already documents.
    pub fn collection_stats(&self, library_id: u32, collection_key: &str) -> BridgeResponse {
        let code = match templates::render_collection_stats(library_id, collection_key) {
            Ok(code) => code,
            Err(err) => return BridgeResponse::failure(err.to_string()),
        };
        self.execute_js(&code, 8)
    }

    // ── Phase 7 Slice 4: Note creation (Bridge-only, single mutation attempt, no retry) ────

    /// `core/notes.py::add_note`'s inline JS block (`Zotero.Items.getByLibraryAndKey` ->
    /// `new Zotero.Item('note')` -> `setNote` -> `saveTx()`), rendered through the shared
    /// `JSON.parse` template mechanism instead of Python's raw string interpolation, so the
    /// note's normalized HTML is never spliced into JS source text.
    ///
    /// Returns the raw success payload (`{key, itemID, title}`) for `notes::add_note` to project
    /// into the Python-compatible result shape. Error handling matches
    /// `core/notes.py::add_note` exactly, at the byte level of both messages:
    /// - transport/script failure (`ok: false`, including the JS template's own
    ///   `'ERROR: parent item not found'` string return, which is *not* a dict) ->
    ///   `"Failed to create note via JS bridge: {error}"`;
    /// - a well-formed (`ok: true`) but non-object payload -> `"Unexpected JS Bridge response
    ///   (expected dict, got {type}): {data}"`;
    /// - an object payload missing concrete saved-note identity (`key` or `itemID`) ->
    ///   `"Invalid note creation response: missing or invalid key/itemID"`.
    ///
    /// Exactly one `execute_js` call: no retry on timeout, transport failure, or an ambiguous/
    /// malformed response -- a second attempt could create a duplicate note server-side, and
    /// `saveTx()` is not idempotent.
    pub fn note_add(&self, library_id: u32, parent_key: &str, note_html: &str) -> Result<Value> {
        let code = templates::render_note_add(library_id, parent_key, note_html)?;
        let resp = self.execute_js(&code, 10);
        if !resp.ok {
            let err = resp.error.as_deref().unwrap_or("unknown error");
            bail!("Failed to create note via JS bridge: {err}");
        }
        let data = resp.data.clone().unwrap_or(Value::Null);
        if !data.is_object() {
            bail!(
                "Unexpected JS Bridge response (expected dict, got {}): {}",
                python_type_name(&data),
                format_bridge_data_for_error(&data)
            );
        }
        let key_valid = data
            .get("key")
            .and_then(Value::as_str)
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        let item_id_valid = data.get("itemID").map(Value::is_i64).unwrap_or(false);
        if !key_valid || !item_id_valid {
            bail!("Invalid note creation response: missing or invalid key/itemID");
        }
        Ok(data)
    }

    /// `core/jsbridge.py::JSBridgeClient.search_fulltext` -- delegates to Zotero's live
    /// `Zotero.Search` engine (`fulltextContent contains query`), never Zotero's FTS SQLite
    /// tables directly. No index-state polling/waiting/retry: an unindexed or not-yet-searchable
    /// PDF simply yields no match, exactly as Python never waits either.
    pub fn search_fulltext(&self, library_id: u32, query: &str, limit: i64) -> BridgeResponse {
        let code = match templates::render_search_fulltext(library_id, query, limit) {
            Ok(code) => code,
            Err(err) => return BridgeResponse::failure(err.to_string()),
        };
        self.execute_js(&code, 8)
    }

    /// `core/jsbridge.py::JSBridgeClient.search_annotations`. Empty `query` searches
    /// `itemType is 'annotation'`; non-empty searches `annotationText contains query` --
    /// `annotationComment` is never searched. Color filtering (when `colors` is non-empty) is
    /// applied before the `limit` slice, matching Python's `filtered.slice(0, limit)` ordering.
    pub fn search_annotations(
        &self,
        library_id: u32,
        query: &str,
        colors: Option<&[String]>,
        limit: i64,
    ) -> BridgeResponse {
        let code = match templates::render_search_annotations(library_id, query, colors, limit) {
            Ok(code) => code,
            Err(err) => return BridgeResponse::failure(err.to_string()),
        };
        self.execute_js(&code, 8)
    }

    /// `core/jsbridge.py::JSBridgeClient.get_annotations`. Accepts a raw item key only: if that
    /// item is itself a PDF attachment, the JS walks up to its bibliographic parent before
    /// collecting annotations from all of the parent's PDF attachments. Per-attachment
    /// `getAnnotations()` errors are swallowed individually (`try {} catch (e) {}` inside the
    /// loop) so one bad PDF never fails the whole call. A not-found item/parent yields a bare
    /// `"ERROR: ..."` string, not a transport failure -- matching Python's `emit_js`, this is
    /// still a *successful* Bridge response at the transport level.
    pub fn get_annotations(&self, library_id: u32, key: &str) -> BridgeResponse {
        let code = match templates::render_get_annotations(library_id, key) {
            Ok(code) => code,
            Err(err) => return BridgeResponse::failure(err.to_string()),
        };
        self.execute_js(&code, 5)
    }

    // ── Add/import composition Bridge primitives ────

    pub fn find_items_by_doi(&self, library_id: u32, doi: &str, limit: i64) -> BridgeResponse {
        let code = match templates::render_find_items_by_doi(library_id, doi, limit) {
            Ok(code) => code,
            Err(err) => return BridgeResponse::failure(err.to_string()),
        };
        self.execute_js(&code, 10)
    }

    pub fn import_from_doi(
        &self,
        library_id: u32,
        doi: &str,
        collection_key: Option<&str>,
        tags: Option<&[String]>,
    ) -> BridgeResponse {
        let code = match templates::render_import_from_doi(library_id, doi, collection_key, tags) {
            Ok(code) => code,
            Err(err) => return BridgeResponse::failure(err.to_string()),
        };
        self.execute_js(&code, 45)
    }

    pub fn import_from_pmid(
        &self,
        library_id: u32,
        pmid: &str,
        collection_key: Option<&str>,
        tags: Option<&[String]>,
    ) -> BridgeResponse {
        let code = match templates::render_import_from_pmid(library_id, pmid, collection_key, tags)
        {
            Ok(code) => code,
            Err(err) => return BridgeResponse::failure(err.to_string()),
        };
        self.execute_js(&code, 45)
    }

    pub fn standalone_pdf_import(
        &self,
        library_id: u32,
        file_path: &str,
        title: &str,
        collection_key: Option<&str>,
        tags: &[String],
    ) -> BridgeResponse {
        let code = match templates::render_standalone_pdf_import(
            library_id,
            file_path,
            title,
            collection_key,
            tags,
        ) {
            Ok(code) => code,
            Err(err) => return BridgeResponse::failure(err.to_string()),
        };
        self.execute_js(&code, 30)
    }
}

/// Mirrors `zotero_cli.py::emit_js`'s payload/exit classification for a raw Bridge transport
/// response: a transport-level failure returns the whole envelope with `false`; a transport
/// success whose `data` is an object carrying `"ok": false` is application-level failure;
/// anything else -- including a bare `"ERROR: ..."` string -- is success, returned as-is.
/// Returns `(payload_to_emit, is_success)`.
pub fn classify_bridge_payload_with_options(
    transport: &BridgeResponse,
    require_data: bool,
) -> (Value, bool) {
    if !transport.ok {
        let mut payload = serde_json::Map::new();
        payload.insert("ok".to_string(), Value::Bool(false));
        payload.insert("data".to_string(), Value::Null);
        payload.insert(
            "error".to_string(),
            transport
                .error
                .clone()
                .map(Value::from)
                .unwrap_or(Value::Null),
        );
        if let Some(error_name) = &transport.error_name {
            payload.insert("error_name".to_string(), Value::from(error_name.clone()));
        }
        if let Some(error_stack) = &transport.error_stack {
            payload.insert("error_stack".to_string(), Value::from(error_stack.clone()));
        }
        if let Some(error_raw) = &transport.error_raw {
            payload.insert("error_raw".to_string(), Value::from(error_raw.clone()));
        }
        return (Value::Object(payload), false);
    }
    match &transport.data {
        Some(Value::Null) if require_data => (
            serde_json::json!({
                "ok": false,
                "data": null,
                "error": "JS bridge returned empty success (data is null)",
                "code": "EMPTY_RESULT",
            }),
            false,
        ),
        Some(Value::Object(map)) if map.get("ok") == Some(&Value::Bool(false)) => {
            (Value::Object(map.clone()), false)
        }
        Some(data) => (data.clone(), true),
        None if require_data => (
            serde_json::json!({
                "ok": false,
                "data": null,
                "error": "JS bridge returned empty success (data is null)",
                "code": "EMPTY_RESULT",
            }),
            false,
        ),
        None => (serde_json::to_value(transport).unwrap_or(Value::Null), true),
    }
}

pub fn classify_bridge_payload(transport: &BridgeResponse) -> (Value, bool) {
    classify_bridge_payload_with_options(transport, false)
}
