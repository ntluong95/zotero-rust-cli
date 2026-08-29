//! Port of `core/session.py`. The read path (`load_session_state`) landed
//! with the Phase 3 vertical slice; `save_session_state`/
//! `append_command_history`/`build_session_payload` land here with the
//! `session *` command slice — the first commands in the port that
//! mutate on-disk state.

use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const COMMAND_HISTORY_LIMIT: usize = 50;
const STATE_DIR_ENV: &str = "CLI_ANYTHING_ZOTERO_STATE_DIR";
const APP_NAME: &str = "cli-anything-zotero";

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SessionState {
    pub current_library: Option<serde_json::Value>,
    pub current_collection: Option<String>,
    pub current_item: Option<String>,
    #[serde(default)]
    pub command_history: Vec<serde_json::Value>,
}

/// `session_state_dir()` (`session.py:14-18`).
pub fn session_state_dir() -> PathBuf {
    if let Ok(override_dir) = std::env::var(STATE_DIR_ENV) {
        let trimmed = override_dir.trim();
        if !trimmed.is_empty() {
            return crate::paths::expand_user_path(trimmed);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join(APP_NAME)
}

/// `session_state_path()` (`session.py:21-22`).
pub fn session_state_path() -> PathBuf {
    session_state_dir().join("session.json")
}

/// `load_session_state()` (`session.py:43-55`): missing file or invalid
/// JSON silently falls back to the default (empty) state.
pub fn load_session_state() -> SessionState {
    let path = session_state_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return SessionState::default();
    };
    let Ok(mut state) = serde_json::from_str::<SessionState>(&text) else {
        return SessionState::default();
    };
    // Only string entries survive (`isinstance(item, str)` in Python),
    // and the history is truncated to the last COMMAND_HISTORY_LIMIT.
    state.command_history.retain(|v| v.is_string());
    if state.command_history.len() > COMMAND_HISTORY_LIMIT {
        let drop = state.command_history.len() - COMMAND_HISTORY_LIMIT;
        state.command_history.drain(0..drop);
    }
    state
}

/// `locked_save_json()` (`session.py:58-80`): best-effort exclusive
/// lock via `fd-lock`, matching Python's `fcntl.flock` — Python
/// silently continues when `flock` is unavailable
/// (`except (ImportError, OSError): pass`, the situation on Windows
/// without an equivalent syscall wired up), so a failure to acquire
/// the lock here must not fail the write, only skip the mutual
/// exclusion it would have provided.
fn locked_save_json(path: &std::path::Path, data: &Value) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(data)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    let mut lock = fd_lock::RwLock::new(file);
    match lock.try_write() {
        Ok(mut guard) => {
            guard.set_len(0)?;
            guard.seek(SeekFrom::Start(0))?;
            guard.write_all(text.as_bytes())?;
        }
        Err(_) => {
            // Lock unavailable (contention, or no OS support) — still
            // write, best-effort, matching Python's fallback path.
            std::fs::write(path, text.as_bytes())?;
        }
    }
    Ok(())
}

