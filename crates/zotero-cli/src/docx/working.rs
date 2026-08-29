//! Working DOCX builder for Zotero conversion.
//!
//! Replaces placeholders with note-citation hyperlinks and updates package relationships.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use super::inspect::{parse_placeholder_keys, PLACEHOLDER_RE};
use super::package::{
    read_document_xml, read_optional_zip_member, validate_docx_path, write_package,
};
use super::validate::validate_placeholders;
use super::xml::{
    create_hyperlink_node, create_paragraph_with_text, create_run_with_text, parse_xml,
    serialize_xml, XmlElement, XmlNode, PACKAGE_REL_NS,
};
use crate::runtime::RuntimeContext;
use crate::session::SessionState;

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn generate_hex_token() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let cnt = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{now:016x}{cnt:016x}")
}

/// Copy a placeholder DOCX and replace Zotero placeholders with note-citation links.
pub fn build_working_docx(
    runtime: &RuntimeContext,
    path: &Path,
    output: &Path,
    session: &SessionState,
    overwrite: bool,
    bibliography: &str,
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
        anyhow::bail!("DOCX placeholders are not ready for zoterify conversion.");
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

    let doc_bytes = read_document_xml(&source_path)?;
    let mut root = parse_xml(&doc_bytes)?;

    let placeholders = replace_placeholders_with_note_links(&mut root, &item_by_key)?;

    let bibliography_placeholder = if bibliography == "auto" {
        Some(insert_bibliography_placeholder(&mut root)?)
    } else {
        None
    };

    let existing_rels_bytes =
        read_optional_zip_member(&source_path, "word/_rels/document.xml.rels")?;
    let mut rels_root = if let Some(bytes) = existing_rels_bytes {
        parse_xml(&bytes)?
    } else {
        let mut elem = XmlElement::new("Relationships");
        elem.set_attr("xmlns", PACKAGE_REL_NS);
        elem
    };

    let mut rel_entries = placeholders.clone();
    if let Some(ref bib) = bibliography_placeholder {
        rel_entries.push(bib.clone());
    }

    for entry in &rel_entries {
        let rel_id = entry
            .get("relationship_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let p_id = entry
            .get("placeholder_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let target = format!("https://www.zotero.org/?{p_id}");

        let mut rel = XmlElement::new("Relationship");
        rel.set_attr("Id", rel_id);
        rel.set_attr(
            "Type",
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink",
        );
        rel.set_attr("Target", target);
        rel.set_attr("TargetMode", "External");
        rels_root.add_element(rel);
    }

    let doc_xml = serialize_xml(&root, true)?;
    let rels_xml = serialize_xml(&rels_root, true)?;

    let mut replaced_parts = HashMap::new();
    replaced_parts.insert("word/document.xml".to_string(), doc_xml);
    replaced_parts.insert("word/_rels/document.xml.rels".to_string(), rels_xml);

    write_package(&source_path, output, overwrite, &replaced_parts)?;

    let mut citation_count = 0;
    for entry in &placeholders {
        if let Some(keys) = entry.get("keys").and_then(Value::as_array) {
            citation_count += keys.len();
        }
    }

    let mut result = Map::new();
    result.insert("ok".to_string(), json!(true));
    result.insert(
        "path".to_string(),
        json!(source_path.to_string_lossy().to_string()),
    );
    result.insert(
        "output".to_string(),
        json!(output.to_string_lossy().to_string()),
    );
    result.insert("placeholder_count".to_string(), json!(placeholders.len()));
    result.insert("citation_count".to_string(), json!(citation_count));
    result.insert("placeholders".to_string(), json!(placeholders));
    result.insert("bibliography".to_string(), json!(bibliography));
    result.insert(
        "bibliography_placeholder".to_string(),
        json!(bibliography_placeholder),
    );
    result.insert("items".to_string(), json!(items_array));

    Ok(Value::Object(result))
}

fn replace_placeholders_with_note_links(
    root: &mut XmlElement,
    item_by_key: &HashMap<String, Value>,
) -> anyhow::Result<Vec<Value>> {
    let mut placeholders = Vec::new();
    replace_in_element(root, item_by_key, &mut placeholders)?;
    Ok(placeholders)
}

fn replace_in_element(
    elem: &mut XmlElement,
    item_by_key: &HashMap<String, Value>,
    placeholders: &mut Vec<Value>,
) -> anyhow::Result<()> {
    if elem.matches_tag("w:p") {
        let mut new_children = Vec::new();
        let mut modified = false;

        for child in &elem.children {
            if let XmlNode::Element(ref run) = child {
                if run.matches_tag("w:r") {
                    let run_text = run.iter_text();
                    if PLACEHOLDER_RE.is_match(&run_text) {
                        modified = true;
                        let mut last_end = 0;

                        for mat in PLACEHOLDER_RE.find_iter(&run_text) {
                            if mat.start() > last_end {
                                let prefix_text = &run_text[last_end..mat.start()];
                                new_children.push(XmlNode::Element(create_run_with_text(
                                    Some(run),
                                    prefix_text,
                                )));
                            }

                            let raw = mat.as_str();
                            let group1 = if let Some(caps) = PLACEHOLDER_RE.captures(raw) {
                                caps.get(1).map(|m| m.as_str()).unwrap_or("")
                            } else {
                                ""
                            };
                            let (keys, invalid) = parse_placeholder_keys(group1);
                            if !invalid.is_empty() || keys.is_empty() {
                                anyhow::bail!("Invalid Zotero placeholder: {raw}");
                            }

                            let token = generate_hex_token();
                            let placeholder_id = format!("ZOTERO_CLI_PLACEHOLDER_{token}");
                            let relationship_id = format!("rIdZoteroCli{}", &token[..16]);

                            let mut items_vec = Vec::new();
                            let mut citation_items = Vec::new();
                            for k in &keys {
                                let item = item_by_key.get(k).ok_or_else(|| {
                                    anyhow::anyhow!("Zotero item key was not resolved: {k}")
                                })?;
                                let item_id =
                                    item.get("itemID").and_then(Value::as_i64).unwrap_or(0);
                                let title = item.get("title").and_then(Value::as_str).unwrap_or("");
                                let lib_id =
                                    item.get("libraryID").and_then(Value::as_i64).unwrap_or(1);

                                items_vec.push(json!({
                                    "itemID": item_id,
                                    "key": k,
                                    "libraryID": lib_id,
                                    "title": title
                                }));
                                citation_items.push(json!({ "id": item_id }));
                            }

                            let citation_payload = json!({
                                "citationItems": citation_items,
                                "properties": { "noteIndex": 0 },
                                "schema": "https://github.com/citation-style-language/schema/raw/master/csl-citation.json"
                            });

                            let mut entry = Map::new();
                            entry.insert("placeholder_id".to_string(), json!(placeholder_id));
                            entry.insert("relationship_id".to_string(), json!(relationship_id));
                            entry.insert("keys".to_string(), json!(keys));
                            entry.insert("items".to_string(), json!(items_vec));
                            entry.insert("citation".to_string(), citation_payload);
                            placeholders.push(Value::Object(entry));

                            new_children.push(XmlNode::Element(create_hyperlink_node(
                                &relationship_id,
                                &placeholder_id,
                            )));

                            last_end = mat.end();
                        }

                        if last_end < run_text.len() {
                            let suffix_text = &run_text[last_end..];
                            new_children.push(XmlNode::Element(create_run_with_text(
                                Some(run),
                                suffix_text,
                            )));
                        }
                        continue;
                    }
                }
            }
            new_children.push(child.clone());
        }

        if modified {
            elem.children = new_children;
            return Ok(());
        }
    }

    for child in &mut elem.children {
        if let XmlNode::Element(child_elem) = child {
            replace_in_element(child_elem, item_by_key, placeholders)?;
        }
    }

    Ok(())
}

fn insert_bibliography_placeholder(root: &mut XmlElement) -> anyhow::Result<Value> {
    let body = root
        .find_first_mut("w:body")
        .ok_or_else(|| anyhow::anyhow!("DOCX document.xml has no w:body element."))?;

    let token = generate_hex_token();
    let placeholder_id = format!("ZOTERO_CLI_BIBLIOGRAPHY_{token}");
    let relationship_id = format!("rIdZoteroCliBib{}", &token[..16]);

    let mut p = XmlElement::new("w:p");
    p.add_element(create_hyperlink_node(&relationship_id, &placeholder_id));

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

    let (insert_pos, placement) = if let Some(idx) = existing_heading_idx {
        (idx + 1, "after-existing-heading")
    } else {
        let sect_pr_idx = body
            .children
            .iter()
            .position(|child| matches!(child, XmlNode::Element(el) if el.matches_tag("w:sectPr")));

        let heading_p = create_paragraph_with_text("References");
        if let Some(pos) = sect_pr_idx {
            body.children.insert(pos, XmlNode::Element(heading_p));
            (pos + 1, "appended-heading")
        } else {
            body.children.push(XmlNode::Element(heading_p));
            (body.children.len(), "appended-heading")
        }
    };

    body.children.insert(insert_pos, XmlNode::Element(p));

    let mut map = Map::new();
    map.insert("placeholder_id".to_string(), json!(placeholder_id));
    map.insert("relationship_id".to_string(), json!(relationship_id));
    map.insert("placement".to_string(), json!(placement));

    Ok(Value::Object(map))
}
