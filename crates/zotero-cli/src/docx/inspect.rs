//! Citation and placeholder inspection for DOCX documents.
//!
//! Ports `docx inspect-citations` and `docx inspect-placeholders`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};
use serde_json::{json, Map, Value};

use super::package::{read_document_xml, read_optional_zip_member, validate_docx_path};
use super::xml::{normalize_space, parse_xml, truncate, visible_text, XmlElement};

static AUTHOR_PATTERN: &str =
    r"[A-Z][A-Za-z'’.-]+(?:\s+(?:&|and)\s+[A-Z][A-Za-z'’.-]+|\s+et\s+al\.)?";

pub static AUTHOR_YEAR_RE: LazyLock<Regex> = LazyLock::new(|| {
    let pattern =
        format!(r"\({AUTHOR_PATTERN},\s+\d{{4}}[a-z]?(?:;\s*{AUTHOR_PATTERN},\s+\d{{4}}[a-z]?)*\)");
    Regex::new(&pattern).expect("AUTHOR_YEAR_RE must compile")
});

pub static NUMERIC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[(?:\d+(?:\s*[-,]\s*\d+)*)\]").expect("NUMERIC_RE must compile")
});

pub static PLACEHOLDER_RE: LazyLock<Regex> = LazyLock::new(|| {
    RegexBuilder::new(r"\{\{\s*zotero\s*:\s*([^}]*)\s*\}\}")
        .case_insensitive(true)
        .build()
        .expect("PLACEHOLDER_RE must compile")
});

pub static ZOTERO_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z0-9]{8}$").expect("ZOTERO_KEY_RE must compile"));

pub static ZOTERO_BOOKMARK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ZOTERO_BREF_(.+)$").expect("ZOTERO_BOOKMARK_RE must compile"));

pub static ZOTERO_CUSTOM_PROP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(ZOTERO_BREF_.+)_(\d+)$").expect("ZOTERO_CUSTOM_PROP_RE must compile")
});

/// Inspect a DOCX file for citation field systems and static citation text.
pub fn inspect_citations(path: &Path, sample_limit: usize) -> anyhow::Result<Value> {
    let docx_path = validate_docx_path(path)?;
    let doc_bytes = read_document_xml(&docx_path)?;
    let root = parse_xml(&doc_bytes)?;

    let instructions = field_instructions(&root);
    let mut fields: Vec<Value> = instructions.iter().map(|inst| field_report(inst)).collect();
    fields.extend(zotero_bookmark_reports(&docx_path, &root)?);

    let mut field_counts: BTreeMap<String, usize> = BTreeMap::new();
    for field in &fields {
        if let Some(sys) = field.get("system").and_then(Value::as_str) {
            *field_counts.entry(sys.to_string()).or_default() += 1;
        }
    }

    let vis_text = visible_text(&root);
    let static_matches = static_citation_matches(&vis_text);

    let mut systems: Vec<String> = field_counts
        .iter()
        .filter(|(_, &count)| count > 0)
        .map(|(k, _)| k.clone())
        .collect();
    systems.sort();
    if !static_matches.is_empty() {
        systems.push("static-text".to_string());
    }

    let notes = build_citation_notes(&field_counts, !static_matches.is_empty());

    let mut map = Map::new();
    map.insert(
        "path".to_string(),
        json!(docx_path.to_string_lossy().to_string()),
    );
    map.insert("has_fields".to_string(), json!(!fields.is_empty()));
    map.insert("systems".to_string(), json!(systems));
    map.insert("field_counts".to_string(), json!(field_counts));
    map.insert("field_count".to_string(), json!(fields.len()));
    let limit = sample_limit.min(fields.len());
    map.insert("fields".to_string(), json!(fields[..limit]));
    map.insert(
        "static_citation_count".to_string(),
        json!(static_matches.len()),
    );
    let sample_limit_matches = sample_limit.min(static_matches.len());
    map.insert(
        "static_citation_samples".to_string(),
        json!(static_matches[..sample_limit_matches]),
    );
    map.insert("notes".to_string(), json!(notes));

    Ok(Value::Object(map))
}

