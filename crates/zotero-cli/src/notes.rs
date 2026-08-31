//! Port of `core/notes.py`'s `get_note`/`add_note` (Phase 7 Slice 4). Backend-only: no CLI
//! dispatch wires into this module yet (deferred to a later, serialized Phase 7 integration
//! slice), so this file is not registered in `lib.rs` and is instead `#[path]`-included directly
//! by `tests/notes.rs`, exactly like `pdf_cascade.rs`/`pdf_fetch.rs` before their own CLI slice
//! landed.
//!
//! `get_item_notes`/`_require_connector` (`core/notes.py:15-17,37-39`) are out of this slice's
//! scope (`item notes` already has its own path via `catalog::item_notes`) and are not ported
//! here.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::bridge::JSBridgeClient;
use crate::catalog;
use crate::db::{self, Item};
use crate::error::DomainError;
use crate::runtime::RuntimeContext;
use crate::session::SessionState;
use crate::target;

/// Mirrors `catalog.rs`'s private `session_library_ref` helper. Duplicated rather than widening
/// `catalog.rs`'s public surface for a single call site: this module isn't registered in
/// `lib.rs` yet, and the two implementations must stay behaviorally identical to
/// `catalog.py`'s own `session.get("current_library")` handling either way.
fn session_library_ref(session: &SessionState) -> Option<String> {
    match &session.current_library {
        None => None,
        Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(v) => Some(match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            other => other.to_string(),
        }),
    }
}

/// Mirrors `lib.rs`'s private `library_id_u32` helper (kept there; `lib.rs` is frozen for this
/// slice, and this module isn't registered in it yet).
fn library_id_u32(library_id: i64) -> anyhow::Result<u32> {
    u32::try_from(library_id).map_err(|_| {
        anyhow::Error::from(DomainError::new(format!(
            "library id {library_id} is out of range for the JS Bridge"
        )))
    })
}

/// `get_note()` (`core/notes.py:20-34`). Deliberately does **not** fall back to
/// `session.current_item` the way `catalog::get_item` does for other item lookups -- Python's
/// `get_note` only accepts `ref: str | int | None` and raises immediately when it's `None`,
/// never consulting the session's current item. `session.current_library` is still respected
/// (via the same `resolve_library_id` path every other catalog lookup uses), and reads go
/// straight through the existing safe, read-only SQLite item resolver -- no Connector, no Local
/// API, no Bridge.
pub fn get_note(
    runtime: &RuntimeContext,
    note_ref: Option<&str>,
    session: &SessionState,
) -> anyhow::Result<Item> {
    let Some(note_ref) = note_ref else {
        return Err(DomainError::new("Note reference required").into());
    };
    let library_id = catalog::resolve_library_id(runtime, session_library_ref(session).as_deref())?;
    let item = db::resolve_item(&runtime.environment.sqlite_path, note_ref, library_id)?;
    let Some(item) = item else {
        return Err(DomainError::new(format!("Note not found: {note_ref}")).into());
    };
    if item.type_name != "note" {
        return Err(DomainError::new(format!("Item is not a note: {note_ref}")).into());
    }
    Ok(item)
}

/// `html.escape(s, quote=True)` (Python stdlib): `&` first, then `<`, `>`, `"`, `'`. Order is
/// safe to do per-character here (unlike Python's sequential whole-string `.replace()` calls)
/// because none of the five replacement outputs re-introduce a character this function would
/// otherwise still need to escape.
fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

/// `_html_paragraphs()` (`core/notes.py:42-50`).
fn html_paragraphs(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut paragraphs: Vec<String> = normalized
        .split("\n\n")
        .map(|segment| segment.trim().to_string())
        .filter(|segment| !segment.is_empty())
        .collect();
    if paragraphs.is_empty() {
        // Python's fallback strips the *original* (pre-CRLF-normalization) `text`, not the
        // normalized copy -- behaviorally identical here (this branch only runs when every
        // "\n\n"-delimited segment was itself all-whitespace, which forces the fallback to ""
        // regardless of which copy is stripped), but kept faithful to the source line.
        paragraphs = vec![text.trim().to_string()];
    }
    paragraphs
        .into_iter()
        .map(|paragraph| {
            let escaped = html_escape(&paragraph).replace('\n', "<br/>");
            format!("<p>{escaped}</p>")
        })
        .collect()
}

