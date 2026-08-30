use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde_json::{Map, Value};

use crate::bridge::client::classify_bridge_payload_with_options;
use crate::bridge::{BridgeResponse, JSBridgeClient, WriteOutcome};
use crate::csl::value_to_python_string;
use crate::import_core::{self, ConnectorImportClient, HttpConnectorImportClient, ImportOptions};
use crate::import_normalization::normalize_doi;
use crate::paths::expand_user_path;
use crate::pdf_fetch::{self, UreqPdfClient};
use crate::runtime::RuntimeContext;
use crate::session::SessionState;

static ARXIV_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:arXiv:)?(\d{4}\.\d{4,5})(?:v\d+)?$").unwrap());
static BARE_ARXIV_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d{4}\.\d{4,5})").unwrap());
static DOI_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)10\.\d{4,9}/[^\s"'<>]+"#).unwrap());
static DOI_URL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"10\.\d{4,9}/[^\s]+"#).unwrap());
static HTML_DOI_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"10\.\d{4,9}/[A-Za-z0-9./_;()-]+").unwrap());
static TITLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap());
static SPACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);
pub const ADD_IMPORT_HTTP_USER_AGENT: &str =
    "cli-anything-zotero/1.2 (mailto:cli-anything@local; research agent)";
pub const ADD_IMPORT_PDF_FETCH_TIMEOUT_SECONDS: u64 = 45;

#[derive(Debug, Clone)]
pub struct AddImportOptions {
    pub collection_key: Option<String>,
    pub tags: Vec<String>,
    pub session: SessionState,
    pub if_exists: String,
    pub dedupe: bool,
    pub prefer_translator: bool,
    pub fetch_pdf: bool,
    pub pdf_sources: Option<String>,
    pub library_id: i64,
    pub connector_timeout: Duration,
}

impl Default for AddImportOptions {
    fn default() -> Self {
        Self {
            collection_key: None,
            tags: Vec::new(),
            session: SessionState::default(),
            if_exists: "file".to_string(),
            dedupe: true,
            prefer_translator: true,
            fetch_pdf: false,
            pdf_sources: None,
            library_id: 1,
            connector_timeout: Duration::from_secs(120),
        }
    }
}

pub trait AddImportBridge {
    fn find_items_by_doi(&mut self, library_id: u32, doi: &str, limit: i64) -> BridgeResponse;
    fn import_from_doi(
        &mut self,
        library_id: u32,
        doi: &str,
        collection_key: Option<&str>,
        tags: Option<&[String]>,
    ) -> BridgeResponse;
    fn import_from_pmid(
        &mut self,
        library_id: u32,
        pmid: &str,
        collection_key: Option<&str>,
        tags: Option<&[String]>,
    ) -> BridgeResponse;
    fn add_to_collection(
        &mut self,
        library_id: u32,
        item_key: &str,
        collection_key: &str,
    ) -> anyhow::Result<WriteOutcome>;
    fn manage_tags(
        &mut self,
        library_id: u32,
        item_key: &str,
        add_tags: &[String],
    ) -> anyhow::Result<WriteOutcome>;
    fn attach_pdf(&mut self, library_id: u32, item_key: &str, path: &Path) -> BridgeResponse;
    fn standalone_pdf_import(
        &mut self,
        library_id: u32,
        file_path: &str,
        title: &str,
        collection_key: Option<&str>,
        tags: &[String],
    ) -> BridgeResponse;
}

impl AddImportBridge for JSBridgeClient {
    fn find_items_by_doi(&mut self, library_id: u32, doi: &str, limit: i64) -> BridgeResponse {
        JSBridgeClient::find_items_by_doi(self, library_id, doi, limit)
    }

    fn import_from_doi(
        &mut self,
        library_id: u32,
        doi: &str,
        collection_key: Option<&str>,
        tags: Option<&[String]>,
    ) -> BridgeResponse {
        JSBridgeClient::import_from_doi(self, library_id, doi, collection_key, tags)
    }

    fn import_from_pmid(
        &mut self,
        library_id: u32,
        pmid: &str,
        collection_key: Option<&str>,
        tags: Option<&[String]>,
    ) -> BridgeResponse {
        JSBridgeClient::import_from_pmid(self, library_id, pmid, collection_key, tags)
    }

    fn add_to_collection(
        &mut self,
        library_id: u32,
        item_key: &str,
        collection_key: &str,
    ) -> anyhow::Result<WriteOutcome> {
        JSBridgeClient::item_add_to_collection(self, library_id, item_key, collection_key)
    }

    fn manage_tags(
        &mut self,
        library_id: u32,
        item_key: &str,
        add_tags: &[String],
    ) -> anyhow::Result<WriteOutcome> {
        JSBridgeClient::item_tag(self, library_id, item_key, add_tags, &[])
    }

    fn attach_pdf(&mut self, library_id: u32, item_key: &str, path: &Path) -> BridgeResponse {
        let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let code = match crate::bridge::templates::render_item_attach(
            library_id,
            item_key,
            &abs_path.to_string_lossy(),
        ) {
            Ok(code) => code,
            Err(err) => return BridgeResponse::failure(err.to_string()),
        };
        self.execute_js(&code, 4)
    }

    fn standalone_pdf_import(
        &mut self,
        library_id: u32,
        file_path: &str,
        title: &str,
        collection_key: Option<&str>,
        tags: &[String],
    ) -> BridgeResponse {
        JSBridgeClient::standalone_pdf_import(
            self,
            library_id,
            file_path,
            title,
            collection_key,
            tags,
        )
    }
}

