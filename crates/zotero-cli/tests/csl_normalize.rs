#[path = "../src/csl.rs"]
mod csl;
#[path = "../src/import_normalization.rs"]
mod import_normalization;

use serde_json::{json, Value};

#[test]
fn csl_article_metadata_matches_python_shape_and_order() {
    let input = json!({
        "type": "article-journal",
        "title": "Hello CSL",
        "DOI": "10.1/csl",
        "URL": "HTTPS://EXAMPLE.test/Paper",
        "abstract": "Summary",
        "container-title": "Nature Methods",
        "volume": 12,
        "issue": 3,
        "page": "1-9",
        "publisher": "Publisher",
        "language": "en",
        "ISSN": ["1111", "2222"],
        "ISBN": ["isbn1"],
        "author": [
            {"family": "Doe", "given": "Jane"},
            {"literal": "Team Name"},
            "ignored"
        ],
        "issued": {"date-parts": [[2024, 1, 2, null]]},
        "keyword": ["alpha", 3, "beta"]
    });
    let item = csl::csl_item_to_connector(input.as_object().unwrap(), 2);
    assert_eq!(
        Value::Object(item),
        json!({
            "itemType": "journalArticle",
            "title": "Hello CSL",
            "id": "cli-anything-csl-2",
            "DOI": "10.1/csl",
            "url": "HTTPS://EXAMPLE.test/Paper",
            "abstractNote": "Summary",
            "publicationTitle": "Nature Methods",
            "volume": "12",
            "issue": "3",
            "pages": "1-9",
            "publisher": "Publisher",
            "language": "en",
            "ISSN": "1111,2222",
            "ISBN": "isbn1",
            "date": "2024-1-2",
            "creators": [
                {"creatorType": "author", "firstName": "Jane", "lastName": "Doe"},
                {"creatorType": "author", "name": "Team Name"}
            ],
            "tags": [{"tag": "alpha"}, {"tag": "beta"}]
        })
    );
}

#[test]
fn csl_missing_optional_fields_get_python_defaults() {
    let input = json!({"type": "dataset", "container-title": "Fallback Title", "editor": [{"family": "Ed"}]});
    let item = csl::csl_item_to_connector(input.as_object().unwrap(), 1);
    assert_eq!(item.get("itemType"), Some(&json!("document")));
    assert_eq!(item.get("title"), Some(&json!("Fallback Title")));
    assert_eq!(
        item.get("creators"),
        Some(&json!([{"creatorType": "author", "firstName": "", "lastName": "Ed"}]))
    );
    assert!(!item.contains_key("DOI"));
    assert!(!item.contains_key("date"));
}

#[test]
fn normalize_crossref_work_uses_first_title_container_and_published_print_date() {
    let payload = json!({
        "message": {
            "title": ["Crossref Title"],
            "DOI": "10.5555/ABC",
            "URL": "https://doi.org/10.5555/ABC",
            "container-title": ["Crossref Journal"],
            "volume": "7",
            "issue": "2",
            "page": "10-20",
            "author": [{"family": "Roe", "given": "Jan"}, "ignored"],
            "published-print": {"date-parts": [[2020, 5]]}
        }
    });
    let (items, format) = import_normalization::normalize_import_json_payload(&payload).unwrap();
    assert_eq!(format, "crossref");
    assert_eq!(
        items,
        vec![json!({
            "itemType": "journalArticle",
            "title": "Crossref Title",
            "id": "cli-anything-csl-1",
            "DOI": "10.5555/ABC",
            "url": "https://doi.org/10.5555/ABC",
            "publicationTitle": "Crossref Journal",
            "volume": "7",
            "issue": "2",
            "pages": "10-20",
            "date": "2020-5",
            "creators": [{"creatorType": "author", "firstName": "Jan", "lastName": "Roe"}]
        })]
    );
}

#[test]
fn normalize_connector_and_fallback_payloads_preserve_python_defaults() {
    let (items, format) = import_normalization::normalize_import_json_payload(
        &json!({"items": [{"itemType": "book", "title": "Book"}]}),
    )
    .unwrap();
    assert_eq!(format, "connector");
    assert_eq!(
        items,
        vec![json!({"itemType": "book", "title": "Book", "id": "cli-anything-zotero-1"})]
    );

    let (items, format) =
        import_normalization::normalize_import_json_payload(&json!([{"title": "Loose"}])).unwrap();
    assert_eq!(format, "connector-fallback");
    assert_eq!(
        items,
        vec![
            json!({"title": "Loose", "itemType": "journalArticle", "id": "cli-anything-zotero-1"})
        ]
    );

    let (items, format) = import_normalization::normalize_import_json_payload(&json!([])).unwrap();
    assert_eq!(format, "empty");
    assert!(items.is_empty());
}

