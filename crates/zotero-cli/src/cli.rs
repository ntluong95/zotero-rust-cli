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
    /// Library inspection commands.
    #[command(subcommand)]
    Library(LibraryCommands),
    /// Saved search commands.
    #[command(subcommand)]
    Search(SearchCommands),
    /// Tag inspection commands.
    #[command(subcommand)]
    Tag(TagCommands),
    /// CSL citation style commands.
    #[command(subcommand)]
    Style(StyleCommands),
    /// Session state commands (current library/collection/item, history).
    #[command(subcommand)]
    Session(SessionCommands),
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
    /// List child items (notes, attachments, annotations) of an item.
    Children {
        #[arg(value_name = "REF")]
        item_ref: Option<String>,
    },
    /// List child notes of an item.
    Notes {
        #[arg(value_name = "REF")]
        item_ref: Option<String>,
    },
    /// List child attachments of an item, with resolved filesystem paths.
    Attachments {
        #[arg(value_name = "REF")]
        item_ref: Option<String>,
    },
    /// Resolve the primary file (or first attachment's file) for an item.
    File {
        #[arg(value_name = "REF")]
        item_ref: Option<String>,
    },
    /// Build the semantic search vector index from your Zotero library.
    BuildIndex,
    /// Semantic search across Zotero library using local embedding model.
    SemanticSearch {
        query: String,
        /// Number of results.
        #[arg(long = "top-k", default_value_t = 10)]
        top_k: usize,
        /// Minimum similarity score (0-1).
        #[arg(long = "min-score", default_value_t = 0.3)]
        min_score: f32,
        /// Filter by language.
        #[arg(long, value_enum, default_value_t = SemanticLanguage::All)]
        language: SemanticLanguage,
    },
    /// Find items similar to a given item using embeddings.
    Similar {
        item_key: String,
        /// Number of similar items.
        #[arg(long = "top-k", default_value_t = 5)]
        top_k: usize,
        /// Minimum similarity score (0-1).
        #[arg(long = "min-score", default_value_t = 0.5)]
        min_score: f32,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum SemanticLanguage {
    #[value(name = "zh")]
    Zh,
    #[value(name = "en")]
    En,
    #[value(name = "all")]
    All,
}

impl std::fmt::Display for SemanticLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SemanticLanguage::Zh => "zh",
            SemanticLanguage::En => "en",
            SemanticLanguage::All => "all",
        };
        write!(f, "{s}")
    }
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
    /// Find collections by name.
    Find {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    /// Get a single collection by ref or session default.
    Get {
        #[arg(value_name = "REF")]
        collection_ref: Option<String>,
    },
    /// List items in a collection.
    Items {
        #[arg(value_name = "REF")]
        collection_ref: Option<String>,
    },
    /// Print the collection hierarchy for the current (or default) library.
    Tree,
}

#[derive(Subcommand, Debug)]
pub enum LibraryCommands {
    /// List all Zotero libraries.
    List,
}

#[derive(Subcommand, Debug)]
pub enum SearchCommands {
    /// List saved searches in the current (or default) library.
    List,
    /// Get a saved search by ref (key or numeric id).
    Get { search_ref: String },
    /// Run a saved search's items through the Zotero Local API.
    Items { search_ref: String },
}

#[derive(Subcommand, Debug)]
pub enum TagCommands {
    /// List tags in the current (or default) library.
    List,
    /// List items carrying a given tag (by name or numeric id).
    Items { tag_ref: String },
}

#[derive(Subcommand, Debug)]
pub enum StyleCommands {
    /// List installed CSL citation styles.
    List,
}

#[derive(Subcommand, Debug)]
pub enum SessionCommands {
    /// Show current session status.
    Status,
    /// Set the current library for this and future commands.
    UseLibrary { library_ref: String },
    /// Set the current collection for this and future commands.
    UseCollection { collection_ref: String },
    /// Set the current item for this and future commands.
    UseItem { item_ref: String },
    /// Clear the current library.
    ClearLibrary,
    /// Clear the current collection.
    ClearCollection,
    /// Clear the current item.
    ClearItem,
    /// Show recent command history.
    History {
        #[arg(long, default_value_t = 10)]
        limit: i64,
    },
}
