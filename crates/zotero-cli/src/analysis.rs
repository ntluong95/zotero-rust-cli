//! LLM context generation and item analysis.
//!
//! Ported from `cli_anything/zotero/core/analysis.py` and `utils/openai_api.py`
//! pinned at `PiaoyangGuohai1/cli-anything-zotero@e42a930e`.

use serde::Serialize;
use serde_json::Value;
use std::io::Read;
use std::time::Duration;

use crate::catalog;
use crate::db;
use crate::error::DomainError;
use crate::rendering;
use crate::runtime::RuntimeContext;
use crate::session::SessionState;

const DEFAULT_OPENAI_URL: &str = "https://api.openai.com/v1/chat/completions";
const OPENAI_USER_AGENT: &str = "cli-anything-zotero/0.7.0";
const OPENAI_TIMEOUT_SECS: u64 = 60;

/// Structured LLM-ready context payload (`analysis.py:88-119`).
#[derive(Debug, Clone, Serialize)]
pub struct ItemContextPayload {
    pub item: db::Item,
    pub attachments: Vec<catalog::ItemWithResolvedPath>,
    pub notes: Vec<db::Item>,
    pub exports: serde_json::Map<String, Value>,
    pub links: serde_json::Map<String, Value>,
    #[serde(rename = "prompt_context")]
    pub prompt_context: String,
}

/// Analysis result payload (`analysis.py:159-166`).
#[derive(Debug, Clone, Serialize)]
pub struct AnalyzeResult {
    #[serde(rename = "itemKey")]
    pub item_key: String,
    pub model: String,
    pub question: String,
    pub answer: String,
    #[serde(rename = "responseID")]
    pub response_id: Option<String>,
    pub context: ItemContextPayload,
}

#[derive(Debug, Clone)]
pub struct OpenAiResponse {
    pub response_id: Option<String>,
    pub answer: String,
}

fn creator_line(creators: &[db::Creator]) -> String {
    if creators.is_empty() {
        return String::new();
    }
    let mut parts = Vec::new();
    for creator in creators {
        let mut name_parts = Vec::new();
        if let Some(first) = &creator.first_name {
            if !first.is_empty() {
                name_parts.push(first.as_str());
            }
        }
        if let Some(last) = &creator.last_name {
            if !last.is_empty() {
                name_parts.push(last.as_str());
            }
        }
        let full_name = if !name_parts.is_empty() {
            name_parts.join(" ")
        } else {
            creator.creator_id.to_string()
        };
        parts.push(full_name);
    }
    parts.join(", ")
}

