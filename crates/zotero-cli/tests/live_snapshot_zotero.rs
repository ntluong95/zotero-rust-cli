//! Opt-in live check: builds the WAL-lock fallback snapshot from a real running Zotero through the
//! owned CLI Bridge and compares every copied table against a direct read of `zotero.sqlite`.
//!
//! Run with Zotero open and the Bridge healthy, while Zotero is *not* holding its lock (so the
//! direct read is possible to compare against):
//!
//! ```text
//! ZOTERO_LIVE_PORT=23119 ZOTERO_LIVE_SQLITE=~/Zotero/zotero.sqlite \
//!   cargo test -p zotero-cli --test live_snapshot_zotero -- --ignored
//! ```

use rusqlite::{Connection, OpenFlags};
use zotero_cli::live_snapshot::{self, BridgeSnapshotSource, SNAPSHOT_TABLES};

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

    live_snapshot::register_source(Box::new(BridgeSnapshotSource::new(port)));
    let started = std::time::Instant::now();
    let snapshot = live_snapshot::connect()
        .expect("snapshot build must not error")
        .expect("Bridge must answer");
    let elapsed = started.elapsed();

    let direct = Connection::open_with_flags(
        format!("file:{sqlite}?mode=ro"),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .expect("direct read (Zotero must not hold its lock for this comparison)");

    let mut compared = 0;
    for table in SNAPSHOT_TABLES {
        let Some(expected) = table_digest(&direct, table) else {
            continue;
        };
        let actual = table_digest(&snapshot, table)
            .unwrap_or_else(|| panic!("table {table} missing from snapshot"));
        assert_eq!(actual, expected, "table {table} differs");
        compared += 1;
    }
    eprintln!("snapshot of {compared} tables built in {elapsed:?}");
    assert!(compared > 10);
    live_snapshot::clear_source();
}
