//! Native DOCX / OOXML processing for Zotero citation workflows.
//!
//! Provides inspection of citation field systems and AI placeholders,
//! validation against the local Zotero library, and static citation rendering.

pub mod inspect;
pub mod package;
pub mod static_render;
pub mod validate;
pub mod working;
pub mod xml;

pub use inspect::{inspect_citations, inspect_placeholders};
pub use package::{read_document_xml, validate_docx_path, write_package};
pub use static_render::{
    render_static_citations, DEFAULT_BIBLIOGRAPHY, DEFAULT_LOCALE, DEFAULT_STYLE,
};
pub use validate::validate_placeholders;
pub use working::build_working_docx;