fn link_payload(item: &db::Item) -> serde_json::Map<String, Value> {
    let mut links = serde_json::Map::new();
    if let Some(url) = item.fields.get("url").and_then(|v| v.as_str()) {
        if !url.is_empty() {
            links.insert("url".to_string(), Value::String(url.to_string()));
        }
    }

    let doi = item
        .fields
        .get("DOI")
        .or_else(|| item.fields.get("doi"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());

    if let Some(doi_val) = doi {
        links.insert("doi".to_string(), Value::String(doi_val.to_string()));
        links.insert(
            "doi_url".to_string(),
            Value::String(format!("https://doi.org/{doi_val}")),
        );
    }

    links
}

fn build_prompt_context(
    item: &db::Item,
    links: &serde_json::Map<String, Value>,
    attachments: &[catalog::ItemWithResolvedPath],
    notes: &[db::Item],
    exports: &serde_json::Map<String, Value>,
) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Title: {}", item.title));
    lines.push(format!("Item Key: {}", item.key));
    lines.push(format!("Item Type: {}", item.type_name));

    let creators_text = creator_line(&item.creators);
    if !creators_text.is_empty() {
        lines.push(format!("Creators: {creators_text}"));
    }

    let mut sorted_field_keys: Vec<&String> = item.fields.keys().collect();
    sorted_field_keys.sort();

    for field_name in sorted_field_keys {
        if field_name == "title" {
            continue;
        }
        let val = &item.fields[field_name];
        let val_str = match val {
            Value::Null => continue,
            Value::String(s) if s.is_empty() => continue,
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            other => other.to_string(),
        };
        lines.push(format!("{field_name}: {val_str}"));
    }

    if !links.is_empty() {
        lines.push("Links:".to_string());
        for (k, v) in links {
            let val_str = v.as_str().unwrap_or("");
            lines.push(format!("- {k}: {val_str}"));
        }
    }

    if !attachments.is_empty() {
        lines.push("Attachments:".to_string());
        for att in attachments {
            let title = if !att.item.title.is_empty() {
                &att.item.title
            } else {
                &att.item.key
            };
            let path = att
                .resolved_path
                .as_deref()
                .or(att.item.attachment_path.as_deref())
                .unwrap_or("<missing>");
            lines.push(format!("- {title}: {path}"));
        }
    }

    if !notes.is_empty() {
        lines.push("Notes:".to_string());
        for note in notes {
            let title = if !note.title.is_empty() {
                &note.title
            } else {
                &note.key
            };
            let text = if !note.note_text.is_empty() {
                &note.note_text
            } else {
                &note.note_preview
            };
            lines.push(format!("- {title}: {text}"));
        }
    }

    if !exports.is_empty() {
        lines.push("Exports:".to_string());
        for (fmt, content) in exports {
            lines.push(format!("[{fmt}]"));
            if let Some(content_str) = content.as_str() {
                lines.push(content_str.to_string());
            }
        }
    }

    lines.join("\n").trim().to_string()
}

/// Build structured and prompt-ready LLM context for an item (`analysis.py:88-119`).
#[allow(clippy::too_many_arguments)]
pub fn build_item_context(
    runtime: &RuntimeContext,
    item_ref: Option<&str>,
    include_notes: bool,
    include_bibtex: bool,
    include_csljson: bool,
    include_links: bool,
    session: &SessionState,
) -> anyhow::Result<ItemContextPayload> {
    let item = catalog::get_item(runtime, item_ref, session)?;
    let attachments = catalog::item_attachments(runtime, Some(&item.key), session)?;

    let notes = if include_notes {
        catalog::item_notes(runtime, Some(&item.key), session)?
    } else {
        Vec::new()
    };

    let mut exports = serde_json::Map::new();
    if include_bibtex {
        let bib = rendering::export_item(runtime, Some(&item.key), "bibtex", session)?;
        exports.insert("bibtex".to_string(), Value::String(bib.content));
    }
    if include_csljson {
        let csl = rendering::export_item(runtime, Some(&item.key), "csljson", session)?;
        exports.insert("csljson".to_string(), Value::String(csl.content));
    }

    let links = if include_links {
        link_payload(&item)
    } else {
        serde_json::Map::new()
    };

    let prompt_context = build_prompt_context(&item, &links, &attachments, &notes, &exports);

    Ok(ItemContextPayload {
        item,
        attachments,
        notes,
        exports,
        links,
        prompt_context,
    })
}

