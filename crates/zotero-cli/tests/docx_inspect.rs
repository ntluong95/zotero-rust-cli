//! Integration tests for DOCX inspect-citations, inspect-placeholders, and validate-placeholders.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use zotero_cli::docx::inspect::{
    AUTHOR_YEAR_RE, NUMERIC_RE, PLACEHOLDER_RE, ZOTERO_BOOKMARK_RE, ZOTERO_CUSTOM_PROP_RE,
    ZOTERO_KEY_RE,
};
use zotero_cli::docx::{inspect_citations, inspect_placeholders, validate_placeholders};
use zotero_cli::runtime::{build_runtime_context, BuildEnvironmentArgs};
use zotero_cli::session::load_session_state;

struct TestDir(PathBuf);
impl TestDir {
    fn new(name: &str) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "zotero-docx-test-{}-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
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

fn create_test_docx(
    dir: &Path,
    filename: &str,
    document_xml_body: &str,
    custom_xml: Option<&str>,
) -> PathBuf {
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
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
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

    // word/document.xml
    let doc_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <w:body>
    {document_xml_body}
    <w:sectPr/>
  </w:body>
</w:document>"#
    );
    zip.start_file("word/document.xml", options).unwrap();
    zip.write_all(doc_xml.as_bytes()).unwrap();

    // docProps/custom.xml if present
    if let Some(custom) = custom_xml {
        zip.start_file("docProps/custom.xml", options).unwrap();
        zip.write_all(custom.as_bytes()).unwrap();
    }

    zip.finish().unwrap();
    path
}

#[test]
fn test_all_five_regexes_compile_and_match() {
    // 1. PLACEHOLDER_RE
    assert!(PLACEHOLDER_RE.is_match("{{zotero:ABCD1234}}"));
    assert!(PLACEHOLDER_RE.is_match("{{ ZOTERO : ABCD1234, EFGH5678 }}"));
    assert!(PLACEHOLDER_RE.is_match("Prefix {{zotero:KEY1}} suffix"));

    // 2. ZOTERO_KEY_RE
    assert!(ZOTERO_KEY_RE.is_match("ABCD1234"));
    assert!(ZOTERO_KEY_RE.is_match("12345678"));
    assert!(!ZOTERO_KEY_RE.is_match("short"));
    assert!(!ZOTERO_KEY_RE.is_match("toolongkey123"));
    assert!(!ZOTERO_KEY_RE.is_match("invalid-"));

    // 3. AUTHOR_YEAR_RE
    assert!(AUTHOR_YEAR_RE.is_match("(Smith, 2020)"));
    assert!(AUTHOR_YEAR_RE.is_match("(Smith & Jones, 2021a)"));
    assert!(AUTHOR_YEAR_RE.is_match("(Smith and Jones, 2021)"));
    assert!(AUTHOR_YEAR_RE.is_match("(Smith et al., 2022b)"));
    assert!(AUTHOR_YEAR_RE.is_match("(Smith, 2020; Jones et al., 2021)"));
    assert!(AUTHOR_YEAR_RE.is_match("(O'Connor, 2019)"));
    assert!(AUTHOR_YEAR_RE.is_match("(Saint-Pierre, 2020)"));

    // 4. NUMERIC_RE
    assert!(NUMERIC_RE.is_match("[1]"));
    assert!(NUMERIC_RE.is_match("[1, 2, 3]"));
    assert!(NUMERIC_RE.is_match("[1-4, 7, 9-12]"));

    // 5. Bookmark and Custom Props
    let cap = ZOTERO_BOOKMARK_RE.captures("ZOTERO_BREF_abcd1234").unwrap();
    assert_eq!(cap.get(1).unwrap().as_str(), "abcd1234");

    let cap_prop = ZOTERO_CUSTOM_PROP_RE
        .captures("ZOTERO_BREF_abcd1234_0")
        .unwrap();
    assert_eq!(cap_prop.get(1).unwrap().as_str(), "ZOTERO_BREF_abcd1234");
    assert_eq!(cap_prop.get(2).unwrap().as_str(), "0");
}

