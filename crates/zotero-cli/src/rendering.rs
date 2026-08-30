//! Port of `core/rendering.py`: read-only item citation/bibliography/export
//! rendering via the Zotero Local API. All three functions here are pure
//! reads -- no SQLite writes, no Zotero item mutation.
//!
//! `export bib`'s CLI-level orchestration (arg validation, collection
//! resolution/filtering, and the standalone output file write) is not part
//! of `core/rendering.py` upstream either -- it lives in `zotero_cli.py`'s
//! `export_bib_command` and is ported the same way here, as a helper in
//! `lib.rs` that calls `export_item` in a loop.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::catalog;
use crate::error::DomainError;
use crate::http;
use crate::runtime::RuntimeContext;
use crate::session::SessionState;

/// `SUPPORTED_EXPORT_FORMATS` (`rendering.py:10`).
pub const SUPPORTED_EXPORT_FORMATS: [&str; 7] = [
    "ris", "bibtex", "biblatex", "csljson", "csv", "mods", "refer",
];

/// `_require_local_api()` (`rendering.py:13-18`).
fn require_local_api(runtime: &RuntimeContext) -> anyhow::Result<()> {
    if !runtime.local_api_available {
        return Err(DomainError::new(
            "Zotero Local API is not available. Start Zotero and enable \
             `extensions.zotero.httpServer.localAPI.enabled` first.",
        )
        .into());
    }
    Ok(())
}

/// `export_item()` (`rendering.py:26-34`) return shape.
#[derive(Debug, Clone, Serialize)]
pub struct ExportItemResult {
    #[serde(rename = "itemKey")]
    pub item_key: String,
    #[serde(rename = "libraryID")]
    pub library_id: i64,
    pub format: String,
    pub content: String,
}

/// `export_item()` (`rendering.py:26-34`).
pub fn export_item(
    runtime: &RuntimeContext,
    item_ref: Option<&str>,
    fmt: &str,
    session: &SessionState,
) -> anyhow::Result<ExportItemResult> {
    require_local_api(runtime)?;
    if !SUPPORTED_EXPORT_FORMATS.contains(&fmt) {
        return Err(DomainError::new(format!("Unsupported export format: {fmt}")).into());
    }
    let item = catalog::get_item(runtime, item_ref, session)?;
    let key = item.key.clone();
    let scope = catalog::local_api_scope(runtime, item.library_id)?;
    let content = http::local_api_get_text(
        runtime.environment.port,
        &format!("{scope}/items/{key}"),
        &[("format", fmt.to_string())],
        Duration::from_secs(15),
    )?;
    Ok(ExportItemResult {
        item_key: key,
        library_id: item.library_id,
        format: fmt.to_string(),
        content,
    })
}

/// `citation_item()` (`rendering.py:37-66`) return shape.
#[derive(Debug, Clone, Serialize)]
pub struct CitationResult {
    #[serde(rename = "itemKey")]
    pub item_key: String,
    #[serde(rename = "libraryID")]
    pub library_id: i64,
    pub style: Option<String>,
    pub locale: Option<String>,
    pub linkwrap: bool,
    pub citation: Option<String>,
}

/// `citation_item()` (`rendering.py:37-66`).
#[allow(clippy::too_many_arguments)]
pub fn citation_item(
    runtime: &RuntimeContext,
    item_ref: Option<&str>,
    style: Option<&str>,
    locale: Option<&str>,
    linkwrap: bool,
    session: &SessionState,
) -> anyhow::Result<CitationResult> {
    require_local_api(runtime)?;
    let item = catalog::get_item(runtime, item_ref, session)?;
    let key = item.key.clone();
    let payload = fetch_render_payload(
        runtime,
        &key,
        item.library_id,
        "citation",
        style,
        locale,
        linkwrap,
    )?;
    let citation = extract_field(&payload, "citation");
    Ok(CitationResult {
        item_key: key,
        library_id: item.library_id,
        style: style.map(str::to_string),
        locale: locale.map(str::to_string),
        linkwrap,
        citation,
    })
}