pub trait AddImportFetcher {
    fn fetch_crossref_bibtex(&mut self, doi: &str, timeout: Duration) -> anyhow::Result<String>;
    fn fetch_arxiv_bibtex(&mut self, arxiv_id: &str, timeout: Duration) -> anyhow::Result<String>;
    fn fetch_html_title_and_doi(
        &mut self,
        url: &str,
        timeout: Duration,
    ) -> anyhow::Result<(Option<String>, Option<String>, String)>;
}

pub struct UreqAddImportFetcher;

impl AddImportFetcher for UreqAddImportFetcher {
    fn fetch_crossref_bibtex(&mut self, doi: &str, timeout: Duration) -> anyhow::Result<String> {
        fetch_text(
            &format!(
                "https://api.crossref.org/works/{}/transform/application/x-bibtex",
                percent_encode_quote_default(doi)
            ),
            timeout,
            "application/x-bibtex",
            Some("cli-anything-zotero/1.0 (mailto:cli-anything@local)"),
        )
        .and_then(|body| {
            require_bibtex(
                body,
                &format!("Crossref returned empty/invalid BibTeX for {doi}"),
            )
        })
        .map_err(|err| anyhow::anyhow!("Crossref BibTeX fetch failed for {doi}: {err}"))
    }

    fn fetch_arxiv_bibtex(&mut self, arxiv_id: &str, timeout: Duration) -> anyhow::Result<String> {
        fetch_text(
            &format!("https://arxiv.org/bibtex/{arxiv_id}"),
            timeout,
            "application/x-bibtex,text/plain,*/*",
            Some(ADD_IMPORT_HTTP_USER_AGENT),
        )
        .and_then(|body| require_bibtex(body, "empty arXiv bibtex"))
    }

    fn fetch_html_title_and_doi(
        &mut self,
        url: &str,
        timeout: Duration,
    ) -> anyhow::Result<(Option<String>, Option<String>, String)> {
        let body = fetch_text(
            url,
            timeout,
            "text/html,*/*",
            Some(ADD_IMPORT_HTTP_USER_AGENT),
        )?;
        let title = TITLE_RE.captures(&body).and_then(|captures| {
            captures.get(1).and_then(|m| {
                let collapsed = SPACE_RE.replace_all(m.as_str(), " ");
                let title = collapsed.trim().chars().take(300).collect::<String>();
                (!title.is_empty()).then_some(title)
            })
        });
        let doi = HTML_DOI_RE
            .find(&body)
            .map(|mat| normalize_doi(Some(mat.as_str())));
        Ok((title, doi, url.to_string()))
    }
}

pub fn add_doi(
    runtime: &RuntimeContext,
    bridge: &mut JSBridgeClient,
    doi: &str,
    options: AddImportOptions,
) -> Value {
    let mut connector = HttpConnectorImportClient;
    let mut fetcher = UreqAddImportFetcher;
    let imported = import_doi_with_clients(
        runtime,
        bridge,
        &mut connector,
        &mut fetcher,
        doi,
        options.clone(),
    );
    let mut payload = result_payload(
        "add_doi",
        imported.get("ok") == Some(&Value::Bool(true)),
        imported
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                if imported.get("ok") == Some(&Value::Bool(true)) {
                    "success"
                } else {
                    "error"
                }
            }),
        imported.get("code").and_then(Value::as_str),
        imported.get("error").and_then(Value::as_str),
        vec![
            (
                "DOI",
                imported
                    .get("DOI")
                    .cloned()
                    .unwrap_or_else(|| Value::from(normalize_doi(Some(doi)))),
            ),
            ("key", imported.get("key").cloned().unwrap_or(Value::Null)),
            (
                "title",
                imported.get("title").cloned().unwrap_or(Value::Null),
            ),
            (
                "source",
                imported.get("source").cloned().unwrap_or(Value::Null),
            ),
            ("import_result", imported.clone()),
        ],
    );
    maybe_fetch_pdf(
        runtime,
        bridge,
        &mut payload,
        &options,
        "zotero,unpaywall,epmc,biorxiv,arxiv",
    );
    payload
}

pub fn add_arxiv(
    runtime: &RuntimeContext,
    bridge: &mut JSBridgeClient,
    arxiv_id: &str,
    mut options: AddImportOptions,
) -> Value {
    let mut connector = HttpConnectorImportClient;
    let mut fetcher = UreqAddImportFetcher;
    let mut payload = add_arxiv_with_clients(
        runtime,
        bridge,
        &mut connector,
        &mut fetcher,
        arxiv_id,
        &mut options,
    );
    maybe_fetch_pdf(
        runtime,
        bridge,
        &mut payload,
        &options,
        "zotero,arxiv,unpaywall",
    );
    payload
}

