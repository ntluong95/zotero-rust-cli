//! Port of `zotero_cli.py`'s output dispatch (`_json_text`, `emit`,
//! `_safe_text_for_stdout`).
//!
//! Python's `ensure_ascii=True` fallback and `_safe_text_for_stdout`'s
//! `backslashreplace` re-encoding exist because Python's text-mode stdout
//! is bound to a console codepage (e.g. cp1252 on some Windows consoles)
//! and can raise `UnicodeEncodeError` on non-representable characters.
//! Rust's stdout has no equivalent failure mode: `println!`/`write!`
//! write raw UTF-8 bytes directly, and the terminal — not the process —
//! decides how to render them. So there is no fallback branch to port;
//! this always emits `ensure_ascii=False`-equivalent (raw UTF-8) output.

use serde_json::Value;

/// `_json_text()` (`zotero_cli.py:173-177`): `json.dumps(data, ensure_ascii=False, indent=2)`.
pub fn json_text(data: &Value) -> String {
    serde_json::to_string_pretty(data).unwrap_or_else(|_| "null".to_string())
}

/// `emit()` (`zotero_cli.py:292-314`).
///
/// Human-mode output for a *list* is deliberately **not** a single valid
/// JSON document: each dict element is printed via its own `json_text`
/// call, newline-separated, with no enclosing `[`/`]` or commas — this
/// exactly matches the Python reference rather than "fixing" it, since the
/// plan requires Exact-class commands to be byte-identical, not merely
/// well-formed.
pub fn emit(json_mode: bool, data: &Value) {
    crate::audit::maybe_audit(data);
    if json_mode {
        println!("{}", json_text(data));
        return;
    }
    match data {
        Value::String(s) => println!("{s}"),
        Value::Array(items) => {
            if items.is_empty() {
                println!("[]");
                return;
            }
            for item in items {
                match item {
                    Value::Object(_) => println!("{}", json_text(item)),
                    other => println!("{}", value_as_text(other)),
                }
            }
        }
        Value::Object(_) => println!("{}", json_text(data)),
        other => println!("{}", value_as_text(other)),
    }
}

fn value_as_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "None".to_string(),
        Value::Bool(b) => {
            if *b {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        other => other.to_string(),
    }
}
