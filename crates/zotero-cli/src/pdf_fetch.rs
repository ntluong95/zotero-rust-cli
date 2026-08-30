//! Open-access PDF cascade for one item (`core/pdf_fetch.py`, Phase 7 Slice 3). Backend-only:
//! no CLI dispatch here (that is a later slice's job) -- every function takes its parameters
//! explicitly (library_id, sources, timeouts) exactly as `pdf_fetch.py`'s own functions do,
//! rather than resolving session/collection state itself.
//!
//! Order (default, matches Python exactly): Zotero's own "Find Available PDF" (JS Bridge) ->
//! Unpaywall -> EuropePMC/PMC -> bioRxiv/medRxiv (one source, "biorxiv") -> arXiv.
//!
//! Transport classification (§ Phase 7 Slice 3 spec):
//! - existing-PDF pre-check: read-only SQLite (`db::resolve_item`, already-ported `has_pdf` field)
//! - Zotero's own PDF finder: JS Bridge (`bridge::JSBridgeClient::find_pdf`)
//! - OA source discovery (Unpaywall/EuropePMC) and PDF download: external HTTP
//! - attach the downloaded PDF: JS Bridge (`bridge::JSBridgeClient::item_attach`, already merged
//!   in Phase 6 -- byte-identical to Python's `attach_pdf`, reused here rather than duplicated)
//! - no Connector API, no Local API, no direct SQLite writes anywhere in this module

use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{Map, Value};

use crate::bridge::JSBridgeClient;
use crate::db;
use crate::import_normalization::normalize_doi;
use crate::runtime::RuntimeContext;

pub const DEFAULT_SOURCES: [&str; 5] = ["zotero", "unpaywall", "epmc", "biorxiv", "arxiv"];
const USER_AGENT: &str = "cli-anything-zotero/1.2 (mailto:cli-anything@local; research agent)";
/// `_is_pdf`'s exact size floor: rejects tiny error-page bodies that happen to start with the
/// PDF magic bytes.
const MIN_PDF_BYTES: usize = 8000;

static ARXIV_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:arxiv(?:\.org)?/(?:abs|pdf)/|arxiv:)?(\d{4}\.\d{4,5})(v\d+)?").unwrap()
});
static BARE_ARXIV_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{4}\.\d{4,5})").unwrap());

/// `parse_sources()`: comma-separated, case-insensitive; `"all"` expands to the full default
/// list; unknown names are a hard error (matches Python's `ValueError`).
pub fn parse_sources(value: Option<&str>) -> anyhow::Result<Vec<String>> {
    let default = || DEFAULT_SOURCES.iter().map(|s| s.to_string()).collect();
    let Some(value) = value.filter(|v| !v.trim().is_empty()) else {
        return Ok(default());
    };
    let parts: Vec<String> = value
        .split(',')
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.iter().any(|p| p == "all") {
        return Ok(default());
    }
    let unknown: Vec<&str> = parts
        .iter()
        .map(String::as_str)
        .filter(|p| !DEFAULT_SOURCES.contains(p))
        .collect();
    if !unknown.is_empty() {
        anyhow::bail!(
            "Unknown PDF sources: {unknown:?}. Allowed: {:?}",
            DEFAULT_SOURCES
        );
    }
    if parts.is_empty() {
        Ok(default())
    } else {
        Ok(parts)
    }
}

