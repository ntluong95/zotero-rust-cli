//! XML tree representations and helper utilities using quick-xml for OOXML documents.

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;
use std::io::Cursor;

pub const WORD_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
pub const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub const PACKAGE_REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlElement {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub children: Vec<XmlNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XmlNode {
    Element(XmlElement),
    Text(String),
    Comment(String),
    CData(String),
}

impl XmlElement {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            attributes: Vec::new(),
            children: Vec::new(),
        }
    }

    pub fn with_attributes(name: impl Into<String>, attrs: Vec<(String, String)>) -> Self {
        Self {
            name: name.into(),
            attributes: attrs,
            children: Vec::new(),
        }
    }

    pub fn get_attr(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(k, _)| k == name || k.ends_with(&format!(":{name}")))
            .map(|(_, v)| v.as_str())
    }

    pub fn set_attr(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name_str = name.into();
        let value_str = value.into();
        if let Some(pos) = self.attributes.iter().position(|(k, _)| k == &name_str) {
            self.attributes[pos].1 = value_str;
        } else {
            self.attributes.push((name_str, value_str));
        }
    }

    pub fn remove_attr(&mut self, name: &str) {
        self.attributes.retain(|(k, _)| k != name);
    }

    pub fn add_child(&mut self, node: XmlNode) {
        self.children.push(node);
    }

    pub fn add_element(&mut self, element: XmlElement) {
        self.children.push(XmlNode::Element(element));
    }

    pub fn add_text(&mut self, text: impl Into<String>) {
        self.children.push(XmlNode::Text(text.into()));
    }

    pub fn iter_text(&self) -> String {
        let mut out = String::new();
        self.collect_text(&mut out);
        out
    }

    fn collect_text(&self, out: &mut String) {
        for child in &self.children {
            match child {
                XmlNode::Element(el) => el.collect_text(out),
                XmlNode::Text(t) => out.push_str(t),
                XmlNode::CData(c) => out.push_str(c),
                XmlNode::Comment(_) => {}
            }
        }
    }

    /// Recursively find all elements matching a tag name (e.g. "w:t" or "t").
    pub fn find_all<'a>(&'a self, tag: &str) -> Vec<&'a XmlElement> {
        let mut results = Vec::new();
        self.collect_find_all(tag, &mut results);
        results
    }

    fn collect_find_all<'a>(&'a self, tag: &str, results: &mut Vec<&'a XmlElement>) {
        if self.matches_tag(tag) {
            results.push(self);
        }
        for child in &self.children {
            if let XmlNode::Element(el) = child {
                el.collect_find_all(tag, results);
            }
        }
    }

    /// Recursively find the first element matching a tag name.
    pub fn find_first<'a>(&'a self, tag: &str) -> Option<&'a XmlElement> {
        if self.matches_tag(tag) {
            return Some(self);
        }
        for child in &self.children {
            if let XmlNode::Element(el) = child {
                if let Some(found) = el.find_first(tag) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Recursively find the first element mutably matching a tag name.
    pub fn find_first_mut<'a>(&'a mut self, tag: &str) -> Option<&'a mut XmlElement> {
        if self.matches_tag(tag) {
            return Some(self);
        }
        for child in &mut self.children {
            if let XmlNode::Element(el) = child {
                if let Some(found) = el.find_first_mut(tag) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Check if this element's tag matches `tag` (exact match or suffix match after colon).
    pub fn matches_tag(&self, tag: &str) -> bool {
        if self.name == tag {
            return true;
        }
        if let Some(stripped) = tag.strip_prefix("w:") {
            if self.name == stripped || self.name.ends_with(&format!(":{stripped}")) {
                return true;
            }
        }
        if let Some(stripped) = tag.strip_prefix("r:") {
            if self.name == stripped || self.name.ends_with(&format!(":{stripped}")) {
                return true;
            }
        }
        false
    }
}

/// Normalizes whitespace by replacing runs of whitespace with a single space and trimming.
pub fn normalize_space(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_whitespace = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !in_whitespace && !result.is_empty() {
                result.push(' ');
            }
            in_whitespace = true;
        } else {
            result.push(c);
            in_whitespace = false;
        }
    }
    result.trim().to_string()
}

/// Truncates a string to `max_len` unicode characters, appending an ellipsis "…" if truncated.
pub fn truncate(text: &str, max_len: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_len {
        text.to_string()
    } else {
        let prefix: String = chars[..max_len - 1].iter().collect();
        format!("{prefix}…")
    }
}

/// Extracts the normalized visible text from a document XML root (`w:t` nodes joined by space).
pub fn visible_text(root: &XmlElement) -> String {
    let t_nodes = root.find_all("w:t");
    let text_parts: Vec<String> = t_nodes.iter().map(|n| n.iter_text()).collect();
    normalize_space(&text_parts.join(" "))
}