/// Inspect a DOCX file for Zotero-bound AI citation placeholders.
pub fn inspect_placeholders(path: &Path, sample_limit: usize) -> anyhow::Result<Value> {
    let docx_path = validate_docx_path(path)?;
    let doc_bytes = read_document_xml(&docx_path)?;
    let root = parse_xml(&doc_bytes)?;

    let vis_text = visible_text(&root);
    let mut placeholders = Vec::new();
    let mut invalid_placeholders = Vec::new();
    let mut key_occurrences = Vec::new();

    for mat in PLACEHOLDER_RE.find_iter(&vis_text) {
        let raw = mat.as_str().to_string();
        let group1 = if let Some(caps) = PLACEHOLDER_RE.captures(mat.as_str()) {
            caps.get(1).map(|m| m.as_str()).unwrap_or("")
        } else {
            ""
        };
        let (keys, invalid_parts) = parse_placeholder_keys(group1);
        let ctx = extract_context(&vis_text, mat.start(), mat.end(), 80);

        let mut entry = Map::new();
        entry.insert("raw".to_string(), json!(raw));
        entry.insert("keys".to_string(), json!(keys));
        entry.insert("context".to_string(), json!(ctx));

        placeholders.push(Value::Object(entry.clone()));
        key_occurrences.extend(keys.clone());

        if !invalid_parts.is_empty() || keys.is_empty() {
            let mut inv_entry = entry;
            let parts = if !invalid_parts.is_empty() {
                invalid_parts
            } else {
                vec![group1.trim().to_string()]
            };
            inv_entry.insert("invalid_parts".to_string(), json!(parts));
            inv_entry.insert(
                "reason".to_string(),
                json!("Expected comma-separated 8-character Zotero item keys."),
            );
            invalid_placeholders.push(Value::Object(inv_entry));
        }
    }

    let mut counts: HashMap<String, usize> = HashMap::new();
    for key in &key_occurrences {
        *counts.entry(key.clone()).or_default() += 1;
    }

    let mut unique_keys: Vec<String> = counts.keys().cloned().collect();
    unique_keys.sort();

    let mut duplicate_keys: Vec<String> = counts
        .iter()
        .filter(|(_, &count)| count > 1)
        .map(|(k, _)| k.clone())
        .collect();
    duplicate_keys.sort();

    let notes = build_placeholder_notes(!placeholders.is_empty(), !invalid_placeholders.is_empty());

    let mut map = Map::new();
    map.insert(
        "path".to_string(),
        json!(docx_path.to_string_lossy().to_string()),
    );
    map.insert("placeholder_count".to_string(), json!(placeholders.len()));
    map.insert("citation_count".to_string(), json!(key_occurrences.len()));
    map.insert("unique_keys".to_string(), json!(unique_keys));
    map.insert("duplicate_keys".to_string(), json!(duplicate_keys));
    let limit_p = sample_limit.min(placeholders.len());
    map.insert("placeholders".to_string(), json!(placeholders[..limit_p]));
    let limit_inv = sample_limit.min(invalid_placeholders.len());
    map.insert(
        "invalid_placeholders".to_string(),
        json!(invalid_placeholders[..limit_inv]),
    );
    map.insert("notes".to_string(), json!(notes));

    Ok(Value::Object(map))
}

pub fn parse_placeholder_keys(raw_keys: &str) -> (Vec<String>, Vec<String>) {
    let mut keys = Vec::new();
    let mut invalid_parts = Vec::new();

    for part in raw_keys.split(',') {
        let candidate = part.trim().to_uppercase();
        if candidate.is_empty() {
            continue;
        }
        if ZOTERO_KEY_RE.is_match(&candidate) {
            keys.push(candidate);
        } else {
            invalid_parts.push(part.trim().to_string());
        }
    }

    (keys, invalid_parts)
}