#[test]
fn normalize_csl_array_and_rejects_malformed_input_like_python() {
    let (items, format) = import_normalization::normalize_import_json_payload(&json!([
        {"type": "article-journal", "title": "Paper", "author": [], "issued": {"raw": "2024"}}
    ]))
    .unwrap();
    assert_eq!(format, "csl-json");
    assert_eq!(items[0]["date"], json!("2024"));

    let err = import_normalization::normalize_import_json_payload(&json!({"unexpected": true}))
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        "JSON import expects an array, {items:[...]}, CSL object, or Crossref work"
    );
    let err =
        import_normalization::normalize_import_json_payload(&json!([{"itemType": "book"}, 3]))
            .unwrap_err()
            .to_string();
    assert_eq!(err, "JSON import item 2 is not an object");
}

#[test]
fn doi_and_bibtex_helpers_match_python_edge_cases() {
    assert_eq!(
        import_normalization::normalize_doi(Some(" https://doi.org/10.1038/s41592-024-02201-0. ")),
        "10.1038/s41592-024-02201-0"
    );
    assert_eq!(
        import_normalization::normalize_doi(Some("doi: 10.1/abc),;")),
        "10.1/abc"
    );
    assert_eq!(import_normalization::normalize_doi(None), "");

    let content = "@article{a, title={A},}\n\n@book{b, title={B},}\n";
    assert_eq!(import_normalization::count_bibtex_entries(content), 2);
    assert_eq!(
        import_normalization::split_bibtex_entries(content),
        vec!["@article{a, title={A},}", "@book{b, title={B},}"]
    );
    assert_eq!(
        import_normalization::split_bibtex_entries("  plain text  "),
        vec!["plain text"]
    );
    assert!(import_normalization::split_bibtex_entries("   ").is_empty());
}

#[test]
fn attachment_descriptors_and_inline_plans_are_pure_python_parity() {
    let descriptor = import_normalization::normalize_attachment_descriptor(
        &json!({"path": " x.pdf ", "title": "", "delay_ms": "5", "timeout": "6"}),
        "JSON import item 1",
        "attachment 1",
        0,
        60,
    )
    .unwrap();
    assert_eq!(descriptor.source_type, "file");
    assert_eq!(descriptor.source, "x.pdf");
    assert_eq!(descriptor.title, "PDF");
    assert_eq!(descriptor.delay_ms, 5);
    assert_eq!(descriptor.timeout, 6);

    let (items, plans) = import_normalization::extract_inline_attachment_plans(
        &[
            json!({"itemType": "journalArticle", "title": "T", "attachments": [{"path": "a.pdf"}]}),
            json!({"itemType": "book", "title": "B", "attachments": null}),
        ],
        0,
        60,
    )
    .unwrap();
    assert_eq!(
        items,
        vec![
            json!({"itemType": "journalArticle", "title": "T"}),
            json!({"itemType": "book", "title": "B"})
        ]
    );
    assert_eq!(
        plans,
        vec![
            json!({"index": 0, "attachments": [{"source_type": "file", "source": "a.pdf", "title": "PDF", "delay_ms": 0, "timeout": 60}]})
        ]
    );

    let err = import_normalization::normalize_attachment_descriptor(
        &json!({"path": "a.pdf", "url": "https://example.test/a.pdf"}),
        "JSON import item 1",
        "attachment 1",
        0,
        60,
    )
    .unwrap_err()
    .to_string();
    assert_eq!(
        err,
        "JSON import item 1 attachment 1 must include exactly one of `path` or `url`"
    );
}

#[test]
fn attachment_descriptor_coercions_follow_python_int_and_str() {
    let descriptor = import_normalization::normalize_attachment_descriptor(
        &json!({"path": 5, "title": true, "delay_ms": true, "timeout": 1.9}),
        "manifest entry 1",
        "attachment 1",
        0,
        60,
    )
    .unwrap();
    assert_eq!(descriptor.source, "5");
    assert_eq!(descriptor.title, "True");
    assert_eq!(descriptor.delay_ms, 1);
    assert_eq!(descriptor.timeout, 1);

    let descriptor = import_normalization::normalize_attachment_descriptor(
        &json!({"url": " https://example.test/a.pdf ", "delay_ms": " 5 ", "timeout": " 6 "}),
        "manifest entry 1",
        "attachment 1",
        0,
        60,
    )
    .unwrap();
    assert_eq!(descriptor.source_type, "url");
    assert_eq!(descriptor.source, "https://example.test/a.pdf");
    assert_eq!(descriptor.delay_ms, 5);
    assert_eq!(descriptor.timeout, 6);
}
