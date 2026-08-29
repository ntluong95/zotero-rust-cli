pub mod catalog;
pub mod cli;
pub mod credentials;
pub mod db;
pub mod docx;
pub mod error;
pub mod http;
pub mod output;
pub mod paths;
pub mod runtime;
pub mod semantic;
pub mod session;
pub mod write;
pub mod write_router;

use clap::{CommandFactory, Parser};
use serde_json::Value;

use cli::{
    AppCommands, Cli, CollectionCommands, Commands, DocxCommands, ItemCommands, LibraryCommands,
    SearchCommands, SessionCommands, StyleCommands, TagCommands,
};

/// Port of `dispatch()`/`entrypoint()` (`zotero_cli.py:2657-2676`).
/// `clap`'s own usage-error handling (missing/invalid args) already exits
/// 2 internally during `Cli::parse()`, matching Click's
/// `UsageError.exit_code == 2` without any code here.
pub fn run() -> i32 {
    let mut cli = Cli::parse();
    let json_mode = cli.json;

    let Some(command) = cli.command.take() else {
        // Approved intentional break: upstream enters a blocking REPL here.
        // A blocking stdin read is the worst failure mode for a
        // non-interactive agent caller, so this prints help and exits 0.
        let _ = Cli::command().print_help();
        println!();
        return 0;
    };

    match dispatch_command(command, &cli, json_mode) {
        Ok(code) => code,
        Err(err) => {
            if json_mode {
                let payload = serde_json::json!({ "error": err.to_string() });
                println!("{}", output::json_text(&payload));
            } else {
                eprintln!("Error: {err}");
            }
            1
        }
    }
}

