//! NIH iCite citation metrics lookup.
//!
//! Fetches citation metrics (citation count, RCR, NIH percentile, etc.)
//! from the NIH iCite API for a given PMID.
//!
//! Ported from `cli_anything/zotero/core/metrics.py` and `zotero_cli.py:1410-1441`
//! pinned at `PiaoyangGuohai1/cli-anything-zotero@e42a930e`.

use anyhow::Result;
use serde_json::Value;
use std::io::Read;
use std::time::Duration;

use crate::catalog;
use crate::error::DomainError;
use crate::runtime::RuntimeContext;
use crate::session::SessionState;

const NIH_ICITE_BASE_URL: &str = "https://icite.od.nih.gov/api/pubs";
const USER_AGENT: &str = "Mozilla/5.0";
const TIMEOUT_SECS: u64 = 15;

/// Fetch citation metrics from NIH iCite for a given PMID (`metrics.py:13-35`).
///
/// External Data Egress: Sends only `pmids=<pmid>` parameter to `https://icite.od.nih.gov/api/pubs`.
/// No API keys, credentials, or other metadata are sent.
pub fn get_metrics(pmid: &str) -> Value {
    let base_url = std::env::var("CLI_ANYTHING_ZOTERO_ICITE_URL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| NIH_ICITE_BASE_URL.to_string());
    let url = format!("{base_url}?pmids={pmid}&format=json");
    let req = ureq::get(&url)
        .header("User-Agent", USER_AGENT)
        .config()
        .timeout_global(Some(Duration::from_secs(TIMEOUT_SECS)))
        .http_status_as_error(false)
        .build();

    let response = match req.call() {
        Ok(r) => r,
        Err(err) => {
            return serde_json::json!({
                "error": format!("Failed to fetch metrics for PMID {pmid}: {err}")
            });
        }
    };

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return serde_json::json!({
            "error": format!("Failed to fetch metrics for PMID {pmid}: HTTP {status}")
        });
    }

    let mut body_str = String::new();
    if let Err(err) = response
        .into_body()
        .into_reader()
        .read_to_string(&mut body_str)
    {
        return serde_json::json!({
            "error": format!("Failed to fetch metrics for PMID {pmid}: {err}")
        });
    }

    let parsed: Value = match serde_json::from_str(&body_str) {
        Ok(v) => v,
        Err(err) => {
            return serde_json::json!({
                "error": format!("Failed to fetch metrics for PMID {pmid}: {err}")
            });
        }
    };

    let data = match parsed.get("data").and_then(|d| d.as_array()) {
        Some(arr) if !arr.is_empty() => &arr[0],
        _ => {
            return serde_json::json!({
                "error": format!("No data for PMID {pmid}")
            });
        }
    };

    let title_full = data.get("title").and_then(|t| t.as_str()).unwrap_or("");
    let title: String = title_full.chars().take(80).collect();

    serde_json::json!({
        "pmid": data.get("pmid"),
        "title": title,
        "year": data.get("year"),
        "journal": data.get("journal").and_then(|j| j.as_str()).unwrap_or(""),
        "citation_count": data.get("citation_count").and_then(|c| c.as_i64()).unwrap_or(0),
        "rcr": data.get("relative_citation_ratio"),
        "nih_percentile": data.get("nih_percentile"),
        "expected_citations": data.get("expected_citations_per_year"),
        "doi": data.get("doi").and_then(|d| d.as_str()).unwrap_or(""),
    })
}

/// Look up citation metrics for an item or direct PMID (`zotero_cli.py:1410-1441`).
pub fn item_metrics(
    runtime: &RuntimeContext,
    ref_id: &str,
    is_pmid: bool,
    session: &SessionState,
) -> Result<Value> {
    let pmid = if is_pmid {
        ref_id.to_string()
    } else {
        let item = catalog::get_item(runtime, Some(ref_id), session)?;
        let mut extracted_pmid: Option<String> = None;

        if let Some(val) = item.fields.get("PMID") {
            let s = match val {
                Value::String(s) => s.trim().to_string(),
                Value::Number(n) => n.to_string(),
                _ => String::new(),
            };
            if !s.is_empty() {
                extracted_pmid = Some(s);
            }
        }

        if extracted_pmid.is_none() {
            if let Some(extra_val) = item.fields.get("extra").and_then(|v| v.as_str()) {
                for line in extra_val.lines() {
                    let stripped = line.trim();
                    if stripped.to_uppercase().starts_with("PMID:") {
                        if let Some((_, rest)) = stripped.split_once(':') {
                            let p = rest.trim();
                            if !p.is_empty() {
                                extracted_pmid = Some(p.to_string());
                                break;
                            }
                        }
                    }
                }
            }
        }

        match extracted_pmid {
            Some(p) => p,
            None => {
                return Err(DomainError::new(format!(
                    "No PMID found in item '{ref_id}' (checked PMID field and extra text). Use --pmid flag to pass a PMID directly."
                ))
                .into());
            }
        }
    };

    Ok(get_metrics(&pmid))
}