/// Extract text from OpenAI-compatible response in order (`openai_api.py:14-40`):
/// 1. `choices[0].message.content`
/// 2. `output_text`
/// 3. `output[].content[].text` joined by `\n\n`
pub fn extract_text(response_payload: &Value) -> Option<String> {
    // 1. Chat Completions format
    if let Some(choices) = response_payload.get("choices").and_then(|c| c.as_array()) {
        if let Some(first) = choices.first() {
            if let Some(content) = first
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
            {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }

    // 2. Responses API format (legacy fallback)
    if let Some(output_text) = response_payload.get("output_text").and_then(|t| t.as_str()) {
        let trimmed = output_text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    // 3. Nested output content parts
    if let Some(output) = response_payload.get("output").and_then(|o| o.as_array()) {
        let mut parts = Vec::new();
        for item in output {
            if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
                for content_item in content {
                    if let Some(text) = content_item.get("text").and_then(|t| t.as_str()) {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            parts.push(trimmed.to_string());
                        }
                    }
                }
            }
        }
        if !parts.is_empty() {
            let joined = parts.join("\n\n").trim().to_string();
            if !joined.is_empty() {
                return Some(joined);
            }
        }
    }

    None
}

/// Send request to OpenAI-compatible Chat Completions endpoint (`openai_api.py:42-85`).
///
/// External Data Egress: Sends `input_text` (containing `prompt_context` and `question`)
/// to the configured OpenAI URL (`CLI_ANYTHING_ZOTERO_OPENAI_URL` or `https://api.openai.com/v1/chat/completions`).
/// Never logs API keys or Authorization headers.
pub fn create_text_response(
    api_key: &str,
    model: &str,
    instructions: &str,
    input_text: &str,
    timeout_secs: u64,
) -> anyhow::Result<OpenAiResponse> {
    let api_url = std::env::var("CLI_ANYTHING_ZOTERO_OPENAI_URL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_OPENAI_URL.to_string());

    let payload = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": instructions},
            {"role": "user", "content": input_text},
        ],
    });

    let req = ureq::post(&api_url)
        .header("Authorization", &format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", OPENAI_USER_AGENT)
        .config()
        .timeout_global(Some(Duration::from_secs(timeout_secs)))
        .http_status_as_error(false)
        .build();

    let response = req.send_json(&payload).map_err(|err| {
        anyhow::Error::from(DomainError::new(format!(
            "OpenAI API request failed: {err}"
        )))
    })?;

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let mut body_str = String::new();
        let _ = response
            .into_body()
            .into_reader()
            .read_to_string(&mut body_str);
        return Err(
            DomainError::new(format!("OpenAI API returned HTTP {status}: {body_str}")).into(),
        );
    }

    let mut body_str = String::new();
    response
        .into_body()
        .into_reader()
        .read_to_string(&mut body_str)?;

    let json_resp: Value = serde_json::from_str(&body_str)
        .map_err(|e| DomainError::new(format!("OpenAI API request failed: {e}")))?;

    let answer = extract_text(&json_resp)
        .ok_or_else(|| DomainError::new("OpenAI API returned no text output"))?;

    let response_id = json_resp
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(OpenAiResponse {
        response_id,
        answer,
    })
}

/// Analyze item context with an LLM (`analysis.py:121-167`).
#[allow(clippy::too_many_arguments)]
pub fn analyze_item(
    runtime: &RuntimeContext,
    item_ref: Option<&str>,
    question: &str,
    model: &str,
    include_notes: bool,
    include_bibtex: bool,
    include_csljson: bool,
    session: &SessionState,
) -> anyhow::Result<AnalyzeResult> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    if api_key.is_empty() {
        return Err(DomainError::new(
            "OPENAI_API_KEY is not set. Use `item context` for model-independent output or configure the API key.",
        )
        .into());
    }

    let context_payload = build_item_context(
        runtime,
        item_ref,
        include_notes,
        include_bibtex,
        include_csljson,
        true, // include_links is always true for analyze
        session,
    )?;

    let input_text = format!(
        "Use the Zotero item context below to answer the user's question.\n\nQuestion:\n{}\n\nContext:\n{}",
        question.trim(),
        context_payload.prompt_context
    );

    let instructions = "You are analyzing a Zotero bibliographic record. Stay grounded in the provided context. If the context is missing an answer, say so explicitly.";

    let resp = create_text_response(
        &api_key,
        model,
        instructions,
        &input_text,
        OPENAI_TIMEOUT_SECS,
    )?;

    Ok(AnalyzeResult {
        item_key: context_payload.item.key.clone(),
        model: model.to_string(),
        question: question.to_string(),
        answer: resp.answer,
        response_id: resp.response_id,
        context: context_payload,
    })
}
