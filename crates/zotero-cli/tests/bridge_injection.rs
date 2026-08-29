// `bridge/mod.rs` (included below via #[path]) resolves the shared WriteOutcome contract via
// `crate::write` -- this re-export provides that path in this test binary's own crate root, the
// same way `bridge` will see it once Slice 6 registers `pub mod bridge;` inside zotero_cli's
// real lib.rs.
pub use zotero_cli::write;

#[path = "../src/bridge/mod.rs"]
mod bridge;

use bridge::templates::*;
use std::collections::HashMap;

#[test]
fn test_d1_injection_resilience_windows_backslashes() {
    let mut fields = HashMap::new();
    fields.insert(
        "title".to_string(),
        r"C:\Users\Alice\Documents\Papers\special\test.pdf".to_string(),
    );
    let js = render_item_update(1, "KEY1", &fields).expect("rendering succeeds");

    // Must be valid JSON string inside JSON.parse("...")
    let line = js.lines().next().unwrap();
    assert!(line.starts_with("const P = JSON.parse("));
    assert!(line.ends_with(");"));

    // Extract the inner JSON string
    let json_literal = &line["const P = JSON.parse(".len()..line.len() - 2];
    let parsed_json_str: String =
        serde_json::from_str(json_literal).expect("outer JSON string literal parses");
    let payload: serde_json::Value =
        serde_json::from_str(&parsed_json_str).expect("inner JSON payload parses");

    assert_eq!(
        payload["fields"]["title"].as_str().unwrap(),
        r"C:\Users\Alice\Documents\Papers\special\test.pdf"
    );
}

#[test]
fn test_d1_injection_resilience_quotes_and_newlines() {
    let mut fields = HashMap::new();
    fields.insert(
        "title".to_string(),
        "Title with 'single' and \"double\" quotes\nand newline\r\nand \t tabs".to_string(),
    );
    let js = render_item_update(1, "KEY2", &fields).expect("rendering succeeds");

    let line = js.lines().next().unwrap();
    let json_literal = &line["const P = JSON.parse(".len()..line.len() - 2];
    let parsed_json_str: String =
        serde_json::from_str(json_literal).expect("outer JSON string literal parses");
    let payload: serde_json::Value =
        serde_json::from_str(&parsed_json_str).expect("inner JSON payload parses");

    assert_eq!(
        payload["fields"]["title"].as_str().unwrap(),
        "Title with 'single' and \"double\" quotes\nand newline\r\nand \t tabs"
    );
}

#[test]
fn test_d1_injection_resilience_script_tags_and_js_interpolation() {
    let add_tags = vec![
        "</script><script>alert('pwned')</script>".to_string(),
        "${process.exit(1)}".to_string(),
        "`+eval('malicious')+`".to_string(),
        "'; Zotero.DB.execute('DROP TABLE items'); //".to_string(),
    ];
    let remove_tags = vec!["`rm -rf /`".to_string()];

    let js = render_item_tag(1, "KEY3", &add_tags, &remove_tags).expect("rendering succeeds");

    let line = js.lines().next().unwrap();
    let json_literal = &line["const P = JSON.parse(".len()..line.len() - 2];
    let parsed_json_str: String = serde_json::from_str(json_literal).expect("outer parses");
    let payload: serde_json::Value = serde_json::from_str(&parsed_json_str).expect("inner parses");

    let parsed_add_tags = payload["addTags"].as_array().unwrap();
    assert_eq!(
        parsed_add_tags[0].as_str().unwrap(),
        "</script><script>alert('pwned')</script>"
    );
    assert_eq!(parsed_add_tags[1].as_str().unwrap(), "${process.exit(1)}");
    assert_eq!(
        parsed_add_tags[2].as_str().unwrap(),
        "`+eval('malicious')+`"
    );
    assert_eq!(
        parsed_add_tags[3].as_str().unwrap(),
        "'; Zotero.DB.execute('DROP TABLE items'); //"
    );
}

#[test]
fn test_d1_injection_resilience_unicode_and_cjk() {
    let name = "糖尿病研究与临床实践 🧬 2026年 [Special Issue]";
    let js = render_collection_create(1, name, None).expect("rendering succeeds");

    let line = js.lines().next().unwrap();
    let json_literal = &line["const P = JSON.parse(".len()..line.len() - 2];
    let parsed_json_str: String = serde_json::from_str(json_literal).expect("outer parses");
    let payload: serde_json::Value = serde_json::from_str(&parsed_json_str).expect("inner parses");

    assert_eq!(payload["name"].as_str().unwrap(), name);
}

#[test]
fn test_d1_render_item_attach_windows_path() {
    let win_path = r"D:\Zotero\Storage\PDFs\2026\08\Paper_Analysis_123.pdf";
    let js = render_item_attach(1, "ITEM_WIN", win_path).expect("render succeeds");

    let line = js.lines().next().unwrap();
    let json_literal = &line["const P = JSON.parse(".len()..line.len() - 2];
    let parsed_json_str: String = serde_json::from_str(json_literal).expect("outer parses");
    let payload: serde_json::Value = serde_json::from_str(&parsed_json_str).expect("inner parses");

    assert_eq!(payload["filePath"].as_str().unwrap(), win_path);
}
