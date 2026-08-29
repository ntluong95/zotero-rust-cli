//! Semantic search vector store module: indexing, search, and similarity lookup.
//!
//! Preserves SQLite schema and little-endian f32 blob layout from `core/semantic.py`.
//! Fixes D2 (SQL injection) using parameter-bound queries for language and key filters.

pub mod embed;
pub mod vectors;

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use serde_json::Value;

use crate::error::DomainError;
pub use embed::SemanticConfig;

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum BuildIndexOutput {
    Success(BuildIndexSuccess),
    Failure(BuildIndexFailure),
}

impl BuildIndexOutput {
    pub fn is_ok(&self) -> bool {
        match self {
            BuildIndexOutput::Success(s) => s.ok,
            BuildIndexOutput::Failure(f) => f.ok,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildIndexSuccess {
    pub ok: bool,
    pub indexed: usize,
    pub skipped: usize,
    pub total: usize,
    pub db_path: String,
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildIndexFailure {
    pub ok: bool,
    pub indexed: usize,
    pub skipped: usize,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchResultItem {
    pub item_key: String,
    pub score: f64,
    pub chunk_text: String,
    pub language: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SemanticErrorOutput {
    pub ok: bool,
    pub data: Option<()>,
    pub error: String,
}

impl SemanticErrorOutput {
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: error.into(),
        }
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| {
            serde_json::json!({
                "ok": false,
                "data": null,
                "error": self.error
            })
        })
    }
}

pub struct VectorRow {
    pub item_key: String,
    pub chunk_id: i64,
    pub vector_blob: Vec<u8>,
    pub chunk_text: Option<String>,
    pub language: Option<String>,
}

/// Connect to vector DB in read-only immutable mode (`semantic.py:184`).
pub fn connect_vector_db_ro(path: &Path) -> Result<Connection, DomainError> {
    if !path.exists() {
        return Err(DomainError::new(format!(
            "Vector DB not found: {}",
            path.display()
        )));
    }
    let posix_path = path.to_string_lossy().replace('\\', "/");
    let uri = format!("file:{posix_path}?mode=ro&immutable=1");
    let conn = Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| DomainError::new(format!("{e}")))?;
    conn.busy_timeout(Duration::from_secs(1))
        .map_err(|e| DomainError::new(format!("{e}")))?;
    Ok(conn)
}

/// Connect to vector DB in read-write mode, creating tables if needed (`semantic.py:117-124`).
pub fn connect_vector_db_rw(path: &Path) -> Result<Connection, DomainError> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    let conn = Connection::open(path).map_err(|e| DomainError::new(format!("{e}")))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|e| DomainError::new(format!("{e}")))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS embeddings (
            item_key TEXT, chunk_id INTEGER, chunk_text TEXT, language TEXT,
            PRIMARY KEY (item_key, chunk_id));
        CREATE TABLE IF NOT EXISTS vectors_f32 (
            item_key TEXT, chunk_id INTEGER, vector BLOB,
            PRIMARY KEY (item_key, chunk_id));",
    )
    .map_err(|e| DomainError::new(format!("{e}")))?;
    Ok(conn)
}

/// Load float32 vectors with metadata with bound parameter queries fixing D2 (`semantic.py:56-67`).
pub fn load_f32_vectors(
    conn: &Connection,
    language: &str,
    exclude_key: Option<&str>,
) -> Result<Vec<VectorRow>, DomainError> {
    let mut sql = String::from(
        "SELECT e.item_key, e.chunk_id, v.vector, e.chunk_text, e.language \
         FROM vectors_f32 v \
         JOIN embeddings e ON v.item_key = e.item_key AND v.chunk_id = e.chunk_id \
         WHERE 1=1",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if language != "all" {
        params.push(Box::new(language.to_string()));
        sql.push_str(&format!(" AND e.language = ?{}", params.len()));
    }
    if let Some(key) = exclude_key {
        params.push(Box::new(key.to_string()));
        sql.push_str(&format!(" AND e.item_key != ?{}", params.len()));
    }

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| DomainError::new(format!("{e}")))?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows = stmt
        .query_map(param_refs.as_slice(), |row| {
            Ok(VectorRow {
                item_key: row.get(0)?,
                chunk_id: row.get(1)?,
                vector_blob: row.get(2)?,
                chunk_text: row.get(3)?,
                language: row.get(4)?,
            })
        })
        .map_err(|e| DomainError::new(format!("{e}")))?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row.map_err(|e| DomainError::new(format!("{e}")))?);
    }
    Ok(result)
}