fn dispatch_command(command: Commands, cli: &Cli, json_mode: bool) -> anyhow::Result<i32> {
    let backend = cli.backend.to_string();
    // Lazy, matching Python's `current_runtime(ctx)` (`zotero_cli.py:235-244`):
    // built only by the command handlers that actually need it. Several
    // `session *` commands touch only local state and issue zero HTTP
    // probes in Python -- an earlier version of this function built the
    // runtime unconditionally for every command, which was invisible while
    // every landed command happened to need it, and only surfaced as a
    // real `http_calls` parity mismatch once `session status` and friends
    // landed. See `phase-05`'s "lazy runtime" clarification: "lazily"
    // means "on first access," not "skipped when unneeded" -- but *which*
    // commands need it at all is still command-specific, exactly as here.
    let build_runtime = || {
        runtime::build_runtime_context(runtime::BuildEnvironmentArgs {
            backend: &backend,
            data_dir: cli.data_dir.as_deref(),
            profile_dir: cli.profile_dir.as_deref(),
            executable: cli.executable.as_deref(),
        })
    };
    let session = session::load_session_state();

    match command {
        Commands::App(AppCommands::Status) => {
            let runtime = build_runtime();
            let payload = runtime.to_status_payload();
            output::emit(json_mode, &Value::Object(payload));
            Ok(0)
        }
        Commands::Item(ItemCommands::List { limit }) => {
            let runtime = build_runtime();
            let items = catalog::list_items(&runtime, &session, Some(limit))?;
            output::emit(json_mode, &serde_json::to_value(items)?);
            Ok(0)
        }
        Commands::Item(ItemCommands::Get { item_ref }) => {
            let runtime = build_runtime();
            let item = catalog::get_item(&runtime, item_ref.as_deref(), &session)?;
            output::emit(json_mode, &serde_json::to_value(item)?);
            Ok(0)
        }
        Commands::Item(ItemCommands::Find {
            query,
            collection,
            limit,
            exact_title,
            scope,
        }) => {
            let runtime = build_runtime();
            let items = catalog::find_items(
                &runtime,
                &query,
                collection.as_deref(),
                limit,
                exact_title,
                &scope.to_string(),
                &session,
            )?;
            output::emit(json_mode, &serde_json::to_value(items)?);
            Ok(0)
        }
        Commands::Item(ItemCommands::BuildIndex) => {
            // `item_build_index_command` (`zotero_cli.py:1211-1215`) is the
            // only one of the three semantic commands that calls
            // `current_runtime(ctx)` -- `semantic-search`/`similar` read the
            // vector DB directly via env vars and never touch it.
            let runtime = build_runtime();
            let config = semantic::SemanticConfig::from_env();
            let result = semantic::build_index(&runtime.environment.sqlite_path, &config, 20);
            let val = serde_json::to_value(&result)?;
            let is_ok = result.is_ok();
            output::emit(json_mode, &val);
            if is_ok {
                Ok(0)
            } else {
                Ok(1)
            }
        }
        Commands::Item(ItemCommands::SemanticSearch {
            query,
            top_k,
            min_score,
            language,
        }) => {
            let config = semantic::SemanticConfig::from_env();
            match semantic::semantic_search(
                &query,
                &config,
                top_k,
                min_score,
                &language.to_string(),
            ) {
                Ok(items) => {
                    output::emit(json_mode, &serde_json::to_value(items)?);
                    Ok(0)
                }
                Err(err_val) => {
                    output::emit(json_mode, &err_val);
                    Ok(1)
                }
            }
        }
        Commands::Item(ItemCommands::Similar {
            item_key,
            top_k,
            min_score,
        }) => {
            let config = semantic::SemanticConfig::from_env();
            match semantic::find_similar(&item_key, &config, top_k, min_score) {
                Ok(items) => {
                    output::emit(json_mode, &serde_json::to_value(items)?);
                    Ok(0)
                }
                Err(err_val) => {
                    output::emit(json_mode, &err_val);
                    Ok(1)
                }
            }
        }
        Commands::Collection(CollectionCommands::List) => {
            let runtime = build_runtime();
            let collections = catalog::list_collections(&runtime, &session)?;
            output::emit(json_mode, &serde_json::to_value(collections)?);
            Ok(0)
        }
        Commands::Collection(CollectionCommands::Find { query, limit }) => {
            let runtime = build_runtime();
            let collections = catalog::find_collections(&runtime, &query, limit, &session)?;
            output::emit(json_mode, &serde_json::to_value(collections)?);
            Ok(0)
        }
        Commands::Collection(CollectionCommands::Get { collection_ref }) => {
            let runtime = build_runtime();
            let collection =
                catalog::get_collection(&runtime, collection_ref.as_deref(), &session)?;
            output::emit(json_mode, &serde_json::to_value(collection)?);
            Ok(0)
        }
        Commands::Collection(CollectionCommands::Items { collection_ref }) => {
            let runtime = build_runtime();
            let items = catalog::collection_items(&runtime, collection_ref.as_deref(), &session)?;
            output::emit(json_mode, &serde_json::to_value(items)?);
            Ok(0)
        }
        Commands::Collection(CollectionCommands::Tree) => {
            let runtime = build_runtime();
            let tree = catalog::collection_tree(&runtime, &session)?;
            if json_mode {
                output::emit(json_mode, &serde_json::to_value(&tree)?);
            } else {
                print_collection_tree(&tree, 0);
            }
            Ok(0)
        }
        Commands::Library(LibraryCommands::List) => {
            let runtime = build_runtime();
            let libraries = catalog::list_libraries(&runtime)?;
            output::emit(json_mode, &serde_json::to_value(libraries)?);
            Ok(0)
        }
        Commands::Item(ItemCommands::Children { item_ref }) => {
            let runtime = build_runtime();
            let children = catalog::item_children(&runtime, item_ref.as_deref(), &session)?;
            output::emit(json_mode, &serde_json::to_value(children)?);
            Ok(0)
        }
        Commands::Item(ItemCommands::Notes { item_ref }) => {
            let runtime = build_runtime();
            let notes = catalog::item_notes(&runtime, item_ref.as_deref(), &session)?;
            output::emit(json_mode, &serde_json::to_value(notes)?);
            Ok(0)
        }
        Commands::Item(ItemCommands::Attachments { item_ref }) => {
            let runtime = build_runtime();
            let attachments = catalog::item_attachments(&runtime, item_ref.as_deref(), &session)?;
            output::emit(json_mode, &serde_json::to_value(attachments)?);
            Ok(0)
        }
        Commands::Item(ItemCommands::File { item_ref }) => {
            let runtime = build_runtime();
            let file = catalog::item_file(&runtime, item_ref.as_deref(), &session)?;
            output::emit(json_mode, &serde_json::to_value(file)?);
            Ok(0)
        }
        Commands::Search(SearchCommands::List) => {
            let runtime = build_runtime();
            let searches = catalog::list_searches(&runtime, &session)?;
            output::emit(json_mode, &serde_json::to_value(searches)?);
            Ok(0)
        }
        Commands::Search(SearchCommands::Get { search_ref }) => {
            let runtime = build_runtime();
            let search = catalog::get_search(&runtime, Some(&search_ref), &session)?;
            output::emit(json_mode, &serde_json::to_value(search)?);
            Ok(0)
        }
        Commands::Search(SearchCommands::Items { search_ref }) => {
            let runtime = build_runtime();
            let items = catalog::search_items(&runtime, Some(&search_ref), &session)?;
            output::emit(json_mode, &items);
            Ok(0)
        }
        Commands::Tag(TagCommands::List) => {
            let runtime = build_runtime();
            let tags = catalog::list_tags(&runtime, &session)?;
            output::emit(json_mode, &serde_json::to_value(tags)?);
            Ok(0)
        }
        Commands::Tag(TagCommands::Items { tag_ref }) => {
            let runtime = build_runtime();
            let items = catalog::tag_items(&runtime, &tag_ref, &session)?;
            output::emit(json_mode, &serde_json::to_value(items)?);
            Ok(0)
        }
        Commands::Style(StyleCommands::List) => {
            let runtime = build_runtime();
            let styles = catalog::list_styles(&runtime)?;
            output::emit(json_mode, &serde_json::to_value(styles)?);
            Ok(0)
        }
        Commands::Session(SessionCommands::Status) => {
            let payload = session::build_session_payload(&session);
            output::emit(json_mode, &serde_json::to_value(payload)?);
            Ok(0)
        }
        Commands::Session(SessionCommands::UseLibrary { library_ref }) => {
            let runtime = build_runtime();
            // `_normalize_session_library()` (`zotero_cli.py:367-374`).
            let library_id = catalog::resolve_library_id(&runtime, Some(&library_ref))?
                .ok_or_else(|| {
                    anyhow::Error::from(error::DomainError::new("Library reference required"))
                })?;
            let mut state = session;
            state.current_library = Some(serde_json::Value::from(library_id));
            session::save_session_state(&state)?;
            session::append_command_history(&format!("session use-library {library_ref}"))?;
            let payload = session::build_session_payload(&state);
            output::emit(json_mode, &serde_json::to_value(payload)?);
            Ok(0)
        }
        Commands::Session(SessionCommands::UseCollection { collection_ref }) => {
            let mut state = session;
            state.current_collection = Some(collection_ref.clone());
            session::save_session_state(&state)?;
            session::append_command_history(&format!("session use-collection {collection_ref}"))?;
            let payload = session::build_session_payload(&state);
            output::emit(json_mode, &serde_json::to_value(payload)?);
            Ok(0)
        }
        Commands::Session(SessionCommands::UseItem { item_ref }) => {
            let mut state = session;
            state.current_item = Some(item_ref.clone());
            session::save_session_state(&state)?;
            session::append_command_history(&format!("session use-item {item_ref}"))?;
            let payload = session::build_session_payload(&state);
            output::emit(json_mode, &serde_json::to_value(payload)?);
            Ok(0)
        }
        Commands::Session(SessionCommands::ClearLibrary) => {
            let mut state = session;
            state.current_library = None;
            session::save_session_state(&state)?;
            session::append_command_history("session clear-library")?;
            let payload = session::build_session_payload(&state);
            output::emit(json_mode, &serde_json::to_value(payload)?);
            Ok(0)
        }
        Commands::Session(SessionCommands::ClearCollection) => {
            let mut state = session;
            state.current_collection = None;
            session::save_session_state(&state)?;
            session::append_command_history("session clear-collection")?;
            let payload = session::build_session_payload(&state);
            output::emit(json_mode, &serde_json::to_value(payload)?);
            Ok(0)
        }
        Commands::Session(SessionCommands::ClearItem) => {
            let mut state = session;
            state.current_item = None;
            session::save_session_state(&state)?;
            session::append_command_history("session clear-item")?;
            let payload = session::build_session_payload(&state);
            output::emit(json_mode, &serde_json::to_value(payload)?);
            Ok(0)
        }
        Commands::Session(SessionCommands::History { limit }) => {
            let entries = session::python_negative_tail_slice(&session.command_history, limit);
            let payload = serde_json::json!({ "history": entries });
            output::emit(json_mode, &payload);
            Ok(0)
        }
        Commands::Docx(DocxCommands::InspectCitations { path, sample_limit }) => {
            let path_buf = std::path::PathBuf::from(&path);
            let payload = docx::inspect_citations(&path_buf, sample_limit)?;
            output::emit(json_mode, &payload);
            Ok(0)
        }
        Commands::Docx(DocxCommands::InspectPlaceholders { path, sample_limit }) => {
            let path_buf = std::path::PathBuf::from(&path);
            let payload = docx::inspect_placeholders(&path_buf, sample_limit)?;
            output::emit(json_mode, &payload);
            Ok(0)
        }
        Commands::Docx(DocxCommands::ValidatePlaceholders { path, sample_limit }) => {
            let runtime = build_runtime();
            let path_buf = std::path::PathBuf::from(&path);
            let payload = docx::validate_placeholders(&runtime, &path_buf, sample_limit, &session)?;
            output::emit(json_mode, &payload);
            Ok(0)
        }
        Commands::Docx(DocxCommands::RenderCitations {
            path,
            output: out_path,
            style,
            locale,
            bibliography,
            force,
        }) => {
            let runtime = build_runtime();
            let src_buf = std::path::PathBuf::from(&path);
            let out_buf = std::path::PathBuf::from(&out_path);
            let payload = docx::render_static_citations(
                &runtime,
                &src_buf,
                &out_buf,
                &style,
                &locale,
                &bibliography,
                &session,
                force,
            )?;
            output::emit(json_mode, &payload);
            Ok(0)
        }
    }
}

/// `_print_collection_tree()` (`zotero_cli.py:352-356`).
fn print_collection_tree(nodes: &[db::CollectionNode], level: usize) {
    let prefix = "  ".repeat(level);
    for node in nodes {
        println!(
            "{prefix}- {} [{}]",
            node.collection_name, node.collection_id
        );
        print_collection_tree(&node.children, level + 1);
    }
}
