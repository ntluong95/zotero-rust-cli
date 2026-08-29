//! Command surface for the read-only vertical slice, ported from the root
//! `cli` group and the `app`/`item`/`collection` subgroups in
//! `zotero_cli.py:383-441,974-1030,734-742`.
//!
//! `--json` is declared `global = true` so it parses at any position after
//! any subcommand, matching Python's `_JsonAwareGroup`/`_JsonAwareCommand`
//! propagation (`zotero_cli.py:63-143`) without needing to reimplement
//! that propagation by hand.
//!
//! Bare invocation (no subcommand) prints help and exits 0 instead of
//! entering upstream's blocking REPL — this is the plan's documented
//! "Approved intentional break": a blocking stdin read is the worst
//! failure mode for a non-interactive agent caller.

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "zotero-cli",
    about = "Agent-native Zotero CLI using SQLite, connector, and Local API backends.",
    version
)]
pub struct Cli {
    /// Emit machine-readable JSON.
    #[arg(long, global = true)]
    pub json: bool,

    #[arg(long, value_enum, default_value_t = Backend::Auto)]
    pub backend: Backend,

    /// Explicit Zotero data directory.
    #[arg(long = "data-dir")]
    pub data_dir: Option<String>,

    /// Explicit Zotero profile directory.
    #[arg(long = "profile-dir")]
    pub profile_dir: Option<String>,

    /// Explicit Zotero executable path.
    #[arg(long)]
    pub executable: Option<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
#[value(rename_all = "lower")]
pub enum Backend {
    Auto,
    Sqlite,
    Api,
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Backend::Auto => "auto",
            Backend::Sqlite => "sqlite",
            Backend::Api => "api",
        };
        write!(f, "{s}")
    }
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Application and runtime inspection commands.
    #[command(subcommand)]
    App(AppCommands),
    /// Item inspection and rendering commands.
    #[command(subcommand)]
    Item(ItemCommands),
    /// Collection inspection and selection commands.
    #[command(subcommand)]
    Collection(CollectionCommands),
}

#[derive(Subcommand, Debug)]
pub enum AppCommands {
    /// Report Zotero installation, data paths, and connector/Local API availability.
    Status,
}

#[derive(Subcommand, Debug)]
pub enum ItemCommands {
    /// List items in the current (or default) library.
    List {
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    /// Get a single item by key or numeric id.
    Get {
        #[arg(value_name = "REF")]
        item_ref: Option<String>,
    },
    /// Find items by title or Local API quick search.
    Find {
        query: String,
        /// Collection ID or key scope.
        #[arg(long = "collection")]
        collection: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: i64,
        /// Use exact title matching via SQLite.
        #[arg(long = "exact-title")]
        exact_title: bool,
        /// Zotero Local API quick-search scope.
        #[arg(long, value_enum, default_value_t = SearchScope::TitleCreatorYear)]
        scope: SearchScope,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum SearchScope {
    #[value(name = "titleCreatorYear")]
    TitleCreatorYear,
    #[value(name = "fields")]
    Fields,
    #[value(name = "everything")]
    Everything,
}

impl std::fmt::Display for SearchScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SearchScope::TitleCreatorYear => "titleCreatorYear",
            SearchScope::Fields => "fields",
            SearchScope::Everything => "everything",
        };
        write!(f, "{s}")
    }
}

#[derive(Subcommand, Debug)]
pub enum CollectionCommands {
    /// List collections in the current (or default) library.
    List,
}
