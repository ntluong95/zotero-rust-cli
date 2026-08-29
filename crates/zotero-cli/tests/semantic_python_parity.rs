use std::path::{Path, PathBuf};
use std::process::Command;

struct TestDir(PathBuf);
impl TestDir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "zotero-test-parity-{}-{}",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&p);
        Self(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn test_python_built_db_readable_by_rust_and_rust_built_db_readable_by_python() {
    let tmp = TestDir::new("py-parity");
    let py_created_db = tmp.path().join("py_created.sqlite");

    // 1. Create a vector DB using Python's sqlite3 and struct.pack
    let py_script = format!(
        r#"
import sqlite3
import struct

conn = sqlite3.connect(r"{}")
conn.execute("""CREATE TABLE embeddings (
    item_key TEXT, chunk_id INTEGER, chunk_text TEXT, language TEXT,
    PRIMARY KEY (item_key, chunk_id))""")
conn.execute("""CREATE TABLE vectors_f32 (
    item_key TEXT, chunk_id INTEGER, vector BLOB,
    PRIMARY KEY (item_key, chunk_id))""")

vec1 = [0.8, 0.2, 0.1]
vec2 = [0.1, 0.9, 0.0]

blob1 = struct.pack("3f", *vec1)
blob2 = struct.pack("3f", *vec2)

conn.execute("INSERT INTO embeddings VALUES (?, 0, ?, ?)", ("ITEM_A", "Text of item A", "en"))
conn.execute("INSERT INTO vectors_f32 VALUES (?, 0, ?)", ("ITEM_A", blob1))

conn.execute("INSERT INTO embeddings VALUES (?, 0, ?, ?)", ("ITEM_B", "Text of item B", "zh"))
conn.execute("INSERT INTO vectors_f32 VALUES (?, 0, ?)", ("ITEM_B", blob2))

conn.commit()
conn.close()
"#,
        py_created_db.display()
    );

    let status = Command::new("python3")
        .arg("-c")
        .arg(&py_script)
        .status()
        .expect("Failed to execute python3 script");
    assert!(status.success(), "Python script failed to create vector DB");

    // 2. Read with Rust find_similar directly on the python-generated DB
    let config = zotero_cli::semantic::SemanticConfig {
        embed_api: "http://127.0.0.1:0/embeddings".to_string(),
        embed_model: "test-model".to_string(),
        embed_key: "".to_string(),
        vector_db: py_created_db.clone(),
    };

    let similar_to_a = zotero_cli::semantic::find_similar("ITEM_A", &config, 5, 0.0).unwrap();
    assert_eq!(similar_to_a.len(), 1);
    assert_eq!(similar_to_a[0].item_key, "ITEM_B");
    assert_eq!(similar_to_a[0].language, "zh");

    // 3. Now build a vector DB with Rust and verify Python can read and unpack it
    let rust_created_db = tmp.path().join("rust_created.sqlite");
    let rust_conn = zotero_cli::semantic::connect_vector_db_rw(&rust_created_db).unwrap();

    let rust_vec = vec![0.123f32, 0.456, 0.789];
    let encoded = zotero_cli::semantic::vectors::encode_f32_vector(&rust_vec);

    rust_conn
        .execute(
            "INSERT INTO embeddings VALUES ('RUST_ITEM', 0, 'Rust text content', 'en')",
            [],
        )
        .unwrap();
    rust_conn
        .execute(
            "INSERT INTO vectors_f32 VALUES ('RUST_ITEM', 0, ?1)",
            rusqlite::params![encoded],
        )
        .unwrap();
    drop(rust_conn);

    let py_read_script = format!(
        r#"
import sqlite3
import struct
import json

conn = sqlite3.connect(r"{}")
row = conn.execute("SELECT e.item_key, e.chunk_text, e.language, v.vector FROM embeddings e JOIN vectors_f32 v ON e.item_key=v.item_key").fetchone()
conn.close()

key, text, lang, blob = row
decoded = list(struct.unpack(f"{{len(blob)//4}}f", blob))

assert key == "RUST_ITEM"
assert text == "Rust text content"
assert lang == "en"
assert len(decoded) == 3
assert abs(decoded[0] - 0.123) < 1e-5
assert abs(decoded[1] - 0.456) < 1e-5
assert abs(decoded[2] - 0.789) < 1e-5

print("PYTHON_READ_SUCCESS")
"#,
        rust_created_db.display()
    );

    let read_output = Command::new("python3")
        .arg("-c")
        .arg(&py_read_script)
        .output()
        .expect("Failed to execute python3 read script");
    assert!(
        read_output.status.success(),
        "Python script failed to read Rust-created vector DB: {}",
        String::from_utf8_lossy(&read_output.stderr)
    );
    let stdout = String::from_utf8_lossy(&read_output.stdout);
    assert!(stdout.contains("PYTHON_READ_SUCCESS"));
}