/// `bibliography_item()` (`rendering.py:69-98`) return shape.
#[derive(Debug, Clone, Serialize)]
pub struct BibliographyResult {
    #[serde(rename = "itemKey")]
    pub item_key: String,
    #[serde(rename = "libraryID")]
    pub library_id: i64,
    pub style: Option<String>,
    pub locale: Option<String>,
    pub linkwrap: bool,
    pub bibliography: Option<String>,
}

/// `bibliography_item()` (`rendering.py:69-98`).
#[allow(clippy::too_many_arguments)]
pub fn bibliography_item(
    runtime: &RuntimeContext,
    item_ref: Option<&str>,
    style: Option<&str>,
    locale: Option<&str>,
    linkwrap: bool,
    session: &SessionState,
) -> anyhow::Result<BibliographyResult> {
    require_local_api(runtime)?;
    let item = catalog::get_item(runtime, item_ref, session)?;
    let key = item.key.clone();
    let payload = fetch_render_payload(
        runtime,
        &key,
        item.library_id,
        "bib",
        style,
        locale,
        linkwrap,
    )?;
    let bibliography = extract_field(&payload, "bib");
    Ok(BibliographyResult {
        item_key: key,
        library_id: item.library_id,
        style: style.map(str::to_string),
        locale: locale.map(str::to_string),
        linkwrap,
        bibliography,
    })
}

/// Shared `GET {scope}/items/{key}?format=json&include=<citation|bib>[&style=...][&locale=...][&linkwrap=1]`
/// call underlying both `citation_item` (`rendering.py:49-57`) and
/// `bibliography_item` (`rendering.py:81-89`) -- the two functions build an
/// identical param set except for the `include` value.
#[allow(clippy::too_many_arguments)]
fn fetch_render_payload(
    runtime: &RuntimeContext,
    key: &str,
    library_id: i64,
    include: &str,
    style: Option<&str>,
    locale: Option<&str>,
    linkwrap: bool,
) -> anyhow::Result<Value> {
    let mut params: Vec<(&str, String)> = vec![
        ("format", "json".to_string()),
        ("include", include.to_string()),
    ];
    if let Some(style) = style {
        params.push(("style", style.to_string()));
    }
    if let Some(locale) = locale {
        params.push(("locale", locale.to_string()));
    }
    if linkwrap {
        params.push(("linkwrap", "1".to_string()));
    }
    let scope = catalog::local_api_scope(runtime, library_id)?;
    http::local_api_get_json(
        runtime.environment.port,
        &format!("{scope}/items/{key}"),
        &params,
        Duration::from_secs(10),
    )
}

/// `payload.get(field) if isinstance(payload, dict) else (payload[0].get(field) if payload else None)`
/// (`rendering.py:58` / `rendering.py:90`).
fn extract_field(payload: &Value, field: &str) -> Option<String> {
    match payload {
        Value::Object(_) => payload
            .get(field)
            .and_then(Value::as_str)
            .map(str::to_string),
        Value::Array(items) => items
            .first()
            .and_then(|first| first.get(field))
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_field_reads_dict_shape() {
        let payload = serde_json::json!({"citation": "(Doe, 2020)"});
        assert_eq!(
            extract_field(&payload, "citation"),
            Some("(Doe, 2020)".to_string())
        );
    }

    #[test]
    fn extract_field_reads_first_array_element() {
        let payload = serde_json::json!([{"bib": "Doe, J. (2020)."}]);
        assert_eq!(
            extract_field(&payload, "bib"),
            Some("Doe, J. (2020).".to_string())
        );
    }

    #[test]
    fn extract_field_empty_array_is_none() {
        let payload = serde_json::json!([]);
        assert_eq!(extract_field(&payload, "citation"), None);
    }

    #[test]
    fn extract_field_missing_key_is_none() {
        let payload = serde_json::json!({"other": "value"});
        assert_eq!(extract_field(&payload, "citation"), None);
    }
}