pub fn add_arxiv_with_clients<B, C, F>(
    runtime: &RuntimeContext,
    bridge: &mut B,
    connector: &mut C,
    fetcher: &mut F,
    arxiv_id: &str,
    options: &mut AddImportOptions,
) -> Value
where
    B: AddImportBridge,
    C: ConnectorImportClient,
    F: AddImportFetcher,
{
    let aid = match normalize_arxiv_id(arxiv_id) {
        Ok(aid) => aid,
        Err(err) => {
            return result_payload(
                "add_arxiv",
                false,
                "error",
                Some("INVALID_ARXIV"),
                Some(&err.to_string()),
                vec![],
            )
        }
    };
    let doi = format!("10.48550/arXiv.{aid}");
    let mut doi_options = options.clone();
    doi_options.prefer_translator = true;
    let mut imported =
        import_doi_with_clients(runtime, bridge, connector, fetcher, &doi, doi_options);
    if imported.get("ok") != Some(&Value::Bool(true)) {
        match import_arxiv_bibtex(runtime, connector, fetcher, &aid, &doi, options) {
            Ok(value) => imported = value,
            Err(err) => {
                return result_payload(
                    "add_arxiv",
                    false,
                    "error",
                    Some("IMPORT_FAILED"),
                    Some(&err.to_string()),
                    vec![
                        ("arxiv_id", Value::from(aid)),
                        ("DOI", Value::from(doi)),
                        ("import_result", imported),
                    ],
                )
            }
        }
    }
    let status = imported
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("success")
        .to_string();
    let code = imported
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("IMPORTED")
        .to_string();
    let error = imported
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string);
    let payload = result_payload(
        "add_arxiv",
        imported.get("ok") == Some(&Value::Bool(true)),
        &status,
        Some(&code),
        error.as_deref(),
        vec![
            ("key", imported.get("key").cloned().unwrap_or(Value::Null)),
            (
                "title",
                imported.get("title").cloned().unwrap_or(Value::Null),
            ),
            (
                "DOI",
                imported
                    .get("DOI")
                    .cloned()
                    .unwrap_or_else(|| Value::from(doi)),
            ),
            ("arxiv_id", Value::from(aid)),
            (
                "source",
                imported.get("source").cloned().unwrap_or(Value::Null),
            ),
            ("import_result", imported),
        ],
    );
    // Generic bridge trait tests cover composition without live PDF mutation; the public JSBridge
    // wrapper applies PDF fetch below.
    payload
}

pub fn add_url(
    runtime: &RuntimeContext,
    bridge: &mut JSBridgeClient,
    url: &str,
    options: AddImportOptions,
) -> Value {
    let mut connector = HttpConnectorImportClient;
    let mut fetcher = UreqAddImportFetcher;
    add_url_with_clients(runtime, bridge, &mut connector, &mut fetcher, url, options)
}

pub fn add_url_with_clients<B, C, F>(
    runtime: &RuntimeContext,
    bridge: &mut B,
    connector: &mut C,
    fetcher: &mut F,
    url: &str,
    options: AddImportOptions,
) -> Value
where
    B: AddImportBridge,
    C: ConnectorImportClient,
    F: AddImportFetcher,
{
    let text = url.trim();
    if text.is_empty() {
        return result_payload(
            "add_url",
            false,
            "error",
            Some("INVALID_URL"),
            Some("empty url"),
            vec![],
        );
    }
    if (text.to_lowercase().contains("arxiv.org") || BARE_ARXIV_RE.is_match(text))
        && normalize_arxiv_id(text).is_ok()
    {
        let mut opts = options.clone();
        let mut payload =
            add_arxiv_with_clients(runtime, bridge, connector, fetcher, text, &mut opts);
        object_insert(&mut payload, "action", Value::from("add_url"));
        object_insert(&mut payload, "url", Value::from(text));
        object_insert(&mut payload, "url_kind", Value::from("arxiv"));
        return payload;
    }
    if text.to_lowercase().contains("doi.org") || DOI_URL_RE.is_match(text) {
        let doi = DOI_URL_RE
            .find(text)
            .map(|mat| normalize_doi(Some(mat.as_str())))
            .unwrap_or_else(|| normalize_doi(Some(text)));
        let mut payload =
            import_wrapped_add_doi(runtime, bridge, connector, fetcher, &doi, options.clone());
        object_insert(&mut payload, "action", Value::from("add_url"));
        object_insert(&mut payload, "url", Value::from(text));
        object_insert(&mut payload, "url_kind", Value::from("doi"));
        return payload;
    }
    add_webpage_url(runtime, bridge, connector, fetcher, text, &options)
}

pub fn add_bibtex(
    runtime: &RuntimeContext,
    path: &Path,
    collection_key: Option<String>,
    tags: Vec<String>,
    session: SessionState,
) -> Value {
    let options = import_options(collection_key, tags, session);
    match import_core::import_file(runtime, path, options) {
        Ok(imported) => {
            let status = imported
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("success")
                .to_string();
            let ok = matches!(status.as_str(), "success" | "partial_success");
            result_payload(
                "add_bibtex",
                ok && status != "error",
                &status,
                Some(if ok { "IMPORTED" } else { "IMPORT_FAILED" }),
                (!ok).then_some("bibtex import failed"),
                vec![
                    (
                        "imported_count",
                        imported
                            .get("imported_count")
                            .cloned()
                            .unwrap_or(Value::Null),
                    ),
                    (
                        "items",
                        imported.get("items").cloned().unwrap_or(Value::Null),
                    ),
                    ("import_result", imported),
                ],
            )
        }
        Err(err) => result_payload(
            "add_bibtex",
            false,
            "error",
            Some("IMPORT_FAILED"),
            Some(&err.to_string()),
            vec![],
        ),
    }
}

pub fn add_file(
    runtime: &RuntimeContext,
    bridge: &mut JSBridgeClient,
    path: &Path,
    options: AddImportOptions,
) -> Value {
    add_file_with_bridge(runtime, bridge, path, options)
}

pub fn add_file_with_bridge<B: AddImportBridge>(
    runtime: &RuntimeContext,
    bridge: &mut B,
    path: &Path,
    options: AddImportOptions,
) -> Value {
    let source = expand_user_path(&path.to_string_lossy());
    if !source.is_file() {
        return result_payload(
            "add_file",
            false,
            "error",
            Some("FILE_NOT_FOUND"),
            Some(&format!("File not found: {}", source.display())),
            vec![],
        );
    }
    add_file_existing(runtime, bridge, &source, options)
}

