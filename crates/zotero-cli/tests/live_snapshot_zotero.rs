//! Opt-in live check: builds the WAL-lock fallback snapshot from a real running Zotero through the
//! owned CLI Bridge, checks that a second build with an unchanged Zotero reuses it, and, when
//! the database can also be read directly, compares the catalog tables row for row.
//!
//! Run with Zotero open and the Bridge healthy:
//!
//! ```text
//! ZOTERO_LIVE_PORT=23119 ZOTERO_LIVE_SQLITE=~/Zotero/zotero.sqlite \
//!   cargo test -p zotero-cli --test live_snapshot_zotero -- --ignored
//! ```

use rusqlite::{Connection, OpenFlags};
use zotero_cli::live_snapshot::{self, BridgeSnapshotSource};

const CATALOG_TABLES: &[&str] = &[
    "libraries",
    "groups",
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
    "deletedItems",
    "savedSearches",
];

fn table_digest(conn: &Connection, table: &str) -> Option<(usize, u64)> {
    use std::hash::{Hash, Hasher};
    let mut stmt = conn.prepare(&format!("SELECT * FROM \"{table}\"")).ok()?;
    let n = stmt.column_count();
    let mut rows: Vec<String> = stmt
        .query_map([], |r| {
            let mut parts = Vec::with_capacity(n);
            for i in 0..n {
                parts.push(format!("{:?}", r.get::<_, rusqlite::types::Value>(i)?));
            }
            Ok(parts.join("\u{1f}"))
        })
        .ok()?
        .map(Result::unwrap)
        .collect();
    // Order-independent: physical row order is not part of the contract.
    rows.sort();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    rows.hash(&mut hasher);
    Some((rows.len(), hasher.finish()))
}

#[test]
#[ignore = "needs a running Zotero with the owned CLI Bridge"]
fn live_snapshot_matches_direct_read_table_by_table() {
    let port: u16 = std::env::var("ZOTERO_LIVE_PORT")
        .expect("set ZOTERO_LIVE_PORT")
        .parse()
        .expect("numeric port");
    let sqlite = std::env::var("ZOTERO_LIVE_SQLITE").expect("set ZOTERO_LIVE_SQLITE");

    let cache = std::env::temp_dir().join(format!("zotero-cli-live-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);
    let sqlite_path = std::path::PathBuf::from(&sqlite);

    live_snapshot::register_source(Box::new(BridgeSnapshotSource::new(port)), cache.clone());
    let started = std::time::Instant::now();
    let snapshot = live_snapshot::connect(&sqlite_path)
        .expect("snapshot build must not error")
        .expect("Bridge must answer");
    let cold = started.elapsed();

    // A fresh registration stands in for the next CLI invocation.
    live_snapshot::register_source(Box::new(BridgeSnapshotSource::new(port)), cache.clone());
    let started = std::time::Instant::now();
    drop(
        live_snapshot::connect(&sqlite_path)
            .expect("warm snapshot must not error")
            .expect("Bridge must answer"),
    );
    let warm = started.elapsed();
    eprintln!("snapshot cold {cold:?}, warm {warm:?}");
    assert!(
        warm < cold,
        "an unchanged Zotero must reuse the cached copy"
    );

    // Zotero 10 holds an exclusive lock while running; compare only when a direct read works.
    if let Ok(direct) = Connection::open_with_flags(
        format!("file:{sqlite}?mode=ro"),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    ) {
        if direct.query_row("SELECT 1", [], |_| Ok(())).is_ok() {
            for table in CATALOG_TABLES {
                let Some(expected) = table_digest(&direct, table) else {
                    continue;
                };
                let actual = table_digest(&snapshot, table)
                    .unwrap_or_else(|| panic!("table {table} missing from snapshot"));
                assert_eq!(actual, expected, "table {table} differs");
            }
        }
    }
    live_snapshot::clear_source();
    let _ = std::fs::remove_dir_all(&cache);
}
