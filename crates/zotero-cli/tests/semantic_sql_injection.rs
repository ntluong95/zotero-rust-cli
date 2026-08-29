use zotero_cli::semantic::vectors::encode_f32_vector;
use zotero_cli::semantic::{connect_vector_db_rw, load_f32_vectors};

struct TestDir(std::path::PathBuf);
impl TestDir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "zotero-test-{}-{}",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&p);
        Self(p)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn test_sql_injection_language_filter_regression() {
    let tmp_dir = TestDir::new("sql-inj-lang");
    let db_path = tmp_dir.path().join("vectors.sqlite");

    let conn = connect_vector_db_rw(&db_path).unwrap();

    let vec_bytes = encode_f32_vector(&[1.0, 0.0]);
    conn.execute(
        "INSERT INTO embeddings VALUES ('KEY1', 0, 'Item 1 text', 'en')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO vectors_f32 VALUES ('KEY1', 0, ?1)",
        rusqlite::params![vec_bytes],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO embeddings VALUES ('KEY2', 0, 'Item 2 text', 'zh')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO vectors_f32 VALUES ('KEY2', 0, ?1)",
        rusqlite::params![vec_bytes],
    )
    .unwrap();

    // In python without parameterized queries:
    // lang_filter = f"AND e.language = '{language}'"
    // With language = "en' OR '1'='1", it becomes:
    // WHERE 1=1 AND e.language = 'en' OR '1'='1' (which returns ALL rows!)
    let hostile_lang = "en' OR '1'='1";
    let rows = load_f32_vectors(&conn, hostile_lang, None).unwrap();
    // In Rust with bound parameter ?1, it looks for literal language == "en' OR '1'='1", which has 0 rows.
    assert_eq!(rows.len(), 0);

    // Test with SQL drop attempt
    let drop_attempt = "en'; DROP TABLE embeddings; --";
    let rows_drop = load_f32_vectors(&conn, drop_attempt, None).unwrap();
    assert_eq!(rows_drop.len(), 0);

    // Verify table still exists and data is intact
    let count: i64 = conn
        .query_row("SELECT count(*) FROM embeddings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);

    // Valid filters still work
    let en_rows = load_f32_vectors(&conn, "en", None).unwrap();
    assert_eq!(en_rows.len(), 1);
    assert_eq!(en_rows[0].item_key, "KEY1");

    let zh_rows = load_f32_vectors(&conn, "zh", None).unwrap();
    assert_eq!(zh_rows.len(), 1);
    assert_eq!(zh_rows[0].item_key, "KEY2");

    let all_rows = load_f32_vectors(&conn, "all", None).unwrap();
    assert_eq!(all_rows.len(), 2);
}

#[test]
fn test_sql_injection_exclude_key_regression() {
    let tmp_dir = TestDir::new("sql-inj-key");
    let db_path = tmp_dir.path().join("vectors.sqlite");

    let conn = connect_vector_db_rw(&db_path).unwrap();

    let vec_bytes = encode_f32_vector(&[1.0, 0.0]);
    conn.execute(
        "INSERT INTO embeddings VALUES ('KEY1', 0, 'Item 1 text', 'en')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO vectors_f32 VALUES ('KEY1', 0, ?1)",
        rusqlite::params![vec_bytes],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO embeddings VALUES ('KEY2', 0, 'Item 2 text', 'en')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO vectors_f32 VALUES ('KEY2', 0, ?1)",
        rusqlite::params![vec_bytes],
    )
    .unwrap();

    // Hostile exclude_key
    let hostile_key = "KEY1' OR '1'='1";
    let rows = load_f32_vectors(&conn, "all", Some(hostile_key)).unwrap();
    // Exclude key was treated as literal, so both rows (neither is named "KEY1' OR '1'='1") are returned.
    assert_eq!(rows.len(), 2);

    // Normal exclude_key works
    let rows_ex = load_f32_vectors(&conn, "all", Some("KEY1")).unwrap();
    assert_eq!(rows_ex.len(), 1);
    assert_eq!(rows_ex[0].item_key, "KEY2");
}
