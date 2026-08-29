//! Structural comparison tests for OOXML DOCX package and XML transformations.
//!
//! Validates:
//! 1. OPC package structure (Content_Types, relationships).
//! 2. Preservation of unmodified parts byte-for-byte.
//! 3. Preservation of run formatting properties (w:rPr).
//! 4. CJK and multi-byte UTF-8 character integrity across transformations.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use zotero_cli::docx::package::{read_document_xml, write_package};
use zotero_cli::docx::xml::{create_run_with_text, parse_xml, serialize_xml, visible_text};

struct TestDir(PathBuf);
impl TestDir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "zotero-docx-struct-test-{}-{}",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&p);
        Self(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn create_rich_test_docx(dir: &Path, filename: &str) -> PathBuf {
    let path = dir.join(filename);
    let file = File::create(&path).expect("create docx file");
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // [Content_Types].xml
    zip.start_file("[Content_Types].xml", options).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Default Extension="png" ContentType="image/png"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
  <Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>
</Types>"#,
    )
    .unwrap();

    // _rels/.rels
    zip.start_file("_rels/.rels", options).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#,
    )
    .unwrap();

    // word/styles.xml (unmodified part)
    zip.start_file("word/styles.xml", options).unwrap();
    zip.write_all(b"<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:docDefaults/></w:styles>").unwrap();

    // word/media/image1.png (binary unmodified part)
    zip.start_file("word/media/image1.png", options).unwrap();
    zip.write_all(b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR...SAMPLE_PNG_BYTES")
        .unwrap();

    // word/document.xml
    let doc_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <w:body>
    <w:p>
      <w:pPr><w:jc w:val="center"/></w:pPr>
      <w:r>
        <w:rPr><w:b/><w:color w:val="FF0000"/></w:rPr>
        <w:t>Formatted text with CJK: 中文, 日本語, 한국어, Tiếng Việt (ñ, ü, é, å).</w:t>
      </w:r>
    </w:p>
    <w:sectPr/>
  </w:body>
</w:document>"#;
    zip.start_file("word/document.xml", options).unwrap();
    zip.write_all(doc_xml.as_bytes()).unwrap();

    zip.finish().unwrap();
    path
}

#[test]
fn test_unmodified_parts_preserved_byte_for_byte() {
    let temp = TestDir::new("struct");
    let src = create_rich_test_docx(temp.path(), "rich_source.docx");
    let out = temp.path().join("rich_output.docx");

    let doc_bytes = read_document_xml(&src).unwrap();
    let mut root = parse_xml(&doc_bytes).unwrap();

    // Modify text in document.xml
    let p_elem = root.find_first_mut("w:p").unwrap();
    p_elem.add_element(create_run_with_text(None, " Appended text."));

    let modified_doc_xml = serialize_xml(&root, true).unwrap();
    let mut replaced_parts = HashMap::new();
    replaced_parts.insert("word/document.xml".to_string(), modified_doc_xml);

    write_package(&src, &out, true, &replaced_parts).unwrap();

    // Verify unmodified parts
    let src_file = File::open(&src).unwrap();
    let mut src_zip = ZipArchive::new(src_file).unwrap();

    let out_file = File::open(&out).unwrap();
    let mut out_zip = ZipArchive::new(out_file).unwrap();

    assert_eq!(src_zip.len(), out_zip.len());

    for name in &[
        "[Content_Types].xml",
        "_rels/.rels",
        "word/styles.xml",
        "word/media/image1.png",
    ] {
        let mut src_entry = src_zip.by_name(name).unwrap();
        let mut src_data = Vec::new();
        src_entry.read_to_end(&mut src_data).unwrap();

        let mut out_entry = out_zip.by_name(name).unwrap();
        let mut out_data = Vec::new();
        out_entry.read_to_end(&mut out_data).unwrap();

        assert_eq!(src_data, out_data, "Part {name} must be byte-identical!");
    }
}

#[test]
fn test_cjk_and_unicode_round_trip() {
    let temp = TestDir::new("struct");
    let src = create_rich_test_docx(temp.path(), "unicode_source.docx");

    let doc_bytes = read_document_xml(&src).unwrap();
    let root = parse_xml(&doc_bytes).unwrap();

    let vis = visible_text(&root);
    assert!(vis.contains("中文"));
    assert!(vis.contains("日本語"));
    assert!(vis.contains("한국어"));
    assert!(vis.contains("Tiếng Việt"));
    assert!(vis.contains("ñ, ü, é, å"));

    let serialized = serialize_xml(&root, true).unwrap();
    let root2 = parse_xml(&serialized).unwrap();
    let vis2 = visible_text(&root2);

    assert_eq!(vis, vis2);
}

#[test]
fn test_run_formatting_preservation() {
    let temp = TestDir::new("struct");
    let src = create_rich_test_docx(temp.path(), "format_source.docx");

    let doc_bytes = read_document_xml(&src).unwrap();
    let root = parse_xml(&doc_bytes).unwrap();

    let r_elem = root.find_first("w:r").unwrap();
    let r_pr = r_elem.find_first("w:rPr").unwrap();

    assert!(r_pr.find_first("w:b").is_some());
    let color = r_pr.find_first("w:color").unwrap();
    assert_eq!(color.get_attr("w:val"), Some("FF0000"));

    // Create a new run using this template
    let cloned_run = create_run_with_text(Some(r_elem), "New text with same formatting");
    let cloned_r_pr = cloned_run.find_first("w:rPr").unwrap();
    assert!(cloned_r_pr.find_first("w:b").is_some());
    assert_eq!(
        cloned_r_pr.find_first("w:color").unwrap().get_attr("w:val"),
        Some("FF0000")
    );
}