static CODE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`]+)`").unwrap());
static BOLD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*([^*]+)\*\*").unwrap());
static EM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*([^*]+)\*").unwrap());

/// `_render_markdown_inline()` (`core/notes.py:100-105`): escape first, then backtick code,
/// then `**bold**`, then `*em*`, in that exact order -- an already-escaped `&`/`<`/`>` inside a
/// later match's captured span is never re-escaped, matching Python's sequential `re.sub` calls
/// over the same string.
fn render_markdown_inline(text: &str) -> String {
    let escaped = html_escape(text);
    let escaped = CODE_RE.replace_all(&escaped, "<code>$1</code>");
    let escaped = BOLD_RE.replace_all(&escaped, "<strong>$1</strong>");
    let escaped = EM_RE.replace_all(&escaped, "<em>$1</em>");
    escaped.into_owned()
}

/// `^(#{1,6})\s+(.*)$` (`core/notes.py:85`), applied to one already-rstripped line. Returns the
/// heading level and the content *after* the run of whitespace immediately following the `#`s
/// (matching `\s+`'s greedy consumption), for the caller to `.trim()` the same way Python's
/// `match.group(2).strip()` does. A run of more than 6 `#`s never matches -- the capture group
/// caps at 6, and every character immediately after any smaller backtracked count is still `#`,
/// so `\s+` can never succeed at any group length; this mirrors that exactly rather than
/// treating "level > 6" as its own case.
fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &line[hashes..];
    let ws_chars = rest.chars().take_while(|c| c.is_whitespace()).count();
    if ws_chars == 0 {
        return None;
    }
    let ws_bytes: usize = rest.chars().take(ws_chars).map(|c| c.len_utf8()).sum();
    Some((hashes, &rest[ws_bytes..]))
}

/// `_simple_markdown_to_safe_html()` (`core/notes.py:53-97`).
fn simple_markdown_to_safe_html(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut rendered = String::new();
    let mut in_list = false;
    let mut paragraph: Vec<String> = Vec::new();

    fn flush_paragraph(rendered: &mut String, paragraph: &mut Vec<String>) {
        if paragraph.is_empty() {
            return;
        }
        let joined = paragraph.join(" ");
        rendered.push_str(&format!("<p>{}</p>", render_markdown_inline(&joined)));
        paragraph.clear();
    }
    fn flush_list(rendered: &mut String, in_list: &mut bool) {
        if *in_list {
            rendered.push_str("</ul>");
            *in_list = false;
        }
    }

    for raw_line in normalized.split('\n') {
        let line = raw_line.trim_end();
        if line.trim().is_empty() {
            flush_paragraph(&mut rendered, &mut paragraph);
            flush_list(&mut rendered, &mut in_list);
            continue;
        }
        if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            flush_paragraph(&mut rendered, &mut paragraph);
            if !in_list {
                rendered.push_str("<ul>");
                in_list = true;
            }
            rendered.push_str(&format!("<li>{}</li>", render_markdown_inline(rest.trim())));
            continue;
        }
        if let Some((level, content)) = parse_heading(line) {
            flush_paragraph(&mut rendered, &mut paragraph);
            flush_list(&mut rendered, &mut in_list);
            let inline = render_markdown_inline(content.trim());
            rendered.push_str(&format!("<h{level}>{inline}</h{level}>"));
            continue;
        }
        flush_list(&mut rendered, &mut in_list);
        paragraph.push(line.trim().to_string());
    }
    flush_paragraph(&mut rendered, &mut paragraph);
    flush_list(&mut rendered, &mut in_list);
    rendered
}

