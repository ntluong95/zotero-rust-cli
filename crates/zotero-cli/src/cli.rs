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

use clap::{ArgAction, Parser, Subcommand, ValueEnum};

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
    /// Read and add child notes.
    #[command(subcommand)]
    Note(NoteCommands),
    /// Unified ingest entrypoints (translator-first with OA fallbacks).
    #[command(subcommand)]
    Add(AddCommands),
    /// Official Zotero import and write commands.
    #[command(subcommand)]
    Import(ImportCommands),
    /// DOCX citation inspection and rendering commands.
    #[command(subcommand)]
    Docx(DocxCommands),
    /// Independent Zotero data export commands.
    #[command(subcommand)]
    Export(ExportCommands),
    /// Inspect local write-operation audit log.
    #[command(subcommand)]
    Audit(AuditCommands),
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
pub enum ExportCommands {
    /// Export real Zotero items to a standalone BibTeX/BibLaTeX file.
    Bib {
        /// Comma-separated item keys/IDs to export.
        #[arg(long)]
        items: Option<String>,
        /// Collection key/ID whose top-level items should be exported.
        #[arg(long = "collection")]
        collection_ref: Option<String>,
        #[arg(long = "format", value_enum, default_value_t = ExportBibFormat::Bibtex)]
        fmt: ExportBibFormat,
        /// Output .bib file path.
        #[arg(long, required = true)]
        output: String,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug)]
#[value(rename_all = "lower")]
pub enum ExportBibFormat {
    Bibtex,
    Biblatex,
}

impl std::fmt::Display for ExportBibFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ExportBibFormat::Bibtex => "bibtex",
            ExportBibFormat::Biblatex => "biblatex",
        };
        write!(f, "{s}")
    }
}

/// `SUPPORTED_EXPORT_FORMATS` (`rendering.py:10`), as a `clap` choice set --
/// matches Python's `click.Choice(list(rendering.SUPPORTED_EXPORT_FORMATS))`
/// on `item export --format`.
#[derive(ValueEnum, Clone, Copy, Debug)]
#[value(rename_all = "lower")]
pub enum ExportFormat {
    Ris,
    Bibtex,
    Biblatex,
    Csljson,
    Csv,
    Mods,
    Refer,
}

impl std::fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ExportFormat::Ris => "ris",
            ExportFormat::Bibtex => "bibtex",
            ExportFormat::Biblatex => "biblatex",
            ExportFormat::Csljson => "csljson",
            ExportFormat::Csv => "csv",
            ExportFormat::Mods => "mods",
            ExportFormat::Refer => "refer",
        };
        write!(f, "{s}")
    }
}

