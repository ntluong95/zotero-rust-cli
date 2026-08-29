//! Static citation rendering for DOCX documents.
//!
//! Ports `docx render-citations`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{json, Map, Value};

use super::inspect::{inspect_citations, parse_placeholder_keys, PLACEHOLDER_RE};
use super::package::{read_document_xml, validate_docx_path, write_package};
use super::validate::validate_placeholders;
use super::xml::{
    create_paragraph_with_text, create_run_with_text, parse_xml, serialize_xml, XmlElement, XmlNode,
};
use crate::catalog;
use crate::http;
use crate::runtime::RuntimeContext;
use crate::session::SessionState;

pub const DEFAULT_STYLE: &str = "apa";
pub const DEFAULT_LOCALE: &str = "en-US";
pub const DEFAULT_BIBLIOGRAPHY: &str = "auto";

static HTML_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<[^>]+>").expect("HTML_TAG_RE must compile"));

#[derive(Debug, Clone)]
pub struct RenderedItem {
    pub citation: String,
    pub bibliography: String,
}

/// Replace Zotero placeholders in a DOCX with static citation text and bibliography.
#[allow(clippy::too_many_arguments)]
pub fn render_static_citations(
    runtime: &RuntimeContext,
    path: &Path,
    output: &Path,
    style: &str,
    locale: &str,
    bibliography: &str,
    session: &SessionState,
    overwrite: bool,
) -> anyhow::Result<Value> {
    if bibliography != "auto" && bibliography != "none" {
        anyhow::bail!("Bibliography mode must be one of: auto, none.");
    }

    let source_path = validate_docx_path(path)?;

    if output.exists() && !overwrite {
        anyhow::bail!("Output already exists: {}", output.display());
    }

    let validation = validate_placeholders(runtime, &source_path, 10, session)?;
    let ok = validation
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !ok {
        anyhow::bail!("DOCX placeholders are not ready for static citation rendering.");
    }

    let placeholder_count = validation
        .get("placeholder_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if placeholder_count == 0 {
        anyhow::bail!(
            "No Zotero placeholders were found. Use {{{{zotero:ITEMKEY}}}} or {{{{zotero:KEY1,KEY2}}}}."
        );
    }

    let items_array = validation
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut item_by_key: HashMap<String, Value> = HashMap::new();
    for item in &items_array {
        if let Some(key) = item.get("key").and_then(Value::as_str) {
            item_by_key.insert(key.to_string(), item.clone());
        }
    }

    if !runtime.local_api_available {
        anyhow::bail!(
            "Zotero Local API is not available after launching Zotero. \
             Open Zotero, enable the Local API, then rerun this command."
        );
    }

    let zotero_startup = json!({
        "attempted": false,
        "ok": true,
        "local_api_ready": true,
        "reason": "local API already available"
    });

    let rendered_items = render_items(runtime, &item_by_key, style, locale)?;

    let doc_bytes = read_document_xml(&source_path)?;
    let mut root = parse_xml(&doc_bytes)?;

    let mut rendered_placeholders = Vec::new();
    replace_placeholders_in_element(&mut root, &rendered_items, &mut rendered_placeholders)?;

    let mut bibliography_entries = Vec::new();
    if bibliography == "auto" {
        bibliography_entries = build_bibliography_entries(&rendered_items, &rendered_placeholders);
        insert_static_bibliography(&mut root, &bibliography_entries)?;
    }

    let modified_doc_xml = serialize_xml(&root, true)?;
    let mut replaced_parts = HashMap::new();
    replaced_parts.insert("word/document.xml".to_string(), modified_doc_xml);

    write_package(&source_path, output, overwrite, &replaced_parts)?;

    let inspection = inspect_citations(output, 10000)?;

    let mut citation_count = 0;
    for entry in &rendered_placeholders {
        if let Some(keys) = entry.get("keys").and_then(Value::as_array) {
            citation_count += keys.len();
        }
    }

    let mut result = Map::new();
    result.insert("ok".to_string(), json!(true));
    result.insert("mode".to_string(), json!("static"));
    result.insert(
        "input".to_string(),
        json!(source_path.to_string_lossy().to_string()),
    );
    result.insert(
        "output".to_string(),
        json!(output.to_string_lossy().to_string()),
    );
    result.insert("style".to_string(), json!(style));
    result.insert("locale".to_string(), json!(locale));
    result.insert("bibliography".to_string(), json!(bibliography));
    result.insert(
        "placeholder_count".to_string(),
        json!(rendered_placeholders.len()),
    );
    result.insert("citation_count".to_string(), json!(citation_count));
    result.insert(
        "bibliography_count".to_string(),
        json!(bibliography_entries.len()),
    );
    result.insert(
        "rendered_placeholders".to_string(),
        json!(rendered_placeholders),
    );
    result.insert("items".to_string(), json!(items_array));
    result.insert("zotero_startup".to_string(), zotero_startup);

    let inspection_fields = json!({
        "field_count": inspection.get("field_count"),
        "field_counts": inspection.get("field_counts"),
        "systems": inspection.get("systems"),
        "static_citation_count": inspection.get("static_citation_count")
    });
    result.insert("inspection".to_string(), inspection_fields);

    result.insert(
        "notes".to_string(),
        json!([
            "Static citations were rendered as ordinary DOCX text.",
            "The output cannot be refreshed by the Zotero word processor plugin; rerender from the placeholder DOCX if citation data changes."
        ]),
    );

    Ok(Value::Object(result))
}