/// `_normalize_note_html()` (`core/notes.py:108-116`). Python reassigns its local `fmt` to
/// lowercase *before* dispatching -- so the "Unsupported note format" error message (and this
/// function's own dispatch) both use the lowered value; the caller's own `fmt` variable
/// (`add_note`'s, used for the result's `"format"` field) is untouched, since Python passes it
/// by value into this nested function.
fn normalize_note_html(content: &str, fmt: &str) -> anyhow::Result<String> {
    let lowered = fmt.to_lowercase();
    match lowered.as_str() {
        "html" => Ok(content.to_string()),
        "markdown" => Ok(simple_markdown_to_safe_html(content)),
        "text" => Ok(html_paragraphs(content)),
        _ => Err(DomainError::new(format!("Unsupported note format: {lowered}")).into()),
    }
}

/// Content source for `add_note`, resolved by [`resolve_note_input`]. An enum makes "exactly
/// one of text or file" a structural invariant for any caller already holding a `NoteInput`;
/// [`resolve_note_input`] is the reusable validation helper for callers (e.g. a future CLI
/// layer) that instead start from two independent `Option`s.
#[derive(Debug)]
pub enum NoteInput<'a> {
    Text(&'a str),
    File(&'a str),
}

/// `if (text is None and file_path is None) or (text is not None and file_path is not None):
/// raise RuntimeError("Provide exactly one of \`text\` or \`file_path\`")` (`core/notes.py:128-129`).
/// Clap-specific mutual-exclusion argument validation is deferred to the later CLI integration
/// slice; this gives any core caller the same rule and exact message in the meantime.
pub fn resolve_note_input<'a>(
    text: Option<&'a str>,
    file_path: Option<&'a str>,
) -> anyhow::Result<NoteInput<'a>> {
    match (text, file_path) {
        (Some(text), None) => Ok(NoteInput::Text(text)),
        (None, Some(file_path)) => Ok(NoteInput::File(file_path)),
        _ => Err(DomainError::new("Provide exactly one of `text` or `file_path`").into()),
    }
}

/// Python-compatible `add_note` result projection (`core/notes.py:164-172`). Exactly these seven
/// fields, in this order -- no additional Bridge/internal fields.
#[derive(Debug, Clone, Serialize)]
pub struct NoteAddResult {
    pub action: &'static str,
    pub key: Option<String>,
    #[serde(rename = "itemID")]
    pub item_id: Option<i64>,
    #[serde(rename = "parentItemKey")]
    pub parent_item_key: String,
    #[serde(rename = "parentItemID")]
    pub parent_item_id: i64,
    pub format: String,
    #[serde(rename = "notePreview")]
    pub note_preview: String,
}