fn extract_context(text: &str, byte_start: usize, byte_end: usize, radius: usize) -> String {
    let char_indices: Vec<(usize, char)> = text.char_indices().collect();
    let start_char_idx = char_indices
        .iter()
        .position(|&(b, _)| b >= byte_start)
        .unwrap_or(char_indices.len());
    let end_char_idx = char_indices
        .iter()
        .position(|&(b, _)| b >= byte_end)
        .unwrap_or(char_indices.len());

    let prefix_char_start = start_char_idx.saturating_sub(radius);
    let suffix_char_end = (end_char_idx + radius).min(char_indices.len());

    let prefix_byte_start = char_indices
        .get(prefix_char_start)
        .map(|&(b, _)| b)
        .unwrap_or(0);
    let suffix_byte_end = if suffix_char_end < char_indices.len() {
        char_indices[suffix_char_end].0
    } else {
        text.len()
    };

    let slice = &text[prefix_byte_start..suffix_byte_end];
    let mut ctx = slice.to_string();
    if prefix_byte_start > 0 {
        ctx = format!("...{ctx}");
    }
    if suffix_byte_end < text.len() {
        ctx = format!("{ctx}...");
    }
    normalize_space(&ctx)
}

fn field_instructions(root: &XmlElement) -> Vec<String> {
    let mut instructions = Vec::new();
    for elem in root.find_all("w:instrText") {
        let text = normalize_space(&elem.iter_text());
        if !text.is_empty() {
            instructions.push(text);
        }
    }
    for elem in root.find_all("w:fldSimple") {
        if let Some(instr) = elem.get_attr("w:instr").or_else(|| elem.get_attr("instr")) {
            let text = normalize_space(instr);
            if !text.is_empty() {
                instructions.push(text);
            }
        }
    }
    instructions
}

fn field_report(instruction: &str) -> Value {
    let system = classify_instruction(instruction);
    let mut map = Map::new();
    map.insert("system".to_string(), json!(system));
    map.insert("instruction".to_string(), json!(truncate(instruction, 240)));
    Value::Object(map)
}

fn classify_instruction(instruction: &str) -> &'static str {
    let upper = instruction.to_uppercase();
    if upper.contains("ADDIN ZOTERO")
        || upper.contains("ZOTERO_ITEM")
        || upper.contains("ZOTERO_BIBL")
    {
        "zotero"
    } else if upper.contains("ADDIN EN.CITE") || upper.contains("ADDIN EN.REFLIST") {
        "endnote"
    } else if upper.contains("MENDELEY") {
        "mendeley"
    } else if upper.contains("CSL_CITATION") || upper.contains("CSL_BIBLIOGRAPHY") {
        "csl"
    } else if upper.contains("ADDIN") {
        "unknown-addin"
    } else {
        "word-field"
    }
}

fn zotero_bookmark_reports(path: &Path, root: &XmlElement) -> anyhow::Result<Vec<Value>> {
    let names = zotero_bookmark_names(root);
    if names.is_empty() {
        return Ok(Vec::new());
    }

    let custom_props = zotero_custom_properties(path)?;
    let mut reports = Vec::new();

    for name in names {
        let code = custom_props.get(&name).cloned().unwrap_or_default();
        let instruction = if !code.is_empty() {
            code
        } else {
            format!("{name} bookmark without custom property data")
        };

        let mut map = Map::new();
        map.insert("system".to_string(), json!("zotero"));
        map.insert(
            "instruction".to_string(),
            json!(truncate(&normalize_space(&instruction), 240)),
        );
        map.insert("field_type".to_string(), json!("bookmark"));
        map.insert("bookmark".to_string(), json!(name));
        reports.push(Value::Object(map));
    }

    Ok(reports)
}

