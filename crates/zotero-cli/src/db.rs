//! Port of `utils/zotero_sqlite.py`'s read paths needed by the vertical
//! slice: libraries, collections, and the item base-select/normalize
//! pipeline shared by `item list`/`item get`/`item find`. Every SQL string
//! here is copied verbatim from the Python source (see per-function doc
//! comments for line references) — do not "clean up" the queries without
//! re-checking against golden fixtures, since SQLite column order feeds
//! JSON key order.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use rusqlite::{Connection, ErrorCode, OpenFlags, Row};
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
pub struct SavedSearchCondition {
    #[serde(rename = "searchConditionID")]
    pub search_condition_id: i64,
    pub condition: String,
    pub operator: String,
    pub value: Option<String>,
    pub required: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SavedSearch {
    #[serde(rename = "savedSearchID")]
    pub saved_search_id: i64,
    #[serde(rename = "savedSearchName")]
    pub saved_search_name: String,
    #[serde(rename = "clientDateModified")]
    pub client_date_modified: String,
    #[serde(rename = "libraryID")]
    pub library_id: i64,
    pub key: String,
    pub version: i64,
    pub conditions: Vec<SavedSearchCondition>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagSummary {
    #[serde(rename = "tagID")]
    pub tag_id: i64,
    pub name: String,
    #[serde(rename = "itemCount")]
    pub item_count: i64,
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

/// `{**collection, "children": []}` (`zotero_sqlite.py:202-219`): dict
/// spread preserves the original SELECT column order, with `children`
/// appended last. Modeled as a distinct struct (not `Collection` plus a
/// wrapper) so the field order matches exactly rather than relying on
/// `#[serde(flatten)]` ordering, which Rust structs already preserve.
#[derive(Debug, Clone, Serialize)]
pub struct CollectionNode {
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
    pub children: Vec<CollectionNode>,
}

impl From<&Collection> for CollectionNode {
    fn from(c: &Collection) -> Self {
        CollectionNode {
            collection_id: c.collection_id,
            key: c.key.clone(),
            collection_name: c.collection_name.clone(),
            parent_collection_id: c.parent_collection_id,
            library_id: c.library_id,
            version: c.version,
            item_count: c.item_count,
            children: Vec::new(),
        }
    }
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

/// `connect_readonly()` (`zotero_sqlite.py:25-32`), corrected for Zotero 10's
/// WAL-mode database per `phase-14-zotero-10-compatibility-gate.md` §1/§1b.
///
/// The original unconditional `mode=ro&immutable=1` silently drops
/// uncheckpointed WAL commits: `immutable=1` tells SQLite the file will
/// never change, so it never attaches `-wal` at all. Zotero 10 enables WAL,
/// so every uncheckpointed row vanished with no error.
///
/// The naive fix ("just drop `immutable=1`") is not sufficient either.
/// Live-verified independently against a real, running Zotero 10.0.1
/// instance (2026-08-29): Zotero holds its own database connection in
/// SQLite's exclusive locking mode on every version, not only in WAL mode —
/// a plain `mode=ro` open fails with `SQLITE_BUSY` on the *first statement*
/// (confirmed with a bare `SELECT 1`, at busy timeouts up to 2s, 5+
/// consecutive reproductions), which is why `immutable=1` was chosen
/// originally: not for staleness tolerance, but because it was the only way
/// to open the file at all while Zotero runs. The failure surfaces at
/// statement-prepare time, not at `Connection::open_with_flags` time, so
/// this function must run a real probe query to detect it here rather than
/// deferring to the caller's first query.
///
/// Corrected policy: try the WAL-safe `mode=ro` open first — this is the
/// only path taken when Zotero is closed, or when the database has no
/// `-wal` sidecar (Zotero <=9's rollback-journal format), i.e. zero
/// behavior change for pre-10 installs. If that fails with `SQLITE_BUSY`
/// specifically (Zotero is running and holds the lock) and a `-wal` file
/// exists, refuse loudly rather than silently returning a possibly
/// incomplete snapshot — `immutable=1` must never be a silent fallback on a
/// WAL database. If `SQLITE_BUSY` occurs with no `-wal` file present, the
/// database is not in WAL mode, so `immutable=1` cannot miss anything it
/// wasn't already going to miss; falling back there matches the
/// unconditional pre-10 behavior exactly.
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

    match open_and_probe(&format!("file:{posix_path}?mode=ro")) {
        Ok(conn) => Ok(conn),
        Err(err) if is_sqlite_busy(&err) => {
            if wal_sidecar_path(sqlite_path).exists() {
                Err(DomainError::new(format!(
                    "Zotero appears to be running and holds an exclusive lock on the \
                     WAL-mode database ({}). Reading with immutable=1 would silently \
                     skip uncheckpointed commits, so this refuses instead of returning \
                     stale data. Close Zotero and retry, or use a command backed by the \
                     Zotero Local API while Zotero is running.",
                    sqlite_path.display()
                ))
                .into())
            } else {
                open_and_probe(&format!("file:{posix_path}?mode=ro&immutable=1"))
            }
        }
        Err(err) => Err(err),
    }
}

/// Opens a read-only SQLite URI and runs a trivial probe query so a lock
/// held by another process (Zotero) surfaces here, not on the caller's
/// first real query — see `connect_readonly`'s doc comment for why the
/// probe is necessary (the failure occurs at prepare time, not open time).
fn open_and_probe(uri: &str) -> anyhow::Result<Connection> {
    let conn = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.busy_timeout(Duration::from_secs_f64(1.0))?;
    conn.query_row("SELECT 1", [], |_| Ok(()))?;
    Ok(conn)
}

fn is_sqlite_busy(err: &anyhow::Error) -> bool {
    matches!(
        err.downcast_ref::<rusqlite::Error>(),
        Some(rusqlite::Error::SqliteFailure(ffi_err, _)) if ffi_err.code == ErrorCode::DatabaseBusy
    )
}

/// The WAL sidecar file SQLite maintains alongside a database in WAL
/// journal mode (e.g. `zotero.sqlite` -> `zotero.sqlite-wal`). Its presence
/// — even at 0 bytes, since Zotero holds the database open continuously —
/// is how `connect_readonly` distinguishes "Zotero 10+ holding a WAL
/// database" (refuse) from "Zotero <=9 holding a rollback-journal database"
/// (safe to fall back to `immutable=1`, matching pre-10 behavior).
fn wal_sidecar_path(sqlite_path: &Path) -> PathBuf {
    let mut name = sqlite_path.file_name().unwrap_or_default().to_os_string();
    name.push("-wal");
    sqlite_path.with_file_name(name)
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

/// `find_collections()` (`zotero_sqlite.py:164-198`).
pub fn find_collections(
    sqlite_path: &Path,
    query: &str,
    library_id: Option<i64>,
    limit: i64,
) -> anyhow::Result<Vec<Collection>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let needle = query.to_lowercase();
    let like_query = format!("%{needle}%");
    let prefix_query = format!("{needle}%");
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
         WHERE (?1 IS NULL OR c.libraryID = ?1) AND LOWER(c.collectionName) LIKE ?2
         GROUP BY c.collectionID, c.key, c.collectionName, c.parentCollectionID, c.libraryID, c.version
         ORDER BY
             CASE
                 WHEN LOWER(c.collectionName) = ?3 THEN 0
                 WHEN LOWER(c.collectionName) LIKE ?4 THEN 1
                 ELSE 2
             END,
             INSTR(LOWER(c.collectionName), ?3),
             c.collectionName COLLATE NOCASE,
             c.collectionID
         LIMIT ?5",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![library_id, like_query, needle, prefix_query, limit],
        collection_from_row,
    )?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// `build_collection_tree()` (`zotero_sqlite.py:202-219`): a collection
/// whose `parentCollectionID` points outside this result set (e.g.
/// filtered by library scope) becomes a root too, matching Python's
/// `by_id.get(parent_id) is None -> roots.append(node)` fallback.
pub fn build_collection_tree(collections: &[Collection]) -> Vec<CollectionNode> {
    use std::collections::{HashMap, HashSet};

    let known_ids: HashSet<i64> = collections.iter().map(|c| c.collection_id).collect();
    let mut children_by_parent: HashMap<i64, Vec<&Collection>> = HashMap::new();
    let mut roots: Vec<&Collection> = Vec::new();
    for c in collections {
        match c.parent_collection_id {
            Some(parent_id) if known_ids.contains(&parent_id) => {
                children_by_parent.entry(parent_id).or_default().push(c);
            }
            _ => roots.push(c),
        }
    }

    fn build(
        c: &Collection,
        children_by_parent: &HashMap<i64, Vec<&Collection>>,
    ) -> CollectionNode {
        let mut node = CollectionNode::from(c);
        if let Some(kids) = children_by_parent.get(&c.collection_id) {
            node.children = kids.iter().map(|k| build(k, children_by_parent)).collect();
        }
        node
    }

    roots
        .into_iter()
        .map(|c| build(c, &children_by_parent))
        .collect()
}

/// `fetch_item_children()` (`zotero_sqlite.py:535-539`).
pub fn fetch_item_children(sqlite_path: &Path, item_ref: &str) -> anyhow::Result<Vec<Item>> {
    let Some(item) = resolve_item(sqlite_path, item_ref, None)? else {
        return Ok(Vec::new());
    };
    fetch_items(
        sqlite_path,
        FetchItemsFilter {
            parent_item_id: Some(item.item_id),
            ..Default::default()
        },
    )
}

/// `fetch_item_notes()` (`zotero_sqlite.py:542-544`).
pub fn fetch_item_notes(sqlite_path: &Path, item_ref: &str) -> anyhow::Result<Vec<Item>> {
    Ok(fetch_item_children(sqlite_path, item_ref)?
        .into_iter()
        .filter(|c| c.type_name == "note")
        .collect())
}

/// `fetch_item_attachments()` (`zotero_sqlite.py:547-549`).
pub fn fetch_item_attachments(sqlite_path: &Path, item_ref: &str) -> anyhow::Result<Vec<Item>> {
    Ok(fetch_item_children(sqlite_path, item_ref)?
        .into_iter()
        .filter(|c| c.type_name == "attachment")
        .collect())
}

/// Minimal percent-decoder matching `urllib.parse.unquote`'s default
/// (`utf-8`, lossy on invalid sequences) closely enough for the file:
/// URI paths this is used on. No new dependency: this is the only call
/// site in the vertical slice, and it's ~15 lines.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Splits a `file://<netloc><path>` URI (with the `file://` prefix
/// already stripped) into `(netloc, path)`, mirroring
/// `urllib.parse.urlparse`'s behavior for this scheme: netloc is
/// everything up to the first `/`, which may be empty
/// (`file:///C:/...` -> netloc="", path="/C:/..."`).
fn split_file_uri_authority(rest: &str) -> (&str, &str) {
    match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, ""),
    }
}

/// `resolve_attachment_real_path()` (`zotero_sqlite.py:552-574`) — the
/// highest cross-platform risk area in the port (flagged in
/// `plans/.../phase-04-*.md`). Every branch below is checked against
/// the Python source line-for-line; see the inline comments for exactly
/// which Python expression each block reproduces.
pub fn resolve_attachment_real_path(
    attachment_path: Option<&str>,
    item_key: &str,
    data_dir: &Path,
) -> Option<String> {
    let raw_path = attachment_path.filter(|s| !s.is_empty())?;

    // `if raw_path.startswith("storage:"): ...` (line 558-560)
    if let Some(filename) = raw_path.strip_prefix("storage:") {
        let resolved = crate::paths::normalize_resolve(
            &data_dir.join("storage").join(item_key).join(filename),
        );
        return Some(resolved.to_string_lossy().into_owned());
    }

    // `if raw_path.startswith("file://"): ...` (line 561-570)
    if let Some(rest) = raw_path.strip_prefix("file://") {
        let (netloc, encoded_path) = split_file_uri_authority(rest);
        let decoded_path = percent_decode(encoded_path);

        // `if parsed.netloc and parsed.netloc.lower() != "localhost": ...`
        if !netloc.is_empty() && netloc.to_lowercase() != "localhost" {
            let normalized_unc_path = decoded_path.replace('/', "\\");
            return Some(format!("\\\\{netloc}{normalized_unc_path}"));
        }

        // `if re.match(r"^/[A-Za-z]:", decoded_path): ...`
        let db = decoded_path.as_bytes();
        let is_drive_letter_path =
            db.len() >= 3 && db[0] == b'/' && db[1].is_ascii_alphabetic() && db[2] == b':';
        if is_drive_letter_path {
            let stripped = decoded_path.trim_start_matches('/');
            return Some(stripped.replace('/', "\\"));
        }

        // `return decoded_path if os.name != "nt" else str(PureWindowsPath(decoded_path))`
        if cfg!(windows) {
            return Some(decoded_path.replace('/', "\\"));
        }
        return Some(decoded_path);
    }

    // `path = Path(raw_path); if path.is_absolute(): return str(path)`
    let path = Path::new(raw_path);
    if path.is_absolute() {
        return Some(path.to_string_lossy().into_owned());
    }
    // `return str((data_dir / raw_path).resolve())`
    let resolved = crate::paths::normalize_resolve(&data_dir.join(raw_path));
    Some(resolved.to_string_lossy().into_owned())
}

/// `fetch_saved_searches()` (`zotero_sqlite.py:577-600`).
pub fn fetch_saved_searches(
    sqlite_path: &Path,
    library_id: Option<i64>,
) -> anyhow::Result<Vec<SavedSearch>> {
    let conn = connect_readonly(sqlite_path)?;
    let mut stmt = conn.prepare(
        "SELECT savedSearchID, savedSearchName, clientDateModified, libraryID, key, version
         FROM savedSearches
         WHERE (?1 IS NULL OR libraryID = ?1)
         ORDER BY savedSearchName COLLATE NOCASE",
    )?;
    let mut searches: Vec<SavedSearch> = stmt
        .query_map([library_id], |row| {
            Ok(SavedSearch {
                saved_search_id: row.get("savedSearchID")?,
                saved_search_name: row.get("savedSearchName")?,
                client_date_modified: row.get("clientDateModified")?,
                library_id: row.get("libraryID")?,
                key: row.get("key")?,
                version: row.get("version")?,
                conditions: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut cond_stmt = conn.prepare(
        "SELECT searchConditionID, condition, operator, value, required
         FROM savedSearchConditions
         WHERE savedSearchID = ?1
         ORDER BY searchConditionID",
    )?;
    for search in &mut searches {
        let rows = cond_stmt.query_map([search.saved_search_id], |row| {
            Ok(SavedSearchCondition {
                search_condition_id: row.get("searchConditionID")?,
                condition: row.get("condition")?,
                operator: row.get("operator")?,
                value: row.get("value")?,
                required: row.get("required")?,
            })
        })?;
        search.conditions = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    }
    Ok(searches)
}

/// `resolve_saved_search()` (`zotero_sqlite.py:603-621`).
pub fn resolve_saved_search(
    sqlite_path: &Path,
    search_ref: &str,
    library_id: Option<i64>,
) -> anyhow::Result<Option<SavedSearch>> {
    let searches = fetch_saved_searches(sqlite_path, library_id)?;
    if is_numeric_ref(search_ref) {
        let target = search_ref.trim();
        return Ok(searches
            .into_iter()
            .find(|s| s.saved_search_id.to_string() == target));
    }

    let mut matches: Vec<SavedSearch> = searches
        .into_iter()
        .filter(|s| s.key == search_ref)
        .collect();
    if matches.is_empty() {
        return Ok(None);
    }
    if matches.len() > 1 && library_id.is_none() {
        let mut libraries: Vec<i64> = matches.iter().map(|s| s.library_id).collect();
        libraries.sort_unstable();
        libraries.dedup();
        let library_text = libraries
            .iter()
            .map(|id| format!("L{id}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(DomainError::new(format!(
            "Ambiguous saved search reference: {search_ref}. Matches found in {library_text}. \
             Set the library with `session use-library <id>` and retry."
        ))
        .into());
    }
    Ok(Some(matches.remove(0)))
}

/// `fetch_tags()` (`zotero_sqlite.py:624-638`).
pub fn fetch_tags(sqlite_path: &Path, library_id: Option<i64>) -> anyhow::Result<Vec<TagSummary>> {
    let conn = connect_readonly(sqlite_path)?;
    let mut stmt = conn.prepare(
        "SELECT t.tagID, t.name, COUNT(it.itemID) AS itemCount
         FROM tags t
         JOIN itemTags it ON it.tagID = t.tagID
         JOIN items i ON i.itemID = it.itemID
         WHERE (?1 IS NULL OR i.libraryID = ?1)
         GROUP BY t.tagID, t.name
         ORDER BY t.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([library_id], |row| {
        Ok(TagSummary {
            tag_id: row.get("tagID")?,
            name: row.get("name")?,
            item_count: row.get("itemCount")?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// `fetch_tag_items()` (`zotero_sqlite.py:641-652`).
pub fn fetch_tag_items(
    sqlite_path: &Path,
    tag_ref: &str,
    library_id: Option<i64>,
) -> anyhow::Result<Vec<Item>> {
    let conn = connect_readonly(sqlite_path)?;
    let tag_name: Option<String> = if is_numeric_ref(tag_ref) {
        conn.query_row(
            "SELECT name FROM tags WHERE tagID = ?1",
            [tag_ref.trim().parse::<i64>()?],
            |row| row.get(0),
        )
        .ok()
    } else {
        conn.query_row("SELECT name FROM tags WHERE name = ?1", [tag_ref], |row| {
            row.get(0)
        })
        .ok()
    };
    drop(conn);
    let Some(tag_name) = tag_name else {
        return Ok(Vec::new());
    };
    fetch_items(
        sqlite_path,
        FetchItemsFilter {
            library_id,
            tag: Some(tag_name),
            ..Default::default()
        },
    )
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

    /// Manual scratch-file helper instead of pulling in the `tempfile`
    /// crate for one test -- keeps the project's stated small
    /// dependency footprint. A monotonic counter plus the process id
    /// is enough to avoid collisions between concurrent `cargo test`
    /// threads on the same machine.
    fn temp_sqlite_path(label: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "zotero-cli-test-{}-{n}-{label}.sqlite",
            std::process::id()
        ))
    }

    // Runtime verification (not just spec-verification) of the Windows
    // URI fix in `connect_readonly`: a dynamic-testing pass found that
    // it built the SQLite `file:` URI with native path separators,
    // which is a broken URI on Windows (SQLite's URI spec requires
    // `/`, not `\`). The fix was correct by inspection of SQLite's own
    // documentation, but nothing actually opened a file through it on
    // that platform -- dev and (until now) the test suite are
    // Darwin/Linux-biased in what they exercise, even though CI's
    // windows-latest leg already runs `cargo test --workspace`. This
    // test uses `std::env::temp_dir()`, which on Windows is a real
    // backslash-separated path (e.g.
    // `C:\Users\runneradmin\AppData\Local\Temp\...`), so it forces the
    // exact `\` -> `/` conversion to run against a real file open on
    // that CI leg, closing the "fixed but unverified" gap.
    #[test]
    fn connect_readonly_opens_a_real_file_through_the_uri_it_builds() {
        let path = temp_sqlite_path("connect-readonly");
        {
            let conn = Connection::open(&path).expect("create scratch sqlite file");
            conn.execute_batch(
                "CREATE TABLE libraries (
                    libraryID INTEGER PRIMARY KEY, type TEXT, editable INTEGER,
                    filesEditable INTEGER, version INTEGER, storageVersion INTEGER,
                    lastSync INTEGER, archived INTEGER
                );
                INSERT INTO libraries VALUES (1, 'user', 1, 1, 1, 1, NULL, 0);",
            )
            .expect("seed scratch sqlite file");
        }

        let libraries = fetch_libraries(&path).expect(
            "connect_readonly must open a real file via its file: URI on every platform, \
             including Windows where the path contains native backslash separators",
        );

        assert_eq!(libraries.len(), 1);
        assert_eq!(libraries[0].library_id, 1);
        assert_eq!(libraries[0].kind, "user");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn connect_readonly_missing_file_is_a_clean_domain_error_not_a_panic() {
        let path = temp_sqlite_path("does-not-exist");
        let err = connect_readonly(&path).unwrap_err();
        assert!(err.to_string().contains("Zotero database not found"));
    }

    // Zotero-10-compatibility-gate regression coverage (`phase-14-zotero-10-
    // compatibility-gate.md` §1/§2). These reproduce the WAL bug and its fix
    // deterministically in-process, without needing a real Zotero instance:
    // a writer connection puts committed rows in `-wal` without
    // checkpointing (SQLite's default auto-checkpoint threshold is ~1000
    // pages, far above what these tests write), which is exactly the state
    // `immutable=1` was blind to.

    fn open_wal_db(path: &Path) -> Connection {
        let conn = Connection::open(path).expect("create scratch sqlite file");
        conn.pragma_update(None, "journal_mode", "WAL")
            .expect("enable WAL journal mode");
        conn.execute_batch("CREATE TABLE items (id INTEGER PRIMARY KEY, val TEXT)")
            .expect("create schema");
        // Checkpoint the schema itself into the main file so the bug this
        // reproduces is specifically "misses uncheckpointed rows" (matching
        // the plan's "1 of 5 rows" reproduction), not the more trivial
        // "misses the whole uncheckpointed database, table included."
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .expect("checkpoint schema");
        conn
    }

    fn immutable_count(path: &Path) -> i64 {
        let posix_path = path.to_string_lossy().replace('\\', "/");
        let uri = format!("file:{posix_path}?mode=ro&immutable=1");
        let conn = Connection::open_with_flags(
            &uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .expect("open immutable=1 connection");
        conn.query_row("SELECT count(*) FROM items", [], |row| row.get(0))
            .expect("count rows")
    }

    /// Reproduces the CRITICAL bug this phase fixes: `immutable=1` never
    /// attaches `-wal`, so it silently under-counts uncheckpointed commits
    /// (matches the plan's live reproduction: "1 of 5 rows" under
    /// `immutable=1` vs "5 of 5" under `mode=ro`) — with exit code 0 and no
    /// error, which is what made it dangerous.
    #[test]
    fn wal_mode_immutable_fallback_silently_misses_uncheckpointed_commits() {
        let path = temp_sqlite_path("wal-immutable-bug");
        let writer = open_wal_db(&path);
        for i in 0..5 {
            writer
                .execute("INSERT INTO items VALUES (?1, ?2)", (i, format!("row{i}")))
                .expect("insert uncheckpointed row");
        }

        let stale_count = immutable_count(&path);
        assert_eq!(
            stale_count, 0,
            "immutable=1 must miss all 5 uncheckpointed WAL rows here, \
             demonstrating the bug this phase fixes -- if this assertion \
             ever fails, SQLite's own behavior changed and this test's \
             premise needs re-checking, not the assertion relaxed"
        );

        drop(writer);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(wal_sidecar_path(&path));
        let _ = std::fs::remove_file({
            let mut name = path.file_name().unwrap().to_os_string();
            name.push("-shm");
            path.with_file_name(name)
        });
    }

    /// The fix: `connect_readonly`'s corrected `mode=ro` path reads the same
    /// uncheckpointed WAL state completely and correctly, with no writer
    /// holding Zotero's exclusive lock (matches "Zotero is closed, or a
    /// `-wal` file simply isn't present" -- the common, zero-behavior-change
    /// case for the corrected policy).
    #[test]
    fn connect_readonly_reads_uncheckpointed_wal_commits_completely() {
        let path = temp_sqlite_path("wal-immutable-fix");
        let writer = open_wal_db(&path);
        for i in 0..5 {
            writer
                .execute("INSERT INTO items VALUES (?1, ?2)", (i, format!("row{i}")))
                .expect("insert uncheckpointed row");
        }
        drop(writer);

        let conn = connect_readonly(&path).expect("mode=ro must succeed with no lock holder");
        let count: i64 = conn
            .query_row("SELECT count(*) FROM items", [], |row| row.get(0))
            .expect("count rows");
        assert_eq!(
            count, 5,
            "connect_readonly must see all 5 WAL rows the old immutable=1 \
             path silently dropped"
        );

        drop(conn);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(wal_sidecar_path(&path));
        let _ = std::fs::remove_file({
            let mut name = path.file_name().unwrap().to_os_string();
            name.push("-shm");
            path.with_file_name(name)
        });
    }

    /// Proves the corrected policy refuses rather than silently degrading:
    /// with another connection holding an exclusive lock on a WAL database
    /// (the same failure shape observed live against a running Zotero
    /// 10.0.1 -- SQLITE_BUSY on the first statement), `connect_readonly`
    /// must return a loud, actionable error, never a silent `immutable=1`
    /// fallback. This is the harness-fixture-fails-when-immutable=1-is-
    /// reintroduced gate criterion, expressed as a direct unit test: the
    /// fix under test refuses, so nothing here ever exercises `immutable=1`
    /// against this locked WAL database.
    #[test]
    fn connect_readonly_refuses_not_falls_back_when_wal_database_is_locked() {
        let path = temp_sqlite_path("wal-locked-refuse");
        let holder = open_wal_db(&path);
        // Plain WAL mode still allows concurrent readers even while a
        // writer holds the write lock -- that's the whole point of WAL.
        // Zotero's own connection instead holds SQLite's exclusive
        // *locking mode* (`PRAGMA locking_mode=EXCLUSIVE`, live-confirmed
        // interpretation against a real Zotero 10.0.1 process), which
        // blocks readers too. The lock isn't actually taken until the next
        // access, hence the trigger write.
        holder
            .pragma_update(None, "locking_mode", "EXCLUSIVE")
            .expect("set exclusive locking mode");
        holder
            .execute(
                "INSERT INTO items VALUES (99, 'trigger-exclusive-lock')",
                [],
            )
            .expect("trigger the exclusive lock to actually take effect");

        let err = connect_readonly(&path).expect_err(
            "must refuse, not silently fall back to immutable=1, while a WAL \
             database is exclusively locked",
        );
        let message = err.to_string();
        assert!(
            message.contains("holds an exclusive lock"),
            "error must explain the refusal, not a generic SQLite error: {message}"
        );

        drop(holder);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(wal_sidecar_path(&path));
        let _ = std::fs::remove_file({
            let mut name = path.file_name().unwrap().to_os_string();
            name.push("-shm");
            path.with_file_name(name)
        });
    }

    /// Non-WAL (rollback-journal) databases keep the pre-Zotero-10 behavior
    /// unconditionally: falling back to `immutable=1` when locked is safe
    /// there because there is no `-wal` file for it to miss. This is the
    /// "zero behavior change on Zotero <=9" guarantee.
    #[test]
    fn connect_readonly_falls_back_to_immutable_when_locked_non_wal_database() {
        let path = temp_sqlite_path("rollback-journal-locked");
        let holder = Connection::open(&path).expect("create scratch sqlite file");
        holder
            .execute_batch("CREATE TABLE items (id INTEGER PRIMARY KEY, val TEXT)")
            .expect("create schema");
        holder
            .execute("INSERT INTO items VALUES (1, 'row0')", [])
            .expect("insert and commit a row");
        holder
            .pragma_update(None, "locking_mode", "EXCLUSIVE")
            .expect("set exclusive locking mode");
        holder
            .execute("INSERT INTO items VALUES (2, 'row1')", [])
            .expect("trigger the exclusive lock to actually take effect");

        let conn = connect_readonly(&path)
            .expect("must fall back to immutable=1 on a locked non-WAL database, not refuse");
        let count: i64 = conn
            .query_row("SELECT count(*) FROM items", [], |row| row.get(0))
            .expect("count rows");
        assert_eq!(count, 2);

        drop(conn);
        drop(holder);
        let _ = std::fs::remove_file(&path);
    }

    // `resolve_attachment_real_path` is the highest cross-platform risk
    // area flagged in the plan. The parity harness's fixture data only
    // exercises the `storage:` branch for the item queried by every
    // `item file`/`item attachments` golden fixture (`REG12345`) — the
    // `file://` drive-letter path belongs to a *different* item
    // (`REG67890`) that no golden fixture command queries, and no
    // fixture attachment uses a non-localhost `file://` host at all.
    // A green harness run therefore does not exercise 4 of these 6
    // branches. Direct unit tests close that gap without needing new
    // SQL fixture rows, since this is a pure function.

    #[test]
    fn resolve_attachment_real_path_none_when_no_path() {
        assert_eq!(
            resolve_attachment_real_path(None, "KEY", Path::new("/data")),
            None
        );
        assert_eq!(
            resolve_attachment_real_path(Some(""), "KEY", Path::new("/data")),
            None
        );
    }

    #[test]
    fn resolve_attachment_real_path_storage_prefix() {
        let resolved = resolve_attachment_real_path(
            Some("storage:paper.pdf"),
            "ATTACHKEY",
            Path::new("/data"),
        )
        .unwrap();
        // normalize_resolve canonicalizes when the path exists, else falls
        // back to lexical `.`/`..` normalization -- assert the tail
        // structure rather than the exact absolute prefix so this passes
        // regardless of whether /data/storage/ATTACHKEY exists on the
        // machine running the test.
        assert!(resolved.ends_with(&format!(
            "storage{}ATTACHKEY{}paper.pdf",
            std::path::MAIN_SEPARATOR,
            std::path::MAIN_SEPARATOR
        )));
    }

    #[test]
    fn resolve_attachment_real_path_file_uri_drive_letter() {
        // `file:///C:/Users/Public/linked.pdf` -- REG67890's attachment
        // in the parity fixture data, but no golden fixture command
        // queries REG67890, so this branch is otherwise untested.
        let resolved = resolve_attachment_real_path(
            Some("file:///C:/Users/Public/linked.pdf"),
            "LINKATT1",
            Path::new("/data"),
        )
        .unwrap();
        assert_eq!(resolved, r"C:\Users\Public\linked.pdf");
    }

    #[test]
    fn resolve_attachment_real_path_file_uri_unc_host() {
        // No fixture attachment uses a non-localhost file:// host at
        // all -- zero coverage from the harness for this branch.
        let resolved = resolve_attachment_real_path(
            Some("file://fileserver/share/paper.pdf"),
            "LINKATT2",
            Path::new("/data"),
        )
        .unwrap();
        assert_eq!(resolved, r"\\fileserver\share\paper.pdf");
    }

    #[test]
    fn resolve_attachment_real_path_file_uri_localhost_is_not_unc() {
        // `localhost` must NOT trigger the UNC branch (Python:
        // `parsed.netloc.lower() != "localhost"`).
        let resolved = resolve_attachment_real_path(
            Some("file://localhost/tmp/x.pdf"),
            "K",
            Path::new("/data"),
        )
        .unwrap();
        assert_ne!(&resolved[..2], r"\\");
    }

    #[test]
    fn resolve_attachment_real_path_file_uri_percent_decoded() {
        let resolved = resolve_attachment_real_path(
            Some("file:///tmp/my%20paper%20%28final%29.pdf"),
            "K",
            Path::new("/data"),
        )
        .unwrap();
        assert!(resolved.contains("my paper (final).pdf"));
    }

    #[test]
    fn resolve_attachment_real_path_absolute_bare_path_unchanged() {
        let abs = if cfg!(windows) {
            r"C:\already\absolute.pdf"
        } else {
            "/already/absolute.pdf"
        };
        assert_eq!(
            resolve_attachment_real_path(Some(abs), "K", Path::new("/data")).unwrap(),
            abs
        );
    }

    #[test]
    fn resolve_attachment_real_path_relative_bare_path_joins_data_dir() {
        let resolved =
            resolve_attachment_real_path(Some("subdir/file.pdf"), "K", Path::new("/data")).unwrap();
        assert!(resolved.ends_with(&format!("subdir{}file.pdf", std::path::MAIN_SEPARATOR)));
    }

    // `build_collection_tree`'s orphan-root case: a `parentCollectionID`
    // pointing outside the result set must become a root, not be
    // silently dropped. Untested by the golden fixtures -- the base
    // fixture's only nested collection ("Nested Collection", parent =
    // "Sample Collection") has its parent in the same result set.
    #[test]
    fn build_collection_tree_orphan_parent_becomes_a_root() {
        let collections = vec![
            Collection {
                collection_id: 1,
                key: "AAAAAAAA".into(),
                collection_name: "Root".into(),
                parent_collection_id: None,
                library_id: 1,
                version: 1,
                item_count: 0,
            },
            Collection {
                collection_id: 2,
                key: "BBBBBBBB".into(),
                collection_name: "Orphan".into(),
                // Parent 999 is not present in this slice (e.g.
                // filtered out by library scope) -- must become a root,
                // matching Python's `by_id.get(parent_id) is None`.
                parent_collection_id: Some(999),
                library_id: 1,
                version: 1,
                item_count: 0,
            },
        ];
        let tree = build_collection_tree(&collections);
        assert_eq!(
            tree.len(),
            2,
            "orphan must become a second root, not vanish"
        );
        assert!(tree
            .iter()
            .any(|n| n.collection_id == 2 && n.children.is_empty()));
    }

    #[test]
    fn build_collection_tree_nests_a_real_child_under_its_parent() {
        let collections = vec![
            Collection {
                collection_id: 1,
                key: "AAAAAAAA".into(),
                collection_name: "Root".into(),
                parent_collection_id: None,
                library_id: 1,
                version: 1,
                item_count: 0,
            },
            Collection {
                collection_id: 2,
                key: "BBBBBBBB".into(),
                collection_name: "Child".into(),
                parent_collection_id: Some(1),
                library_id: 1,
                version: 1,
                item_count: 0,
            },
        ];
        let tree = build_collection_tree(&collections);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].collection_id, 2);
    }
}
