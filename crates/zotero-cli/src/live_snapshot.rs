//! Live, WAL-consistent read snapshot for when a running Zotero holds the database lock.
//!
//! A running Zotero 10 keeps `zotero.sqlite` in WAL mode under an exclusive lock, so
//! [`crate::db::connect_readonly`] cannot open it and correctly refuses rather than reading a
//! stale `immutable=1` view. Every catalog read (`collection *`, `item get/children/...`,
//! `note get`, `tag *`, `search *`, `session use-library`, item resolution for rendering) used
//! to fail in exactly the state a user is most often in.
//!
//! When that refusal happens and an owned CLI Bridge answers, this module asks the running
//! Zotero to read the catalog tables through its *own* connection -- which sees every committed
//! WAL frame -- and loads them into a private in-memory SQLite database with Zotero's exact DDL.
//! `connect_readonly` then hands back a read-only connection to that copy, so the existing SQL in
//! `db.rs` runs unchanged and produces byte-identical output to the offline path.
//!
//! Properties:
//!
//! - **Never speculative.** Only reached after `mode=ro` has actually refused with
//!   [`crate::db::DatabaseLocked`] on a WAL database. Offline runs issue no extra requests.
//! - **Never stale.** Built from the live connection once per process (one CLI invocation) and
//!   never persisted to disk.
//! - **Read-only.** The Bridge template only runs `SELECT` statements against `sqlite_master`
//!   and the catalog tables below; no Zotero object is loaded or saved.
//! - **Honest failure.** If no source is registered or the Bridge cannot answer, the caller
//!   returns the original refusal verbatim.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};

/// Every table any `db.rs` query reads. Missing tables (older schemas) are simply absent from the
/// live schema and skipped.
pub const SNAPSHOT_TABLES: &[&str] = &[
    "libraries",
    "groups",
    "feeds",
    "itemTypes",
    "fields",
    "items",
    "itemData",
    "itemDataValues",
    "creators",
    "itemCreators",
    "tags",
    "itemTags",
    "collections",
    "collectionItems",
    "itemNotes",
    "itemAttachments",
    "itemAnnotations",
    "deletedItems",
    "deletedCollections",
    "savedSearches",
    "savedSearchConditions",
];

/// Response-size budget per Bridge page. The Bridge client reads bodies with ureq's default
/// 10 MB limit, and the page is JSON-encoded twice on the way back, so stay well under it.
const PAGE_BYTE_BUDGET: usize = 1_000_000;
/// Upper bound on serialized rows retained temporarily inside Zotero. Actual JS heap use is
/// higher; refuse large catalogs instead of risking an incomplete or unbounded capture.
const CAPTURE_BYTE_BUDGET: usize = 256_000_000;

/// Read-only by construction: `SELECT` against `sqlite_master` and a caller-supplied table name
/// that Rust only ever takes from [`SNAPSHOT_TABLES`].
const T_SNAPSHOT: &str = r#"
var cache = globalThis.__zoteroCliCatalogCaptures ||
  (globalThis.__zoteroCliCatalogCaptures = new Map());