pub fn import_doi(
    runtime: &RuntimeContext,
    bridge: &mut JSBridgeClient,
    doi: &str,
    options: AddImportOptions,
) -> Value {
    let mut connector = HttpConnectorImportClient;
    let mut fetcher = UreqAddImportFetcher;
    import_doi_with_clients(runtime, bridge, &mut connector, &mut fetcher, doi, options)
}

pub fn import_doi_with_clients<B, C, F>(
    runtime: &RuntimeContext,
    bridge: &mut B,
    connector: &mut C,
    fetcher: &mut F,
    doi: &str,
    mut options: AddImportOptions,
) -> Value
where
    B: AddImportBridge,
    C: ConnectorImportClient,
    F: AddImportFetcher,
{
    let if_exists = match normalize_if_exists(&options.if_exists) {
        Ok(value) => value,
        Err(err) => {
            return result_payload(
                "import_doi",
                false,
                "error",
                Some("INVALID_IF_EXISTS"),
                Some(&err),
                vec![("DOI", Value::from(doi))],
            )
        }
    };
    options.dedupe = if_exists != "duplicate";
    let normalized = normalize_doi(Some(doi));
    if normalized.is_empty() || !DOI_RE.is_match(&normalized) {
        return result_payload(
            "import_doi",
            false,
            "error",
            Some("INVALID_DOI"),
            Some(&format!("Invalid DOI: {doi:?}")),
            vec![("DOI", Value::from(doi))],
        );
    }

    let library_id = options.library_id.max(0) as u32;
    let tags = normalize_tags(&options.tags);
    let mut attempts = Vec::new();

    if options.dedupe {
        let existing_transport = bridge.find_items_by_doi(library_id, &normalized, 20);
        let existing_data = if existing_transport.ok {
            existing_transport.data.clone()
        } else {
            None
        };
        if let Some(Value::Array(items)) =
            existing_data.filter(|v| !v.as_array().unwrap_or(&Vec::new()).is_empty())
        {
            let item = items[0].clone();
            let key = item.get("key").and_then(Value::as_str);
            let mut modified = false;
            if if_exists == "file" {
                if let (Some(key), Some(collection)) = (key, options.collection_key.as_deref()) {
                    if bridge
                        .add_to_collection(library_id, key, collection)
                        .is_ok()
                    {
                        modified = true;
                    }
                }
                if let Some(key) = key.filter(|_| !tags.is_empty()) {
                    if bridge.manage_tags(library_id, key, &tags).is_ok() {
                        modified = true;
                    }
                }
            }
            return result_payload(
                "import_doi",
                true,
                "already_exists",
                Some("ALREADY_EXISTS"),
                None,
                vec![
                    ("DOI", Value::from(normalized)),
                    ("key", item.get("key").cloned().unwrap_or(Value::Null)),
                    ("title", item.get("title").cloned().unwrap_or(Value::Null)),
                    ("source", Value::from("library-dedupe")),
                    ("if_exists", Value::from(if_exists)),
                    ("modified", Value::from(modified)),
                    ("existing_count", Value::from(items.len())),
                    ("attempts", Value::Array(attempts)),
                ],
            );
        }
    }

    let mut translator_error = None;
    if options.prefer_translator {
        let transport = bridge.import_from_doi(
            library_id,
            &normalized,
            options.collection_key.as_deref(),
            (!tags.is_empty()).then_some(tags.as_slice()),
        );
        let app = application_import_payload(&transport);
        attempts.push(attempt("zotero-translator", &app));
        if app.get("ok") == Some(&Value::Bool(true))
            && (app.get("key").is_some() || app.get("error").is_none())
        {
            return result_payload(
                "import_doi",
                true,
                "success",
                Some(
                    app.get("code")
                        .and_then(Value::as_str)
                        .unwrap_or("IMPORTED"),
                ),
                None,
                vec![
                    ("DOI", Value::from(normalized)),
                    ("key", app.get("key").cloned().unwrap_or(Value::Null)),
                    ("title", app.get("title").cloned().unwrap_or(Value::Null)),
                    (
                        "source",
                        app.get("source")
                            .cloned()
                            .unwrap_or_else(|| Value::from("zotero-translator")),
                    ),
                    (
                        "message",
                        app.get("message").cloned().unwrap_or(Value::Null),
                    ),
                    ("if_exists", Value::from(if_exists)),
                    ("attempts", Value::Array(attempts)),
                ],
            );
        }
        translator_error = app
            .get("error")
            .or_else(|| app.get("code"))
            .map(value_to_python_string)
            .or_else(|| Some("translator failed".to_string()));
    }

    match import_crossref_bibtex(
        runtime,
        connector,
        fetcher,
        &normalized,
        &tags,
        &options,
        &if_exists,
        translator_error.clone(),
        &mut attempts,
    ) {
        Ok(payload) => payload,
        Err(err) => {
            attempts.push(json_object(vec![
                ("step", Value::from("crossref-bibtex")),
                ("ok", Value::Bool(false)),
                ("error", Value::from(err.to_string())),
            ]));
            result_payload(
                "import_doi",
                false,
                "error",
                Some("IMPORT_FAILED"),
                Some(&err.to_string()),
                vec![
                    ("DOI", Value::from(normalized)),
                    (
                        "translator_error",
                        translator_error.map(Value::from).unwrap_or(Value::Null),
                    ),
                    ("if_exists", Value::from(if_exists)),
                    ("attempts", Value::Array(attempts)),
                ],
            )
        }
    }
}