/// Build vector index from Zotero SQLite database (`semantic.py:83-164`).
pub fn build_index(
    zotero_sqlite: &Path,
    config: &SemanticConfig,
    batch_size: usize,
) -> BuildIndexOutput {
    if !zotero_sqlite.exists() {
        return BuildIndexOutput::Failure(BuildIndexFailure {
            ok: false,
            indexed: 0,
            skipped: 0,
            error: format!("Zotero DB not found: {}", zotero_sqlite.display()),
        });
    }

    let src = match crate::db::connect_readonly(zotero_sqlite) {
        Ok(c) => c,
        Err(e) => {
            return BuildIndexOutput::Failure(BuildIndexFailure {
                ok: false,
                indexed: 0,
                skipped: 0,
                error: format!("Read error: {e}"),
            });
        }
    };

    let sql = "SELECT i.key, MAX(CASE WHEN f.fieldName='title' THEN iv.value END), \
                      MAX(CASE WHEN f.fieldName='abstractNote' THEN iv.value END) \
               FROM items i \
               JOIN itemData id ON i.itemID = id.itemID \
               JOIN itemDataValues iv ON id.valueID = iv.valueID \
               JOIN fields f ON id.fieldID = f.fieldID \
               WHERE i.itemTypeID NOT IN (1, 14) \
                 AND f.fieldName IN ('title', 'abstractNote') \
               GROUP BY i.key";

    let mut stmt = match src.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            return BuildIndexOutput::Failure(BuildIndexFailure {
                ok: false,
                indexed: 0,
                skipped: 0,
                error: format!("Read error: {e}"),
            });
        }
    };

    let rows: Vec<(String, Option<String>, Option<String>)> = match stmt.query_map([], |row| {
        let key: String = row.get(0)?;
        let title: Option<String> = row.get(1)?;
        let abstract_note: Option<String> = row.get(2)?;
        Ok((key, title, abstract_note))
    }) {
        Ok(iter) => {
            let mut items = Vec::new();
            for r in iter {
                match r {
                    Ok(item) => items.push(item),
                    Err(e) => {
                        return BuildIndexOutput::Failure(BuildIndexFailure {
                            ok: false,
                            indexed: 0,
                            skipped: 0,
                            error: format!("Read error: {e}"),
                        });
                    }
                }
            }
            items
        }
        Err(e) => {
            return BuildIndexOutput::Failure(BuildIndexFailure {
                ok: false,
                indexed: 0,
                skipped: 0,
                error: format!("Read error: {e}"),
            });
        }
    };
    drop(stmt);
    drop(src);

    if rows.is_empty() {
        return BuildIndexOutput::Success(BuildIndexSuccess {
            ok: true,
            indexed: 0,
            skipped: 0,
            total: 0,
            db_path: config.vector_db.display().to_string(),
            error: None,
            errors: None,
        });
    }

    let mut db = match connect_vector_db_rw(&config.vector_db) {
        Ok(c) => c,
        Err(e) => {
            return BuildIndexOutput::Failure(BuildIndexFailure {
                ok: false,
                indexed: 0,
                skipped: 0,
                error: format!("DB error: {e}"),
            });
        }
    };

    let existing: HashSet<String> = {
        let mut existing_stmt = match db.prepare("SELECT DISTINCT item_key FROM embeddings") {
            Ok(s) => s,
            Err(e) => {
                return BuildIndexOutput::Failure(BuildIndexFailure {
                    ok: false,
                    indexed: 0,
                    skipped: 0,
                    error: format!("DB error: {e}"),
                });
            }
        };
        let mapped = existing_stmt.query_map([], |r| r.get(0));
        match mapped {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(_) => HashSet::new(),
        }
    };

    let mut indexed = 0usize;
    let mut skipped = 0usize;
    let mut errors: Vec<String> = Vec::new();

    let total = rows.len();

    // Use transaction for batch insertion performance
    let mut tx = match db.transaction() {
        Ok(t) => t,
        Err(e) => {
            return BuildIndexOutput::Failure(BuildIndexFailure {
                ok: false,
                indexed: 0,
                skipped: 0,
                error: format!("DB error: {e}"),
            });
        }
    };

    for (key, title, abstract_note) in rows {
        if existing.contains(&key) {
            skipped += 1;
            continue;
        }

        let mut text = String::new();
        if let Some(t) = &title {
            text.push_str(t);
        }
        text.push('\n');
        if let Some(a) = &abstract_note {
            text.push_str(a);
        }
        let text = text.trim().to_string();
        if text.is_empty() {
            skipped += 1;
            continue;
        }

        let vec = match embed::get_embedding(&text, config) {
            Ok(v) => v,
            Err(e) => {
                errors.push(format!("{key}: {e}"));
                continue;
            }
        };

        let lang = vectors::detect_language(&text);
        let chunk_text: String = text.chars().take(2000).collect();
        let encoded_vec = vectors::encode_f32_vector(&vec);

        if let Err(e) = tx.execute(
            "INSERT OR REPLACE INTO embeddings VALUES (?1, 0, ?2, ?3)",
            rusqlite::params![key, chunk_text, lang],
        ) {
            errors.push(format!("{key}: {e}"));
            continue;
        }

        if let Err(e) = tx.execute(
            "INSERT OR REPLACE INTO vectors_f32 VALUES (?1, 0, ?2)",
            rusqlite::params![key, encoded_vec],
        ) {
            errors.push(format!("{key}: {e}"));
            continue;
        }

        indexed += 1;

        if indexed.is_multiple_of(batch_size) {
            if let Err(e) = tx.commit() {
                return BuildIndexOutput::Failure(BuildIndexFailure {
                    ok: false,
                    indexed,
                    skipped,
                    error: format!("DB error: {e}"),
                });
            }
            tx = match db.transaction() {
                Ok(t) => t,
                Err(e) => {
                    return BuildIndexOutput::Failure(BuildIndexFailure {
                        ok: false,
                        indexed,
                        skipped,
                        error: format!("DB error: {e}"),
                    });
                }
            };
        }
    }

    if let Err(e) = tx.commit() {
        return BuildIndexOutput::Failure(BuildIndexFailure {
            ok: false,
            indexed,
            skipped,
            error: format!("DB error: {e}"),
        });
    }

    let error_sample = if errors.is_empty() {
        None
    } else {
        Some(errors.into_iter().take(10).collect())
    };

    BuildIndexOutput::Success(BuildIndexSuccess {
        ok: true,
        indexed,
        skipped,
        total,
        db_path: config.vector_db.display().to_string(),
        error: None,
        errors: error_sample,
    })
}