var now = Date.now();
for (var entry of cache) {
  if (now - entry[1].created > 300000) { cache.delete(entry[0]); }
}
if (P.op === 'release') { cache.delete(P.token); return JSON.stringify({released: true}); }
if (P.op === 'capture') {
  var marks = P.tables.map(function () { return '?'; }).join(',');
  var captured = {created: now, tables: Object.create(null), schema: []};
  var total = 0;
  // Zotero's DB connection runs the callback in one SQLite transaction. All table reads see
  // the same point in time, even if the library changes while pages are transferred later.
  await Zotero.DB.executeTransaction(async function () {
    var schema = await Zotero.DB.queryAsync(
      "SELECT type, name, tbl_name, sql FROM sqlite_master WHERE sql IS NOT NULL " +
      "AND type IN ('table', 'index') AND substr(name, 1, 7) != 'sqlite_' " +
      "AND tbl_name IN (" + marks + ")", P.tables);
    captured.schema = (schema || []).map(function (r) {
      return {type: r.type, name: r.name, table: r.tbl_name, sql: r.sql};
    });
    for (var table of P.tables) {
      if (!captured.schema.some(function (s) { return s.type === 'table' && s.table === table; })) {
        continue;
      }
      var rows = [];
      await Zotero.DB.queryAsync('SELECT * FROM "' + table + '"', [], {
        onRow: function (row) {
          var values = [];
          for (var i = 0; i < row.numEntries; i++) {
            var v = row.getResultByIndex(i);
            if (row.getTypeOfIndex(i) === 4) { v = {'$b': Array.prototype.slice.call(v)}; }
            values.push(v);
          }
          var size = JSON.stringify(values).length;
          if (size > P.maxPageBytes) {
            throw new Error('live snapshot: one catalog row exceeds page byte budget');
          }
          total += size;
          if (total > P.maxCaptureBytes) {
            throw new Error('live snapshot: catalog exceeds capture byte budget');
          }
          rows.push(values);
        }
      });
      captured.tables[table] = rows;
    }
  });
  var token = Zotero.Utilities.randomString(24);
  cache.set(token, captured);
  setTimeout(function () { cache.delete(token); }, 300000);
  return JSON.stringify({token: token, schema: captured.schema});
}
var captured = cache.get(P.token);
if (!captured) { throw new Error('live snapshot: capture expired'); }
var sourceRows = captured.tables[P.table];
if (!sourceRows) { throw new Error('live snapshot: table absent from capture'); }
var out = [];
var bytes = 0;
var offset = P.offset;
while (offset < sourceRows.length) {
  var size = JSON.stringify(sourceRows[offset]).length;
  if (bytes + size > P.maxBytes && out.length) { break; }
  out.push(sourceRows[offset]);
  bytes += size;
  offset++;
}
return JSON.stringify({rows: out, next: offset < sourceRows.length ? offset : null});
"#;

/// Where snapshot data comes from. The production source is the owned Bridge; tests supply a
/// fake that answers the same two requests from a fixture database.
pub trait SnapshotSource {
    /// Answers capture with `{token, schema}`, then tokenized rows with `{rows, next}`.
    /// `None` means the source cannot answer at all.
    fn request(&self, request: &Value) -> Option<Value>;
}

/// The production source: the owned CLI Bridge on `port`, whose ownership handshake runs before
/// any code is sent.
pub struct BridgeSnapshotSource {
    port: u16,
}

impl BridgeSnapshotSource {
    pub fn new(port: u16) -> Self {
        Self { port }
    }
}

impl SnapshotSource for BridgeSnapshotSource {
    fn request(&self, request: &Value) -> Option<Value> {
        let code = crate::bridge::templates::render(T_SNAPSHOT, request).ok()?;
        let resp = crate::bridge::JSBridgeClient::new(self.port).execute_js(&code, 60);
        if !resp.ok {
            // Preserve the caller's original locked-database refusal when the Bridge is absent.
            // A capture rejected inside Zotero must retain its explicit reason.
            let error = resp.error?;
            return error
                .contains("live snapshot:")
                .then(|| json!({"error": error}));
        }
        match resp.data? {
            Value::String(text) => serde_json::from_str(&text).ok(),
            other => Some(other),
        }
    }
}

struct Built {
    uri: String,
    /// Keeps the shared in-memory database alive for the rest of the process.
    _keeper: Connection,
}