/// `add_note()` (`core/notes.py:119-172`).
///
/// Targeting: `item_ref` is resolved through `target::resolve_item` (the same live-first path
/// every other write command uses), so `session.current_library` scoping applies; rejects a
/// `note`/`attachment`/`annotation` parent with Python's exact message, and no further ("must be
/// top-level") restriction beyond that.
///
/// The parent is resolved through the *same live Zotero runtime* this note is about to be
/// written into, not through SQLite. A running Zotero holds an exclusive lock on its WAL-mode
/// database, so the previous `catalog::get_item` call made this command fail during target
/// lookup in exactly the situation it is designed for -- Zotero up, Bridge healthy.
///
/// Transport: Bridge-only, exactly one `saveTx()` mutation attempt (see
/// `JSBridgeClient::note_add`'s own doc comment) -- no Connector fallback, no Local API
/// fallback, no direct SQLite write, and no automatic re-verification read after the Bridge call
/// resolves (the saved note's own `key`/`itemID`, read back by the Bridge call itself inside
/// Zotero's live runtime, is treated as authoritative).
pub fn add_note(
    runtime: &RuntimeContext,
    bridge: &JSBridgeClient,
    item_ref: &str,
    input: NoteInput,
    fmt: Option<&str>,
    session: &SessionState,
) -> anyhow::Result<NoteAddResult> {
    let fmt = fmt.unwrap_or("text");

    let parent_item = target::resolve_item(
        runtime,
        bridge,
        Some(item_ref),
        session,
        target::Prefer::Bridge,
    )?;
    if matches!(
        parent_item.item_type.as_str(),
        "note" | "attachment" | "annotation"
    ) {
        return Err(DomainError::new(
            "Child notes can only be attached to top-level bibliographic items",
        )
        .into());
    }

    let content = match input {
        NoteInput::Text(text) => text.to_string(),
        NoteInput::File(file_path) => {
            let expanded = crate::paths::expand_user_path(file_path);
            std::fs::read_to_string(&expanded)?
        }
    };

    let note_html = normalize_note_html(&content, fmt)?;
    let library_id = library_id_u32(parent_item.library_id)?;
    let data = bridge.note_add(library_id, &parent_item.key, &note_html)?;

    let note_text = db::note_html_to_text(Some(&note_html));
    let note_preview = db::note_preview(&note_text);

    Ok(NoteAddResult {
        action: "note_add",
        key: data.get("key").and_then(Value::as_str).map(str::to_string),
        item_id: data.get("itemID").and_then(Value::as_i64),
        parent_item_key: parent_item.key,
        parent_item_id: parent_item.item_id,
        format: fmt.to_string(),
        note_preview,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── TEXT normalization ──────────────────────────────────────────────

    #[test]
    fn html_paragraphs_escapes_html() {
        assert_eq!(
            html_paragraphs("Tom & Jerry <b>bold</b> \"quoted\" 'single'"),
            "<p>Tom &amp; Jerry &lt;b&gt;bold&lt;/b&gt; &quot;quoted&quot; &#x27;single&#x27;</p>"
        );
    }

    #[test]
    fn html_paragraphs_normalizes_crlf() {
        assert_eq!(
            html_paragraphs("line one\r\nline two"),
            "<p>line one<br/>line two</p>"
        );
    }

    #[test]
    fn html_paragraphs_normalizes_cr() {
        assert_eq!(
            html_paragraphs("line one\rline two"),
            "<p>line one<br/>line two</p>"
        );
    }

    #[test]
    fn html_paragraphs_splits_on_blank_line() {
        assert_eq!(
            html_paragraphs("first paragraph\n\nsecond paragraph"),
            "<p>first paragraph</p><p>second paragraph</p>"
        );
    }

    #[test]
    fn html_paragraphs_intra_paragraph_br() {
        assert_eq!(
            html_paragraphs("line one\nline two\nline three"),
            "<p>line one<br/>line two<br/>line three</p>"
        );
    }

    #[test]
    fn html_paragraphs_trims_segments() {
        assert_eq!(
            html_paragraphs("  padded  \n\n  also padded  "),
            "<p>padded</p><p>also padded</p>"
        );
    }

    #[test]
    fn html_paragraphs_empty_input_is_empty_p() {
        assert_eq!(html_paragraphs(""), "<p></p>");
    }

    #[test]
    fn html_paragraphs_whitespace_only_input_is_empty_p() {
        assert_eq!(html_paragraphs("   \n\n\t\n  "), "<p></p>");
    }

    // ── MARKDOWN normalization ──────────────────────────────────────────

    #[test]
    fn markdown_headings_h1_through_h6() {
        for level in 1..=6usize {
            let hashes = "#".repeat(level);
            let input = format!("{hashes} Heading {level}");
            let expected = format!("<h{level}>Heading {level}</h{level}>");
            assert_eq!(
                simple_markdown_to_safe_html(&input),
                expected,
                "level={level}"
            );
        }
    }

    #[test]
    fn markdown_seven_hashes_is_not_a_heading() {
        // Regex `^(#{1,6})\s+(.*)$` can never match 7+ leading `#`s followed by more `#`s: the
        // capped group always leaves another `#` immediately before `\s+`. Falls through to a
        // plain paragraph instead.
        assert_eq!(
            simple_markdown_to_safe_html("####### not a heading"),
            "<p>####### not a heading</p>"
        );
    }

    #[test]
    fn markdown_dash_list() {
        assert_eq!(
            simple_markdown_to_safe_html("- one\n- two"),
            "<ul><li>one</li><li>two</li></ul>"
        );
    }

    #[test]
    fn markdown_star_list() {
        assert_eq!(
            simple_markdown_to_safe_html("* one\n* two"),
            "<ul><li>one</li><li>two</li></ul>"
        );
    }

    #[test]
    fn markdown_indented_list_marker_is_not_a_list() {
        // `line.startswith(("- ", "* "))` runs against the rstripped-but-not-lstripped line, so
        // leading whitespace before the marker disqualifies it.
        assert_eq!(
            simple_markdown_to_safe_html("  - not a list"),
            "<p>- not a list</p>"
        );
    }

    #[test]
    fn markdown_paragraph_lines_joined_with_space() {
        assert_eq!(
            simple_markdown_to_safe_html("first line\nsecond line"),
            "<p>first line second line</p>"
        );
    }

    #[test]
    fn markdown_inline_code() {
        assert_eq!(
            simple_markdown_to_safe_html("use `code` here"),
            "<p>use <code>code</code> here</p>"
        );
    }

    #[test]
    fn markdown_bold() {
        assert_eq!(
            simple_markdown_to_safe_html("**bold** text"),
            "<p><strong>bold</strong> text</p>"
        );
    }

    #[test]
    fn markdown_emphasis() {
        assert_eq!(
            simple_markdown_to_safe_html("*em* text"),
            "<p><em>em</em> text</p>"
        );
    }

    #[test]
    fn markdown_escaping() {
        assert_eq!(
            simple_markdown_to_safe_html("Tom & Jerry <script>"),
            "<p>Tom &amp; Jerry &lt;script&gt;</p>"
        );
    }

    #[test]
    fn markdown_combination_matches_python_shape() {
        let input = "# Title\n\nSome **bold** and *em* and `code`.\n\n- item one\n- item two\n\nTrailing paragraph.";
        let expected = "<h1>Title</h1><p>Some <strong>bold</strong> and <em>em</em> and <code>code</code>.</p><ul><li>item one</li><li>item two</li></ul><p>Trailing paragraph.</p>";
        assert_eq!(simple_markdown_to_safe_html(input), expected);
    }

    // ── HTML format ──────────────────────────────────────────────────────

    #[test]
    fn html_format_passthrough_exact() {
        let raw = "<p>Already <b>HTML</b> &amp; unescaped as-is</p>";
        assert_eq!(normalize_note_html(raw, "html").unwrap(), raw);
    }

    // ── format dispatch / output quirk ───────────────────────────────────

    #[test]
    fn normalize_note_html_unsupported_format_uses_lowered_fmt_in_message() {
        let err = normalize_note_html("x", "RTF").unwrap_err();
        assert_eq!(err.to_string(), "Unsupported note format: rtf");
    }

    #[test]
    fn normalize_note_html_dispatch_is_case_insensitive() {
        assert_eq!(normalize_note_html("<b>x</b>", "HTML").unwrap(), "<b>x</b>");
        assert_eq!(normalize_note_html("hi", "Text").unwrap(), "<p>hi</p>");
        assert_eq!(
            normalize_note_html("# h", "MarkDown").unwrap(),
            "<h1>h</h1>"
        );
    }

    // ── exactly-one-of validation helper ─────────────────────────────────

    #[test]
    fn resolve_note_input_requires_exactly_one() {
        assert!(resolve_note_input(None, None).is_err());
        assert!(resolve_note_input(Some("hi"), Some("/tmp/x")).is_err());
        assert!(matches!(
            resolve_note_input(Some("hi"), None).unwrap(),
            NoteInput::Text("hi")
        ));
        assert!(matches!(
            resolve_note_input(None, Some("/tmp/x")).unwrap(),
            NoteInput::File("/tmp/x")
        ));
    }

    #[test]
    fn resolve_note_input_error_message_matches_python() {
        let err = resolve_note_input(None, None).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Provide exactly one of `text` or `file_path`"
        );
    }
}