/// `extract_arxiv_id()`.
pub fn extract_arxiv_id(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    ARXIV_RE
        .captures(trimmed)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// `_is_pdf()`: magic bytes (`%PDF`) **and** a minimum size, so a mislabeled server can still be
/// accepted (wrong `Content-Type`) while a tiny error page that happens to start with `%PDF` is
/// rejected. Deliberately does not check `Content-Type` at all -- matches Python exactly.
fn is_valid_pdf(data: &[u8]) -> bool {
    data.starts_with(b"%PDF") && data.len() >= MIN_PDF_BYTES
}

/// `urllib.parse.quote(value)`'s default `safe='/'` behavior: percent-encode everything except
/// unreserved characters and `/`. No new dependency added for this (small-dependency-footprint
/// principle) -- DOIs and the fixed contact email are simple enough for a direct byte loop.
fn percent_encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
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

/// External HTTP for the two OA-metadata APIs (Unpaywall, EuropePMC). A trait -- not a bare
/// `ureq` call -- so the cascade's branching logic is unit-testable without real network access,
/// matching the `AttachmentConnector`/`RemotePdfFetcher` mock-trait convention Phase 7 Slice 2
/// already established for the same reason.
pub trait PdfMetadataClient {
    fn fetch_json(&self, url: &str, timeout: Duration) -> anyhow::Result<Value>;
}

/// External HTTP for the final PDF byte download.
pub trait PdfDownloadClient {
    fn fetch_bytes(&self, url: &str, timeout: Duration) -> anyhow::Result<Vec<u8>>;
}

pub struct UreqPdfClient;

impl PdfMetadataClient for UreqPdfClient {
    fn fetch_json(&self, url: &str, timeout: Duration) -> anyhow::Result<Value> {
        let mut response = ureq::get(url)
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/json")
            .config()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build()
            .call()
            .map_err(|err| anyhow::anyhow!("HTTP request failed for {url}: {err}"))?;
        let status = response.status().as_u16();
        let bytes = response
            .body_mut()
            .read_to_vec()
            .map_err(|err| anyhow::anyhow!("Failed to read response body for {url}: {err}"))?;
        if status != 200 {
            anyhow::bail!("HTTP {status} for {url}");
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
}

impl PdfDownloadClient for UreqPdfClient {
    fn fetch_bytes(&self, url: &str, timeout: Duration) -> anyhow::Result<Vec<u8>> {
        let mut response = ureq::get(url)
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/pdf,*/*")
            .config()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build()
            .call()
            .map_err(|err| anyhow::anyhow!("HTTP request failed for {url}: {err}"))?;
        let status = response.status().as_u16();
        let bytes = response
            .body_mut()
            .read_to_vec()
            .map_err(|err| anyhow::anyhow!("Failed to read response body for {url}: {err}"))?;
        if status != 200 {
            anyhow::bail!("HTTP {status} for {url}");
        }
        Ok(bytes)
    }
}

fn dedupe_preserve_order(urls: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    urls.into_iter()
        .filter(|u| seen.insert(u.clone()))
        .collect()
}

/// `unpaywall_pdf_urls()`. Any network/parse failure is swallowed to an empty list (matches
/// Python's broad `except Exception: return []`) -- a genuinely-empty result and a transport
/// failure are indistinguishable in the cascade's `attempts` output, exactly as upstream.
pub fn unpaywall_pdf_urls<M: PdfMetadataClient>(client: &M, doi: &str) -> Vec<String> {
    let doi = normalize_doi(Some(doi));
    let url = format!(
        "https://api.unpaywall.org/v2/{}?email={}",
        percent_encode_query(&doi),
        percent_encode_query("cli-anything@local")
    );
    let Ok(payload) = client.fetch_json(&url, Duration::from_secs(25)) else {
        return Vec::new();
    };
    let mut urls = Vec::new();
    if let Some(best) = payload.get("best_oa_location") {
        for key in ["url_for_pdf", "url"] {
            if let Some(u) = best.get(key).and_then(Value::as_str) {
                if !u.is_empty() {
                    urls.push(u.to_string());
                }
            }
        }
    }
    if let Some(locations) = payload.get("oa_locations").and_then(Value::as_array) {
        for loc in locations {
            if let Some(u) = loc.get("url_for_pdf").and_then(Value::as_str) {
                if !u.is_empty() {
                    urls.push(u.to_string());
                }
            }
        }
    }
    dedupe_preserve_order(urls)
}

/// `epmc_pdf_urls()`.
pub fn epmc_pdf_urls<M: PdfMetadataClient>(client: &M, doi: &str) -> Vec<String> {
    let doi = normalize_doi(Some(doi));
    let query = percent_encode_query(&format!("DOI:\"{doi}\""));
    let url = format!(
        "https://www.ebi.ac.uk/europepmc/webservices/rest/search?query={query}&format=json&resultType=core&pageSize=1"
    );
    let Ok(payload) = client.fetch_json(&url, Duration::from_secs(25)) else {
        return Vec::new();
    };
    let Some(result) = payload
        .get("resultList")
        .and_then(|v| v.get("result"))
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
    else {
        return Vec::new();
    };
    let mut urls = Vec::new();
    if let Some(pmcid) = result.get("pmcid").and_then(Value::as_str) {
        urls.push(format!(
            "https://europepmc.org/backend/ptpmcrender.fcgi?accid={pmcid}&blobtype=pdf"
        ));
        urls.push(format!(
            "https://www.ncbi.nlm.nih.gov/pmc/articles/{pmcid}/pdf"
        ));
    }
    if let Some(entries) = result
        .get("fullTextUrlList")
        .and_then(|v| v.get("fullTextUrl"))
        .and_then(Value::as_array)
    {
        for entry in entries {
            let Some(u) = entry.get("url").and_then(Value::as_str) else {
                continue;
            };
            let is_pdf_style = u.to_lowercase().contains("pdf")
                || entry.get("documentStyle").and_then(Value::as_str) == Some("pdf");
            if is_pdf_style {
                urls.push(u.to_string());
            }
        }
    }
    urls
}

/// `preprint_pdf_urls()` -- covers **both** bioRxiv and medRxiv under the single `"biorxiv"`
/// source name (there is no independently-selectable `"medrxiv"` source in
/// `parse_sources`/`DEFAULT_SOURCES`; the phase-07 plan doc's table implying otherwise does not
/// match the actual Python contract). Only triggers for `10.1101/`/`10.64898/`-prefixed DOIs.
pub fn preprint_pdf_urls(doi: &str) -> Vec<String> {
    let doi = normalize_doi(Some(doi));
    let lower = doi.to_lowercase();
    if lower.starts_with("10.1101/") || lower.starts_with("10.64898/") {
        vec![
            format!("https://www.biorxiv.org/content/{doi}v1.full.pdf"),
            format!("https://www.biorxiv.org/content/{doi}v2.full.pdf"),
            format!("https://www.biorxiv.org/content/{doi}.full.pdf"),
            format!("https://www.medrxiv.org/content/{doi}v1.full.pdf"),
        ]
    } else {
        Vec::new()
    }
}

/// `arxiv_pdf_urls()`.
pub fn arxiv_pdf_urls(doi_or_id: &str) -> Vec<String> {
    let text = doi_or_id;
    let mut arxiv_id = extract_arxiv_id(text);
    if arxiv_id.is_none() && text.to_lowercase().contains("arxiv") {
        arxiv_id = BARE_ARXIV_ID_RE
            .captures(text)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string());
    }
    let Some(arxiv_id) = arxiv_id else {
        return Vec::new();
    };
    vec![
        format!("https://arxiv.org/pdf/{arxiv_id}.pdf"),
        format!("https://export.arxiv.org/pdf/{arxiv_id}.pdf"),
    ]
}

/// One entry in `cascade_download_pdf`'s `attempts` list: per-URL entries never carry an
/// `"error"` field (only `"ok"`); per-source "nothing to try" entries do. Matches Python's exact
/// two attempt shapes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CascadeAttempt {
    pub source: String,
    pub url: Option<String>,
    pub ok: bool,
    pub error: Option<String>,
}

impl CascadeAttempt {
    fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("source".to_string(), Value::String(self.source.clone()));
        if let Some(url) = &self.url {
            map.insert("url".to_string(), Value::String(url.clone()));
        }
        map.insert("ok".to_string(), Value::Bool(self.ok));
        if let Some(error) = &self.error {
            map.insert("error".to_string(), Value::String(error.clone()));
        } else if self.url.is_some() {
            map.insert("path".to_string(), Value::Null);
        }
        Value::Object(map)
    }
}

pub fn cascade_attempts_to_json(attempts: &[CascadeAttempt]) -> Value {
    Value::Array(attempts.iter().map(CascadeAttempt::to_json).collect())
}

fn download_from_url<D: PdfDownloadClient>(
    client: &D,
    url: &str,
    timeout: Duration,
) -> Option<Vec<u8>> {
    let data = client.fetch_bytes(url, timeout).ok()?;
    is_valid_pdf(&data).then_some(data)
}

/// `cascade_download_pdf()`: tries every non-`"zotero"` source in `sources`' order, stopping at
/// the first URL that downloads and validates as a PDF. `doi_or_key` mirrors Python's own
/// `fetch_pdf_for_item(doi=doi or item_key, ...)` call site exactly, including its quirk: when
/// an item genuinely has no DOI, the raw item key is passed through as if it were one, so
/// `unpaywall`/`epmc`/`biorxiv` still attempt a real (doomed) network call rather than being
/// cleanly skipped -- preserved here for parity, not "fixed."
pub fn cascade_download_pdf<M: PdfMetadataClient, D: PdfDownloadClient>(
    metadata_client: &M,
    download_client: &D,
    doi_or_key: &str,
    sources: &[String],
    timeout: Duration,
) -> (Option<Vec<u8>>, Vec<CascadeAttempt>) {
    let mut attempts = Vec::new();
    let doi_n = normalize_doi(Some(doi_or_key));

    let try_urls = |source: &str, urls: Vec<String>, attempts: &mut Vec<CascadeAttempt>| {
        for url in &urls {
            let path = download_from_url(download_client, url, timeout);
            let ok = path.is_some();
            attempts.push(CascadeAttempt {
                source: source.to_string(),
                url: Some(url.clone()),
                ok,
                error: None,
            });
            if ok {
                return path;
            }
        }
        if urls.is_empty() {
            attempts.push(CascadeAttempt {
                source: source.to_string(),
                url: None,
                ok: false,
                error: Some("no candidate urls".to_string()),
            });
        }
        None
    };

    for source in sources {
        if source == "zotero" {
            // Handled by the caller (`fetch_pdf_for_item`) via the JS Bridge, before this
            // function is ever reached.
            continue;
        }
        match source.as_str() {
            "unpaywall" => {
                if doi_n.is_empty() {
                    attempts.push(CascadeAttempt {
                        source: source.clone(),
                        url: None,
                        ok: false,
                        error: Some("no DOI".to_string()),
                    });
                    continue;
                }
                let urls = unpaywall_pdf_urls(metadata_client, &doi_n);
                if let Some(path) = try_urls(source, urls, &mut attempts) {
                    return (Some(path), attempts);
                }
            }
            "epmc" => {
                if doi_n.is_empty() {
                    attempts.push(CascadeAttempt {
                        source: source.clone(),
                        url: None,
                        ok: false,
                        error: Some("no DOI".to_string()),
                    });
                    continue;
                }
                let urls = epmc_pdf_urls(metadata_client, &doi_n);
                if let Some(path) = try_urls(source, urls, &mut attempts) {
                    return (Some(path), attempts);
                }
            }
            "biorxiv" => {
                if doi_n.is_empty() {
                    attempts.push(CascadeAttempt {
                        source: source.clone(),
                        url: None,
                        ok: false,
                        error: Some("no DOI".to_string()),
                    });
                    continue;
                }
                let urls = preprint_pdf_urls(&doi_n);
                if let Some(path) = try_urls(source, urls, &mut attempts) {
                    return (Some(path), attempts);
                }
            }
            "arxiv" => {
                let urls = arxiv_pdf_urls(if doi_n.is_empty() { doi_or_key } else { &doi_n });
                if let Some(path) = try_urls(source, urls, &mut attempts) {
                    return (Some(path), attempts);
                }
            }
            _ => {}
        }
    }
    (None, attempts)
}

/// One outcome of `find_pdf_for_item`/`fetch_pdf_for_item`, deliberately shaped to serialize
/// into the exact `result_payload()` JSON Python's CLI layer builds -- so a later CLI-dispatch
/// slice can emit this value directly without re-deriving field names/order.
pub fn result_payload(
    action: &str,
    ok: bool,
    status: &str,
    code: Option<&str>,
    error: Option<&str>,
    extra: Vec<(&str, Value)>,
) -> Value {
    let mut map = Map::new();
    map.insert("action".to_string(), Value::String(action.to_string()));
    map.insert("ok".to_string(), Value::Bool(ok));
    map.insert("status".to_string(), Value::String(status.to_string()));
    if let Some(code) = code {
        map.insert("code".to_string(), Value::String(code.to_string()));
    }
    if let Some(error) = error {
        map.insert("error".to_string(), Value::String(error.to_string()));
    }
    for (key, value) in extra {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}

/// `item_find_pdf_command`'s core (`zotero_cli.py`), minus the CLI/session plumbing: triggers
/// Zotero's own PDF finder for one item and classifies the raw Bridge response. Does not touch
/// SQLite, the OA cascade, or attach anything -- this is discovery-only, exactly like Python
/// (despite `addAvailablePDF` itself being able to trigger a download/attach inside Zotero --
/// that side effect is Zotero's own, not something this function initiates a *second* time).
pub fn find_pdf_for_item(
    bridge: &JSBridgeClient,
    item_key: &str,
    library_id: i64,
    timeout_secs: u64,
) -> Value {
    let library_id_u32 = library_id.max(0) as u32;
    let transport = bridge.find_pdf(library_id_u32, item_key, timeout_secs);
    let text = transport
        .data
        .as_ref()
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();
    if transport.is_ok() && text.starts_with("FOUND:") {
        result_payload(
            "item_find_pdf",
            true,
            "success",
            Some("FOUND"),
            None,
            vec![
                ("key", Value::String(item_key.to_string())),
                (
                    "attachment_key",
                    Value::String(text.trim_start_matches("FOUND:").trim().to_string()),
                ),
                ("result", Value::String(text.clone())),
            ],
        )
    } else if transport.is_ok() && text.starts_with("NOT_FOUND") {
        result_payload(
            "item_find_pdf",
            false,
            "not_found",
            Some("NOT_FOUND"),
            Some(&text),
            vec![("key", Value::String(item_key.to_string()))],
        )
    } else {
        let error = transport
            .error_message()
            .map(str::to_string)
            .unwrap_or(text);
        result_payload(
            "item_find_pdf",
            false,
            "error",
            Some("FIND_PDF_FAILED"),
            Some(&error),
            vec![("key", Value::String(item_key.to_string()))],
        )
    }
}

/// `fetch_pdf_for_item()`: existing-PDF gate (SQLite) -> Zotero's own finder (Bridge) -> OA
/// cascade (external HTTP) -> attach (Bridge, reusing the already-merged `item_attach`). No
/// verification read-back after a successful attach (matches Python -- the transport ack is
/// trusted at face value; see the Slice 3 spec's open question on this).
#[allow(clippy::too_many_arguments)]
pub fn fetch_pdf_for_item<M: PdfMetadataClient, D: PdfDownloadClient>(
    runtime: &RuntimeContext,
    bridge: &JSBridgeClient,
    metadata_client: &M,
    download_client: &D,
    item_key: &str,
    sources: &[String],
    library_id: i64,
    zotero_timeout: u64,
    download_timeout: u64,
    force: bool,
) -> Value {
    let library_id_u32 = library_id.max(0) as u32;
    let item = match db::resolve_item(&runtime.environment.sqlite_path, item_key, Some(library_id))
    {
        Ok(Some(item)) => item,
        Ok(None) => {
            return result_payload(
                "item_fetch_pdf",
                false,
                "error",
                Some("ITEM_NOT_FOUND"),
                Some(&format!("Item not found: {item_key}")),
                vec![("key", Value::String(item_key.to_string()))],
            )
        }
        Err(err) => {
            return result_payload(
                "item_fetch_pdf",
                false,
                "error",
                Some("ITEM_NOT_FOUND"),
                Some(&err.to_string()),
                vec![("key", Value::String(item_key.to_string()))],
            )
        }
    };

    let doi = item.doi.clone();

    if item.has_pdf && !force {
        return result_payload(
            "item_fetch_pdf",
            true,
            "already_has_pdf",
            Some("ALREADY_HAS_PDF"),
            None,
            vec![
                ("key", Value::String(item_key.to_string())),
                ("title", Value::String(item.title.clone())),
                ("DOI", Value::String(doi.clone())),
                ("source", Value::String("existing".to_string())),
            ],
        );
    }

    let mut attempts: Vec<Value> = Vec::new();

    if sources.iter().any(|s| s == "zotero") {
        let transport = bridge.find_pdf(library_id_u32, item_key, zotero_timeout);
        let text = transport
            .data
            .as_ref()
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_default();
        let ok_zotero = transport.is_ok() && text.starts_with("FOUND:");
        attempts.push(serde_json::json!({
            "source": "zotero",
            "ok": ok_zotero,
            "message": if transport.is_ok() { Value::String(text.clone()) } else {
                transport.error_message().map(Value::from).unwrap_or(Value::Null)
            },
        }));
        if ok_zotero {
            return result_payload(
                "item_fetch_pdf",
                true,
                "success",
                Some("FOUND"),
                None,
                vec![
                    ("key", Value::String(item_key.to_string())),
                    ("title", Value::String(item.title.clone())),
                    ("DOI", Value::String(doi.clone())),
                    ("source", Value::String("zotero".to_string())),
                    (
                        "attachment_key",
                        Value::String(text.trim_start_matches("FOUND:").trim().to_string()),
                    ),
                    ("attempts", Value::Array(attempts)),
                ],
            );
        }
    }

    let oa_sources: Vec<String> = sources.iter().filter(|s| *s != "zotero").cloned().collect();
    let doi_or_key = if doi.is_empty() { item_key } else { &doi };
    let (downloaded, dl_attempts) = cascade_download_pdf(
        metadata_client,
        download_client,
        doi_or_key,
        &oa_sources,
        Duration::from_secs(download_timeout),
    );
    attempts.extend(dl_attempts.iter().map(CascadeAttempt::to_json));

    let Some(pdf_bytes) = downloaded else {
        return result_payload(
            "item_fetch_pdf",
            false,
            "not_found",
            Some("PDF_NOT_FOUND"),
            Some("No PDF found via configured sources"),
            vec![
                ("key", Value::String(item_key.to_string())),
                ("title", Value::String(item.title.clone())),
                ("DOI", Value::String(doi.clone())),
                ("attempts", Value::Array(attempts)),
            ],
        );
    };

    let temp_path = match write_temp_pdf(&pdf_bytes) {
        Ok(path) => path,
        Err(err) => {
            return result_payload(
                "item_fetch_pdf",
                false,
                "error",
                Some("ATTACH_FAILED"),
                Some(&err.to_string()),
                vec![
                    ("key", Value::String(item_key.to_string())),
                    ("title", Value::String(item.title.clone())),
                    ("DOI", Value::String(doi.clone())),
                    ("attempts", Value::Array(attempts)),
                ],
            )
        }
    };

    let attach_outcome = bridge.item_attach(library_id_u32, item_key, &temp_path);
    // Best-effort temp-file cleanup regardless of outcome (matches Python's
    // `path.unlink(missing_ok=True)`).
    let _ = std::fs::remove_file(&temp_path);

    match attach_outcome {
        Ok(crate::write::WriteOutcome::Applied { affected_key }) => result_payload(
            "item_fetch_pdf",
            true,
            "success",
            Some("ATTACHED"),
            None,
            vec![
                ("key", Value::String(item_key.to_string())),
                ("title", Value::String(item.title.clone())),
                ("DOI", Value::String(doi.clone())),
                ("source", Value::String("oa-cascade".to_string())),
                ("attach_result", Value::String(affected_key)),
                ("attempts", Value::Array(attempts)),
            ],
        ),
        Ok(other) => result_payload(
            "item_fetch_pdf",
            false,
            "error",
            Some("ATTACH_FAILED"),
            Some(&format!("{other:?}")),
            vec![
                ("key", Value::String(item_key.to_string())),
                ("title", Value::String(item.title.clone())),
                ("DOI", Value::String(doi.clone())),
                ("attempts", Value::Array(attempts)),
            ],
        ),
        Err(err) => result_payload(
            "item_fetch_pdf",
            false,
            "error",
            Some("ATTACH_FAILED"),
            Some(&err.to_string()),
            vec![
                ("key", Value::String(item_key.to_string())),
                ("title", Value::String(item.title.clone())),
                ("DOI", Value::String(doi.clone())),
                ("attempts", Value::Array(attempts)),
            ],
        ),
    }
}

fn write_temp_pdf(data: &[u8]) -> anyhow::Result<PathBuf> {
    let dir = std::env::temp_dir();
    let name = format!(
        "zotero-pdf-{}-{}.pdf",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let path = dir.join(name);
    std::fs::write(&path, data)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sources_defaults_when_empty() {
        assert_eq!(
            parse_sources(None).unwrap(),
            vec!["zotero", "unpaywall", "epmc", "biorxiv", "arxiv"]
        );
        assert_eq!(parse_sources(Some("")).unwrap().len(), 5);
    }

    #[test]
    fn parse_sources_all_expands_to_default() {
        assert_eq!(parse_sources(Some("all")).unwrap().len(), 5);
    }

    #[test]
    fn parse_sources_rejects_unknown_names() {
        assert!(parse_sources(Some("zotero,medrxiv")).is_err());
    }

    #[test]
    fn parse_sources_lowercases_and_trims() {
        assert_eq!(
            parse_sources(Some(" Zotero , ARXIV ")).unwrap(),
            vec!["zotero", "arxiv"]
        );
    }

    #[test]
    fn extract_arxiv_id_matches_common_shapes() {
        assert_eq!(
            extract_arxiv_id("https://arxiv.org/abs/2401.12345"),
            Some("2401.12345".to_string())
        );
        assert_eq!(
            extract_arxiv_id("arXiv:2401.12345v2"),
            Some("2401.12345".to_string())
        );
        assert_eq!(extract_arxiv_id(""), None);
        assert_eq!(extract_arxiv_id("10.1000/xyz"), None);
    }

    #[test]
    fn is_valid_pdf_requires_magic_bytes_and_min_size() {
        let mut small = b"%PDF-1.4".to_vec();
        small.resize(100, 0);
        assert!(!is_valid_pdf(&small), "too small must be rejected");

        let mut large_wrong_magic = vec![0u8; 9000];
        large_wrong_magic[..4].copy_from_slice(b"XPDF");
        assert!(!is_valid_pdf(&large_wrong_magic));

        let mut large_pdf = b"%PDF-1.4".to_vec();
        large_pdf.resize(9000, 0);
        assert!(is_valid_pdf(&large_pdf));
    }

    #[test]
    fn preprint_pdf_urls_only_triggers_for_biorxiv_medrxiv_dois() {
        assert!(preprint_pdf_urls("10.1101/2024.01.01.000001").len() == 4);
        assert!(preprint_pdf_urls("10.1000/unrelated").is_empty());
    }

    #[test]
    fn arxiv_pdf_urls_extracts_id_from_doi_style_string() {
        let urls = arxiv_pdf_urls("10.48550/arXiv.2401.12345");
        assert_eq!(
            urls,
            vec![
                "https://arxiv.org/pdf/2401.12345.pdf",
                "https://export.arxiv.org/pdf/2401.12345.pdf",
            ]
        );
    }

    #[test]
    fn arxiv_pdf_urls_empty_when_no_id_found() {
        assert!(arxiv_pdf_urls("10.1000/xyz").is_empty());
    }

    struct FailingMetadataClient;
    impl PdfMetadataClient for FailingMetadataClient {
        fn fetch_json(&self, _url: &str, _timeout: Duration) -> anyhow::Result<Value> {
            anyhow::bail!("network unavailable")
        }
    }

    struct StubDownloadClient(Option<Vec<u8>>);
    impl PdfDownloadClient for StubDownloadClient {
        fn fetch_bytes(&self, _url: &str, _timeout: Duration) -> anyhow::Result<Vec<u8>> {
            self.0.clone().ok_or_else(|| anyhow::anyhow!("not found"))
        }
    }

    #[test]
    fn cascade_download_pdf_no_doi_produces_no_doi_attempts_for_unpaywall_epmc_biorxiv() {
        let (result, attempts) = cascade_download_pdf(
            &FailingMetadataClient,
            &StubDownloadClient(None),
            "",
            &[
                "unpaywall".to_string(),
                "epmc".to_string(),
                "biorxiv".to_string(),
            ],
            Duration::from_secs(1),
        );
        assert!(result.is_none());
        assert!(attempts
            .iter()
            .all(|a| a.error.as_deref() == Some("no DOI")));
    }

    #[test]
    fn cascade_download_pdf_missing_doi_quirk_still_attempts_arxiv_with_raw_item_key() {
        // No DOI at all: `doi_or_key` is the raw item key, matching Python's exact
        // `cascade_download_pdf(doi=doi or item_key, ...)` call site.
        let (result, attempts) = cascade_download_pdf(
            &FailingMetadataClient,
            &StubDownloadClient(None),
            "ITEM0001",
            &["arxiv".to_string()],
            Duration::from_secs(1),
        );
        assert!(result.is_none());
        // No arXiv-shaped id in "ITEM0001" -> no candidate urls, not skipped for "no DOI".
        assert_eq!(attempts[0].error.as_deref(), Some("no candidate urls"));
    }

    #[test]
    fn cascade_download_pdf_stops_at_first_successful_source() {
        let mut pdf = b"%PDF-1.4".to_vec();
        pdf.resize(9000, 0);
        let (result, attempts) = cascade_download_pdf(
            &FailingMetadataClient,
            &StubDownloadClient(Some(pdf.clone())),
            "10.1101/2024.01.01.000001",
            &["biorxiv".to_string(), "arxiv".to_string()],
            Duration::from_secs(1),
        );
        assert_eq!(result, Some(pdf));
        assert_eq!(attempts[0].source, "biorxiv");
        assert!(attempts[0].ok);
        // arxiv never attempted -- cascade stopped at the first success.
        assert!(attempts.iter().all(|a| a.source != "arxiv"));
    }

    #[test]
    fn cascade_download_pdf_rejects_non_pdf_body_and_tries_next_url() {
        let (result, attempts) = cascade_download_pdf(
            &FailingMetadataClient,
            &StubDownloadClient(Some(b"not a pdf at all".to_vec())),
            "10.1101/2024.01.01.000001",
            &["biorxiv".to_string()],
            Duration::from_secs(1),
        );
        assert!(result.is_none());
        assert!(attempts.iter().all(|a| !a.ok));
    }
}
