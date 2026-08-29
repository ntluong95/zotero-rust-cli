//! Port of `core/session.py`'s read path. None of the 5 vertical-slice
//! commands write session state, so only `load_session_state()` and its
//! dependencies are ported; `save_session_state`/`append_command_history`
//! land with the first session-mutating command.

use std::path::PathBuf;

use serde::Deserialize;

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