pub fn import_pmid<B: AddImportBridge>(
    bridge: &mut B,
    pmid: &str,
    collection_key: Option<&str>,
    tags: &[String],
    library_id: i64,
) -> Value {
    let transport = bridge.import_from_pmid(
        library_id.max(0) as u32,
        pmid,
        collection_key,
        (!tags.is_empty()).then_some(tags),
    );
    classify_bridge_payload_with_options(&transport, true).0
}

fn add_file_existing<B: AddImportBridge>(
    runtime: &RuntimeContext,
    bridge: &mut B,
    source: &Path,
    options: AddImportOptions,
) -> Value {
    let suffix = source
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase();
    if matches!(
        suffix.as_str(),
        "bib" | "bibtex" | "ris" | "enw" | "csv" | "json"
    ) {
        if suffix == "json" {
            return match import_core::import_json(
                runtime,
                source,
                import_options(
                    options.collection_key.clone(),
                    options.tags.clone(),
                    options.session.clone(),
                ),
            ) {
                Ok(imported) => result_payload(
                    "add_file",
                    imported.get("status").and_then(Value::as_str) != Some("error"),
                    imported
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("success"),
                    Some("IMPORTED"),
                    None,
                    vec![
                        ("path", Value::from(source.to_string_lossy().into_owned())),
                        ("kind", Value::from("json")),
                        ("import_result", imported.clone()),
                        (
                            "imported_count",
                            imported
                                .get("submitted_count")
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                    ],
                ),
                Err(err) => result_payload(
                    "add_file",
                    false,
                    "error",
                    Some("IMPORT_FAILED"),
                    Some(&err.to_string()),
                    vec![],
                ),
            };
        }
        return match import_core::import_file(
            runtime,
            source,
            import_options(
                options.collection_key.clone(),
                options.tags.clone(),
                options.session.clone(),
            ),
        ) {
            Ok(imported) => {
                let status = imported
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("success");
                result_payload(
                    "add_file",
                    matches!(status, "success" | "partial_success"),
                    status,
                    Some("IMPORTED"),
                    None,
                    vec![
                        ("path", Value::from(source.to_string_lossy().into_owned())),
                        ("kind", Value::from(suffix)),
                        ("import_result", imported.clone()),
                        (
                            "imported_count",
                            imported
                                .get("imported_count")
                                .cloned()
                                .unwrap_or(Value::Null),
                        ),
                    ],
                )
            }
            Err(err) => result_payload(
                "add_file",
                false,
                "error",
                Some("IMPORT_FAILED"),
                Some(&err.to_string()),
                vec![],
            ),
        };
    }
    if suffix == "pdf" {
        return add_pdf_file(runtime, bridge, source, options);
    }
    result_payload(
        "add_file",
        false,
        "error",
        Some("UNSUPPORTED_FILE"),
        Some(&format!("Unsupported file type: .{suffix}")),
        vec![("path", Value::from(source.to_string_lossy().into_owned()))],
    )
}

fn add_pdf_file<B: AddImportBridge>(
    runtime: &RuntimeContext,
    bridge: &mut B,
    source: &Path,
    options: AddImportOptions,
) -> Value {
    let stem = source
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .replace('_', "/");
    let doi = DOI_URL_RE
        .find(&stem)
        .map(|mat| normalize_doi(Some(mat.as_str())));
    if let Some(doi) = doi.filter(|doi| DOI_RE.is_match(doi)) {
        let mut connector = HttpConnectorImportClient;
        let mut fetcher = UreqAddImportFetcher;
        let imported = import_doi_with_clients(
            runtime,
            bridge,
            &mut connector,
            &mut fetcher,
            &doi,
            options.clone(),
        );
        if imported.get("ok") == Some(&Value::Bool(true)) {
            if let Some(key) = imported.get("key").and_then(Value::as_str) {
                let attach = bridge.attach_pdf(options.library_id.max(0) as u32, key, source);
                let attach_data = attach.data.clone().unwrap_or(Value::Null);
                return result_payload(
                    "add_file",
                    true,
                    if attach.ok {
                        "success"
                    } else {
                        "partial_success"
                    },
                    Some(if attach.ok {
                        "IMPORTED_WITH_PDF"
                    } else {
                        "IMPORTED_ATTACH_FAILED"
                    }),
                    attach.error.as_deref(),
                    vec![
                        ("path", Value::from(source.to_string_lossy().into_owned())),
                        ("kind", Value::from("pdf")),
                        ("DOI", Value::from(doi)),
                        ("key", Value::from(key)),
                        (
                            "title",
                            imported.get("title").cloned().unwrap_or(Value::Null),
                        ),
                        ("import_result", imported),
                        ("attach_result", attach_data),
                    ],
                );
            }
        }
    }
    let resolved = source
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(source));
    let title = source
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_string();
    let tags = normalize_tags(&options.tags);
    let transport = bridge.standalone_pdf_import(
        options.library_id.max(0) as u32,
        &resolved.to_string_lossy(),
        &title,
        options.collection_key.as_deref(),
        &tags,
    );
    let data = transport.data.clone().unwrap_or(Value::Null);
    if transport.ok && data.get("ok") == Some(&Value::Bool(true)) {
        return result_payload(
            "add_file",
            true,
            "success",
            Some("ATTACHED_STANDALONE"),
            None,
            vec![
                ("path", Value::from(source.to_string_lossy().into_owned())),
                ("kind", Value::from("pdf")),
                ("key", data.get("key").cloned().unwrap_or(Value::Null)),
                (
                    "title",
                    data.get("title")
                        .cloned()
                        .unwrap_or_else(|| Value::from(title)),
                ),
                ("source", Value::from("attachment-import")),
            ],
        );
    }
    result_payload(
        "add_file",
        false,
        "error",
        Some("PDF_IMPORT_FAILED"),
        transport
            .error
            .as_deref()
            .or_else(|| data.get("error").and_then(Value::as_str)),
        vec![("path", Value::from(source.to_string_lossy().into_owned()))],
    )
}

