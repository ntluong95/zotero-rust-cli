//! Port of `utils/zotero_sqlite.py`'s read paths needed by the vertical
//! slice: libraries, collections, and the item base-select/normalize
//! pipeline shared by `item list`/`item get`/`item find`. Every SQL string
//! here is copied verbatim from the Python source (see per-function doc
//! comments for line references) — do not "clean up" the queries without
//! re-checking against golden fixtures, since SQLite column order feeds
//! JSON key order.

use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use rusqlite::{Connection, OpenFlags, Row};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::DomainError;

const NOTE_PREVIEW_LENGTH: usize = 160;

#[derive(Debug, Clone, Serialize)]
pub struct Creator {
    #[serde(rename = "creatorID")]
    pub creator_id: i64,
    #[serde(rename = "firstName")]
    pub first_name: Option<String>,
    #[serde(rename = "lastName")]
    pub last_name: Option<String>,
    #[serde(rename = "fieldMode")]
    pub field_mode: i64,
    #[serde(rename = "creatorTypeID")]
    pub creator_type_id: i64,
    #[serde(rename = "orderIndex")]
    pub order_index: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Tag {
    #[serde(rename = "tagID")]
    pub tag_id: i64,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    #[serde(rename = "itemID")]
    pub item_id: i64,
    pub key: String,
    #[serde(rename = "libraryID")]
    pub library_id: i64,
    #[serde(rename = "itemTypeID")]
    pub item_type_id: i64,
    #[serde(rename = "typeName")]
    pub type_name: String,
    #[serde(rename = "dateAdded")]
    pub date_added: String,
    #[serde(rename = "dateModified")]
    pub date_modified: String,
    pub version: i64,
    pub title: String,
    #[serde(rename = "DOI")]
    pub doi: String,
    pub date: Option<String>,
    #[serde(rename = "hasPdf")]
    pub has_pdf: bool,
    #[serde(rename = "noteParentItemID")]
    pub note_parent_item_id: Option<i64>,
    #[serde(rename = "noteContent")]
    pub note_content: Option<String>,
    #[serde(rename = "attachmentParentItemID")]
    pub attachment_parent_item_id: Option<i64>,
    #[serde(rename = "annotationParentItemID")]
    pub annotation_parent_item_id: Option<i64>,
    #[serde(rename = "annotationText")]
    pub annotation_text: Option<String>,
    #[serde(rename = "annotationComment")]
    pub annotation_comment: Option<String>,
    #[serde(rename = "linkMode")]
    pub link_mode: Option<i64>,
    #[serde(rename = "contentType")]
    pub content_type: Option<String>,
    #[serde(rename = "attachmentPath")]
    pub attachment_path: Option<String>,
    pub fields: Map<String, Value>,
    pub creators: Vec<Creator>,
    pub tags: Vec<Tag>,
    #[serde(rename = "isAttachment")]
    pub is_attachment: bool,
    #[serde(rename = "isNote")]
    pub is_note: bool,
    #[serde(rename = "isAnnotation")]
    pub is_annotation: bool,
    #[serde(rename = "parentItemID")]
    pub parent_item_id: Option<i64>,
    #[serde(rename = "noteText")]
    pub note_text: String,
    #[serde(rename = "notePreview")]
    pub note_preview: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Collection {
    #[serde(rename = "collectionID")]
    pub collection_id: i64,
    pub key: String,
    #[serde(rename = "collectionName")]
    pub collection_name: String,
    #[serde(rename = "parentCollectionID")]
    pub parent_collection_id: Option<i64>,
    #[serde(rename = "libraryID")]
    pub library_id: i64,
    pub version: i64,
    #[serde(rename = "itemCount")]
    pub item_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Library {
    #[serde(rename = "libraryID")]
    pub library_id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub editable: i64,
    #[serde(rename = "filesEditable")]
    pub files_editable: i64,
    pub version: i64,
    #[serde(rename = "storageVersion")]
    pub storage_version: i64,
    #[serde(rename = "lastSync")]
    pub last_sync: Option<i64>,
    pub archived: i64,
}

/// `connect_readonly()` (`zotero_sqlite.py:25-32`).
pub fn connect_readonly(sqlite_path: &Path) -> anyhow::Result<Connection> {
    if !sqlite_path.exists() {
        return Err(DomainError::new(format!(
            "Zotero database not found: {}",
            sqlite_path.display()
        ))
        .into());
    }
    // Matches Python's `path.as_posix()`: SQLite's URI spec requires
    // `\` -> `/` on Windows before building a `file:` URI, and
    // `to_string_lossy()` alone preserves native backslash separators
    // there, which silently fails to open on that platform. Deliberately
    // not using `std::fs::canonicalize` here: on Windows it prepends the
    // `\\?\` extended-length prefix, which SQLite's URI parser does not
    // accept — that would trade this bug for a different one.
    let posix_path = sqlite_path.to_string_lossy().replace('\\', "/");
    let uri = format!("file:{posix_path}?mode=ro&immutable=1");
    let conn = Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.busy_timeout(Duration::from_secs_f64(1.0))?;
    Ok(conn)
}

/// `_is_numeric_ref()` (`zotero_sqlite.py:48-53`): Python's `int(str(value))`
/// trims surrounding whitespace before parsing, so this must too — a bare
/// `.parse()` would misclassify `" 5 "` as a non-numeric key ref.
fn is_numeric_ref(value: &str) -> bool {
    value.trim().parse::<i64>().is_ok()
}

/// `normalize_library_ref()` (`zotero_sqlite.py:56-65`).
pub fn normalize_library_ref(library_ref: &str) -> anyhow::Result<i64> {
    let text = library_ref.trim();
    if text.is_empty() {
        return Err(DomainError::new("Library reference must not be empty").into());
    }
    let upper = text.to_uppercase();
    if let Some(rest) = upper.strip_prefix('L') {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return Ok(rest.parse()?);
        }
    }
    if text.chars().all(|c| c.is_ascii_digit()) && !text.is_empty() {
        return Ok(text.parse()?);
    }
    Err(DomainError::new(format!("Unsupported library reference: {library_ref}")).into())
}

fn library_from_row(row: &Row) -> rusqlite::Result<Library> {
    Ok(Library {
        library_id: row.get("libraryID")?,
        kind: row.get("type")?,
        editable: row.get("editable")?,
        files_editable: row.get("filesEditable")?,
        version: row.get("version")?,
        storage_version: row.get("storageVersion")?,
        last_sync: row.get("lastSync")?,
        archived: row.get("archived")?,
    })
}

/// `fetch_libraries()` (`zotero_sqlite.py:105-114`).
pub fn fetch_libraries(sqlite_path: &Path) -> anyhow::Result<Vec<Library>> {
    let conn = connect_readonly(sqlite_path)?;
    let mut stmt = conn.prepare(
        "SELECT libraryID, type, editable, filesEditable, version, storageVersion, lastSync, archived
         FROM libraries
         ORDER BY libraryID",
    )?;
    let rows = stmt.query_map([], library_from_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// `resolve_library()` (`zotero_sqlite.py:117-128`).
pub fn resolve_library(sqlite_path: &Path, library_ref: &str) -> anyhow::Result<Option<Library>> {
    let library_id = normalize_library_ref(library_ref)?;
    let conn = connect_readonly(sqlite_path)?;
    let mut stmt = conn.prepare(
        "SELECT libraryID, type, editable, filesEditable, version, storageVersion, lastSync, archived
         FROM libraries
         WHERE libraryID = ?1",
    )?;
    let mut rows = stmt.query_map([library_id], library_from_row)?;
    Ok(rows.next().transpose()?)
}

/// `default_library_id()` (`zotero_sqlite.py:131-138`).
pub fn default_library_id(sqlite_path: &Path) -> anyhow::Result<Option<i64>> {
    let libraries = fetch_libraries(sqlite_path)?;
    if libraries.is_empty() {
        return Ok(None);
    }
    if let Some(user_lib) = libraries.iter().find(|l| l.kind == "user") {
        return Ok(Some(user_lib.library_id));
    }
    Ok(Some(libraries[0].library_id))
}

/// `fetch_collections()` (`zotero_sqlite.py:141-161`).
pub fn fetch_collections(
    sqlite_path: &Path,
    library_id: Option<i64>,
) -> anyhow::Result<Vec<Collection>> {
    let conn = connect_readonly(sqlite_path)?;
    let mut stmt = conn.prepare(
        "SELECT
            c.collectionID,
            c.key,
            c.collectionName,
            c.parentCollectionID,
            c.libraryID,
            c.version,
            COUNT(ci.itemID) AS itemCount
         FROM collections c
         LEFT JOIN collectionItems ci ON ci.collectionID = c.collectionID
         WHERE (?1 IS NULL OR c.libraryID = ?1)
         GROUP BY c.collectionID, c.key, c.collectionName, c.parentCollectionID, c.libraryID, c.version
         ORDER BY c.collectionName COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([library_id], |row| {
        Ok(Collection {
            collection_id: row.get("collectionID")?,
            key: row.get("key")?,
            collection_name: row.get("collectionName")?,
            parent_collection_id: row.get("parentCollectionID")?,
            library_id: row.get("libraryID")?,
            version: row.get("version")?,
            item_count: row.get("itemCount")?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn collection_from_row(row: &Row) -> rusqlite::Result<Collection> {
    Ok(Collection {
        collection_id: row.get("collectionID")?,
        key: row.get("key")?,
        collection_name: row.get("collectionName")?,
        parent_collection_id: row.get("parentCollectionID")?,
        library_id: row.get("libraryID")?,
        version: row.get("version")?,
        item_count: row.get("itemCount")?,
    })
}

/// `resolve_collection()` (`zotero_sqlite.py:231-259`).
pub fn resolve_collection(
    sqlite_path: &Path,
    collection_ref: &str,
    library_id: Option<i64>,
) -> anyhow::Result<Option<Collection>> {
    let conn = connect_readonly(sqlite_path)?;
    if is_numeric_ref(collection_ref) {
        let mut stmt = conn.prepare(
            "SELECT c.collectionID, c.key, c.collectionName, c.parentCollectionID, c.libraryID, c.version, \
             COUNT(ci.itemID) AS itemCount FROM collections c \
             LEFT JOIN collectionItems ci ON ci.collectionID = c.collectionID \
             WHERE c.collectionID = ? GROUP BY c.collectionID",
        )?;
        let mut rows =
            stmt.query_map([collection_ref.trim().parse::<i64>()?], collection_from_row)?;
        return Ok(rows.next().transpose()?);
    }

    let mut sql = "SELECT c.collectionID, c.key, c.collectionName, c.parentCollectionID, c.libraryID, c.version, \
         COUNT(ci.itemID) AS itemCount FROM collections c \
         LEFT JOIN collectionItems ci ON ci.collectionID = c.collectionID \
         WHERE c.key = ?"
        .to_string();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(collection_ref.to_string())];
    if let Some(library_id) = library_id {
        sql.push_str(" AND c.libraryID = ?");
        params.push(Box::new(library_id));
    }
    sql.push_str(" GROUP BY c.collectionID ORDER BY c.libraryID, c.collectionID");
    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut result_rows = Vec::new();
    {
        let mut rows_iter = stmt.query(param_refs.as_slice())?;
        while let Some(row) = rows_iter.next()? {
            result_rows.push(collection_from_row(row)?);
        }
    }
    if result_rows.is_empty() {
        return Ok(None);
    }
    if result_rows.len() > 1 && library_id.is_none() {
        let mut libraries: Vec<i64> = result_rows.iter().map(|r| r.library_id).collect();
        libraries.sort_unstable();
        libraries.dedup();
        let library_text = libraries
            .iter()
            .map(|id| format!("L{id}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(DomainError::new(format!(
            "Ambiguous collection reference: {collection_ref}. Matches found in {library_text}. \
             Set the library with `session use-library <id>` and retry."
        ))
        .into());
    }
    Ok(Some(result_rows.into_iter().next().unwrap()))
}

static BR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>").unwrap());
static P_CLOSE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</p\s*>").unwrap());
static DIV_CLOSE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</div\s*>").unwrap());
static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
static NEWLINE_RUN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());

/// `note_html_to_text()` (`zotero_sqlite.py:85-95`).
///
/// Known accepted gap: entity decoding uses `html_escape::decode_html_entities`,
/// which (per its own docs) does not implement legacy semicolonless
/// references (e.g. `&amp;nbsp` without the trailing `;`) the way Python's
/// `html.unescape` does. Zotero's own note editor always emits well-formed,
/// semicolon-terminated entities, so this only diverges on notes containing
/// hand-crafted or scraped legacy HTML — low-likelihood, not fixed here;
/// revisit if a real library surfaces it.
pub fn note_html_to_text(note_html: Option<&str>) -> String {
    let Some(note_html) = note_html.filter(|s| !s.is_empty()) else {
        return String::new();
    };

    let text = BR_RE.replace_all(note_html, "\n");
    let text = P_CLOSE_RE.replace_all(&text, "\n\n");
    let text = DIV_CLOSE_RE.replace_all(&text, "\n");
    let text = TAG_RE.replace_all(&text, "");
    let text = html_escape::decode_html_entities(&text).into_owned();
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let text = NEWLINE_RUN_RE.replace_all(&text, "\n\n");
    text.trim().to_string()
}

/// `note_preview()` (`zotero_sqlite.py:98-102`). Operates on Unicode
/// scalar values, matching Python's codepoint-based `len()`/slicing.
/// Takes already-converted plain text (from `note_html_to_text`) rather
/// than raw HTML, so callers that need both don't pay for the HTML-to-text
/// conversion twice.
pub fn note_preview(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= NOTE_PREVIEW_LENGTH {
        return text.to_string();
    }
    let take = NOTE_PREVIEW_LENGTH.saturating_sub(1);
    let truncated: String = chars[..take].iter().collect();
    format!("{}\u{2026}", truncated.trim_end())
}

/// `_base_item_select()` (`zotero_sqlite.py:323-385`).
fn base_item_select() -> &'static str {
    "
        SELECT
            i.itemID,
            i.key,
            i.libraryID,
            i.itemTypeID,
            it.typeName,
            i.dateAdded,
            i.dateModified,
            i.version,
            COALESCE(
                (
                    SELECT v.value
                    FROM itemData d
                    JOIN fields f ON f.fieldID = d.fieldID
                    JOIN itemDataValues v ON v.valueID = d.valueID
                    WHERE d.itemID = i.itemID AND f.fieldName = 'title'
                    LIMIT 1
                ),
                n.title,
                ''
            ) AS title,
            (
                SELECT v.value
                FROM itemData d
                JOIN fields f ON f.fieldID = d.fieldID
                JOIN itemDataValues v ON v.valueID = d.valueID
                WHERE d.itemID = i.itemID AND f.fieldName = 'DOI'
                LIMIT 1
            ) AS DOI,
            (
                SELECT v.value
                FROM itemData d
                JOIN fields f ON f.fieldID = d.fieldID
                JOIN itemDataValues v ON v.valueID = d.valueID
                WHERE d.itemID = i.itemID AND f.fieldName = 'date'
                LIMIT 1
            ) AS date,
            EXISTS (
                SELECT 1
                FROM itemAttachments ia
                WHERE ia.parentItemID = i.itemID
                  AND (
                    LOWER(COALESCE(ia.contentType, '')) = 'application/pdf'
                    OR LOWER(COALESCE(ia.path, '')) LIKE '%.pdf'
                  )
            ) AS hasPdf,
            n.parentItemID AS noteParentItemID,
            n.note AS noteContent,
            a.parentItemID AS attachmentParentItemID,
            an.parentItemID AS annotationParentItemID,
            an.text AS annotationText,
            an.comment AS annotationComment,
            a.linkMode,
            a.contentType,
            a.path AS attachmentPath
        FROM items i
        JOIN itemTypes it ON it.itemTypeID = i.itemTypeID
        LEFT JOIN itemNotes n ON n.itemID = i.itemID
        LEFT JOIN itemAttachments a ON a.itemID = i.itemID
        LEFT JOIN itemAnnotations an ON an.itemID = i.itemID
    "
}

/// `_fetch_item_fields()` (`zotero_sqlite.py:280-292`).
fn fetch_item_fields(conn: &Connection, item_id: i64) -> anyhow::Result<Map<String, Value>> {
    let mut stmt = conn.prepare(
        "SELECT f.fieldName, v.value
         FROM itemData d
         JOIN fields f ON f.fieldID = d.fieldID
         JOIN itemDataValues v ON v.valueID = d.valueID
         WHERE d.itemID = ?1
         ORDER BY f.fieldName COLLATE NOCASE",
    )?;
    let mut map = Map::new();
    let rows = stmt.query_map([item_id], |row| {
        let name: String = row.get(0)?;
        let value: String = row.get(1)?;
        Ok((name, value))
    })?;
    for row in rows {
        let (name, value) = row?;
        map.insert(name, Value::String(value));
    }
    Ok(map)
}

/// `_fetch_item_creators()` (`zotero_sqlite.py:295-306`).
fn fetch_item_creators(conn: &Connection, item_id: i64) -> anyhow::Result<Vec<Creator>> {
    let mut stmt = conn.prepare(
        "SELECT c.creatorID, c.firstName, c.lastName, c.fieldMode, ic.creatorTypeID, ic.orderIndex
         FROM itemCreators ic
         JOIN creators c ON c.creatorID = ic.creatorID
         WHERE ic.itemID = ?1
         ORDER BY ic.orderIndex",
    )?;
    let rows = stmt.query_map([item_id], |row| {
        Ok(Creator {
            creator_id: row.get("creatorID")?,
            first_name: row.get("firstName")?,
            last_name: row.get("lastName")?,
            field_mode: row.get("fieldMode")?,
            creator_type_id: row.get("creatorTypeID")?,
            order_index: row.get("orderIndex")?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// `_fetch_item_tags()` (`zotero_sqlite.py:309-320`).
fn fetch_item_tags(conn: &Connection, item_id: i64) -> anyhow::Result<Vec<Tag>> {
    let mut stmt = conn.prepare(
        "SELECT t.tagID, t.name, it.type
         FROM itemTags it
         JOIN tags t ON t.tagID = it.tagID
         WHERE it.itemID = ?1
         ORDER BY t.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([item_id], |row| {
        Ok(Tag {
            tag_id: row.get("tagID")?,
            name: row.get("name")?,
            kind: row.get("type")?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// `_normalize_item()` (`zotero_sqlite.py:388-404`).
fn normalize_item(conn: &Connection, row: &Row, include_related: bool) -> anyhow::Result<Item> {
    let item_id: i64 = row.get("itemID")?;
    let type_name: String = row.get("typeName")?;
    let has_pdf: i64 = row.get("hasPdf")?;
    let doi: Option<String> = row.get("DOI")?;
    let note_parent_item_id: Option<i64> = row.get("noteParentItemID")?;
    let attachment_parent_item_id: Option<i64> = row.get("attachmentParentItemID")?;
    let annotation_parent_item_id: Option<i64> = row.get("annotationParentItemID")?;
    let note_content: Option<String> = row.get("noteContent")?;
    let note_text = note_html_to_text(note_content.as_deref());

    let (fields, creators, tags) = if include_related {
        (
            fetch_item_fields(conn, item_id)?,
            fetch_item_creators(conn, item_id)?,
            fetch_item_tags(conn, item_id)?,
        )
    } else {
        (Map::new(), Vec::new(), Vec::new())
    };

    Ok(Item {
        item_id,
        key: row.get("key")?,
        library_id: row.get("libraryID")?,
        item_type_id: row.get("itemTypeID")?,
        type_name: type_name.clone(),
        date_added: row.get("dateAdded")?,
        date_modified: row.get("dateModified")?,
        version: row.get("version")?,
        title: row.get("title")?,
        doi: doi.unwrap_or_default(),
        date: row.get("date")?,
        has_pdf: has_pdf != 0,
        note_parent_item_id,
        note_content: note_content.clone(),
        attachment_parent_item_id,
        annotation_parent_item_id,
        annotation_text: row.get("annotationText")?,
        annotation_comment: row.get("annotationComment")?,
        link_mode: row.get("linkMode")?,
        content_type: row.get("contentType")?,
        attachment_path: row.get("attachmentPath")?,
        fields,
        creators,
        tags,
        is_attachment: type_name == "attachment",
        is_note: type_name == "note",
        is_annotation: type_name == "annotation",
        parent_item_id: attachment_parent_item_id
            .or(note_parent_item_id)
            .or(annotation_parent_item_id),
        note_text: note_text.clone(),
        note_preview: note_preview(&note_text),
    })
}

#[derive(Default)]
pub struct FetchItemsFilter {
    pub library_id: Option<i64>,
    pub collection_id: Option<i64>,
    pub parent_item_id: Option<i64>,
    pub tag: Option<String>,
    pub limit: Option<i64>,
}

/// `fetch_items()` (`zotero_sqlite.py:407-446`).
pub fn fetch_items(sqlite_path: &Path, filter: FetchItemsFilter) -> anyhow::Result<Vec<Item>> {
    let mut where_clauses = vec!["1=1".to_string()];
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(library_id) = filter.library_id {
        where_clauses.push("i.libraryID = ?".to_string());
        params.push(Box::new(library_id));
    }
    if let Some(collection_id) = filter.collection_id {
        where_clauses.push(
            "EXISTS (SELECT 1 FROM collectionItems ci WHERE ci.itemID = i.itemID AND ci.collectionID = ?)"
                .to_string(),
        );
        params.push(Box::new(collection_id));
    }
    match filter.parent_item_id {
        None => where_clauses
            .push("COALESCE(a.parentItemID, n.parentItemID, an.parentItemID) IS NULL".to_string()),
        Some(parent_id) => {
            where_clauses
                .push("COALESCE(a.parentItemID, n.parentItemID, an.parentItemID) = ?".to_string());
            params.push(Box::new(parent_id));
        }
    }
    if let Some(tag) = &filter.tag {
        where_clauses.push(
            "EXISTS (
                SELECT 1
                FROM itemTags it2
                JOIN tags t2 ON t2.tagID = it2.tagID
                WHERE it2.itemID = i.itemID AND (t2.name = ? OR t2.tagID = ?)
            )"
            .to_string(),
        );
        params.push(Box::new(tag.clone()));
        params.push(Box::new(if is_numeric_ref(tag) {
            tag.trim().parse::<i64>().unwrap()
        } else {
            -1
        }));
    }

    let mut sql = format!(
        "{}\nWHERE {}\nORDER BY i.dateModified DESC, i.itemID DESC",
        base_item_select(),
        where_clauses.join(" AND ")
    );
    if let Some(limit) = filter.limit {
        sql.push_str(&format!("\nLIMIT {limit}"));
    }

    let conn = connect_readonly(sqlite_path)?;
    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut rows = stmt.query(param_refs.as_slice())?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        items.push(normalize_item(&conn, row, false)?);
    }
    Ok(items)
}

/// `find_items_by_title()` (`zotero_sqlite.py:449-512`).
pub fn find_items_by_title(
    sqlite_path: &Path,
    query: &str,
    library_id: Option<i64>,
    collection_id: Option<i64>,
    limit: i64,
    exact_title: bool,
) -> anyhow::Result<Vec<Item>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let title_expr = "
        LOWER(
            COALESCE(
                (
                    SELECT v.value
                    FROM itemData d
                    JOIN fields f ON f.fieldID = d.fieldID
                    JOIN itemDataValues v ON v.valueID = d.valueID
                    WHERE d.itemID = i.itemID AND f.fieldName = 'title'
                    LIMIT 1
                ),
                n.title,
                ''
            )
        )
    ";
    let mut where_clauses = vec!["1=1".to_string()];
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(library_id) = library_id {
        where_clauses.push("i.libraryID = ?".to_string());
        params.push(Box::new(library_id));
    }
    if let Some(collection_id) = collection_id {
        where_clauses.push(
            "EXISTS (SELECT 1 FROM collectionItems ci WHERE ci.itemID = i.itemID AND ci.collectionID = ?)"
                .to_string(),
        );
        params.push(Box::new(collection_id));
    }
    where_clauses
        .push("COALESCE(a.parentItemID, n.parentItemID, an.parentItemID) IS NULL".to_string());
    let needle = query.to_lowercase();
    if exact_title {
        where_clauses.push(format!("{title_expr} = ?"));
        params.push(Box::new(needle.clone()));
    } else {
        where_clauses.push(format!("{title_expr} LIKE ?"));
        params.push(Box::new(format!("%{needle}%")));
    }

    let sql = format!(
        "SELECT * FROM ({}\nWHERE {}\n) AS base\n
         ORDER BY
             CASE
                 WHEN LOWER(title) = ? THEN 0
                 WHEN LOWER(title) LIKE ? THEN 1
                 ELSE 2
             END,
             INSTR(LOWER(title), ?),
             dateModified DESC,
             itemID DESC
         LIMIT ?",
        base_item_select(),
        where_clauses.join(" AND ")
    );
    params.push(Box::new(needle.clone()));
    params.push(Box::new(format!("{needle}%")));
    params.push(Box::new(needle));
    params.push(Box::new(limit));

    let conn = connect_readonly(sqlite_path)?;
    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut rows = stmt.query(param_refs.as_slice())?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        items.push(normalize_item(&conn, row, false)?);
    }
    Ok(items)
}

/// `_ambiguous_reference()` (`zotero_sqlite.py:222-228`): sorted, deduped
/// library IDs, `L{id}` formatted.
fn ambiguous_reference_error(kind: &str, item_ref: &str, rows: &[Item]) -> anyhow::Error {
    let mut libraries: Vec<i64> = rows.iter().map(|r| r.library_id).collect();
    libraries.sort_unstable();
    libraries.dedup();
    let library_text = if libraries.is_empty() {
        "multiple libraries".to_string()
    } else {
        libraries
            .iter()
            .map(|id| format!("L{id}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    DomainError::new(format!(
        "Ambiguous {kind} reference: {item_ref}. Matches found in {library_text}. \
         Set the library with `session use-library <id>` and retry."
    ))
    .into()
}

/// `resolve_item()` (`zotero_sqlite.py:515-532`).
pub fn resolve_item(
    sqlite_path: &Path,
    item_ref: &str,
    library_id: Option<i64>,
) -> anyhow::Result<Option<Item>> {
    let conn = connect_readonly(sqlite_path)?;
    let (where_clause, params): (String, Vec<Box<dyn rusqlite::ToSql>>) =
        if is_numeric_ref(item_ref) {
            (
                "i.itemID = ?".to_string(),
                vec![Box::new(item_ref.trim().parse::<i64>()?)],
            )
        } else if let Some(library_id) = library_id {
            (
                "i.key = ? AND i.libraryID = ?".to_string(),
                vec![Box::new(item_ref.to_string()), Box::new(library_id)],
            )
        } else {
            (
                "i.key = ?".to_string(),
                vec![Box::new(item_ref.to_string())],
            )
        };

    let sql = format!(
        "{}\nWHERE {}\nORDER BY i.libraryID, i.itemID",
        base_item_select(),
        where_clause
    );
    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let mut result_rows = Vec::new();
    {
        let mut rows_iter = stmt.query(param_refs.as_slice())?;
        while let Some(row) = rows_iter.next()? {
            result_rows.push(normalize_item(&conn, row, true)?);
        }
    }

    if result_rows.is_empty() {
        return Ok(None);
    }
    if result_rows.len() > 1 && library_id.is_none() && !is_numeric_ref(item_ref) {
        return Err(ambiguous_reference_error("item", item_ref, &result_rows));
    }
    Ok(Some(result_rows.into_iter().next().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression coverage for bugs a review pass found in scenarios the
    // parity harness's fixed golden fixtures don't exercise.

    #[test]
    fn is_numeric_ref_trims_whitespace_like_python_int_str() {
        assert!(is_numeric_ref(" 5 "));
        assert!(is_numeric_ref("5\t"));
        assert!(is_numeric_ref("5"));
        assert!(!is_numeric_ref("abc"));
        assert!(!is_numeric_ref(""));
    }

    #[test]
    fn is_numeric_ref_rejects_i64_overflow() {
        // Python's arbitrary-precision int would accept this; Rust's i64
        // parse fails, which routes to a clean "not found" instead of a
        // crash — see error.rs's doc comment for the accepted divergence.
        assert!(!is_numeric_ref("99999999999999999999"));
    }

    #[test]
    fn note_html_to_text_empty_and_none_are_empty_string() {
        assert_eq!(note_html_to_text(None), "");
        assert_eq!(note_html_to_text(Some("")), "");
    }

    #[test]
    fn note_html_to_text_converts_br_p_div_and_strips_tags() {
        let html = "<p>Hello<br>world</p><div>Next</div>";
        assert_eq!(note_html_to_text(Some(html)), "Hello\nworld\n\nNext");
    }

    #[test]
    fn note_preview_truncates_at_160_chars_with_ellipsis() {
        let long = "a".repeat(200);
        let preview = note_preview(&long);
        assert_eq!(preview.chars().count(), NOTE_PREVIEW_LENGTH);
        assert!(preview.ends_with('\u{2026}'));
    }

    #[test]
    fn note_preview_short_text_unchanged() {
        assert_eq!(note_preview("short"), "short");
    }
}
