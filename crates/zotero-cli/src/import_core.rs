use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

use crate::csl::{is_truthy, value_to_python_string};
use crate::import_attachments::{
    perform_attachment_upload, AttachmentConnector, HttpAttachmentConnector, RemotePdfFetcher,
    UreqRemotePdfFetcher,
};
use crate::import_normalization::{
    count_bibtex_entries, extract_inline_attachment_plans, normalize_attachment_descriptor,
    normalize_import_json_payload, split_bibtex_entries,
};
use crate::paths::expand_user_path;
use crate::runtime::RuntimeContext;
use crate::session::SessionState;

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub collection_ref: Option<String>,
    pub tags: Vec<String>,
    pub session: SessionState,
    pub attachment_manifest: Option<PathBuf>,
    pub attachment_delay_ms: i64,
    pub attachment_timeout: i64,
    pub connector_timeout: Duration,
    pub split_bib: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            collection_ref: None,
            tags: Vec::new(),
            session: SessionState::default(),
            attachment_manifest: None,
            attachment_delay_ms: 0,
            attachment_timeout: 30,
            connector_timeout: Duration::from_secs(30),
            split_bib: false,
        }
    }
}

pub trait ConnectorImportClient {
    fn import_text(
        &mut self,
        port: u16,
        content: &[u8],
        session_id: &str,
        content_type: &str,
        timeout: Duration,
    ) -> anyhow::Result<Vec<Value>>;
    fn save_items(
        &mut self,
        port: u16,
        items: &[Value],
        session_id: &str,
        timeout: Duration,
    ) -> anyhow::Result<()>;
    fn update_session(
        &mut self,
        port: u16,
        session_id: &str,
        target: &str,
        tags: &[String],
        timeout: Duration,
    ) -> anyhow::Result<Value>;
    fn get_selected_collection(&mut self, port: u16, timeout: Duration) -> anyhow::Result<Value>;
}

pub struct HttpConnectorImportClient;

impl ConnectorImportClient for HttpConnectorImportClient {
    fn import_text(
        &mut self,
        port: u16,
        content: &[u8],
        session_id: &str,
        content_type: &str,
        timeout: Duration,
    ) -> anyhow::Result<Vec<Value>> {
        crate::http::connector_import_text(port, content, Some(session_id), content_type, timeout)
    }

    fn save_items(
        &mut self,
        port: u16,
        items: &[Value],
        session_id: &str,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        crate::http::connector_save_items(port, items, session_id, timeout)
    }

    fn update_session(
        &mut self,
        port: u16,
        session_id: &str,
        target: &str,
        tags: &[String],
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        crate::http::connector_update_session(port, session_id, target, tags, timeout)
    }

    fn get_selected_collection(&mut self, port: u16, timeout: Duration) -> anyhow::Result<Value> {
        crate::http::get_selected_collection(port, timeout)
    }
}

pub fn import_file(
    runtime: &RuntimeContext,
    source_path: &Path,
    options: ImportOptions,
) -> anyhow::Result<Value> {
    let mut client = HttpConnectorImportClient;
    let mut attachment_client = HttpAttachmentConnector;
    let mut fetcher = UreqRemotePdfFetcher;
    import_file_with_clients(
        runtime,
        source_path,
        options,
        &mut client,
        &mut attachment_client,
        &mut fetcher,
    )
}

pub fn import_json(
    runtime: &RuntimeContext,
    source_path: &Path,
    options: ImportOptions,
) -> anyhow::Result<Value> {
    let mut client = HttpConnectorImportClient;
    let mut attachment_client = HttpAttachmentConnector;
    let mut fetcher = UreqRemotePdfFetcher;
    import_json_with_clients(
        runtime,
        source_path,
        options,
        &mut client,
        &mut attachment_client,
        &mut fetcher,
    )
}