#[test]
fn test_inspect_citations_on_empty_and_static_and_fields() {
    let temp = TestDir::new("inspect");

    // 1. Empty document
    let p_empty = create_test_docx(
        temp.path(),
        "empty.docx",
        "<w:p><w:r><w:t>Hello world, no citations here.</w:t></w:r></w:p>",
        None,
    );
    let rep_empty = inspect_citations(&p_empty, 10).unwrap();
    assert_eq!(rep_empty["has_fields"], false);
    assert_eq!(rep_empty["field_count"], 0);
    assert_eq!(rep_empty["static_citation_count"], 0);
    let notes = rep_empty["notes"].as_array().unwrap();
    assert!(notes
        .iter()
        .any(|n| n.as_str().unwrap().contains("No citation fields")));

    // 2. EndNote field + numeric citation
    let p_endnote = create_test_docx(
        temp.path(),
        "endnote.docx",
        r#"<w:p>
          <w:r><w:instrText xml:space="preserve"> ADDIN EN.CITE </w:instrText></w:r>
          <w:r><w:t>[1]</w:t></w:r>
        </w:p>"#,
        None,
    );
    let rep_endnote = inspect_citations(&p_endnote, 10).unwrap();
    assert_eq!(rep_endnote["has_fields"], true);
    assert_eq!(rep_endnote["field_count"], 1);
    assert_eq!(rep_endnote["field_counts"]["endnote"], 1);
    assert_eq!(rep_endnote["static_citation_count"], 1);
    assert_eq!(rep_endnote["static_citation_samples"][0], "[1]");
    let sys = rep_endnote["systems"].as_array().unwrap();
    assert!(sys.iter().any(|s| s == "endnote"));
    assert!(sys.iter().any(|s| s == "static-text"));

    // 3. Zotero bookmarks + custom properties
    let custom_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
  <property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="2" name="ZOTERO_BREF_item1_0">
    <vt:lpwstr>ITEM CSL_CITATION {"citationItems":[{"id":123}]}</vt:lpwstr>
  </property>
</Properties>"#;
    let p_bookmark = create_test_docx(
        temp.path(),
        "bookmark.docx",
        r#"<w:p>
          <w:bookmarkStart w:id="0" w:name="ZOTERO_BREF_item1"/>
          <w:r><w:t>(Doe, 2023)</w:t></w:r>
          <w:bookmarkEnd w:id="0"/>
        </w:p>"#,
        Some(custom_xml),
    );
    let rep_bookmark = inspect_citations(&p_bookmark, 10).unwrap();
    assert_eq!(rep_bookmark["has_fields"], true);
    assert_eq!(rep_bookmark["field_count"], 1);
    assert_eq!(rep_bookmark["fields"][0]["system"], "zotero");
    assert_eq!(rep_bookmark["fields"][0]["field_type"], "bookmark");
    assert_eq!(rep_bookmark["fields"][0]["bookmark"], "ZOTERO_BREF_item1");
}

#[test]
fn test_inspect_placeholders_variations_and_cjk() {
    let temp = TestDir::new("inspect");

    let p_docx = create_test_docx(
        temp.path(),
        "placeholders.docx",
        r#"<w:p>
          <w:r><w:t>Known {{zotero:REG12345}} and missing {{zotero:NOITEM99}} and invalid {{zotero:bad, ALSO1234}} 中文测试.</w:t></w:r>
        </w:p>"#,
        None,
    );

    let rep = inspect_placeholders(&p_docx, 10).unwrap();
    assert_eq!(rep["placeholder_count"], 3);
    assert_eq!(rep["citation_count"], 3); // REG12345, NOITEM99, ALSO1234 (invalid 'bad' is not a valid citation key)

    let unique = rep["unique_keys"].as_array().unwrap();
    assert!(unique.iter().any(|k| k == "REG12345"));
    assert!(unique.iter().any(|k| k == "NOITEM99"));
    assert!(unique.iter().any(|k| k == "ALSO1234"));

    let invalid = rep["invalid_placeholders"].as_array().unwrap();
    assert_eq!(invalid.len(), 1);
    assert_eq!(invalid[0]["invalid_parts"][0], "bad");

    let placeholders = rep["placeholders"].as_array().unwrap();
    assert!(placeholders[0]["context"]
        .as_str()
        .unwrap()
        .contains("中文测试"));
}