/// Semantic search across vector store (`semantic.py:166-213`).
pub fn semantic_search(
    query: &str,
    config: &SemanticConfig,
    top_k: usize,
    min_score: f32,
    language: &str,
) -> Result<Vec<SearchResultItem>, Value> {
    if !config.vector_db.exists() {
        return Err(SemanticErrorOutput::new(format!(
            "Vector DB not found: {}",
            config.vector_db.display()
        ))
        .to_value());
    }

    let query_vec = match embed::get_embedding(query, config) {
        Ok(v) => v,
        Err(e) => {
            return Err(SemanticErrorOutput::new(format!("Embedding API error: {e}")).to_value());
        }
    };

    let conn = match connect_vector_db_ro(&config.vector_db) {
        Ok(c) => c,
        Err(e) => {
            return Err(SemanticErrorOutput::new(format!("DB error: {e}")).to_value());
        }
    };

    let rows = match load_f32_vectors(&conn, language, None) {
        Ok(r) => r,
        Err(e) => {
            return Err(SemanticErrorOutput::new(format!("DB error: {e}")).to_value());
        }
    };

    let mut scored = Vec::new();
    for row in rows {
        let vec = match vectors::decode_f32_vector(&row.vector_blob) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let score = vectors::cosine_similarity(&query_vec, &vec);
        if score >= min_score {
            let chunk_text: String = row
                .chunk_text
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            scored.push(SearchResultItem {
                item_key: row.item_key,
                score: vectors::round_score(score),
                chunk_text,
                language: row.language.unwrap_or_else(|| "en".to_string()),
            });
        }
    }

    // Sort descending by score; break ties deterministically by item_key ascending
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.item_key.cmp(&b.item_key))
    });

    let mut seen = HashSet::new();
    let mut results = Vec::new();
    for item in scored {
        if seen.insert(item.item_key.clone()) {
            results.push(item);
            if results.len() >= top_k {
                break;
            }
        }
    }

    Ok(results)
}