pub fn import_file_with_clients<C, A, F>(
    runtime: &RuntimeContext,
    source_path: &Path,
    options: ImportOptions,
    client: &mut C,
    attachment_client: &mut A,
    fetcher: &mut F,
) -> anyhow::Result<Value>
where
    C: ConnectorImportClient,
    A: AttachmentConnector,
    F: RemotePdfFetcher,
{
    require_connector(runtime)?;
    let path = expand_existing_path(source_path, "Import file not found")?;
    let content = read_text_file(&path)?;
    let content_type = content_type_for_path(&path);
    let plans = read_attachment_manifest_option(&options)?;
    let entry_count = if content_type == "text/x-bibtex" {
        count_bibtex_entries(&content)
    } else {
        1
    };
    if options.split_bib && content_type == "text/x-bibtex" && entry_count > 1 {
        return import_split_bibtex(
            runtime,
            &path,
            &content,
            &plans,
            options,
            client,
            attachment_client,
            fetcher,
        );
    }

    let session_id = new_session_id("import-file");
    let items = client.import_text(
        runtime.environment.port,
        content.as_bytes(),
        &session_id,
        content_type,
        options.connector_timeout,
    )?;
    let target = resolve_target(
        runtime,
        options.collection_ref.as_deref(),
        &options.session,
        client,
    )?;
    let tags = normalize_tags(&options.tags);
    client.update_session(
        runtime.environment.port,
        &session_id,
        target["treeViewID"].as_str().unwrap_or(""),
        &tags,
        options.connector_timeout,
    )?;
    let (attachment_summary, attachment_results) = perform_attachment_upload(
        runtime,
        &session_id,
        &items,
        &plans,
        attachment_client,
        fetcher,
    );

    let status = if failed_count(&attachment_summary) > 0 {
        "partial_success"
    } else {
        "success"
    };
    Ok(json_object(vec![
        ("action", Value::from("import_file")),
        ("path", Value::from(path.to_string_lossy().into_owned())),
        ("status", Value::from(status)),
        ("sessionID", Value::from(session_id)),
        ("target", target),
        ("tags", string_array(&tags)),
        ("imported_count", Value::from(items.len())),
        ("items", Value::Array(items)),
        ("attachment_summary", attachment_summary),
        ("attachment_results", Value::Array(attachment_results)),
        (
            "connector_timeout",
            Value::from(options.connector_timeout.as_secs()),
        ),
        ("entry_count", Value::from(entry_count)),
    ]))
}

pub fn import_json_with_clients<C, A, F>(
    runtime: &RuntimeContext,
    source_path: &Path,
    options: ImportOptions,
    client: &mut C,
    attachment_client: &mut A,
    fetcher: &mut F,
) -> anyhow::Result<Value>
where
    C: ConnectorImportClient,
    A: AttachmentConnector,
    F: RemotePdfFetcher,
{
    require_connector(runtime)?;
    let path = expand_existing_path(source_path, "Import JSON file not found")?;
    let raw: Value = read_json_file(&path, "import file")?;
    let (items_with_attachments, format) = normalize_import_json_payload(&raw)?;
    let (items, inline_plans) = extract_inline_attachment_plans(
        &items_with_attachments,
        options.attachment_delay_ms,
        options.attachment_timeout,
    )?;
    let session_id = new_session_id("import-json");
    client.save_items(
        runtime.environment.port,
        &items,
        &session_id,
        options.connector_timeout,
    )?;
    let target = resolve_target(
        runtime,
        options.collection_ref.as_deref(),
        &options.session,
        client,
    )?;
    let tags = normalize_tags(&options.tags);
    client.update_session(
        runtime.environment.port,
        &session_id,
        target["treeViewID"].as_str().unwrap_or(""),
        &tags,
        options.connector_timeout,
    )?;
    let (attachment_summary, attachment_results) = perform_attachment_upload(
        runtime,
        &session_id,
        &items,
        &inline_plans,
        attachment_client,
        fetcher,
    );
    let status = if failed_count(&attachment_summary) > 0 {
        "partial_success"
    } else {
        "success"
    };
    Ok(json_object(vec![
        ("action", Value::from("import_json")),
        ("path", Value::from(path.to_string_lossy().into_owned())),
        ("status", Value::from(status)),
        ("sessionID", Value::from(session_id)),
        ("target", target),
        ("tags", string_array(&tags)),
        ("submitted_count", Value::from(items.len())),
        ("format", Value::from(format)),
        (
            "items",
            Value::Array(items.iter().map(project_item).collect()),
        ),
        ("attachment_summary", attachment_summary),
        ("attachment_results", Value::Array(attachment_results)),
    ]))
}