/// Parses raw XML bytes into an `XmlElement` tree.
pub fn parse_xml(bytes: &[u8]) -> anyhow::Result<XmlElement> {
    let mut reader = Reader::from_reader(Cursor::new(bytes));
    reader.config_mut().trim_text(false);

    let mut stack: Vec<XmlElement> = Vec::new();
    let mut root: Option<XmlElement> = None;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Decl(_) => {}
            Event::Start(e) => {
                let name = e.name().as_ref().to_string();
                let mut attrs = Vec::new();
                for attr in e.attributes() {
                    let attr = attr?;
                    let k = attr.key.as_ref().to_string();
                    let v = attr.value.to_string();
                    attrs.push((k, v));
                }
                let elem = XmlElement::with_attributes(name, attrs);
                stack.push(elem);
            }
            Event::Empty(e) => {
                let name = e.name().as_ref().to_string();
                let mut attrs = Vec::new();
                for attr in e.attributes() {
                    let attr = attr?;
                    let k = attr.key.as_ref().to_string();
                    let v = attr.value.to_string();
                    attrs.push((k, v));
                }
                let elem = XmlElement::with_attributes(name, attrs);
                if let Some(parent) = stack.last_mut() {
                    parent.add_element(elem);
                } else if root.is_none() {
                    root = Some(elem);
                }
            }
            Event::End(e) => {
                let name = e.name().as_ref().to_string();
                if let Some(elem) = stack.pop() {
                    if elem.name != name && !elem.name.ends_with(&name) {
                        // Tolerate slight namespace variations on end tag if any
                    }
                    if let Some(parent) = stack.last_mut() {
                        parent.add_element(elem);
                    } else {
                        root = Some(elem);
                    }
                }
            }
            Event::Text(e) => {
                let text = html_escape::decode_html_entities(e.as_ref()).to_string();
                if let Some(parent) = stack.last_mut() {
                    parent.add_text(text);
                }
            }
            Event::CData(e) => {
                let text = e.as_ref().to_string();
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(XmlNode::CData(text));
                }
            }
            Event::Comment(e) => {
                let text = e.as_ref().to_string();
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(XmlNode::Comment(text));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    root.ok_or_else(|| anyhow::anyhow!("Empty or invalid XML document"))
}

/// Serializes an `XmlElement` tree back to XML bytes with an optional XML declaration.
pub fn serialize_xml(root: &XmlElement, with_decl: bool) -> anyhow::Result<Vec<u8>> {
    let mut writer = Writer::new(Cursor::new(Vec::new()));

    if with_decl {
        writer.write_event(Event::Decl(BytesDecl::new(
            "1.0",
            Some("UTF-8"),
            Some("yes"),
        )))?;
    }

    write_element(&mut writer, root)?;

    Ok(writer.into_inner().into_inner())
}

fn write_element<W: std::io::Write>(
    writer: &mut Writer<W>,
    elem: &XmlElement,
) -> anyhow::Result<()> {
    if elem.children.is_empty() {
        let mut start = BytesStart::new(&elem.name);
        for (k, v) in &elem.attributes {
            start.push_attribute((k.as_str(), v.as_str()));
        }
        writer.write_event(Event::Empty(start))?;
    } else {
        let mut start = BytesStart::new(&elem.name);
        for (k, v) in &elem.attributes {
            start.push_attribute((k.as_str(), v.as_str()));
        }
        writer.write_event(Event::Start(start))?;

        for child in &elem.children {
            match child {
                XmlNode::Element(child_elem) => write_element(writer, child_elem)?,
                XmlNode::Text(text) => {
                    writer.write_event(Event::Text(BytesText::new(text)))?;
                }
                XmlNode::CData(cdata) => {
                    writer.write_event(Event::CData(quick_xml::events::BytesCData::new(cdata)))?;
                }
                XmlNode::Comment(comment) => {
                    writer
                        .write_event(Event::Comment(quick_xml::events::BytesText::new(comment)))?;
                }
            }
        }

        writer.write_event(Event::End(BytesEnd::new(&elem.name)))?;
    }
    Ok(())
}

/// Creates a `<w:t>` XML element with optional `xml:space="preserve"` if the text has leading/trailing spaces.
pub fn create_w_t(text: &str) -> XmlElement {
    let mut elem = XmlElement::new("w:t");
    if text.starts_with(char::is_whitespace) || text.ends_with(char::is_whitespace) {
        elem.set_attr("xml:space", "preserve");
    }
    elem.add_text(text);
    elem
}

/// Creates a `<w:r>` run containing a `<w:t>` element, optionally copying formatting from `template_run`.
pub fn create_run_with_text(template_run: Option<&XmlElement>, text: &str) -> XmlElement {
    let mut run = XmlElement::new("w:r");
    if let Some(tmpl) = template_run {
        if let Some(r_pr) = tmpl.find_first("w:rPr") {
            run.add_element(r_pr.clone());
        }
    }
    run.add_element(create_w_t(text));
    run
}

/// Creates a `<w:p>` paragraph containing a single run with the given text.
pub fn create_paragraph_with_text(text: &str) -> XmlElement {
    let mut p = XmlElement::new("w:p");
    p.add_element(create_run_with_text(None, text));
    p
}

/// Creates a `<w:hyperlink r:id="...">` element with a styled run.
pub fn create_hyperlink_node(rel_id: &str, text: &str) -> XmlElement {
    let mut hyperlink = XmlElement::new("w:hyperlink");
    hyperlink.set_attr("r:id", rel_id);

    let mut run = XmlElement::new("w:r");
    let mut r_pr = XmlElement::new("w:rPr");
    let mut r_style = XmlElement::new("w:rStyle");
    r_style.set_attr("w:val", "Hyperlink");
    r_pr.add_element(r_style);
    run.add_element(r_pr);
    run.add_element(create_w_t(text));

    hyperlink.add_element(run);
    hyperlink
}