#[test]
fn test_validate_placeholders_with_mock_db() {
    let temp = TestDir::new("validate");

    // Create complete sqlite db with items
    let sqlite_path = temp.path().join("zotero.sqlite");
    {
        let conn = rusqlite::Connection::open(&sqlite_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE libraries (libraryID INTEGER PRIMARY KEY, type TEXT, editable INTEGER, filesEditable INTEGER, version INTEGER, storageVersion INTEGER, lastSync INTEGER, archived INTEGER);
            INSERT INTO libraries VALUES (1, 'user', 1, 1, 1, 1, 0, 0);
            CREATE TABLE itemTypes (itemTypeID INTEGER PRIMARY KEY, typeName TEXT, templateItemTypeID INTEGER, display INTEGER);
            INSERT INTO itemTypes VALUES (1, 'journalArticle', NULL, 1);
            CREATE TABLE items (itemID INTEGER PRIMARY KEY, itemTypeID INTEGER, dateAdded TEXT, dateModified TEXT, clientDateModified TEXT, libraryID INTEGER, key TEXT, version INTEGER, synced INTEGER);
            INSERT INTO items VALUES (1, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'REG12345', 1, 1);
            CREATE TABLE fields (fieldID INTEGER PRIMARY KEY, fieldName TEXT, fieldFormatID INTEGER);
            INSERT INTO fields VALUES (1, 'title', 0), (2, 'date', 0), (3, 'DOI', 0);
            CREATE TABLE itemDataValues (valueID INTEGER PRIMARY KEY, value TEXT);
            INSERT INTO itemDataValues VALUES (1, 'Sample Title'), (2, '2023'), (3, '10.1000/sample');
            CREATE TABLE itemData (itemID INTEGER, fieldID INTEGER, valueID INTEGER);
            INSERT INTO itemData VALUES (1, 1, 1), (1, 2, 2), (1, 3, 3);
            CREATE TABLE creators (creatorID INTEGER PRIMARY KEY, firstName TEXT, lastName TEXT, fieldMode INTEGER);
            CREATE TABLE itemCreators (itemID INTEGER, creatorID INTEGER, creatorTypeID INTEGER, orderIndex INTEGER);
            CREATE TABLE tags (tagID INTEGER PRIMARY KEY, name TEXT);
            CREATE TABLE itemTags (itemID INTEGER, tagID INTEGER, type INTEGER);
            CREATE TABLE collections (collectionID INTEGER PRIMARY KEY, collectionName TEXT, parentCollectionID INTEGER, clientDateModified TEXT, libraryID INTEGER, key TEXT, version INTEGER, synced INTEGER);
            CREATE TABLE collectionItems (collectionID INTEGER, itemID INTEGER, orderIndex INTEGER);
            CREATE TABLE itemNotes (itemID INTEGER PRIMARY KEY, parentItemID INTEGER, note TEXT, title TEXT);
            CREATE TABLE itemAttachments (itemID INTEGER PRIMARY KEY, parentItemID INTEGER, linkMode INTEGER, contentType TEXT, charsetID INTEGER, path TEXT, syncState INTEGER, storageModTime INTEGER, storageHash TEXT, lastProcessedModificationTime INTEGER);
            CREATE TABLE itemAnnotations (itemID INTEGER PRIMARY KEY, parentItemID INTEGER, type INTEGER, authorName TEXT, text TEXT, comment TEXT, color TEXT, pageLabel TEXT, sortIndex TEXT, position TEXT, isExternal INTEGER);
            CREATE TABLE savedSearches (savedSearchID INTEGER PRIMARY KEY, savedSearchName TEXT, clientDateModified TEXT, libraryID INTEGER, key TEXT, version INTEGER, synced INTEGER);
            CREATE TABLE savedSearchConditions (savedSearchID INTEGER, searchConditionID INTEGER, condition TEXT, operator TEXT, value TEXT, required INTEGER);
            "#,
        ).unwrap();
    }

    let p_docx = create_test_docx(
        temp.path(),
        "placeholders.docx",
        r#"<w:p><w:r><w:t>Known {{zotero:REG12345}} and missing {{zotero:NOITEM99}}.</w:t></w:r></w:p>"#,
        None,
    );

    let runtime = build_runtime_context(BuildEnvironmentArgs {
        backend: "sqlite",
        data_dir: Some(temp.path().to_str().unwrap()),
        profile_dir: None,
        executable: None,
    });
    let session = load_session_state();

    let rep = validate_placeholders(&runtime, &p_docx, 10, &session).unwrap();
    assert_eq!(rep["ok"], false);
    assert_eq!(rep["valid_count"], 1);
    assert_eq!(rep["missing_count"], 1);
    assert_eq!(rep["missing_keys"][0], "NOITEM99");

    let item0 = &rep["items"][0];
    assert_eq!(item0["key"], "REG12345");
    assert_eq!(item0["title"], "Sample Title");
    assert_eq!(item0["year"], "2023");
    assert_eq!(item0["doi"], "10.1000/sample");
}

#[test]
fn test_malformed_and_corrupt_docx_handling() {
    let temp = TestDir::new("corrupt");

    // 1. Non-existent file
    let non_existent = temp.path().join("does_not_exist.docx");
    let err = inspect_citations(&non_existent, 10).unwrap_err();
    assert!(
        err.to_string().contains("DOCX file not found") || err.to_string().contains("not found")
    );

    // 2. Corrupt file (not a zip archive)
    let corrupt_zip = temp.path().join("not_a_zip.docx");
    std::fs::write(&corrupt_zip, b"this is raw plain text not a zip").unwrap();
    let err = inspect_citations(&corrupt_zip, 10).unwrap_err();
    assert!(err.to_string().contains("Invalid DOCX file"));

    // 3. Valid zip archive but missing word/document.xml
    let missing_doc_xml = temp.path().join("missing_doc.docx");
    {
        let file = File::create(&missing_doc_xml).unwrap();
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        zip.start_file("[Content_Types].xml", options).unwrap();
        zip.write_all(b"<Types></Types>").unwrap();
        zip.finish().unwrap();
    }
    let err = inspect_citations(&missing_doc_xml, 10).unwrap_err();
    assert!(err.to_string().contains("missing word/document.xml"));

    // 4. Invalid XML syntax in word/document.xml
    let invalid_xml = temp.path().join("invalid_xml.docx");
    {
        let file = File::create(&invalid_xml).unwrap();
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(b"not xml at all <<<>>>").unwrap();
        zip.finish().unwrap();
    }
    let err = inspect_citations(&invalid_xml, 10).unwrap_err();
    assert!(!err.to_string().is_empty());
}