pub fn resolve_target<C: ConnectorImportClient>(
    runtime: &RuntimeContext,
    collection_ref: Option<&str>,
    session: &SessionState,
    client: &mut C,
) -> anyhow::Result<Value> {
    if let Some(collection_ref) = collection_ref.filter(|value| !value.trim().is_empty()) {
        if is_tree_view_ref(collection_ref) {
            return Ok(library_or_collection_target(collection_ref, "explicit"));
        }
        let library_id = session_library_id(session)?;
        let collection = crate::db::resolve_collection(
            &runtime.environment.sqlite_path,
            collection_ref,
            library_id,
        )?
        .ok_or_else(|| anyhow::anyhow!("Collection not found: {collection_ref}"))?;
        return Ok(collection_target(&collection, "explicit"));
    }
    if let Some(current) = session
        .current_collection
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        if is_tree_view_ref(current) {
            return Ok(library_or_collection_target(current, "session"));
        }
        if let Some(collection) = crate::db::resolve_collection(
            &runtime.environment.sqlite_path,
            current,
            session_library_id(session)?,
        )? {
            return Ok(collection_target(&collection, "session"));
        }
    }
    if runtime.connector_available {
        let selected =
            client.get_selected_collection(runtime.environment.port, Duration::from_secs(5))?;
        return selected_target(&selected);
    }
    Ok(json_object(vec![
        (
            "treeViewID",
            Value::from(default_user_library_target(runtime)?),
        ),
        ("source", Value::from("user_library")),
        ("kind", Value::from("library")),
    ]))
}

#[allow(clippy::too_many_arguments)]
fn import_split_bibtex<C, A, F>(
    runtime: &RuntimeContext,
    path: &Path,
    content: &str,
    plans: &[Value],
    options: ImportOptions,
    client: &mut C,
    attachment_client: &mut A,
    fetcher: &mut F,
) -> anyhow::Result<Value>
where
    C: ConnectorImportClient,
    A: AttachmentConnector,
    F: RemotePdfFetcher,
{
    let entries = split_bibtex_entries(content);
    let target = resolve_target(
        runtime,
        options.collection_ref.as_deref(),
        &options.session,
        client,
    )?;
    let tags = normalize_tags(&options.tags);
    let mut all_items = Vec::new();
    let mut failures = Vec::new();
    let mut attachment_results = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let session_id = new_session_id(&format!("import-file-part-{index}"));
        match client
            .import_text(
                runtime.environment.port,
                entry.as_bytes(),
                &session_id,
                "text/x-bibtex",
                options.connector_timeout,
            )
            .and_then(|items| {
                client.update_session(
                    runtime.environment.port,
                    &session_id,
                    target["treeViewID"].as_str().unwrap_or(""),
                    &tags,
                    options.connector_timeout,
                )?;
                if !plans.is_empty() && entries.len() == 1 {
                    let (_, mut results) = perform_attachment_upload(
                        runtime,
                        &session_id,
                        &items,
                        plans,
                        attachment_client,
                        fetcher,
                    );
                    attachment_results.append(&mut results);
                }
                Ok(items)
            }) {
            Ok(items) => all_items.extend(items),
            Err(error) => failures.push(json_object(vec![
                ("index", Value::from(index)),
                ("error", Value::from(error.to_string())),
                (
                    "entry_preview",
                    Value::from(entry.chars().take(120).collect::<String>()),
                ),
            ])),
        }
    }
    let attachment_summary = crate::import_attachments::attachment_summary(&attachment_results);
    let status = if failures.is_empty() {
        "success"
    } else if all_items.is_empty() {
        "error"
    } else {
        "partial_success"
    };
    Ok(json_object(vec![
        ("action", Value::from("import_file")),
        ("path", Value::from(path.to_string_lossy().into_owned())),
        ("status", Value::from(status)),
        ("target", target),
        ("tags", string_array(&tags)),
        ("imported_count", Value::from(all_items.len())),
        ("items", Value::Array(all_items)),
        ("failed_count", Value::from(failures.len())),
        ("failures", Value::Array(failures)),
        ("split_bib", Value::Bool(true)),
        ("entry_count", Value::from(entries.len())),
        (
            "connector_timeout",
            Value::from(options.connector_timeout.as_secs()),
        ),
        ("attachment_summary", attachment_summary),
        ("attachment_results", Value::Array(attachment_results)),
    ]))
}