fn import_wrapped_add_doi<B, C, F>(
    runtime: &RuntimeContext,
    bridge: &mut B,
    connector: &mut C,
    fetcher: &mut F,
    doi: &str,
    options: AddImportOptions,
) -> Value
where
    B: AddImportBridge,
    C: ConnectorImportClient,
    F: AddImportFetcher,
{
    let imported = import_doi_with_clients(runtime, bridge, connector, fetcher, doi, options);
    let status = imported
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            if imported.get("ok") == Some(&Value::Bool(true)) {
                "success"
            } else {
                "error"
            }
        })
        .to_string();
    let code = imported
        .get("code")
        .and_then(Value::as_str)
        .map(str::to_string);
    let error = imported
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string);
    result_payload(
        "add_doi",
        imported.get("ok") == Some(&Value::Bool(true)),
        &status,
        code.as_deref(),
        error.as_deref(),
        vec![
            (
                "DOI",
                imported
                    .get("DOI")
                    .cloned()
                    .unwrap_or_else(|| Value::from(normalize_doi(Some(doi)))),
            ),
            ("key", imported.get("key").cloned().unwrap_or(Value::Null)),
            (
                "title",
                imported.get("title").cloned().unwrap_or(Value::Null),
            ),
            (
                "source",
                imported.get("source").cloned().unwrap_or(Value::Null),
            ),
            ("import_result", imported),
        ],
    )
}

#[allow(clippy::too_many_arguments)]
fn import_crossref_bibtex<C, F>(
    runtime: &RuntimeContext,
    connector: &mut C,
    fetcher: &mut F,
    doi: &str,
    tags: &[String],
    options: &AddImportOptions,
    if_exists: &str,
    translator_error: Option<String>,
    attempts: &mut Vec<Value>,
) -> anyhow::Result<Value>
where
    C: ConnectorImportClient,
    F: AddImportFetcher,
{
    require_connector(runtime)?;
    let bibtex = fetcher.fetch_crossref_bibtex(doi, Duration::from_secs(30))?;
    let session_id = session_id("import-doi-crossref");
    let imported = connector.import_text(
        runtime.environment.port,
        bibtex.as_bytes(),
        &session_id,
        "text/x-bibtex",
        options.connector_timeout,
    )?;
    let target = import_core::resolve_target(
        runtime,
        options.collection_key.as_deref(),
        &options.session,
        connector,
    )?;
    connector.update_session(
        runtime.environment.port,
        &session_id,
        target["treeViewID"].as_str().unwrap_or(""),
        tags,
        options.connector_timeout,
    )?;
    let item0 = imported
        .first()
        .cloned()
        .unwrap_or(Value::Object(Map::new()));
    let key = item0.get("key").cloned().unwrap_or(Value::Null);
    attempts.push(json_object(vec![
        ("step", Value::from("crossref-bibtex")),
        ("ok", Value::Bool(true)),
        ("key", key.clone()),
    ]));
    Ok(result_payload(
        "import_doi",
        true,
        "success",
        Some("IMPORTED"),
        None,
        vec![
            ("DOI", Value::from(doi)),
            ("key", key),
            ("title", item0.get("title").cloned().unwrap_or(Value::Null)),
            ("source", Value::from("crossref-bibtex")),
            ("items", Value::Array(imported)),
            ("target", target),
            (
                "tags",
                Value::Array(tags.iter().cloned().map(Value::from).collect()),
            ),
            (
                "translator_error",
                translator_error.map(Value::from).unwrap_or(Value::Null),
            ),
            ("if_exists", Value::from(if_exists)),
            ("attempts", Value::Array(attempts.clone())),
        ],
    ))
}

fn import_arxiv_bibtex<C, F>(
    runtime: &RuntimeContext,
    connector: &mut C,
    fetcher: &mut F,
    arxiv_id: &str,
    doi: &str,
    options: &AddImportOptions,
) -> anyhow::Result<Value>
where
    C: ConnectorImportClient,
    F: AddImportFetcher,
{
    require_connector(runtime)?;
    let bibtex = fetcher.fetch_arxiv_bibtex(arxiv_id, Duration::from_secs(30))?;
    let session_id = session_id("add-arxiv");
    let imported = connector.import_text(
        runtime.environment.port,
        bibtex.as_bytes(),
        &session_id,
        "text/x-bibtex",
        options.connector_timeout,
    )?;
    let target = import_core::resolve_target(
        runtime,
        options.collection_key.as_deref(),
        &options.session,
        connector,
    )?;
    let tags = normalize_tags(&options.tags);
    connector.update_session(
        runtime.environment.port,
        &session_id,
        target["treeViewID"].as_str().unwrap_or(""),
        &tags,
        options.connector_timeout,
    )?;
    let item0 = imported
        .first()
        .cloned()
        .unwrap_or(Value::Object(Map::new()));
    Ok(result_payload(
        "import_doi",
        true,
        "success",
        Some("IMPORTED"),
        None,
        vec![
            ("key", item0.get("key").cloned().unwrap_or(Value::Null)),
            ("title", item0.get("title").cloned().unwrap_or(Value::Null)),
            ("DOI", Value::from(doi)),
            ("source", Value::from("arxiv-bibtex")),
            ("items", Value::Array(imported)),
        ],
    ))
}