fn render_items(
    runtime: &RuntimeContext,
    item_by_key: &HashMap<String, Value>,
    style: &str,
    locale: &str,
) -> anyhow::Result<HashMap<String, RenderedItem>> {
    let mut rendered = HashMap::new();
    let mut sorted_keys: Vec<&String> = item_by_key.keys().collect();
    sorted_keys.sort();

    for key in sorted_keys {
        let item = &item_by_key[key];
        let library_id = item.get("libraryID").and_then(Value::as_i64).unwrap_or(1);
        let scope = catalog::local_api_scope(runtime, library_id)?;
        let path = format!("{scope}/items/{key}");

        let cit_params = [
            ("format", "json".to_string()),
            ("include", "citation".to_string()),
            ("style", style.to_string()),
            ("locale", locale.to_string()),
        ];
        let cit_payload = http::local_api_get_json(
            runtime.environment.port,
            &path,
            &cit_params,
            Duration::from_secs(5),
        )?;
        let raw_citation = extract_field_text(&cit_payload, "citation");

        let bib_params = [
            ("format", "json".to_string()),
            ("include", "bib".to_string()),
            ("style", style.to_string()),
            ("locale", locale.to_string()),
        ];
        let bib_payload = http::local_api_get_json(
            runtime.environment.port,
            &path,
            &bib_params,
            Duration::from_secs(5),
        )?;
        let raw_bib = extract_field_text(&bib_payload, "bib");

        rendered.insert(
            key.clone(),
            RenderedItem {
                citation: plain_text(&raw_citation),
                bibliography: plain_text(&raw_bib),
            },
        );
    }

    Ok(rendered)
}

fn extract_field_text(payload: &Value, field: &str) -> String {
    if let Some(val) = payload.get(field).and_then(Value::as_str) {
        val.to_string()
    } else if let Some(first) = payload.as_array().and_then(|a| a.first()) {
        first
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    } else {
        String::new()
    }
}

pub fn plain_text(val: &str) -> String {
    let stripped = HTML_TAG_RE.replace_all(val, "");
    let unescaped = html_escape::decode_html_entities(&stripped);
    unescaped.trim().to_string()
}

pub fn combined_citation(citations: &[String]) -> String {
    let cleaned: Vec<String> = citations
        .iter()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();

    if cleaned.is_empty() {
        return String::new();
    }
    if cleaned.len() == 1 {
        return cleaned[0].clone();
    }
    if cleaned
        .iter()
        .all(|c| c.starts_with('(') && c.ends_with(')'))
    {
        let inside: Vec<&str> = cleaned.iter().map(|c| c[1..c.len() - 1].trim()).collect();
        return format!("({})", inside.join("; "));
    }
    cleaned.join("; ")
}