pub fn read_attachment_manifest(
    path: &Path,
    default_delay_ms: i64,
    default_timeout: i64,
) -> anyhow::Result<Vec<Value>> {
    let path = expand_existing_path(path, "Attachment manifest not found")?;
    let raw = read_json_file(&path, "attachment manifest")?;
    let Some(entries) = raw.as_array() else {
        anyhow::bail!("Attachment manifest expects an array of {{index, attachments}} objects");
    };
    let mut seen = std::collections::HashSet::new();
    entries
        .iter()
        .enumerate()
        .map(|(entry_index, entry)| {
            let Some(obj) = entry.as_object() else {
                anyhow::bail!("manifest entry {} must be an object", entry_index + 1);
            };
            let index_value = obj.get("index").ok_or_else(|| {
                anyhow::anyhow!(
                    "manifest entry {} is missing required `index`",
                    entry_index + 1
                )
            })?;
            let index = normalize_manifest_index(index_value, entry_index)?;
            if !seen.insert(index) {
                anyhow::bail!(
                    "manifest entry {} reuses import index {}",
                    entry_index + 1,
                    index
                );
            }
            let attachments = obj
                .get("attachments")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "manifest entry {} attachments must be an array",
                        entry_index + 1
                    )
                })?;
            let normalized = attachments
                .iter()
                .enumerate()
                .map(|(attachment_index, descriptor)| {
                    let descriptor = normalize_attachment_descriptor(
                        descriptor,
                        &format!("manifest entry {}", entry_index + 1),
                        &format!("attachment {}", attachment_index + 1),
                        default_delay_ms,
                        default_timeout,
                    )?;
                    Ok(serde_json::json!({
                        "source_type": descriptor.source_type,
                        "source": descriptor.source,
                        "title": descriptor.title,
                        "delay_ms": descriptor.delay_ms,
                        "timeout": descriptor.timeout,
                    }))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let mut out = Map::new();
            out.insert("index".to_string(), Value::from(index));
            if let Some(expected_title) = obj.get("expected_title").filter(|value| !value.is_null())
            {
                let Some(expected_title) = expected_title.as_str() else {
                    anyhow::bail!(
                        "manifest entry {} expected_title must be a string",
                        entry_index + 1
                    );
                };
                out.insert("expected_title".to_string(), Value::from(expected_title));
            }
            out.insert("attachments".to_string(), Value::Array(normalized));
            Ok(Value::Object(out))
        })
        .collect()
}

fn require_connector(runtime: &RuntimeContext) -> anyhow::Result<()> {
    if runtime.connector_available {
        Ok(())
    } else {
        anyhow::bail!(
            "Zotero connector is not available: {}",
            runtime.connector_message
        )
    }
}

fn read_attachment_manifest_option(options: &ImportOptions) -> anyhow::Result<Vec<Value>> {
    options.attachment_manifest.as_deref().map_or_else(
        || Ok(Vec::new()),
        |path| {
            read_attachment_manifest(
                path,
                options.attachment_delay_ms,
                options.attachment_timeout,
            )
        },
    )
}

fn expand_existing_path(path: &Path, message: &str) -> anyhow::Result<PathBuf> {
    let expanded = expand_user_path(&path.to_string_lossy());
    if expanded.is_file() {
        Ok(expanded)
    } else {
        anyhow::bail!("{message}: {}", path.display())
    }
}

fn read_text_file(path: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path)?;
    for decoder in [decode_utf8, decode_utf8_sig, decode_utf16] {
        if let Some(text) = decoder(&bytes) {
            return Ok(text);
        }
    }
    Ok(bytes.iter().map(|byte| char::from(*byte)).collect())
}

fn decode_utf8(bytes: &[u8]) -> Option<String> {
    std::str::from_utf8(bytes).ok().map(ToString::to_string)
}

fn decode_utf8_sig(bytes: &[u8]) -> Option<String> {
    bytes
        .strip_prefix(&[0xEF, 0xBB, 0xBF])
        .and_then(decode_utf8)
}

fn decode_utf16(bytes: &[u8]) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let (bytes, big_endian) = if bytes.starts_with(&[0xFF, 0xFE]) {
        (&bytes[2..], false)
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        (&bytes[2..], true)
    } else {
        (bytes, false)
    };
    let units = bytes
        .chunks_exact(2)
        .map(|chunk| {
            if big_endian {
                u16::from_be_bytes([chunk[0], chunk[1]])
            } else {
                u16::from_le_bytes([chunk[0], chunk[1]])
            }
        })
        .collect::<Vec<_>>();
    String::from_utf16(&units).ok()
}

fn read_json_file(path: &Path, label: &str) -> anyhow::Result<Value> {
    let text = std::fs::read_to_string(path)?;
    serde_json::from_str(&text)
        .map_err(|err| anyhow::anyhow!("Invalid JSON {label}: {}: {err}", path.display()))
}

fn content_type_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "bib" | "bibtex" => "text/x-bibtex",
        "ris" => "application/x-research-info-systems",
        "enw" | "refer" => "application/x-endnote-refer",
        "xml" | "mods" => "text/xml",
        "csv" => "text/csv",
        _ => "text/plain",
    }
}