thread_local! {
    static SOURCE: RefCell<Option<Box<dyn SnapshotSource>>> = RefCell::new(None);
    static BUILT: RefCell<Option<Built>> = const { RefCell::new(None) };
    static SUPPRESSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f` with the snapshot fallback switched off, so a locked database is refused as before.
///
/// For callers that own a cheaper, purpose-built live path (`search.rs`: `item find` and
/// `library list` answer with a single Bridge query) and need the refusal to trigger it rather
/// than paying for a full catalog copy.
pub fn without_snapshot<T>(f: impl FnOnce() -> T) -> T {
    let previous = SUPPRESSED.with(|s| s.replace(true));
    let result = f();
    SUPPRESSED.with(|s| s.set(previous));
    result
}

static SNAPSHOT_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Registers the source used when the database is locked. Per-thread, so the CLI's single
/// dispatch thread gets it and parallel tests stay isolated. Clears any previously built copy.
pub fn register_source(source: Box<dyn SnapshotSource>) {
    SOURCE.with(|s| *s.borrow_mut() = Some(source));
    BUILT.with(|b| *b.borrow_mut() = None);
}

/// Removes the registered source and any built copy (tests).
pub fn clear_source() {
    SOURCE.with(|s| *s.borrow_mut() = None);
    BUILT.with(|b| *b.borrow_mut() = None);
}

/// A read-only connection to the live snapshot, building it on first use. `Ok(None)` when no
/// source is registered or the source cannot answer -- the caller then reports its refusal.
pub fn connect() -> anyhow::Result<Option<Connection>> {
    if SUPPRESSED.with(|s| s.get()) {
        return Ok(None);
    }
    let existing = BUILT.with(|b| b.borrow().as_ref().map(|built| built.uri.clone()));
    let uri = match existing {
        Some(uri) => uri,
        None => {
            let built = SOURCE.with(|s| s.borrow().as_ref().map(|source| build(source.as_ref())));
            match built {
                Some(Ok(Some(built))) => {
                    let uri = built.uri.clone();
                    BUILT.with(|b| *b.borrow_mut() = Some(built));
                    uri
                }
                Some(Err(err)) => return Err(err),
                Some(Ok(None)) | None => return Ok(None),
            }
        }
    };
    let conn = Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    // SQLite ignores SQLITE_OPEN_READ_ONLY for a shared-cache in-memory database, so enforce
    // read-only at the connection level: any write statement now fails.
    conn.pragma_update(None, "query_only", true)?;
    conn.query_row("SELECT 1", [], |_| Ok(()))?;
    Ok(Some(conn))
}

fn build(source: &dyn SnapshotSource) -> anyhow::Result<Option<Built>> {
    let Some(capture) = source.request(&json!({
        "op": "capture", "tables": SNAPSHOT_TABLES,
        "maxPageBytes": PAGE_BYTE_BUDGET, "maxCaptureBytes": CAPTURE_BYTE_BUDGET,
    })) else {
        return Ok(None);
    };
    if let Some(error) = capture.get("error").and_then(Value::as_str) {
        anyhow::bail!("{error}");
    }
    let token = capture
        .get("token")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("live snapshot: missing capture token"))?;
    let result = build_captured(source, &capture, token);
    // A failed import must not strand a large copy inside Zotero. Interrupted processes are
    // covered by the Bridge cache's five-minute expiry.
    let _ = source.request(&json!({"op": "release", "token": token}));
    result.map(Some)
}

fn build_captured(
    source: &dyn SnapshotSource,
    capture: &Value,
    token: &str,
) -> anyhow::Result<Built> {
    let Some(entries) = capture.get("schema").and_then(Value::as_array) else {
        anyhow::bail!("live snapshot: unexpected schema response: {capture}");
    };

    let uri = format!(
        "file:zotero-live-snapshot-{}-{}?mode=memory&cache=shared",
        std::process::id(),
        SNAPSHOT_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let mut keeper = Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI,
    )?;

    // rusqlite's bundled SQLite enables foreign keys by default; Zotero's DDL references tables
    // this copy deliberately leaves out, and the rows are already consistent, so skip enforcement.
    keeper.pragma_update(None, "foreign_keys", false)?;

    let entry_str = |entry: &Value, key: &str| -> Option<String> {
        entry.get(key).and_then(Value::as_str).map(str::to_string)
    };
    let mut tables = Vec::new();
    let mut indexes = Vec::new();
    for entry in entries {
        let (Some(kind), Some(table), Some(sql)) = (
            entry_str(entry, "type"),
            entry_str(entry, "table"),
            entry_str(entry, "sql"),
        ) else {
            continue;
        };
        // Only tables Rust asked for; the Bridge response is not trusted to widen the scope.
        if !SNAPSHOT_TABLES.contains(&table.as_str()) {
            continue;
        }
        match kind.as_str() {
            "table" => tables.push((table, sql)),
            "index" => indexes.push(sql),
            _ => {}
        }
    }

    let tx = keeper.transaction()?;
    for (_, sql) in &tables {
        tx.execute_batch(sql)?;
    }
    for (table, _) in &tables {
        let mut offset: i64 = 0;
        loop {
            let page = source
                .request(&json!({
                    "op": "rows",
                    "token": token,
                    "table": table,
                    "offset": offset,
                    "maxBytes": PAGE_BYTE_BUDGET,
                }))
                .ok_or_else(|| anyhow::anyhow!("live snapshot: no response reading {table}"))?;
            let rows = page
                .get("rows")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow::anyhow!("live snapshot: bad page for {table}"))?;
            for row in rows {
                let values = row
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("live snapshot: bad row in {table}"))?;
                let placeholders = vec!["?"; values.len()].join(", ");
                let mut stmt =
                    tx.prepare_cached(&format!("INSERT INTO \"{table}\" VALUES ({placeholders})"))?;
                let params: Vec<rusqlite::types::Value> = values.iter().map(to_sql_value).collect();
                stmt.execute(rusqlite::params_from_iter(params))?;
            }
            match page.get("next").and_then(Value::as_i64) {
                Some(next) if next > offset => offset = next,
                _ => break,
            }
        }
    }
    for sql in &indexes {
        tx.execute_batch(sql)?;
    }
    tx.commit()?;

    Ok(Built {
        uri,
        _keeper: keeper,
    })
}

fn to_sql_value(value: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as Sql;
    match value {
        Value::Null => Sql::Null,
        Value::Bool(b) => Sql::Integer(i64::from(*b)),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Sql::Integer(i),
            None => Sql::Real(n.as_f64().unwrap_or(0.0)),
        },
        Value::String(s) => Sql::Text(s.clone()),
        Value::Object(map) => match map.get("$b").and_then(Value::as_array) {
            Some(bytes) => Sql::Blob(
                bytes
                    .iter()
                    .map(|b| b.as_u64().unwrap_or(0) as u8)
                    .collect(),
            ),
            None => Sql::Text(value.to_string()),
        },
        Value::Array(_) => Sql::Text(value.to_string()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Answers snapshot requests from a real SQLite file, exactly as the Bridge would from
    /// Zotero's own connection.
    pub(crate) struct FixtureSource {
        pub path: std::path::PathBuf,
        pub page_rows: usize,
    }

    impl SnapshotSource for FixtureSource {
        fn request(&self, request: &Value) -> Option<Value> {
            let captured_path = self.path.with_extension("captured.sqlite");
            if request["op"] == "release" {
                let _ = std::fs::remove_file(captured_path);
                return Some(json!({"released": true}));
            }
            if request["op"] == "capture" {
                std::fs::copy(&self.path, &captured_path).ok()?;
            }
            let conn = Connection::open(&captured_path).ok()?;
            if request["op"] == "capture" {
                let wanted: Vec<String> = request["tables"]
                    .as_array()?
                    .iter()
                    .filter_map(|t| t.as_str().map(str::to_string))
                    .collect();
                let mut stmt = conn
                    .prepare(
                        "SELECT type, name, tbl_name, sql FROM sqlite_master WHERE sql IS NOT NULL \
                         AND type IN ('table','index') AND name NOT LIKE 'sqlite_%'",
                    )
                    .ok()?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok(
                            json!({"type": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?,
                                  "table": r.get::<_, String>(2)?, "sql": r.get::<_, String>(3)?}),
                        )
                    })
                    .ok()?
                    .filter_map(Result::ok)
                    .filter(|e| wanted.iter().any(|w| e["table"] == w.as_str()))
                    .collect::<Vec<_>>();
                return Some(json!({"token": "fixture", "schema": rows}));
            }
            let table = request["table"].as_str()?;
            let offset = request["offset"].as_i64()?;
            let mut stmt = conn
                .prepare(&format!("SELECT * FROM \"{table}\" LIMIT ?1 OFFSET ?2"))
                .ok()?;
            let n = stmt.column_count();
            let rows: Vec<Value> = stmt
                .query_map([self.page_rows as i64 + 1, offset], |r| {
                    let mut values = Vec::new();
                    for i in 0..n {
                        let v: rusqlite::types::Value = r.get(i)?;
                        values.push(match v {
                            rusqlite::types::Value::Null => Value::Null,
                            rusqlite::types::Value::Integer(i) => json!(i),
                            rusqlite::types::Value::Real(f) => json!(f),
                            rusqlite::types::Value::Text(t) => json!(t),
                            rusqlite::types::Value::Blob(b) => json!({"$b": b}),
                        });
                    }
                    Ok(Value::Array(values))
                })
                .ok()?
                .filter_map(Result::ok)
                .collect();
            let more = rows.len() > self.page_rows;
            let rows: Vec<Value> = rows.into_iter().take(self.page_rows).collect();
            let next = more.then_some(offset + self.page_rows as i64);
            Some(json!({"rows": rows, "next": next}))
        }
    }

    struct Unavailable;
    impl SnapshotSource for Unavailable {
        fn request(&self, _request: &Value) -> Option<Value> {
            None
        }
    }

    struct MutatingSource {
        fixture: FixtureSource,
        changed: std::cell::Cell<bool>,
    }

    impl SnapshotSource for MutatingSource {
        fn request(&self, request: &Value) -> Option<Value> {
            let response = self.fixture.request(request);
            if request["op"] == "rows" && request["table"] == "tags" && !self.changed.replace(true)
            {
                // Insert ahead of the next OFFSET and delete a later row after the first page.
                // Independent live queries would return a skipped or mixed catalog.
                let conn = Connection::open(&self.fixture.path).ok()?;
                conn.execute("DELETE FROM tags WHERE tagID = 3", []).ok()?;
                conn.execute("INSERT INTO tags VALUES (0, 'new')", [])
                    .ok()?;
            }
            response
        }
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "zotero-cli-live-snapshot-{name}-{}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE libraries (libraryID INTEGER PRIMARY KEY, type TEXT, editable INT);
             INSERT INTO libraries VALUES (1, 'user', 1), (2, 'group', 0);
             CREATE TABLE tags (tagID INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
             CREATE INDEX tags_name ON tags(name);
             INSERT INTO tags VALUES (1, 'alpha'), (2, 'beta'), (3, 'gamma');
             CREATE TABLE unrelated (x INT); INSERT INTO unrelated VALUES (42);",
        )
        .unwrap();
        path
    }

    #[test]
    fn snapshot_copies_requested_tables_across_pages_and_skips_others() {
        let path = fixture("pages");
        register_source(Box::new(FixtureSource {
            path: path.clone(),
            page_rows: 1,
        }));
        let conn = connect().unwrap().expect("snapshot built");
        let names: Vec<String> = conn
            .prepare("SELECT name FROM tags ORDER BY tagID")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(names, ["alpha", "beta", "gamma"]);
        let libraries: i64 = conn
            .query_row("SELECT COUNT(*) FROM libraries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(libraries, 2);
        assert!(
            conn.query_row("SELECT x FROM unrelated", [], |r| r.get::<_, i64>(0))
                .is_err(),
            "tables outside SNAPSHOT_TABLES must never be copied"
        );
        assert!(
            conn.execute("DELETE FROM tags", []).is_err(),
            "the handed-out connection must be read-only"
        );
        clear_source();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn source_mutation_between_pages_does_not_change_captured_catalog() {
        let path = fixture("mutation");
        register_source(Box::new(MutatingSource {
            fixture: FixtureSource {
                path: path.clone(),
                page_rows: 1,
            },
            changed: std::cell::Cell::new(false),
        }));
        let conn = connect().unwrap().expect("snapshot built");
        let names: Vec<String> = conn
            .prepare("SELECT name FROM tags ORDER BY tagID")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(names, ["alpha", "beta", "gamma"]);
        clear_source();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn snapshot_is_built_once_per_registration() {
        let path = fixture("once");
        register_source(Box::new(FixtureSource {
            path: path.clone(),
            page_rows: 100,
        }));
        let first = connect().unwrap().unwrap();
        std::fs::remove_file(&path).unwrap();
        // The source file is gone; a second connect must reuse the built copy.
        let second = connect().unwrap().expect("reused snapshot");
        let count = |c: &Connection| -> i64 {
            c.query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(count(&first), count(&second));
        clear_source();
    }

    #[test]
    fn no_source_or_unavailable_source_yields_none() {
        clear_source();
        assert!(connect().unwrap().is_none());
        register_source(Box::new(Unavailable));
        assert!(connect().unwrap().is_none());
        clear_source();
    }

    #[test]
    fn snapshot_template_is_read_only() {
        for verb in [
            "saveTx", "eraseTx", "merge", "trash", "setField", "INSERT", "UPDATE", "DELETE",
            "DROP", "ALTER",
        ] {
            assert!(
                !T_SNAPSHOT.contains(verb),
                "snapshot template must not contain `{verb}`"
            );
        }
    }

    #[test]
    fn without_snapshot_suppresses_and_restores() {
        let path = fixture("suppressed");
        register_source(Box::new(FixtureSource {
            path: path.clone(),
            page_rows: 100,
        }));
        assert!(without_snapshot(|| connect().unwrap().is_none()));
        assert!(
            connect().unwrap().is_some(),
            "suppression must end with the closure"
        );
        clear_source();
        let _ = std::fs::remove_file(&path);
    }
}