fn zotero_bookmark_names(root: &XmlElement) -> Vec<String> {
    let mut names = Vec::new();
    let mut seen = HashSet::new();

    for elem in root.find_all("w:bookmarkStart") {
        let name = elem
            .get_attr("w:name")
            .or_else(|| elem.get_attr("name"))
            .unwrap_or("");
        if ZOTERO_BOOKMARK_RE.is_match(name) && !seen.contains(name) {
            seen.insert(name.to_string());
            names.push(name.to_string());
        }
    }

    names
}

fn zotero_custom_properties(path: &Path) -> anyhow::Result<HashMap<String, String>> {
    let custom_xml = read_optional_zip_member(path, "docProps/custom.xml")?;
    let Some(custom_xml) = custom_xml else {
        return Ok(HashMap::new());
    };

    let root = parse_xml(&custom_xml)?;
    let mut chunks: HashMap<String, Vec<(i64, String)>> = HashMap::new();

    for prop in root.find_all("property") {
        let name = prop.get_attr("name").unwrap_or("");
        if let Some(caps) = ZOTERO_CUSTOM_PROP_RE.captures(name) {
            let base = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            let idx_str = caps.get(2).map(|m| m.as_str()).unwrap_or("0");
            let idx = idx_str.parse::<i64>().unwrap_or(0);
            let text = prop.iter_text();
            chunks.entry(base).or_default().push((idx, text));
        }
    }

    let mut result = HashMap::new();
    for (base, mut parts) in chunks {
        parts.sort_by_key(|(idx, _)| *idx);
        let joined: String = parts.into_iter().map(|(_, text)| text).collect();
        result.insert(base, joined);
    }

    Ok(result)
}

fn static_citation_matches(text: &str) -> Vec<String> {
    let mut matches = Vec::new();
    let mut seen = HashSet::new();

    for mat in AUTHOR_YEAR_RE.find_iter(text) {
        let s = mat.as_str().to_string();
        if !seen.contains(&s) {
            seen.insert(s.clone());
            matches.push(s);
        }
    }

    for mat in NUMERIC_RE.find_iter(text) {
        let s = mat.as_str().to_string();
        if !seen.contains(&s) {
            seen.insert(s.clone());
            matches.push(s);
        }
    }

    matches
}

fn build_citation_notes(
    field_counts: &BTreeMap<String, usize>,
    has_static_text: bool,
) -> Vec<String> {
    let mut notes = Vec::new();
    if field_counts.get("endnote").copied().unwrap_or(0) > 0 {
        notes.push(
            "EndNote fields are present; Zotero cannot refresh these as Zotero citations."
                .to_string(),
        );
    }
    if field_counts.get("zotero").copied().unwrap_or(0) > 0 {
        notes.push(
            "Zotero citation fields are present and should be managed with the Zotero word processor plugin."
                .to_string(),
        );
    }
    if field_counts.get("csl").copied().unwrap_or(0) > 0
        || field_counts.get("mendeley").copied().unwrap_or(0) > 0
    {
        notes.push(
            "CSL/Mendeley-like fields are present; verify which word processor plugin created them before editing."
                .to_string(),
        );
    }
    if has_static_text {
        notes.push("Static citation-looking text is present; these citations may not be refreshable fields.".to_string());
    }
    if field_counts.is_empty() && !has_static_text {
        notes.push(
            "No citation fields or common static citation patterns were detected.".to_string(),
        );
    }
    notes
}

fn build_placeholder_notes(has_placeholders: bool, has_invalid: bool) -> Vec<String> {
    let mut notes = Vec::new();
    if has_placeholders {
        notes.push(
            "Zotero placeholders are present; validate them before converting or finalizing the DOCX."
                .to_string(),
        );
    } else {
        notes.push(
            "No Zotero placeholders were detected. AI-authored DOCX citation insertion should use {{zotero:ITEMKEY}} placeholders."
                .to_string(),
        );
    }
    if has_invalid {
        notes.push(
            "Some Zotero placeholders are malformed and should be fixed before document conversion."
                .to_string(),
        );
    }
    notes
}
