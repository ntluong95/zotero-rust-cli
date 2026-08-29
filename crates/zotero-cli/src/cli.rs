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
    /// DOCX citation inspection and rendering commands.
    #[command(subcommand)]
    Docx(DocxCommands),
    /// Execute raw JavaScript inside Zotero via the CLI Bridge (privileged; JS Bridge only).
    Js {
        code: String,
        /// Seconds to wait for the script to complete.
        #[arg(long, default_value_t = 10)]
        wait: u64,
    },
    /// Trigger a Zotero sync cycle (privileged; JS Bridge only).
    Sync,
}

#[derive(Subcommand, Debug)]
pub enum DocxCommands {
    /// Inspect a DOCX file for Zotero, EndNote, CSL, and static citations.
    InspectCitations {
        path: String,
        #[arg(long = "sample-limit", default_value_t = 10)]
        sample_limit: usize,
    },
    /// Inspect DOCX Zotero placeholders such as {{zotero:ITEMKEY}}.
    InspectPlaceholders {
        path: String,
        #[arg(long = "sample-limit", default_value_t = 10)]
        sample_limit: usize,
    },
    /// Validate DOCX Zotero placeholders against the local Zotero database.
    ValidatePlaceholders {
        path: String,
        #[arg(long = "sample-limit", default_value_t = 10)]
        sample_limit: usize,
    },
    /// Convert Zotero placeholders into static citation and bibliography text.
    RenderCitations {
        path: String,
        #[arg(long = "output", required = true)]
        output: String,
        #[arg(long = "style", default_value = "apa")]
        style: String,
        #[arg(long = "locale", default_value = "en-US")]
        locale: String,
        #[arg(long = "bibliography", default_value = "auto")]
        bibliography: String,
        #[arg(long = "force")]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum AppCommands {
    /// Report Zotero installation, data paths, and connector/Local API availability.
    Status,
    /// Stage the fork-owned CLI Bridge XPI plugin to a local output directory.
    InstallPlugin {
        /// Directory to stage the XPI file into (must then be installed manually via
        /// Zotero's Add-ons manager -- this never touches the Zotero profile directly).
        #[arg(long = "output-dir")]
        output_dir: Option<String>,
    },
    /// Report whether the CLI Bridge endpoint is active and who owns it.
    PluginStatus {
        #[arg(long = "output-dir")]
        output_dir: Option<String>,
    },
    /// Remove the staged CLI Bridge XPI artifact (does not touch an installed extension).
    UninstallPlugin {
        #[arg(long = "output-dir")]
        output_dir: Option<String>,
    },
    /// Perform the explicit, deliberate Local API write-authorization handshake
    /// (`POST /api/local/authorize`). Blocks on a human consent dialog in Zotero.
    AuthorizeLocalApi {
        #[arg(long = "app-name", default_value = "zotero-rust-cli")]
        app_name: String,
    },
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
    /// Update one or more fields on an item. Local API when available, JS Bridge fallback.
    Update {
        item_key: String,
        /// A `field=value` pair; may be repeated.
        #[arg(long = "field", value_parser = parse_key_val)]
        fields: Vec<(String, String)>,
    },
    /// Add or remove tags on an item. Local API when available, JS Bridge fallback.
    Tag {
        item_key: String,
        #[arg(long = "add")]
        add: Vec<String>,
        #[arg(long = "remove")]
        remove: Vec<String>,
    },
    /// Delete an item. Local API when available, JS Bridge fallback.
    Delete {
        item_key: String,
        /// Required to actually perform the deletion (safety confirmation).
        #[arg(long)]
        confirm: bool,
    },
    /// Attach a file to an item (JS Bridge only -- Local API's multi-step upload
    /// protocol is not implemented in this build).
    Attach { item_key: String, pdf_path: String },
    /// Add an item to a collection without disturbing its other collection memberships.
    /// Local API when available (read-modify-write full-array-replace), JS Bridge fallback.
    AddToCollection {
        item_ref: String,
        collection_ref: String,
    },
    /// Move an item to a collection, one Zotero-side operation. Local API when available
    /// (read-modify-write full-array-replace), JS Bridge fallback (supports at most one
    /// `--from` source and does not support `--all-other-collections`).
    MoveToCollection {
        item_ref: String,
        collection_ref: String,
        /// Source collection(s) to remove the item from. May be repeated.
        #[arg(long = "from")]
        from: Vec<String>,
        /// Remove the item from every other collection it currently belongs to.
        #[arg(long = "all-other-collections")]
        all_other_collections: bool,
    },
    /// Find duplicate items in a library (privileged; JS Bridge only).
    Duplicates {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Merge one or more items into a target item (privileged; JS Bridge only).
    Merge {
        keep_key: String,
        merge_keys: Vec<String>,
        /// Required to actually perform the merge (safety confirmation).
        #[arg(long)]
        confirm: bool,
    },
}

/// Parses a `key=value` command-line argument into a tuple, for `--field key=value`.
fn parse_key_val(s: &str) -> Result<(String, String), String> {
    let (key, value) = s
        .split_once('=')
        .ok_or_else(|| format!("expected `key=value`, got: {s}"))?;
    if key.is_empty() {
        return Err(format!("expected `key=value`, got: {s}"));
    }
    Ok((key.to_string(), value.to_string()))
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
    /// Create a new collection. Local API when available, JS Bridge fallback.
    Create {
        name: String,
        #[arg(long)]
        parent: Option<String>,
    },
    /// Rename a collection and/or move it under a new parent. Local API when available,
    /// JS Bridge fallback.
    Rename {
        collection_key: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        parent: Option<String>,
    },
    /// Delete a collection. Local API when available, JS Bridge fallback.
    Delete {
        collection_key: String,
        /// Also delete the items contained in this collection (JS Bridge only --
        /// no Local API primitive for cascading item deletion exists in this build).
        #[arg(long = "delete-items")]
        delete_items: bool,
        /// Required to actually perform the deletion (safety confirmation).
        #[arg(long)]
        confirm: bool,
    },
    /// Remove an item from a collection without disturbing its other memberships.
    /// Local API when available (read-modify-write full-array-replace), JS Bridge fallback.
    RemoveItem {
        collection_key: String,
        item_key: String,
    },
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