fn add_webpage_url<B, C, F>(
    runtime: &RuntimeContext,
    bridge: &mut B,
    connector: &mut C,
    fetcher: &mut F,
    text: &str,
    options: &AddImportOptions,
) -> Value
where
    B: AddImportBridge,
    C: ConnectorImportClient,
    F: AddImportFetcher,
{
    if let Err(err) = require_connector(runtime) {
        return result_payload(
            "add_url",
            false,
            "error",
            Some("IMPORT_FAILED"),
            Some(&err.to_string()),
            vec![("url", Value::from(text))],
        );
    }
    let mut title = text.to_string();
    let mut final_url = text.to_string();
    if let Ok((found_title, found_doi, found_final)) =
        fetcher.fetch_html_title_and_doi(text, Duration::from_secs(20))
    {
        final_url = found_final;
        if let Some(found_title) = found_title {
            title = found_title;
        }
        if let Some(doi) = found_doi {
            return add_url_with_clients(
                runtime,
                bridge,
                connector,
                fetcher,
                &format!("https://doi.org/{doi}"),
                options.clone(),
            );
        }
    }
    let item = json_object(vec![
        ("itemType", Value::from("webpage")),
        ("title", Value::from(title.clone())),
        ("url", Value::from(text)),
        ("id", Value::from("cli-anything-url-1")),
        ("accessDate", Value::from("")),
    ]);
    let session_id = session_id("add-url");
    if let Err(err) = connector.save_items(
        runtime.environment.port,
        &[item],
        &session_id,
        options.connector_timeout,
    ) {
        return result_payload(
            "add_url",
            false,
            "error",
            Some("IMPORT_FAILED"),
            Some(&err.to_string()),
            vec![("url", Value::from(text))],
        );
    }
    let target = match import_core::resolve_target(
        runtime,
        options.collection_key.as_deref(),
        &options.session,
        connector,
    ) {
        Ok(target) => target,
        Err(err) => {
            return result_payload(
                "add_url",
                false,
                "error",
                Some("IMPORT_FAILED"),
                Some(&err.to_string()),
                vec![("url", Value::from(text))],
            )
        }
    };
    let tags = normalize_tags(&options.tags);
    if let Err(err) = connector.update_session(
        runtime.environment.port,
        &session_id,
        target["treeViewID"].as_str().unwrap_or(""),
        &tags,
        options.connector_timeout,
    ) {
        return result_payload(
            "add_url",
            false,
            "error",
            Some("IMPORT_FAILED"),
            Some(&err.to_string()),
            vec![("url", Value::from(text))],
        );
    }
    result_payload(
        "add_url",
        true,
        "success",
        Some("IMPORTED"),
        None,
        vec![
            ("url", Value::from(text)),
            ("url_kind", Value::from("webpage")),
            ("title", Value::from(title)),
            ("source", Value::from("connector-webpage")),
            ("final_url", Value::from(final_url)),
            ("target", target),
        ],
    )
}

fn maybe_fetch_pdf(
    runtime: &RuntimeContext,
    bridge: &mut JSBridgeClient,
    payload: &mut Value,
    options: &AddImportOptions,
    default_sources: &str,
) {
    if !options.fetch_pdf || payload.get("ok") != Some(&Value::Bool(true)) {
        return;
    }
    let Some(key) = payload.get("key").and_then(Value::as_str) else {
        return;
    };
    let sources =
        match pdf_fetch::parse_sources(options.pdf_sources.as_deref().or(Some(default_sources))) {
            Ok(sources) => sources,
            Err(err) => {
                object_insert(payload, "status", Value::from("partial_success"));
                object_insert(payload, "code", Value::from("IMPORTED_PDF_MISSING"));
                object_insert(
                    payload,
                    "pdf",
                    result_payload(
                        "item_fetch_pdf",
                        false,
                        "error",
                        Some("PDF_SOURCES_INVALID"),
                        Some(&err.to_string()),
                        vec![],
                    ),
                );
                return;
            }
        };
    let client = UreqPdfClient;
    let pdf = pdf_fetch::fetch_pdf_for_item(
        runtime,
        bridge,
        &client,
        &client,
        key,
        &sources,
        options.library_id,
        ADD_IMPORT_PDF_FETCH_TIMEOUT_SECONDS,
        ADD_IMPORT_PDF_FETCH_TIMEOUT_SECONDS,
        false,
    );
    object_insert(payload, "pdf", pdf.clone());
    if pdf.get("ok") != Some(&Value::Bool(true))
        && pdf.get("status").and_then(Value::as_str) != Some("already_has_pdf")
    {
        object_insert(payload, "status", Value::from("partial_success"));
        object_insert(payload, "code", Value::from("IMPORTED_PDF_MISSING"));
    }
}