fn replace_placeholders_in_element(
    elem: &mut XmlElement,
    rendered_items: &HashMap<String, RenderedItem>,
    rendered_placeholders: &mut Vec<Value>,
) -> anyhow::Result<()> {
    if elem.matches_tag("w:t") {
        let current_text = elem.iter_text();
        if PLACEHOLDER_RE.is_match(&current_text) {
            let mut new_text = String::new();
            let mut last_end = 0;

            for mat in PLACEHOLDER_RE.find_iter(&current_text) {
                new_text.push_str(&current_text[last_end..mat.start()]);
                let raw = mat.as_str().to_string();
                let group1 = if let Some(caps) = PLACEHOLDER_RE.captures(mat.as_str()) {
                    caps.get(1).map(|m| m.as_str()).unwrap_or("")
                } else {
                    ""
                };
                let (keys, invalid) = parse_placeholder_keys(group1);
                if !invalid.is_empty() || keys.is_empty() {
                    anyhow::bail!("Invalid Zotero placeholder: {raw}");
                }

                let citations: Vec<String> = keys
                    .iter()
                    .map(|k| {
                        rendered_items
                            .get(k)
                            .map(|r| r.citation.clone())
                            .unwrap_or_default()
                    })
                    .collect();
                let combined = combined_citation(&citations);

                let mut p_map = Map::new();
                p_map.insert("raw".to_string(), json!(raw));
                p_map.insert("keys".to_string(), json!(keys));
                p_map.insert("citation".to_string(), json!(combined));
                rendered_placeholders.push(Value::Object(p_map));

                new_text.push_str(&combined);
                last_end = mat.end();
            }
            new_text.push_str(&current_text[last_end..]);

            elem.children.clear();
            elem.children.push(XmlNode::Text(new_text.clone()));
            if new_text.starts_with(char::is_whitespace) || new_text.ends_with(char::is_whitespace)
            {
                elem.set_attr("xml:space", "preserve");
            }
            return Ok(());
        }
    }

    // Check if placeholder is split across multiple w:r / w:t runs inside a paragraph w:p
    if elem.matches_tag("w:p") {
        let p_text = elem
            .find_all("w:t")
            .into_iter()
            .map(|t| t.iter_text())
            .collect::<Vec<_>>()
            .join("");

        let individual_has_match = elem
            .find_all("w:t")
            .into_iter()
            .any(|t| PLACEHOLDER_RE.is_match(&t.iter_text()));

        if !individual_has_match && PLACEHOLDER_RE.is_match(&p_text) {
            // Split across runs: collapse paragraph's runs into formatted replacement
            let mut new_text = String::new();
            let mut last_end = 0;
            for mat in PLACEHOLDER_RE.find_iter(&p_text) {
                new_text.push_str(&p_text[last_end..mat.start()]);
                let raw = mat.as_str().to_string();
                let group1 = if let Some(caps) = PLACEHOLDER_RE.captures(mat.as_str()) {
                    caps.get(1).map(|m| m.as_str()).unwrap_or("")
                } else {
                    ""
                };
                let (keys, invalid) = parse_placeholder_keys(group1);
                if !invalid.is_empty() || keys.is_empty() {
                    anyhow::bail!("Invalid Zotero placeholder: {raw}");
                }

                let citations: Vec<String> = keys
                    .iter()
                    .map(|k| {
                        rendered_items
                            .get(k)
                            .map(|r| r.citation.clone())
                            .unwrap_or_default()
                    })
                    .collect();
                let combined = combined_citation(&citations);

                let mut p_map = Map::new();
                p_map.insert("raw".to_string(), json!(raw));
                p_map.insert("keys".to_string(), json!(keys));
                p_map.insert("citation".to_string(), json!(combined));
                rendered_placeholders.push(Value::Object(p_map));

                new_text.push_str(&combined);
                last_end = mat.end();
            }
            new_text.push_str(&p_text[last_end..]);

            let template_r = elem.find_first("w:r").cloned();
            elem.children
                .retain(|c| matches!(c, XmlNode::Element(el) if el.matches_tag("w:pPr")));
            elem.add_element(create_run_with_text(template_r.as_ref(), &new_text));
            return Ok(());
        }
    }

    for child in &mut elem.children {
        if let XmlNode::Element(child_elem) = child {
            replace_placeholders_in_element(child_elem, rendered_items, rendered_placeholders)?;
        }
    }

    Ok(())
}

fn build_bibliography_entries(
    rendered_items: &HashMap<String, RenderedItem>,
    rendered_placeholders: &[Value],
) -> Vec<Value> {
    let mut keys = Vec::new();
    let mut seen = HashSet::new();

    for p in rendered_placeholders {
        if let Some(p_keys) = p.get("keys").and_then(Value::as_array) {
            for k_val in p_keys {
                if let Some(k) = k_val.as_str() {
                    if !seen.contains(k) {
                        seen.insert(k.to_string());
                        keys.push(k.to_string());
                    }
                }
            }
        }
    }

    let mut entries = Vec::new();
    for key in keys {
        if let Some(item) = rendered_items.get(&key) {
            if !item.bibliography.is_empty() {
                let mut map = Map::new();
                map.insert("key".to_string(), json!(key));
                map.insert("bibliography".to_string(), json!(item.bibliography));
                entries.push(Value::Object(map));
            }
        }
    }

    entries
}

fn insert_static_bibliography(root: &mut XmlElement, entries: &[Value]) -> anyhow::Result<()> {
    if entries.is_empty() {
        return Ok(());
    }

    let body = root
        .find_first_mut("w:body")
        .ok_or_else(|| anyhow::anyhow!("DOCX document.xml has no w:body element."))?;

    let headings = [
        "references",
        "bibliography",
        "works cited",
        "参考文献",
        "參考文獻",
    ];
    let mut existing_heading_idx = None;

    for (idx, child) in body.children.iter().enumerate() {
        if let XmlNode::Element(el) = child {
            if el.matches_tag("w:p") {
                let text = el.iter_text().trim().to_lowercase();
                if headings.contains(&text.as_str()) {
                    existing_heading_idx = Some(idx);
                    break;
                }
            }
        }
    }

    let insert_pos = if let Some(idx) = existing_heading_idx {
        idx + 1
    } else {
        let sect_pr_idx = body
            .children
            .iter()
            .position(|child| matches!(child, XmlNode::Element(el) if el.matches_tag("w:sectPr")));

        let heading_p = create_paragraph_with_text("References");
        if let Some(pos) = sect_pr_idx {
            body.children.insert(pos, XmlNode::Element(heading_p));
            pos + 1
        } else {
            body.children.push(XmlNode::Element(heading_p));
            body.children.len()
        }
    };

    for (offset, entry) in entries.iter().enumerate() {
        let bib_text = entry
            .get("bibliography")
            .and_then(Value::as_str)
            .unwrap_or("");
        let p = create_paragraph_with_text(bib_text);
        body.children
            .insert(insert_pos + offset, XmlNode::Element(p));
    }

    Ok(())
}