#[derive(Subcommand, Debug)]
pub enum AppCommands {
    /// Report Zotero installation, data paths, and connector/Local API availability.
    Status,
    /// Print CLI and detected Zotero version.
    Version,
    /// Check connector availability (ping).
    Ping,
    /// Diagnose local Zotero + CLI Bridge readiness for agent workflows.
    Doctor,
    /// Launch the local Zotero desktop application and wait for the connector (and, if
    /// configured, the Local API) to come up.
    Launch {
        #[arg(long = "wait-timeout", default_value_t = 30)]
        wait_timeout: i64,
    },
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
pub enum AuditCommands {
    /// Print the audit log file path.
    Path,
    /// Show the latest audit log entries.
    Tail {
        #[arg(long, default_value_t = 20)]
        limit: usize,
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
    /// Merge one or more items into a target item (privileged; JS Bridge only).
    ///
    /// Defaults to a zero-mutation dry-run preview; pass --confirm to apply.
    Merge {
        keep_key: String,
        merge_keys: Vec<String>,
        /// Preview only (default): resolve items and report the merge plan, mutate nothing.
        #[arg(long = "dry-run", overrides_with = "confirm", action = ArgAction::SetTrue)]
        dry_run: bool,
        /// Apply the merge (privileged JS Bridge write).
        #[arg(long = "confirm", overrides_with = "dry_run", action = ArgAction::SetTrue)]
        confirm: bool,
    },
    /// Trigger Zotero's "Find Available PDF" for a single item (via JS bridge).
    FindPdf {
        item_key: String,
        /// Seconds to wait for PDF download.
        #[arg(long, default_value_t = 30)]
        timeout: i64,
    },
    /// Fetch a PDF via Zotero find-pdf + open-access cascade, then attach.
    FetchPdf {
        item_key: String,
        /// Comma-separated cascade: zotero,unpaywall,epmc,biorxiv,arxiv.
        #[arg(long, default_value = "zotero,unpaywall,epmc,biorxiv,arxiv")]
        sources: String,
        /// Fetch even if item already has a PDF.
        #[arg(long)]
        force: bool,
        #[arg(long = "zotero-timeout", default_value_t = 45)]
        zotero_timeout: u64,
        #[arg(long = "download-timeout", default_value_t = 45)]
        download_timeout: u64,
    },
    /// Search full-text content of PDFs in the Zotero library (via JS bridge).
    SearchFulltext {
        query: String,
        /// Maximum number of results.
        #[arg(long, default_value_t = 10)]
        limit: i64,
    },
    /// Search annotations across all items by keyword and/or color.
    SearchAnnotations {
        #[arg(default_value = "")]
        query: String,
        /// Filter by annotation color (repeatable). E.g. yellow, red, #ffd400.
        #[arg(long = "color")]
        colors: Vec<String>,
        /// Max results.
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    /// View annotations and highlights for a Zotero item (via JS bridge).
    Annotations { item_key: String },
    /// Export a single item's reference data via the Zotero Local API.
    Export {
        #[arg(value_name = "REF")]
        item_ref: Option<String>,
        #[arg(long = "format", value_enum)]
        fmt: ExportFormat,
    },
    /// Render an item's inline citation via the Zotero Local API.
    Citation {
        #[arg(value_name = "REF")]
        item_ref: Option<String>,
        #[arg(long)]
        style: Option<String>,
        #[arg(long)]
        locale: Option<String>,
        #[arg(long)]
        linkwrap: bool,
    },
    /// Render an item's bibliography entry via the Zotero Local API.
    Bibliography {
        #[arg(value_name = "REF")]
        item_ref: Option<String>,
        #[arg(long)]
        style: Option<String>,
        #[arg(long)]
        locale: Option<String>,
        #[arg(long)]
        linkwrap: bool,
    },
    /// Build LLM-ready context for an item.
    Context {
        #[arg(value_name = "REF")]
        item_ref: Option<String>,
        #[arg(long = "include-notes")]
        include_notes: bool,
        #[arg(long = "include-bibtex")]
        include_bibtex: bool,
        #[arg(long = "include-csljson")]
        include_csljson: bool,
        #[arg(long = "include-links")]
        include_links: bool,
    },
    /// Find duplicate items (by DOI, title, or native Zotero detector).
    Duplicates {
        #[arg(long, value_enum, default_value_t = DuplicatesBy::Doi)]
        by: DuplicatesBy,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Fetch NIH iCite citation metrics for an item (by PMID or item key).
    Metrics {
        #[arg(value_name = "REF")]
        ref_id: String,
        /// Treat REF as a PMID directly instead of a Zotero item key.
        #[arg(long = "pmid")]
        pmid: bool,
    },
    /// Analyze a Zotero item using an OpenAI-compatible LLM.
    Analyze {
        #[arg(value_name = "REF")]
        item_ref: Option<String>,
        #[arg(long = "question", required = true)]
        question: String,
        #[arg(long = "model", required = true)]
        model: String,
        #[arg(long = "include-notes")]
        include_notes: bool,
        #[arg(long = "include-bibtex")]
        include_bibtex: bool,
        #[arg(long = "include-csljson")]
        include_csljson: bool,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "lower")]
pub enum DuplicatesBy {
    Doi,
    Title,
    Zotero,
}

impl std::fmt::Display for DuplicatesBy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            DuplicatesBy::Doi => "doi",
            DuplicatesBy::Title => "title",
            DuplicatesBy::Zotero => "zotero",
        };
        write!(f, "{s}")
    }
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
    /// Find available PDFs for items missing PDFs (per-item, via JS bridge).
    FindPdfs {
        collection_key: String,
        /// Seconds to wait for each item's PDF lookup.
        #[arg(long = "timeout-per-item", default_value_t = 45)]
        timeout_per_item: u64,
        /// Only process the first N items missing PDFs.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Fetch PDFs for items missing attachments using Zotero + OA cascade.
    FetchPdfs {
        collection_key: String,
        /// Comma-separated cascade: zotero,unpaywall,epmc,biorxiv,arxiv.
        #[arg(long, default_value = "zotero,unpaywall,epmc,biorxiv,arxiv")]
        sources: String,
        /// Only process the first N items missing PDFs.
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long = "zotero-timeout", default_value_t = 45)]
        zotero_timeout: u64,
        #[arg(long = "download-timeout", default_value_t = 45)]
        download_timeout: u64,
        /// Print one JSON progress object per item to stdout.
        #[arg(long = "jsonl-progress")]
        jsonl_progress: bool,
        /// Skip keys completed in prior --resume runs.
        #[arg(long)]
        resume: bool,
        /// Clear resume state for this collection before running.
        #[arg(long = "reset-resume")]
        reset_resume: bool,
    },
    /// Read the collection currently selected in the Zotero GUI (via Connector) and persist it
    /// as the CLI session's current library/collection.
    UseSelected,
    /// Get statistics for a Zotero collection (via JS bridge): item/attachment/PDF counts,
    /// publication-year histogram, and top journals.
    Stats { collection_key: String },
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
    /// Read the collection currently selected in the Zotero GUI (via Connector) and persist it
    /// as the CLI session's current library/collection.
    UseSelected,
}