fn normalize_tags(tags: &[String]) -> Vec<String> {
    tags.iter()
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn normalize_manifest_index(value: &Value, entry_index: usize) -> anyhow::Result<u64> {
    let parsed = match value {
        Value::Number(number) => number
            .as_u64()
            .or_else(|| number.as_i64().and_then(|n| u64::try_from(n).ok()))
            .or_else(|| number.as_f64().filter(|n| *n >= 0.0).map(|n| n as u64)),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        Value::Bool(value) => Some(u64::from(*value)),
        _ => None,
    }
    .ok_or_else(|| {
        anyhow::anyhow!(
            "manifest entry {} index must be an integer greater than or equal to 0",
            entry_index + 1
        )
    })?;
    Ok(parsed)
}

fn session_library_id(session: &SessionState) -> anyhow::Result<Option<i64>> {
    session
        .current_library
        .as_ref()
        .map(value_to_python_string)
        .map(|text| crate::db::normalize_library_ref(&text))
        .transpose()
}

fn default_user_library_target(runtime: &RuntimeContext) -> anyhow::Result<String> {
    if runtime.environment.sqlite_exists {
        if let Some(library_id) = crate::db::default_library_id(&runtime.environment.sqlite_path)? {
            return Ok(format!("L{library_id}"));
        }
    }
    Ok("L1".to_string())
}

fn is_tree_view_ref(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some('L' | 'C'))
        && chars.clone().next().is_some()
        && chars.all(|ch| ch.is_ascii_digit())
}

fn library_or_collection_target(value: &str, source: &str) -> Value {
    json_object(vec![
        ("treeViewID", Value::from(value)),
        ("source", Value::from(source)),
        (
            "kind",
            Value::from(if value.starts_with('L') {
                "library"
            } else {
                "collection"
            }),
        ),
    ])
}

fn collection_target(collection: &crate::db::Collection, source: &str) -> Value {
    json_object(vec![
        (
            "treeViewID",
            Value::from(format!("C{}", collection.collection_id)),
        ),
        ("source", Value::from(source)),
        ("kind", Value::from("collection")),
        ("collectionID", Value::from(collection.collection_id)),
        ("collectionKey", Value::from(collection.key.clone())),
        (
            "collectionName",
            Value::from(collection.collection_name.clone()),
        ),
        ("libraryID", Value::from(collection.library_id)),
    ])
}

fn selected_target(selected: &Value) -> anyhow::Result<Value> {
    if selected.get("id").is_some_and(|value| !value.is_null()) {
        let collection_id = selected.get("id").unwrap();
        return Ok(json_object(vec![
            (
                "treeViewID",
                Value::from(format!("C{}", value_to_python_string(collection_id))),
            ),
            ("source", Value::from("selected")),
            ("kind", Value::from("collection")),
            ("collectionID", collection_id.clone()),
            (
                "collectionName",
                selected.get("name").cloned().unwrap_or(Value::Null),
            ),
            (
                "libraryID",
                selected.get("libraryID").cloned().unwrap_or(Value::Null),
            ),
            (
                "libraryName",
                selected.get("libraryName").cloned().unwrap_or(Value::Null),
            ),
        ]));
    }
    let library_id = selected
        .get("libraryID")
        .ok_or_else(|| anyhow::anyhow!("Selected collection response is missing libraryID"))?;
    Ok(json_object(vec![
        (
            "treeViewID",
            Value::from(format!("L{}", value_to_python_string(library_id))),
        ),
        ("source", Value::from("selected")),
        ("kind", Value::from("library")),
        ("libraryID", library_id.clone()),
        (
            "libraryName",
            selected.get("libraryName").cloned().unwrap_or(Value::Null),
        ),
    ]))
}

fn project_item(item: &Value) -> Value {
    let title = ["title", "bookTitle", "publicationTitle"]
        .iter()
        .filter_map(|key| item.get(*key))
        .find(|value| is_truthy(Some(value)))
        .cloned()
        .unwrap_or(Value::Null);
    json_object(vec![
        ("id", item.get("id").cloned().unwrap_or(Value::Null)),
        (
            "itemType",
            item.get("itemType").cloned().unwrap_or(Value::Null),
        ),
        ("title", title),
    ])
}

fn failed_count(summary: &Value) -> u64 {
    summary
        .get("failed_count")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn new_session_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let count = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{nanos:x}{count:x}")
}

fn string_array(values: &[String]) -> Value {
    Value::Array(values.iter().cloned().map(Value::from).collect())
}

fn json_object(entries: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}
