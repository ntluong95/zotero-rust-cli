//! Integration tests for DOCX render-citations and build_working_docx.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use zotero_cli::docx::package::read_document_xml;
use zotero_cli::docx::static_render::{combined_citation, plain_text};
use zotero_cli::docx::working::build_working_docx;
use zotero_cli::docx::xml::parse_xml;
use zotero_cli::runtime::{build_runtime_context, BuildEnvironmentArgs};
use zotero_cli::session::load_session_state;

struct TestDir(PathBuf);
impl TestDir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "zotero-docx-render-test-{}-{}",
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

fn create_test_docx(dir: &Path, filename: &str, document_xml_body: &str) -> PathBuf {
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

    zip.finish().unwrap();
    path
}

fn setup_mock_db(dir: &Path) -> PathBuf {
    let sqlite_path = dir.join("zotero.sqlite");
    let conn = rusqlite::Connection::open(&sqlite_path).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE libraries (libraryID INTEGER PRIMARY KEY, type TEXT, editable INTEGER, filesEditable INTEGER, version INTEGER, storageVersion INTEGER, lastSync INTEGER, archived INTEGER);
        INSERT INTO libraries VALUES (1, 'user', 1, 1, 1, 1, 0, 0);
        CREATE TABLE itemTypes (itemTypeID INTEGER PRIMARY KEY, typeName TEXT, templateItemTypeID INTEGER, display INTEGER);
        INSERT INTO itemTypes VALUES (1, 'journalArticle', NULL, 1);
        CREATE TABLE items (itemID INTEGER PRIMARY KEY, itemTypeID INTEGER, dateAdded TEXT, dateModified TEXT, clientDateModified TEXT, libraryID INTEGER, key TEXT, version INTEGER, synced INTEGER);
        INSERT INTO items VALUES (1, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'REG12345', 1, 1);
        INSERT INTO items VALUES (2, 1, '2026-01-01', '2026-01-01', '2026-01-01', 1, 'REG67890', 1, 1);
        CREATE TABLE fields (fieldID INTEGER PRIMARY KEY, fieldName TEXT, fieldFormatID INTEGER);
        INSERT INTO fields VALUES (1, 'title', 0), (2, 'date', 0), (3, 'DOI', 0);
        CREATE TABLE itemDataValues (valueID INTEGER PRIMARY KEY, value TEXT);
        INSERT INTO itemDataValues VALUES (1, 'Sample Title 1'), (2, '2023'), (3, '10.1000/sample1');
        INSERT INTO itemDataValues VALUES (4, 'Sample Title 2'), (5, '2024'), (6, '10.1000/sample2');
        CREATE TABLE itemData (itemID INTEGER, fieldID INTEGER, valueID INTEGER);
        INSERT INTO itemData VALUES (1, 1, 1), (1, 2, 2), (1, 3, 3);
        INSERT INTO itemData VALUES (2, 1, 4), (2, 2, 5), (2, 3, 6);
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
    sqlite_path
}

#[test]
fn test_combined_citation_formatting() {
    assert_eq!(combined_citation(&[]), "");
    assert_eq!(
        combined_citation(&["(Doe, 2020)".to_string()]),
        "(Doe, 2020)"
    );
    assert_eq!(
        combined_citation(&["(Doe, 2020)".to_string(), "(Smith, 2021)".to_string()]),
        "(Doe, 2020; Smith, 2021)"
    );
    assert_eq!(
        combined_citation(&["[1]".to_string(), "[2]".to_string()]),
        "[1]; [2]"
    );
}

#[test]
fn test_plain_text_html_stripping() {
    let raw = "<div>Smith &amp; <i>Jones</i> (2020). &quot;A &lt;B&gt; Study&quot;.</div>";
    assert_eq!(plain_text(raw), "Smith & Jones (2020). \"A <B> Study\".");
}

#[test]
fn test_build_working_docx_generates_valid_hyperlinks_and_rels() {
    let temp = TestDir::new("render");
    setup_mock_db(temp.path());

    let src = create_test_docx(
        temp.path(),
        "input.docx",
        "<w:p><w:r><w:t>Discussion {{zotero:REG12345, REG67890}}.</w:t></w:r></w:p>",
    );
    let out = temp.path().join("working.docx");

    let runtime = build_runtime_context(BuildEnvironmentArgs {
        backend: "sqlite",
        data_dir: Some(temp.path().to_str().unwrap()),
        profile_dir: None,
        executable: None,
    });
    let session = load_session_state();

    let result = build_working_docx(&runtime, &src, &out, &session, true, "auto").unwrap();
    assert_eq!(result["ok"], true);
    assert_eq!(result["placeholder_count"], 1);
    assert_eq!(result["citation_count"], 2);

    // Verify output DOCX XML
    let doc_bytes = read_document_xml(&out).unwrap();
    let root = parse_xml(&doc_bytes).unwrap();
    let hyperlinks = root.find_all("w:hyperlink");
    assert!(hyperlinks.len() >= 2); // 1 citation + 1 bibliography placeholder
}