/// `save_session_state()` (`session.py:83-92`): rebuilds exactly these
/// 4 keys in this order, discarding anything else that might be in
/// `state`, and re-truncates history defensively even on save.
pub fn save_session_state(state: &SessionState) -> anyhow::Result<()> {
    let mut history = state.command_history.clone();
    if history.len() > COMMAND_HISTORY_LIMIT {
        let drop = history.len() - COMMAND_HISTORY_LIMIT;
        history.drain(0..drop);
    }
    let mut payload = Map::new();
    payload.insert(
        "current_library".to_string(),
        state.current_library.clone().unwrap_or(Value::Null),
    );
    payload.insert(
        "current_collection".to_string(),
        state
            .current_collection
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    payload.insert(
        "current_item".to_string(),
        state
            .current_item
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    payload.insert("command_history".to_string(), Value::Array(history));
    locked_save_json(&session_state_path(), &Value::Object(payload))
}

/// `append_command_history()` (`session.py:95-103`): reloads state from
/// disk rather than using a caller-supplied copy, matching Python
/// exactly — each CLI invocation is a fresh process, so "current state"
/// only ever means "what's on disk right now."
pub fn append_command_history(command_line: &str) -> anyhow::Result<()> {
    let trimmed = command_line.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let mut state = load_session_state();
    state
        .command_history
        .push(Value::String(trimmed.to_string()));
    if state.command_history.len() > COMMAND_HISTORY_LIMIT {
        let drop = state.command_history.len() - COMMAND_HISTORY_LIMIT;
        state.command_history.drain(0..drop);
    }
    save_session_state(&state)
}

/// `build_session_payload()` (`session.py:106-114`).
#[derive(Debug, Clone, Serialize)]
pub struct SessionPayload {
    pub current_library: Option<serde_json::Value>,
    pub current_collection: Option<String>,
    pub current_item: Option<String>,
    pub state_path: String,
    pub history_count: usize,
}

/// `session history`'s `current_session().get("command_history", [])[-limit:]`
/// (`zotero_cli.py:2417-2420`). Python's `[-limit:]` is a *different*
/// slice shape from `python_slice_to_limit`'s `[:limit]` (used by `item
/// find`/`item list`) and has its own three-way behavior worth getting
/// exactly right rather than assuming symmetry with that helper:
/// `limit == 0` returns the **entire** list (`-0 == 0` in Python, so
/// `xs[-0:]` is `xs[0:]`, not an empty slice); `limit > 0` returns the
/// last `limit` elements; `limit < 0` drops the first `-limit` elements.
pub fn python_negative_tail_slice(history: &[Value], limit: i64) -> Vec<Value> {
    let len = history.len() as i64;
    let start = if limit == 0 {
        0
    } else if limit > 0 {
        (len - limit).max(0)
    } else {
        (-limit).min(len).max(0)
    };
    history[start as usize..].to_vec()
}

pub fn build_session_payload(state: &SessionState) -> SessionPayload {
    SessionPayload {
        current_library: state.current_library.clone(),
        current_collection: state.current_collection.clone(),
        current_item: state.current_item.clone(),
        state_path: session_state_path().to_string_lossy().into_owned(),
        history_count: state.command_history.len(),
    }
}

/// `session_library_id()` (`session.py:29-40`): `None`/empty-string
/// current_library falls back to `default`, matching the documented
/// quirk that a naive `.get("current_library", default)` would miss.
///
/// A non-numeric `current_library` (a corrupted `session.json`) is a real
/// error, not silently defaulted: Python's `int(value)` raises uncaught on
/// bad input, and substituting `default` here instead would make the tool
/// silently operate on the wrong library with no error at all.
pub fn session_library_id(state: &SessionState, default: i64) -> anyhow::Result<i64> {
    match &state.current_library {
        None => Ok(default),
        Some(serde_json::Value::Null) => Ok(default),
        Some(serde_json::Value::String(s)) if s.is_empty() => Ok(default),
        Some(serde_json::Value::Number(n)) => n.as_i64().ok_or_else(|| {
            crate::error::DomainError::new(format!("Invalid current_library in session: {n}"))
                .into()
        }),
        Some(serde_json::Value::String(s)) => s.parse::<i64>().map_err(|_| {
            crate::error::DomainError::new(format!("Invalid current_library in session: {s}"))
                .into()
        }),
        Some(other) => Err(crate::error::DomainError::new(format!(
            "Invalid current_library in session: {other}"
        ))
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // `session_library_id` is dead code today -- no command in the
    // current vertical slice calls it -- but it is the exact function
    // the first write command (Phase 5+) will depend on to know which
    // Zotero library to write into. A silent wrong-default here would
    // point a write at the wrong library with zero error output, so
    // this is regression-tested now, ahead of that call site existing,
    // rather than left to be "caught when it's wired in."

    fn state_with(current_library: Option<serde_json::Value>) -> SessionState {
        SessionState {
            current_library,
            ..SessionState::default()
        }
    }

    #[test]
    fn missing_current_library_falls_back_to_default() {
        let state = state_with(None);
        assert_eq!(session_library_id(&state, 7).unwrap(), 7);
    }

    #[test]
    fn null_current_library_falls_back_to_default() {
        let state = state_with(Some(serde_json::Value::Null));
        assert_eq!(session_library_id(&state, 7).unwrap(), 7);
    }

    #[test]
    fn empty_string_current_library_falls_back_to_default() {
        // Python's `session.get("current_library", default)` alone would
        // miss this: the default session state always sets the key with
        // value "", so `.get` never uses the fallback. See the doc
        // comment on this function.
        let state = state_with(Some(json!("")));
        assert_eq!(session_library_id(&state, 7).unwrap(), 7);
    }

    #[test]
    fn numeric_current_library_is_used_directly() {
        let state = state_with(Some(json!(3)));
        assert_eq!(session_library_id(&state, 7).unwrap(), 3);
    }

    #[test]
    fn numeric_string_current_library_parses() {
        let state = state_with(Some(json!("3")));
        assert_eq!(session_library_id(&state, 7).unwrap(), 3);
    }

    #[test]
    fn corrupted_non_numeric_string_is_a_real_error_not_a_silent_default() {
        // A hand-edited or corrupted session.json with a garbage
        // current_library ("L1", "abc", ...) must fail loudly. Silently
        // substituting `default` here would make write commands operate
        // on the wrong library with no error at all -- exactly the
        // landmine this test exists to prevent from regressing.
        let state = state_with(Some(json!("not-a-library-id")));
        let err = session_library_id(&state, 7).unwrap_err();
        assert!(err.to_string().contains("Invalid current_library"));
    }

    #[test]
    fn corrupted_object_current_library_is_a_real_error() {
        let state = state_with(Some(json!({"unexpected": "shape"})));
        assert!(session_library_id(&state, 7).is_err());
    }

    #[test]
    fn corrupted_array_current_library_is_a_real_error() {
        let state = state_with(Some(json!([1, 2, 3])));
        assert!(session_library_id(&state, 7).is_err());
    }

    fn history(n: usize) -> Vec<Value> {
        (1..=n).map(|i| json!(i)).collect()
    }

    #[test]
    fn negative_tail_slice_zero_limit_returns_entire_list() {
        // `-0 == 0` in Python: xs[-0:] is xs[0:], the whole list, not empty.
        assert_eq!(python_negative_tail_slice(&history(5), 0), history(5));
    }

    #[test]
    fn negative_tail_slice_positive_limit_returns_last_n() {
        assert_eq!(
            python_negative_tail_slice(&history(10), 3),
            vec![json!(8), json!(9), json!(10)]
        );
    }

    #[test]
    fn negative_tail_slice_positive_limit_larger_than_list_returns_whole_list() {
        assert_eq!(python_negative_tail_slice(&history(3), 10), history(3));
    }

    #[test]
    fn negative_tail_slice_negative_limit_drops_from_front() {
        // xs[-(-3):] == xs[3:] -- drops the first 3, keeps the rest.
        assert_eq!(
            python_negative_tail_slice(&history(5), -3),
            vec![json!(4), json!(5)]
        );
    }

    #[test]
    fn negative_tail_slice_negative_limit_beyond_list_length_is_empty() {
        assert_eq!(
            python_negative_tail_slice(&history(3), -10),
            Vec::<Value>::new()
        );
    }

    #[test]
    fn negative_tail_slice_empty_history() {
        assert_eq!(python_negative_tail_slice(&[], 5), Vec::<Value>::new());
        assert_eq!(python_negative_tail_slice(&[], 0), Vec::<Value>::new());
    }

    // `STATE_DIR_ENV` is a process-global env var, and Rust's default test
    // runner runs tests in parallel *threads within the same process* --
    // so any two tests that both mutate it race, regardless of whether
    // either test's own comment claims otherwise. (An earlier version of
    // this file had exactly that bug: two tests, both setting this env
    // var, each individually commented "safe, no other test reads this,"
    // which was false the moment the second test was added. `cargo test`
    // in debug mode didn't catch it -- timing happened not to overlap --
    // but `cargo test --release` did, which is why CI runs release.)
    // Every test that touches `STATE_DIR_ENV` must hold this lock for its
    // entire duration.
    static STATE_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn temp_state_dir(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "zotero-cli-test-session-{}-{n}-{label}",
            std::process::id()
        ))
    }

    // `save_session_state`/`load_session_state`/`append_command_history`
    // round-trip through the real locked-write path (`fd-lock`), not a
    // mock -- CI's Windows leg is what actually proves the "locking
    // degrades gracefully" success criterion, but only if a test here
    // exercises the write, not just documents the intent.
    #[test]
    fn save_and_load_and_append_history_round_trip_through_a_real_locked_file() {
        let _guard = STATE_DIR_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_state_dir("round-trip");
        // SAFETY: serialized against every other STATE_DIR_ENV-mutating
        // test by `_guard`, held for this whole function.
        unsafe {
            std::env::set_var(STATE_DIR_ENV, &dir);
        }

        let loaded_before = load_session_state();
        assert_eq!(loaded_before.current_library, None);

        let mut state = load_session_state();
        state.current_library = Some(json!(2));
        state.current_collection = Some("COLLABCD".to_string());
        save_session_state(&state).expect("save_session_state must succeed");

        let reloaded = load_session_state();
        assert_eq!(reloaded.current_library, Some(json!(2)));
        assert_eq!(reloaded.current_collection, Some("COLLABCD".to_string()));
        assert_eq!(reloaded.current_item, None);
        assert_eq!(reloaded.command_history.len(), 0);

        append_command_history("session use-library 2").expect("append must succeed");
        append_command_history("session use-collection COLLABCD").expect("append must succeed");
        // Blank/whitespace-only lines are a no-op, matching Python's
        // `command_line.strip(); if not command_line: return`.
        append_command_history("   ").expect("blank append must be a no-op, not an error");

        let after_history = load_session_state();
        assert_eq!(after_history.command_history.len(), 2);
        assert_eq!(
            after_history.command_history[0],
            json!("session use-library 2")
        );
        // The library/collection set earlier must survive the history
        // appends untouched -- append_command_history reloads-and-saves
        // its own copy, and must not clobber fields it doesn't own.
        assert_eq!(after_history.current_library, Some(json!(2)));

        let payload = build_session_payload(&after_history);
        assert_eq!(payload.history_count, 2);

        std::fs::remove_dir_all(&dir).ok();
        unsafe {
            std::env::remove_var(STATE_DIR_ENV);
        }
    }

    #[test]
    fn append_command_history_caps_at_50_entries() {
        let _guard = STATE_DIR_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_state_dir("history-cap");
        // SAFETY: serialized against every other STATE_DIR_ENV-mutating
        // test by `_guard`, held for this whole function.
        unsafe {
            std::env::set_var(STATE_DIR_ENV, &dir);
        }

        for i in 0..55 {
            append_command_history(&format!("command {i}")).expect("append must succeed");
        }
        let state = load_session_state();
        assert_eq!(state.command_history.len(), COMMAND_HISTORY_LIMIT);
        // Oldest entries must be dropped, newest kept -- matches
        // `history[-COMMAND_HISTORY_LIMIT:]` in both Python and the
        // Rust `save_session_state`/`append_command_history` ports.
        assert_eq!(state.command_history[0], json!("command 5"));
        assert_eq!(state.command_history[49], json!("command 54"));

        std::fs::remove_dir_all(&dir).ok();
        unsafe {
            std::env::remove_var(STATE_DIR_ENV);
        }
    }
}