#[derive(Subcommand, Debug)]
pub enum NoteCommands {
    /// Get a note by key.
    Get {
        #[arg(value_name = "REF")]
        note_ref: String,
    },
    /// Add a child note to a top-level item.
    Add {
        item_ref: String,
        /// Inline note content.
        #[arg(long)]
        text: Option<String>,
        /// Read note content from a file.
        #[arg(long = "file")]
        file_path: Option<String>,
        #[arg(long = "format", value_enum, default_value_t = NoteFormat::Text)]
        fmt: NoteFormat,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug)]
#[value(rename_all = "lower")]
pub enum NoteFormat {
    Text,
    Markdown,
    Html,
}

impl std::fmt::Display for NoteFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            NoteFormat::Text => "text",
            NoteFormat::Markdown => "markdown",
            NoteFormat::Html => "html",
        };
        write!(f, "{s}")
    }
}

#[derive(ValueEnum, Clone, Copy, Debug)]
#[value(rename_all = "lower")]
pub enum IfExists {
    File,
    Skip,
    Duplicate,
}

impl std::fmt::Display for IfExists {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            IfExists::File => "file",
            IfExists::Skip => "skip",
            IfExists::Duplicate => "duplicate",
        };
        write!(f, "{s}")
    }
}

#[derive(Subcommand, Debug)]
pub enum AddCommands {
    /// Ingest an item by DOI: translator-first, Crossref BibTeX fallback.
    Doi {
        doi: String,
        #[arg(long = "collection")]
        collection_key: Option<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long = "if-exists", value_enum, default_value_t = IfExists::File)]
        if_exists: IfExists,
        /// Try Zotero DOI translator before Crossref BibTeX fallback.
        #[arg(long = "translator", overrides_with = "no_translator", action = ArgAction::SetTrue)]
        translator: bool,
        #[arg(long = "no-translator", overrides_with = "translator", action = ArgAction::SetTrue)]
        no_translator: bool,
        /// Also run PDF cascade after import.
        #[arg(long = "fetch-pdf", overrides_with = "no_fetch_pdf", action = ArgAction::SetTrue)]
        fetch_pdf: bool,
        #[arg(long = "no-fetch-pdf", overrides_with = "fetch_pdf", action = ArgAction::SetTrue)]
        no_fetch_pdf: bool,
        #[arg(
            long = "pdf-sources",
            default_value = "zotero,unpaywall,epmc,biorxiv,arxiv"
        )]
        pdf_sources: String,
    },
    /// Ingest an item by arXiv id.
    Arxiv {
        arxiv_id: String,
        #[arg(long = "collection")]
        collection_key: Option<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long = "if-exists", value_enum, default_value_t = IfExists::File)]
        if_exists: IfExists,
        #[arg(long = "fetch-pdf", overrides_with = "no_fetch_pdf", action = ArgAction::SetTrue)]
        fetch_pdf: bool,
        #[arg(long = "no-fetch-pdf", overrides_with = "fetch_pdf", action = ArgAction::SetTrue)]
        no_fetch_pdf: bool,
        #[arg(long = "pdf-sources", default_value = "zotero,arxiv,unpaywall")]
        pdf_sources: String,
    },
    /// Ingest a local file (translator-routed by extension).
    File {
        #[arg(value_name = "PATH")]
        path: String,
        #[arg(long = "collection")]
        collection_key: Option<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long = "if-exists", value_enum, default_value_t = IfExists::File)]
        if_exists: IfExists,
    },
    /// Ingest entries from a local BibTeX file via the connector.
    Bibtex {
        #[arg(value_name = "PATH")]
        path: String,
        #[arg(long = "collection")]
        collection_key: Option<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
    /// Ingest from arXiv/DOI/webpage URL.
    Url {
        url: String,
        #[arg(long = "collection")]
        collection_key: Option<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long = "if-exists", value_enum, default_value_t = IfExists::File)]
        if_exists: IfExists,
        #[arg(long = "fetch-pdf", overrides_with = "no_fetch_pdf", action = ArgAction::SetTrue)]
        fetch_pdf: bool,
        #[arg(long = "no-fetch-pdf", overrides_with = "fetch_pdf", action = ArgAction::SetTrue)]
        no_fetch_pdf: bool,
        #[arg(
            long = "pdf-sources",
            default_value = "zotero,unpaywall,epmc,biorxiv,arxiv"
        )]
        pdf_sources: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ImportCommands {
    /// Import records from a file via the Zotero connector.
    File {
        #[arg(value_name = "PATH")]
        path: String,
        /// Collection ID, key, or treeViewID target.
        #[arg(long = "collection")]
        collection_ref: Option<String>,
        /// Tag to apply after import. Repeatable.
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Optional JSON manifest describing attachments for imported records.
        #[arg(long = "attachments-manifest")]
        attachments_manifest: Option<String>,
        /// Default delay before each URL attachment download.
        #[arg(long = "attachment-delay-ms", default_value_t = 0)]
        attachment_delay_ms: i64,
        /// Default timeout in seconds for attachment download/upload.
        #[arg(long = "attachment-timeout", default_value_t = 60)]
        attachment_timeout: i64,
        /// Timeout in seconds for connector/import HTTP calls.
        #[arg(long = "connector-timeout", default_value_t = 120)]
        connector_timeout: u64,
        /// Auto-split multi-entry BibTeX into per-entry imports.
        #[arg(long = "split-bib", overrides_with = "no_split_bib", action = ArgAction::SetTrue)]
        split_bib: bool,
        #[arg(long = "no-split-bib", overrides_with = "split_bib", action = ArgAction::SetTrue)]
        no_split_bib: bool,
    },
    /// Import records from a connector-format JSON file.
    Json {
        #[arg(value_name = "PATH")]
        path: String,
        #[arg(long = "collection")]
        collection_ref: Option<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long = "attachment-delay-ms", default_value_t = 0)]
        attachment_delay_ms: i64,
        #[arg(long = "attachment-timeout", default_value_t = 60)]
        attachment_timeout: i64,
    },
    /// Import an item by DOI.
    Doi {
        doi: String,
        /// Collection key to add the imported item to.
        #[arg(long = "collection")]
        collection_key: Option<String>,
        /// Tag to apply after import. Repeatable.
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Reuse an existing library item with the same DOI when present.
        #[arg(long = "dedupe", overrides_with = "no_dedupe", action = ArgAction::SetTrue)]
        dedupe: bool,
        #[arg(long = "no-dedupe", overrides_with = "dedupe", action = ArgAction::SetTrue)]
        no_dedupe: bool,
        #[arg(long = "if-exists", value_enum, default_value_t = IfExists::File)]
        if_exists: IfExists,
        /// Try Zotero DOI translator before Crossref BibTeX fallback.
        #[arg(long = "translator", overrides_with = "no_translator", action = ArgAction::SetTrue)]
        translator: bool,
        #[arg(long = "no-translator", overrides_with = "translator", action = ArgAction::SetTrue)]
        no_translator: bool,
        /// Timeout for Crossref -> connector fallback import.
        #[arg(long = "connector-timeout", default_value_t = 120)]
        connector_timeout: u64,
    },
    /// Import an item by PMID using Zotero's built-in translator (via JS bridge).
    Pmid {
        pmid: String,
        /// Collection key to add the imported item to.
        #[arg(long = "collection")]
        collection_key: Option<String>,
        /// Tag to apply after import. Repeatable.
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
}