/// Find items similar to a given item (`semantic.py:215-260`).
pub fn find_similar(
    item_key: &str,
    config: &SemanticConfig,
    top_k: usize,
    min_score: f32,
) -> Result<Vec<SearchResultItem>, Value> {
    if !config.vector_db.exists() {
        return Err(SemanticErrorOutput::new(format!(
            "Vector DB not found: {}",
            config.vector_db.display()
        ))
        .to_value());
    }

    let conn = match connect_vector_db_ro(&config.vector_db) {
        Ok(c) => c,
        Err(e) => {
            return Err(SemanticErrorOutput::new(format!("DB error: {e}")).to_value());
        }
    };

    let mut target_stmt =
        match conn.prepare("SELECT vector FROM vectors_f32 WHERE item_key = ?1 AND chunk_id = 0") {
            Ok(s) => s,
            Err(e) => {
                return Err(SemanticErrorOutput::new(format!("DB error: {e}")).to_value());
            }
        };

    let target_blob_res = target_stmt.query_row(rusqlite::params![item_key], |row| row.get(0));
    let target_blob: Vec<u8> = match target_blob_res {
        Ok(b) => b,
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            return Err(
                SemanticErrorOutput::new(format!("No embedding for item {item_key}")).to_value(),
            );
        }
        Err(e) => {
            return Err(SemanticErrorOutput::new(format!("DB error: {e}")).to_value());
        }
    };
    drop(target_stmt);

    let target_vec = match vectors::decode_f32_vector(&target_blob) {
        Ok(v) => v,
        Err(e) => {
            return Err(SemanticErrorOutput::new(format!("DB error: {e}")).to_value());
        }
    };

    let rows = match load_f32_vectors(&conn, "all", Some(item_key)) {
        Ok(r) => r,
        Err(e) => {
            return Err(SemanticErrorOutput::new(format!("DB error: {e}")).to_value());
        }
    };

    let mut scored = Vec::new();
    for row in rows {
        let vec = match vectors::decode_f32_vector(&row.vector_blob) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let score = vectors::cosine_similarity(&target_vec, &vec);
        if score >= min_score {
            let chunk_text: String = row
                .chunk_text
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            scored.push(SearchResultItem {
                item_key: row.item_key,
                score: vectors::round_score(score),
                chunk_text,
                language: row.language.unwrap_or_else(|| "en".to_string()),
            });
        }
    }

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.item_key.cmp(&b.item_key))
    });

    let mut seen = HashSet::new();
    let mut results = Vec::new();
    for item in scored {
        if seen.insert(item.item_key.clone()) {
            results.push(item);
            if results.len() >= top_k {
                break;
            }
        }
    }

    Ok(results)
}