fn application_import_payload(transport: &BridgeResponse) -> Value {
    if !transport.ok {
        return result_payload(
            "import_bridge",
            false,
            "error",
            Some("BRIDGE_ERROR"),
            Some(
                transport
                    .error
                    .as_deref()
                    .unwrap_or("JS bridge transport failed"),
            ),
            vec![
                (
                    "error_name",
                    transport
                        .error_name
                        .clone()
                        .map(Value::from)
                        .unwrap_or(Value::Null),
                ),
                (
                    "error_stack",
                    transport
                        .error_stack
                        .clone()
                        .map(Value::from)
                        .unwrap_or(Value::Null),
                ),
            ],
        );
    }
    match transport.data.clone() {
        None | Some(Value::Null) => result_payload(
            "import_bridge",
            false,
            "error",
            Some("EMPTY_RESULT"),
            Some("JS bridge returned empty success (data is null) - import did not complete"),
            vec![],
        ),
        Some(Value::Object(map)) if map.contains_key("ok") => Value::Object(map),
        Some(Value::Object(map)) => result_payload(
            "import_bridge",
            true,
            "success",
            None,
            None,
            vec![("result", Value::Object(map))],
        ),
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            if trimmed.starts_with("OK:") || trimmed.starts_with("FOUND:") {
                result_payload(
                    "import_bridge",
                    true,
                    "success",
                    Some("IMPORTED"),
                    None,
                    vec![
                        ("message", Value::from(trimmed)),
                        (
                            "key",
                            legacy_key(trimmed).map(Value::from).unwrap_or(Value::Null),
                        ),
                    ],
                )
            } else if trimmed.starts_with("ERROR:")
                || trimmed.starts_with("NOT_FOUND")
                || trimmed.starts_with("TIMEOUT")
            {
                result_payload(
                    "import_bridge",
                    false,
                    "error",
                    Some("LEGACY_ERROR"),
                    Some(trimmed),
                    vec![],
                )
            } else {
                result_payload(
                    "import_bridge",
                    false,
                    "error",
                    Some("UNEXPECTED_RESULT"),
                    Some(trimmed),
                    vec![],
                )
            }
        }
        Some(other) => result_payload(
            "import_bridge",
            false,
            "error",
            Some("UNEXPECTED_RESULT"),
            Some(&format!(
                "Unexpected bridge data type: {}",
                json_type_name(&other)
            )),
            vec![],
        ),
    }
}

fn result_payload(
    action: &str,
    ok: bool,
    status: &str,
    code: Option<&str>,
    error: Option<&str>,
    extra: Vec<(&str, Value)>,
) -> Value {
    let mut out = Map::new();
    out.insert("action".to_string(), Value::from(action));
    out.insert("ok".to_string(), Value::Bool(ok));
    out.insert("status".to_string(), Value::from(status));
    if let Some(code) = code {
        out.insert("code".to_string(), Value::from(code));
    }
    if let Some(error) = error {
        out.insert("error".to_string(), Value::from(error));
    }
    for (key, value) in extra {
        out.insert(key.to_string(), value);
    }
    Value::Object(out)
}

fn import_options(
    collection_key: Option<String>,
    tags: Vec<String>,
    session: SessionState,
) -> ImportOptions {
    ImportOptions {
        collection_ref: collection_key,
        tags,
        session,
        connector_timeout: Duration::from_secs(120),
        split_bib: true,
        ..Default::default()
    }
}

pub fn normalize_arxiv_id(value: &str) -> anyhow::Result<String> {
    let text = value.trim();
    let text = Regex::new(r"(?i)^https?://(www\.)?arxiv\.org/(abs|pdf)/")
        .unwrap()
        .replace(text, "");
    let text = text.replace(".pdf", "");
    if let Some(captures) = ARXIV_ID_RE.captures(&text) {
        return Ok(captures.get(1).unwrap().as_str().to_string());
    }
    if let Some(found) = BARE_ARXIV_RE.find(&text) {
        return Ok(found.as_str().to_string());
    }
    anyhow::bail!("Invalid arXiv id: {value:?}");
}

fn normalize_if_exists(value: &str) -> Result<String, String> {
    let text = if value.trim().is_empty() {
        "file"
    } else {
        value
    }
    .trim()
    .to_lowercase();
    if matches!(text.as_str(), "file" | "skip" | "duplicate") {
        Ok(text)
    } else {
        Err(format!(
            "Unsupported if-exists policy: {value:?} (use file|skip|duplicate)"
        ))
    }
}

fn normalize_tags(tags: &[String]) -> Vec<String> {
    tags.iter()
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty())
        .map(ToString::to_string)
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

fn fetch_text(
    url: &str,
    timeout: Duration,
    accept: &str,
    user_agent: Option<&str>,
) -> anyhow::Result<String> {
    let mut request = ureq::get(url).header("Accept", accept);
    if let Some(user_agent) = user_agent {
        request = request.header("User-Agent", user_agent);
    }
    let mut response = request
        .config()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|err| anyhow::anyhow!("{err}"))?;
    let status = response.status().as_u16();
    let bytes = response.body_mut().read_to_vec()?;
    let body = String::from_utf8_lossy(&bytes).trim().to_string();
    if status != 200 {
        let detail = body.chars().take(300).collect::<String>();
        anyhow::bail!("HTTP {status} {detail}");
    }
    Ok(body)
}

fn require_bibtex(body: String, error: &str) -> anyhow::Result<String> {
    let body = body.trim().to_string();
    if body.is_empty() || !body.contains('@') {
        anyhow::bail!("{error}");
    }
    Ok(body)
}

fn percent_encode_quote_default(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn session_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let count = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{nanos:x}{count:x}")
}

fn attempt(step: &str, app: &Value) -> Value {
    json_object(vec![
        ("step", Value::from(step)),
        ("ok", app.get("ok").cloned().unwrap_or(Value::Null)),
        ("code", app.get("code").cloned().unwrap_or(Value::Null)),
        ("error", app.get("error").cloned().unwrap_or(Value::Null)),
        ("key", app.get("key").cloned().unwrap_or(Value::Null)),
    ])
}

fn object_insert(object: &mut Value, key: &str, value: Value) {
    if let Value::Object(map) = object {
        map.insert(key.to_string(), value);
    }
}

fn json_object(entries: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}

fn legacy_key(text: &str) -> Option<String> {
    let marker = "(key:";
    let start = text.find(marker)? + marker.len();
    let tail = &text[start..];
    let end = tail.find(')')?;
    let key = tail[..end].trim();
    (!key.is_empty()).then(|| key.to_string())
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}
