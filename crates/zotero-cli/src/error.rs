//! Error contract matching the Python reference's `dispatch()` behaviour
//! (`zotero_cli.py:2657-2676`): a domain error prints as `{"error": msg}`
//! on stdout in `--json` mode, or `Error: {msg}` on stderr otherwise, and
//! the process exits 1. `clap`'s own usage errors (missing/invalid
//! arguments) already exit 2 on their own, matching Click's
//! `UsageError.exit_code == 2` without any code here.
//!
//! Python's `FileNotFoundError` for a missing `zotero.sqlite` propagates
//! uncaught (raw traceback, exit 1) because `dispatch()` only special-cases
//! `ClickException`/`RuntimeError`. This is treated here as a plain
//! `DomainError` instead (clean message, exit 1) — a deliberate, minor,
//! documented improvement over the accidental Python behaviour rather than
//! a faithfully reproduced defect.
//!
//! Same category, second instance: an item/collection numeric ref that
//! overflows `i64` (e.g. `item get 99999999999999999999`) raises an
//! uncaught `OverflowError` in Python when SQLite binds the value (raw
//! traceback, empty stdout even in `--json` mode). `db::is_numeric_ref`'s
//! `str::parse::<i64>()` simply fails on overflow and falls through to the
//! key-lookup path, producing a clean `DomainError` ("Item not found: ...")
//! instead — exit code coincidentally matches (1) but the message shape
//! does not, by design.

use std::fmt;

#[derive(Debug)]
pub struct DomainError(pub String);

impl DomainError {
    pub fn new(message: impl Into<String>) -> Self {
        DomainError(message.into())
    }
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for DomainError {}

pub type Result<T> = std::result::Result<T, anyhow::Error>;
