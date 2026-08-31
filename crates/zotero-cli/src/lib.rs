pub mod add_import;
pub mod analysis;
pub mod annotations;
pub mod app_launch;
pub mod audit;
pub mod bridge;
pub mod catalog;
pub mod cli;
pub mod credentials;
pub mod csl;
pub mod db;
pub mod doctor;
pub mod docx;
pub mod error;
pub mod fulltext;
pub mod http;
pub mod hygiene;
pub mod import_attachments;
pub mod import_core;
pub mod import_normalization;
pub mod lifecycle;
pub mod metrics;
pub mod notes;
pub mod output;
pub mod paths;
pub mod pdf_cascade;
pub mod pdf_fetch;
pub mod plugin;
pub mod rendering;
pub mod runtime;
pub mod search;
pub mod semantic;
pub mod session;
pub mod target;
pub mod write;
pub mod write_router;

use clap::{CommandFactory, Parser};
use serde_json::Value;

use cli::{
    AddCommands, AppCommands, AuditCommands, Cli, CollectionCommands, Commands, DocxCommands,
    ExportCommands, ImportCommands, ItemCommands, LibraryCommands, NoteCommands, SearchCommands,
    SessionCommands, StyleCommands, TagCommands,
};
use write::WriteOutcome;

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
    // Every command that needs a live Zotero goes through this instead of `build_runtime()`:
    // it probes the named capability, launches Zotero exactly once if (and only if) Zotero
    // appears closed, waits for that specific backend, and hands back a re-probed runtime.
    // Diagnostics (`app doctor`/`status`/`ping`), offline SQLite reads, `session`, `docx`,
    // `audit` and `export` deliberately keep using `build_runtime()` and never launch anything.
    let live_runtime = |need: lifecycle::Backend| -> anyhow::Result<runtime::RuntimeContext> {
        let mut spawner = lifecycle::real_spawner();
        lifecycle::ensure_backend(build_runtime(), need, &mut spawner)
    };
    // Bridge-only commands take this cheaper path: the port is resolved from the filesystem and
    // the only probe is the Bridge's own ownership handshake. It never issues the connector /
    // Local-API probes unless the Bridge is missing and it has to decide whether to launch.
    let live_bridge = || -> anyhow::Result<bridge::JSBridgeClient> {
        let environment = paths::build_environment(
            cli.data_dir.as_deref(),
            cli.profile_dir.as_deref(),
            cli.executable.as_deref(),
            &paths::current_env_map(),
        );
        let mut spawner = lifecycle::real_spawner();
        lifecycle::ensure_bridge(&environment, &build_runtime, &mut spawner)
    };
    let session = session::load_session_state();

    match command {
        Commands::App(AppCommands::Status) => {
            let runtime = build_runtime();
            let payload = runtime.to_status_payload();
            output::emit(json_mode, &Value::Object(payload));
            Ok(0)
        }
        // `app_version()` (`zotero_cli.py:444-450`).
        Commands::App(AppCommands::Version) => {
            let runtime = build_runtime();
            let zotero_version = if runtime.environment.version.is_empty()
                || runtime.environment.version == "unknown"
            {
                None
            } else {
                Some(runtime.environment.version.as_str())
            };
            if json_mode {
                let payload = serde_json::json!({
                    "package_version": env!("CARGO_PKG_VERSION"),
                    "zotero_version": zotero_version,
                });
                output::emit(json_mode, &payload);
            } else {
                let val = match zotero_version {
                    Some(v) => Value::String(v.to_string()),
                    None => Value::Null,
                };
                output::emit(json_mode, &val);
            }
            Ok(0)
        }
        // `app_ping()` (`zotero_cli.py:475-482`).
        Commands::App(AppCommands::Ping) => {
            let runtime = build_runtime();
            if !runtime.connector_available {
                return Err(error::DomainError::new(runtime.connector_message).into());
            }
            let payload = serde_json::json!({
                "connector_available": true,
                "message": runtime.connector_message,
            });
            output::emit(json_mode, &payload);
            Ok(0)
        }
        // `app_doctor()` (`zotero_cli.py:534-543`).
        Commands::App(AppCommands::Doctor) => {
            let runtime = build_runtime();
            let bridge = bridge::JSBridgeClient::new(runtime.environment.port);
            // The same default staging directory `app install-plugin` writes to, so doctor can
            // tell "staged, awaiting the Zotero dialog" from "nothing staged at all".
            let payload = doctor::run_doctor(&runtime, &bridge, &plugin_output_dir(None));
            let code = exit_code_for(&payload);
            output::emit(json_mode, &payload);
            Ok(code)
        }
        // `app_launch()` (`zotero_cli.py:453-461`). `ctx.find_root().obj["runtime"] = None`
        // has no Rust equivalent to port: that line invalidates a cross-command runtime cache
        // that only matters across multiple commands in Python's REPL, which this build doesn't
        // implement (`repl` is a dropped command, per the plan's Challenge C4) -- this process
        // exits immediately after emitting `payload` regardless.
        Commands::App(AppCommands::Launch { wait_timeout }) => {
            let runtime = build_runtime();
            let mut spawner = app_launch::RealProcessSpawner;
            let payload = app_launch::launch_zotero(&runtime, wait_timeout, &mut spawner)?;
            output::emit(json_mode, &payload);
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
            all_libraries,
            include_feeds,
        }) => {
            // `item find` reads, so it never launches Zotero -- but it does prefer a Bridge that
            // is already up, which is the only way to search while Zotero holds the database
            // lock. `live_bridge` would launch on an unreachable endpoint, so the client is
            // built directly from the resolved port and simply declines when nothing answers.
            let runtime = build_runtime();
            let bridge = runtime.bridge_client();
            let libraries = if all_libraries {
                search::SearchScopeRequest::AllLibraries { include_feeds }
            } else {
                search::SearchScopeRequest::CurrentLibrary
            };
            let (items, _source) = search::find_items(
                &runtime,
                &bridge,
                &session,
                search::SearchRequest {
                    query: &query,
                    collection_ref: collection.as_deref(),
                    limit,
                    exact_title,
                    scope: &scope.to_string(),
                    libraries,
                },
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
            // Live-first, like `item find`: discovering which library to work in must not
            // require closing Zotero. Falls back to SQLite when no Bridge answers.
            let runtime = build_runtime();
            let bridge = runtime.bridge_client();
            let (libraries, _source) = search::list_libraries(&runtime, &bridge)?;
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
            let runtime = live_runtime(lifecycle::Backend::LocalApi)?;
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
            state.current_collection = Some(Value::String(collection_ref.clone()));
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
        // `session_use_selected()` (`zotero_cli.py:2371-2378`): same `catalog::use_selected_collection`
        // Connector call as `collection use-selected` -- Python has no separate
        // "getSelectedItems" endpoint -- but emits both the raw selection and the resulting
        // session payload, not just the raw selection.
        //
        // `history_count` in that emitted `"session"` payload under-reports by one: like
        // Python's `append_command_history()` (`session.py:95-103`), ours reloads its own fresh
        // copy of on-disk state, appends, and saves that -- neither ever mutates the in-memory
        // `state` this arm already holds. The append still lands correctly on disk (a later
        // `session status` sees it); only this command's own echoed count is stale, exactly
        // matching Python.
        Commands::Session(SessionCommands::UseSelected) => {
            let runtime = live_runtime(lifecycle::Backend::Connector)?;
            let selected = catalog::use_selected_collection(&runtime)?;
            let state = persist_selected_collection(&selected, session)?;
            session::append_command_history("session use-selected")?;
            let payload = serde_json::json!({
                "selected": selected,
                "session": session::build_session_payload(&state),
            });
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
        Commands::Item(ItemCommands::Update { item_key, fields }) => {
            let runtime = live_runtime(lifecycle::Backend::Write)?;
            item_update_command(&runtime, &session, json_mode, &item_key, &fields)
        }
        Commands::Item(ItemCommands::Tag {
            item_key,
            add,
            remove,
        }) => {
            let runtime = live_runtime(lifecycle::Backend::Write)?;
            item_tag_command(&runtime, &session, json_mode, &item_key, &add, &remove)
        }
        Commands::Item(ItemCommands::Delete { item_key, confirm }) => {
            let runtime = live_runtime(lifecycle::Backend::Write)?;
            item_delete_command(&runtime, &session, json_mode, &item_key, confirm)
        }
        Commands::Item(ItemCommands::Attach { item_key, pdf_path }) => {
            let runtime = live_runtime(lifecycle::Backend::Bridge)?;
            item_attach_command(&runtime, &session, json_mode, &item_key, &pdf_path)
        }
        Commands::Item(ItemCommands::AddToCollection {
            item_ref,
            collection_ref,
        }) => {
            let runtime = live_runtime(lifecycle::Backend::Write)?;
            item_add_to_collection_command(
                &runtime,
                &session,
                json_mode,
                &item_ref,
                &collection_ref,
            )
        }
        Commands::Item(ItemCommands::MoveToCollection {
            item_ref,
            collection_ref,
            from,
            all_other_collections,
        }) => {
            let runtime = live_runtime(lifecycle::Backend::Write)?;
            item_move_to_collection_command(
                &runtime,
                &session,
                json_mode,
                &item_ref,
                &collection_ref,
                &from,
                all_other_collections,
            )
        }
        Commands::Item(ItemCommands::Merge {
            keep_key,
            merge_keys,
            dry_run,
            confirm,
        }) => {
            // `--dry-run/--confirm` (`zotero_cli.py:1504`): a Click boolean flag pair,
            // `default=True` (dry-run). `resolve_bool_flag` + clap's `overrides_with` together
            // give the same "last flag wins, default dry-run" semantics as Click.
            let dry_run = resolve_bool_flag(dry_run, confirm, true);
            // Safe-by-default is preserved here: the preview path keeps its Bridge-first,
            // SQLite-fallback behavior and never launches Zotero. Only `--confirm`, which
            // actually mutates, requires the owned Bridge.
            let runtime = if dry_run {
                build_runtime()
            } else {
                live_runtime(lifecycle::Backend::Bridge)?
            };
            item_merge_command(
                &runtime,
                &session,
                json_mode,
                &keep_key,
                &merge_keys,
                dry_run,
            )
        }
        Commands::Collection(CollectionCommands::Create { name, parent }) => {
            let runtime = live_runtime(lifecycle::Backend::Write)?;
            collection_create_command(&runtime, &session, json_mode, &name, parent.as_deref())
        }
        Commands::Collection(CollectionCommands::Rename {
            collection_key,
            name,
            parent,
        }) => {
            let runtime = live_runtime(lifecycle::Backend::Write)?;
            collection_rename_command(
                &runtime,
                &session,
                json_mode,
                &collection_key,
                name.as_deref(),
                parent.as_deref(),
            )
        }
        Commands::Collection(CollectionCommands::Delete {
            collection_key,
            delete_items,
            confirm,
        }) => {
            let runtime = live_runtime(lifecycle::Backend::Write)?;
            collection_delete_command(
                &runtime,
                &session,
                json_mode,
                &collection_key,
                delete_items,
                confirm,
            )
        }
        Commands::Collection(CollectionCommands::RemoveItem {
            collection_key,
            item_key,
        }) => {
            let runtime = live_runtime(lifecycle::Backend::Write)?;
            collection_remove_item_command(
                &runtime,
                &session,
                json_mode,
                &collection_key,
                &item_key,
            )
        }
        Commands::App(AppCommands::InstallPlugin { output_dir }) => {
            let runtime = build_runtime();
            app_install_plugin_command(&runtime, json_mode, output_dir.as_deref())
        }
        Commands::App(AppCommands::PluginStatus { output_dir }) => {
            let runtime = build_runtime();
            app_plugin_status_command(&runtime, json_mode, output_dir.as_deref())
        }
        Commands::App(AppCommands::UninstallPlugin { output_dir }) => {
            app_uninstall_plugin_command(json_mode, output_dir.as_deref())
        }
        Commands::App(AppCommands::AuthorizeLocalApi { app_name }) => {
            let runtime = live_runtime(lifecycle::Backend::LocalApi)?;
            app_authorize_local_api_command(&runtime, json_mode, &app_name)
        }
        Commands::Item(ItemCommands::FindPdf { item_key, timeout }) => {
            let bridge = live_bridge()?;
            let payload = pdf_fetch::find_pdf_for_item(&bridge, &item_key, 1, timeout as u64);
            let code = exit_code_for(&payload);
            output::emit(json_mode, &payload);
            Ok(code)
        }
        Commands::Item(ItemCommands::FetchPdf {
            item_key,
            sources,
            force,
            zotero_timeout,
            download_timeout,
        }) => {
            let runtime = build_runtime();
            let bridge = runtime.bridge_client();
            let source_list = pdf_fetch::parse_sources(Some(&sources))?;
            let library_id = session::session_library_id(&session, 1)?;
            let client = pdf_fetch::UreqPdfClient;
            let payload = pdf_fetch::fetch_pdf_for_item(
                &runtime,
                &bridge,
                &client,
                &client,
                &item_key,
                &source_list,
                library_id,
                zotero_timeout,
                download_timeout,
                force,
            );
            let code = exit_code_for(&payload);
            output::emit(json_mode, &payload);
            Ok(code)
        }
        Commands::Item(ItemCommands::SearchFulltext { query, limit }) => {
            let bridge = live_bridge()?;
            let (payload, is_success) = fulltext::search_fulltext(&bridge, &query, limit);
            output::emit(json_mode, &payload);
            Ok(if is_success { 0 } else { 1 })
        }
        Commands::Item(ItemCommands::SearchAnnotations {
            query,
            colors,
            limit,
        }) => {
            let bridge = live_bridge()?;
            let colors_opt = (!colors.is_empty()).then_some(colors.as_slice());
            let (payload, is_success) =
                annotations::search_annotations(&bridge, &query, colors_opt, limit);
            output::emit(json_mode, &payload);
            Ok(if is_success { 0 } else { 1 })
        }
        Commands::Item(ItemCommands::Annotations { item_key }) => {
            let bridge = live_bridge()?;
            let (payload, is_success) = annotations::get_annotations(&bridge, &item_key);
            output::emit(json_mode, &payload);
            Ok(if is_success { 0 } else { 1 })
        }
        // `item_export()` (`zotero_cli.py:1249-1256`).
        Commands::Item(ItemCommands::Export { item_ref, fmt }) => {
            let runtime = build_runtime();
            let payload =
                rendering::export_item(&runtime, item_ref.as_deref(), &fmt.to_string(), &session)?;
            if json_mode {
                output::emit(json_mode, &serde_json::to_value(&payload)?);
            } else {
                output::emit(json_mode, &Value::String(payload.content));
            }
            Ok(0)
        }
        // `item_citation()` (`zotero_cli.py:1259-1268`).
        Commands::Item(ItemCommands::Citation {
            item_ref,
            style,
            locale,
            linkwrap,
        }) => {
            let runtime = build_runtime();
            let payload = rendering::citation_item(
                &runtime,
                item_ref.as_deref(),
                style.as_deref(),
                locale.as_deref(),
                linkwrap,
                &session,
            )?;
            if json_mode {
                output::emit(json_mode, &serde_json::to_value(&payload)?);
            } else {
                output::emit(
                    json_mode,
                    &Value::String(payload.citation.unwrap_or_default()),
                );
            }
            Ok(0)
        }
        // `item_bibliography()` (`zotero_cli.py:1271-1280`).
        Commands::Item(ItemCommands::Bibliography {
            item_ref,
            style,
            locale,
            linkwrap,
        }) => {
            let runtime = build_runtime();
            let payload = rendering::bibliography_item(
                &runtime,
                item_ref.as_deref(),
                style.as_deref(),
                locale.as_deref(),
                linkwrap,
                &session,
            )?;
            if json_mode {
                output::emit(json_mode, &serde_json::to_value(&payload)?);
            } else {
                output::emit(
                    json_mode,
                    &Value::String(payload.bibliography.unwrap_or_default()),
                );
            }
            Ok(0)
        }
        Commands::Item(ItemCommands::Context {
            item_ref,
            include_notes,
            include_bibtex,
            include_csljson,
            include_links,
        }) => {
            let runtime = build_runtime();
            let payload = analysis::build_item_context(
                &runtime,
                item_ref.as_deref(),
                include_notes,
                include_bibtex,
                include_csljson,
                include_links,
                &session,
            )?;
            if json_mode {
                output::emit(json_mode, &serde_json::to_value(&payload)?);
            } else {
                println!("{}", payload.prompt_context);
            }
            Ok(0)
        }
        Commands::Item(ItemCommands::Duplicates { by, limit }) => match by {
            cli::DuplicatesBy::Zotero => {
                let bridge = live_bridge()?;
                let (payload, exit_code) = hygiene::find_duplicates_zotero(&bridge, limit);
                output::emit(json_mode, &payload);
                Ok(exit_code)
            }
            cli::DuplicatesBy::Doi | cli::DuplicatesBy::Title => {
                let runtime = build_runtime();
                let library_id = session::session_library_id(&session, 1)?;
                let payload = hygiene::find_duplicates(
                    &runtime.environment.sqlite_path,
                    by,
                    library_id,
                    limit,
                )?;
                output::emit(json_mode, &serde_json::to_value(&payload)?);
                Ok(0)
            }
        },
        Commands::Item(ItemCommands::Metrics { ref_id, pmid }) => {
            let runtime = build_runtime();
            let payload = metrics::item_metrics(&runtime, &ref_id, pmid, &session)?;
            output::emit(json_mode, &payload);
            let exit_code = if payload.get("error").is_some() { 1 } else { 0 };
            Ok(exit_code)
        }
        Commands::Item(ItemCommands::Analyze {
            item_ref,
            question,
            model,
            include_notes,
            include_bibtex,
            include_csljson,
        }) => {
            let runtime = build_runtime();
            let payload = analysis::analyze_item(
                &runtime,
                item_ref.as_deref(),
                &question,
                &model,
                include_notes,
                include_bibtex,
                include_csljson,
                &session,
            )?;
            if json_mode {
                output::emit(json_mode, &serde_json::to_value(&payload)?);
            } else {
                println!("{}", payload.answer);
            }
            Ok(0)
        }
        // `export_bib_command()` (`zotero_cli.py:1906-1955`).
        Commands::Export(ExportCommands::Bib {
            items,
            collection_ref,
            fmt,
            output,
        }) => {
            let runtime = build_runtime();
            export_bib_command(
                &runtime,
                &session,
                json_mode,
                items.as_deref(),
                collection_ref.as_deref(),
                &fmt.to_string(),
                &output,
            )
        }
        Commands::Collection(CollectionCommands::FindPdfs {
            collection_key,
            timeout_per_item,
            limit,
        }) => {
            let bridge = live_bridge()?;
            let result = pdf_cascade::find_pdfs_in_collection(
                &bridge,
                &collection_key,
                1,
                timeout_per_item,
                limit,
            );
            let (payload, code) = unwrap_transport_envelope(result, true);
            output::emit(json_mode, &payload);
            Ok(code)
        }
        Commands::Collection(CollectionCommands::FetchPdfs {
            collection_key,
            sources,
            limit,
            zotero_timeout,
            download_timeout,
            jsonl_progress,
            resume,
            reset_resume,
        }) => {
            let runtime = live_runtime(lifecycle::Backend::Bridge)?;
            let bridge = runtime.bridge_client();
            let source_list = pdf_fetch::parse_sources(Some(&sources))?;
            let library_id = session::session_library_id(&session, 1)?;
            let client = pdf_fetch::UreqPdfClient;
            let mut progress = |row: &Value| {
                if jsonl_progress {
                    println!("{}", serde_json::to_string(row).unwrap_or_default());
                }
            };
            let payload = pdf_cascade::fetch_pdfs_for_collection(
                &runtime,
                &bridge,
                &client,
                &client,
                &collection_key,
                &source_list,
                library_id,
                limit,
                zotero_timeout,
                download_timeout,
                Some(&mut progress),
                resume,
                reset_resume,
            );
            let code = exit_code_for(&payload);
            output::emit(json_mode, &payload);
            Ok(code)
        }
        // `collection_use_selected()` (`zotero_cli.py:789-796`).
        Commands::Collection(CollectionCommands::UseSelected) => {
            let runtime = live_runtime(lifecycle::Backend::Connector)?;
            let selected = catalog::use_selected_collection(&runtime)?;
            persist_selected_collection(&selected, session)?;
            session::append_command_history("collection use-selected")?;
            output::emit(json_mode, &selected);
            Ok(0)
        }
        // `collection_stats_command()` (`zotero_cli.py:916-922`): `library_id` is hardcoded to
        // `1`, matching Python's CLI layer (no `--library` option exists for this command --
        // a group-library collection can never be targeted through it).
        Commands::Collection(CollectionCommands::Stats { collection_key }) => {
            let bridge = live_bridge()?;
            let transport = bridge.collection_stats(1, &collection_key);
            let (payload, is_success) = bridge::client::classify_bridge_payload(&transport);
            output::emit(json_mode, &payload);
            Ok(if is_success { 0 } else { 1 })
        }
        Commands::Note(NoteCommands::Get { note_ref }) => {
            let runtime = build_runtime();
            let item = notes::get_note(&runtime, Some(&note_ref), &session)?;
            if json_mode {
                output::emit(json_mode, &serde_json::to_value(&item)?);
            } else {
                println!("{}", item.note_text);
            }
            Ok(0)
        }
        Commands::Note(NoteCommands::Add {
            item_ref,
            text,
            file_path,
            fmt,
        }) => {
            // Argument validation first: `--text`/`--file` mutual exclusion is a usage error and
            // must never require -- let alone start -- a live Zotero.
            let input = notes::resolve_note_input(text.as_deref(), file_path.as_deref())?;
            let runtime = live_runtime(lifecycle::Backend::Bridge)?;
            let bridge = runtime.bridge_client();
            let fmt_str = fmt.to_string();
            let result = notes::add_note(
                &runtime,
                &bridge,
                &item_ref,
                input,
                Some(&fmt_str),
                &session,
            )?;
            output::emit(json_mode, &serde_json::to_value(&result)?);
            Ok(0)
        }
        Commands::Add(AddCommands::Doi {
            doi,
            collection_key,
            tags,
            if_exists,
            translator,
            no_translator,
            fetch_pdf,
            no_fetch_pdf,
            pdf_sources,
        }) => {
            let runtime = build_runtime();
            let mut bridge = runtime.bridge_client();
            let prefer_translator = resolve_bool_flag(translator, no_translator, true);
            let fetch_pdf = resolve_bool_flag(fetch_pdf, no_fetch_pdf, false);
            let library_id = session::session_library_id(&session, 1)?;
            let options = add_import::AddImportOptions {
                collection_key,
                tags,
                session,
                if_exists: if_exists.to_string(),
                dedupe: true,
                prefer_translator,
                fetch_pdf,
                pdf_sources: Some(pdf_sources),
                library_id,
                connector_timeout: std::time::Duration::from_secs(120),
            };
            let payload = add_import::add_doi(&runtime, &mut bridge, &doi, options);
            let code = exit_code_for(&payload);
            output::emit(json_mode, &payload);
            Ok(code)
        }
        Commands::Add(AddCommands::Arxiv {
            arxiv_id,
            collection_key,
            tags,
            if_exists,
            fetch_pdf,
            no_fetch_pdf,
            pdf_sources,
        }) => {
            let runtime = build_runtime();
            let mut bridge = runtime.bridge_client();
            let fetch_pdf = resolve_bool_flag(fetch_pdf, no_fetch_pdf, true);
            let library_id = session::session_library_id(&session, 1)?;
            let options = add_import::AddImportOptions {
                collection_key,
                tags,
                session,
                if_exists: if_exists.to_string(),
                dedupe: true,
                prefer_translator: true,
                fetch_pdf,
                pdf_sources: Some(pdf_sources),
                library_id,
                connector_timeout: std::time::Duration::from_secs(120),
            };
            let payload = add_import::add_arxiv(&runtime, &mut bridge, &arxiv_id, options);
            let code = exit_code_for(&payload);
            output::emit(json_mode, &payload);
            Ok(code)
        }
        Commands::Add(AddCommands::File {
            path,
            collection_key,
            tags,
            if_exists,
        }) => {
            let runtime = build_runtime();
            let mut bridge = runtime.bridge_client();
            let library_id = session::session_library_id(&session, 1)?;
            let options = add_import::AddImportOptions {
                collection_key,
                tags,
                session,
                if_exists: if_exists.to_string(),
                dedupe: true,
                prefer_translator: true,
                fetch_pdf: false,
                pdf_sources: None,
                library_id,
                connector_timeout: std::time::Duration::from_secs(120),
            };
            let payload =
                add_import::add_file(&runtime, &mut bridge, std::path::Path::new(&path), options);
            let code = exit_code_for(&payload);
            output::emit(json_mode, &payload);
            Ok(code)
        }
        Commands::Add(AddCommands::Bibtex {
            path,
            collection_key,
            tags,
        }) => {
            let runtime = build_runtime();
            let payload = add_import::add_bibtex(
                &runtime,
                std::path::Path::new(&path),
                collection_key,
                tags,
                session,
            );
            let code = exit_code_for(&payload);
            output::emit(json_mode, &payload);
            Ok(code)
        }
        Commands::Add(AddCommands::Url {
            url,
            collection_key,
            tags,
            if_exists,
            fetch_pdf,
            no_fetch_pdf,
            pdf_sources,
        }) => {
            let runtime = build_runtime();
            let mut bridge = runtime.bridge_client();
            let fetch_pdf = resolve_bool_flag(fetch_pdf, no_fetch_pdf, false);
            let library_id = session::session_library_id(&session, 1)?;
            let options = add_import::AddImportOptions {
                collection_key,
                tags,
                session,
                if_exists: if_exists.to_string(),
                dedupe: true,
                prefer_translator: true,
                fetch_pdf,
                pdf_sources: Some(pdf_sources),
                library_id,
                connector_timeout: std::time::Duration::from_secs(120),
            };
            let payload = add_import::add_url(&runtime, &mut bridge, &url, options);
            let code = exit_code_for(&payload);
            output::emit(json_mode, &payload);
            Ok(code)
        }
        Commands::Import(ImportCommands::File {
            path,
            collection_ref,
            tags,
            attachments_manifest,
            attachment_delay_ms,
            attachment_timeout,
            connector_timeout,
            split_bib,
            no_split_bib,
        }) => {
            let runtime = build_runtime();
            let split_bib = resolve_bool_flag(split_bib, no_split_bib, true);
            let options = import_core::ImportOptions {
                collection_ref,
                tags,
                session,
                attachment_manifest: attachments_manifest.map(std::path::PathBuf::from),
                attachment_delay_ms,
                attachment_timeout,
                connector_timeout: std::time::Duration::from_secs(connector_timeout),
                split_bib,
            };
            let payload = import_core::import_file(&runtime, std::path::Path::new(&path), options)?;
            let code = exit_code_for(&payload);
            output::emit(json_mode, &payload);
            Ok(code)
        }
        Commands::Import(ImportCommands::Json {
            path,
            collection_ref,
            tags,
            attachment_delay_ms,
            attachment_timeout,
        }) => {
            let runtime = build_runtime();
            let options = import_core::ImportOptions {
                collection_ref,
                tags,
                session,
                attachment_manifest: None,
                attachment_delay_ms,
                attachment_timeout,
                connector_timeout: std::time::Duration::from_secs(120),
                split_bib: false,
            };
            let payload = import_core::import_json(&runtime, std::path::Path::new(&path), options)?;
            let code = exit_code_for(&payload);
            output::emit(json_mode, &payload);
            Ok(code)
        }
        Commands::Import(ImportCommands::Doi {
            doi,
            collection_key,
            tags,
            dedupe,
            no_dedupe,
            if_exists,
            translator,
            no_translator,
            connector_timeout,
        }) => {
            let runtime = build_runtime();
            let mut bridge = runtime.bridge_client();
            let dedupe = resolve_bool_flag(dedupe, no_dedupe, true);
            let prefer_translator = resolve_bool_flag(translator, no_translator, true);
            let library_id = session::session_library_id(&session, 1)?;
            let options = add_import::AddImportOptions {
                collection_key,
                tags,
                session,
                if_exists: if_exists.to_string(),
                dedupe,
                prefer_translator,
                fetch_pdf: false,
                pdf_sources: None,
                library_id,
                connector_timeout: std::time::Duration::from_secs(connector_timeout),
            };
            let payload = add_import::import_doi(&runtime, &mut bridge, &doi, options);
            let code = exit_code_for(&payload);
            output::emit(json_mode, &payload);
            Ok(code)
        }
        Commands::Import(ImportCommands::Pmid {
            pmid,
            collection_key,
            tags,
        }) => {
            let mut bridge = live_bridge()?;
            let payload =
                add_import::import_pmid(&mut bridge, &pmid, collection_key.as_deref(), &tags, 1);
            let code = exit_code_for(&payload);
            output::emit(json_mode, &payload);
            Ok(code)
        }
        // `audit_path_command()` (`zotero_cli.py:551-564`).
        Commands::Audit(AuditCommands::Path) => {
            let path = audit::audit_path();
            let payload = serde_json::json!({
                "action": "audit_path",
                "ok": true,
                "status": "success",
                "path": path.to_string_lossy(),
            });
            if json_mode {
                println!("{}", output::json_text(&payload));
            } else {
                println!("{}", path.to_string_lossy());
            }
            Ok(0)
        }
        // `audit_tail_command()` (`zotero_cli.py:566-590`).
        Commands::Audit(AuditCommands::Tail { limit }) => {
            let path = audit::audit_path();
            let entries = audit::tail(limit);
            let payload = serde_json::json!({
                "action": "audit_tail",
                "ok": true,
                "status": "success",
                "path": path.to_string_lossy(),
                "count": entries.len(),
                "entries": entries,
            });
            if json_mode {
                println!("{}", output::json_text(&payload));
            } else {
                for entry in &entries {
                    println!("{}", output::json_text(entry));
                }
                if entries.is_empty() {
                    println!("(empty audit log)");
                }
            }
            Ok(0)
        }
        Commands::Js { code, wait } => js_command(&live_bridge()?, json_mode, &code, wait),
        Commands::Sync => sync_command(&live_bridge()?, json_mode),
    }
}

/// `exit_code_for()` (`core/results.py::exit_code_for`): `ok: false` or a
/// `status` of `partial_success`/`error`/`failed`/`timeout` maps to exit 1;
/// everything else (including a missing/non-boolean `ok`) is exit 0.
fn exit_code_for(payload: &Value) -> i32 {
    if payload.get("ok") == Some(&Value::Bool(false)) {
        return 1;
    }
    let status = payload.get("status").and_then(Value::as_str).unwrap_or("");
    if matches!(status, "partial_success" | "error" | "failed" | "timeout") {
        return 1;
    }
    0
}

/// `emit_js()`'s transport-envelope unwrap (`zotero_cli.py:317-349`), for callers (only
/// `collection find-pdfs` in this slice) whose core function returns the raw `{ok, data, error}`
/// wrapper rather than an already-flattened `result_payload()`-shaped value.
fn unwrap_transport_envelope(result: Value, require_data: bool) -> (Value, i32) {
    if result.get("ok") != Some(&Value::Bool(true)) {
        return (result, 1);
    }
    let data = result.get("data").cloned();
    match data {
        None | Some(Value::Null) if require_data => (
            serde_json::json!({
                "ok": false,
                "data": null,
                "error": "JS bridge returned empty success (data is null)",
                "code": "EMPTY_RESULT",
            }),
            1,
        ),
        Some(Value::Object(ref map)) if map.get("ok") == Some(&Value::Bool(false)) => {
            (data.unwrap(), 1)
        }
        Some(d) => (d, 0),
        None => (result, 0),
    }
}

/// Resolves a Click-style `--flag/--no-flag` boolean pair (mutually exclusive via clap's
/// `overrides_with`) to its effective value: an explicit flag wins over `default`.
fn resolve_bool_flag(positive: bool, negative: bool, default: bool) -> bool {
    if positive {
        true
    } else if negative {
        false
    } else {
        default
    }
}

/// Maps a non-`Applied` `WriteOutcome` to its `(exit_code, json_payload)` per §3.3's
/// machine-distinguishable "needs human action" signal: `Required`/`Revoked`/`Denied` are a
/// dedicated exit code (3) an agent caller can branch on without parsing prose; `RateLimited` is
/// transient/backoff-safe and must not be conflated with that signal (exit 1, same as any other
/// non-actionable failure). Returns `None` for `Applied` -- the caller renders that case itself
/// via the §3.5 compatibility renderer, never from this function.
fn write_outcome_failure(outcome: &WriteOutcome) -> Option<(i32, Value)> {
    use write::AuthorizationReason;
    match outcome {
        WriteOutcome::Applied { .. } => None,
        WriteOutcome::AuthorizationFailed {
            reason,
            source,
            detail,
        } => {
            let needs_human_action = !matches!(reason, AuthorizationReason::RateLimited);
            let exit_code = if needs_human_action { 3 } else { 1 };
            Some((
                exit_code,
                serde_json::json!({
                    "outcome": "authorization_failed",
                    "reason": reason,
                    "source": source,
                    "detail": detail,
                    "needs_human_action": needs_human_action,
                }),
            ))
        }
        WriteOutcome::PreconditionFailed { detail } => Some((
            1,
            serde_json::json!({
                "outcome": "precondition_failed",
                "detail": detail,
                "needs_human_action": false,
            }),
        )),
        WriteOutcome::Conflict { detail } => Some((
            1,
            serde_json::json!({
                "outcome": "conflict",
                "detail": detail,
                "needs_human_action": false,
            }),
        )),
        WriteOutcome::TransportError { detail } => Some((
            1,
            serde_json::json!({
                "outcome": "transport_error",
                "detail": detail,
                "needs_human_action": false,
            }),
        )),
    }
}

/// §3.5's compatibility renderer contract, generalized to be genuinely backend-neutral (review
/// finding: the same command must never change JSON schema depending on which backend an
/// invisible capability flag happened to select). A `CanonicalWriteView` is the one shape every
/// write command renders through, regardless of whether it came from a Local API GET
/// (`write_router::LocalApiItemSummary`) or a live JS Bridge readback (`parse_live_object`
/// below) -- never from SQLite, which cannot be trusted to see a write made moments earlier by
/// this same process (Zotero's exclusive SQLite lock / WAL checkpoint delay). Excludes any raw
/// Local-API-internal `version` field on purpose -- the standing backend-identity denylist
/// (§3.5/Testing Strategy) forbids it from ever reaching stdout JSON.
struct CanonicalWriteView {
    key: String,
    library_id: i64,
    item_type: String,
    data: serde_json::Map<String, Value>,
}

impl From<&write_router::LocalApiItemSummary> for CanonicalWriteView {
    fn from(summary: &write_router::LocalApiItemSummary) -> Self {
        CanonicalWriteView {
            key: summary.key.clone(),
            library_id: summary.library_id,
            item_type: summary.item_type.clone(),
            data: summary.data.clone(),
        }
    }
}

/// Live Zotero-runtime item readback (never SQLite): returns the same envelope shape produced by
/// `POST /cli-bridge/eval`'s ownership-gated `execute_js`, using the real `Item#toJSON()`/
/// `Collection#toJSON()` Zotero API already relied on for sync -- a JSON shape close enough to
/// the Local API's own `data` object that both sources normalize into one `CanonicalWriteView`.
/// Written as an inline template (not a new `bridge/js/*.js` file) to stay within this slice's
/// `cli.rs`/`lib.rs` file-ownership boundary; still routed through `bridge::templates::render`
/// for the same D1 `JSON.parse`-based parameter safety every other Bridge call uses.
const LIVE_ITEM_READBACK_JS: &str = r#"
var item = Zotero.Items.getByLibraryAndKey(P.libraryID, P.key);
if (!item) { return JSON.stringify({found: false}); }
return JSON.stringify({found: true, key: item.key, libraryID: item.libraryID, data: item.toJSON()});
"#;

const LIVE_COLLECTION_READBACK_JS: &str = r#"
var col = Zotero.Collections.getByLibraryAndKey(P.libraryID, P.key);
if (!col) { return JSON.stringify({found: false}); }
return JSON.stringify({found: true, key: col.key, libraryID: col.libraryID, data: col.toJSON()});
"#;

/// Runs a live readback template against the JS Bridge. Reuses the same
/// `bridge_endpoint_active()` ownership gate and positive-probe cache every other Bridge call
/// goes through (`execute_js`) -- a second live readback in the same process after an already-
/// successful write costs exactly one more request, not a second ownership probe.
fn bridge_live_read(
    client: &bridge::JSBridgeClient,
    template: &str,
    library_id: u32,
    key: &str,
) -> anyhow::Result<Value> {
    let params = serde_json::json!({ "libraryID": library_id, "key": key });
    let code = bridge::templates::render(template, &params)?;
    client.execute_raw_js(&code, 10)
}

/// Whether a `bridge_live_read` response reports the object absent (`{"found": false}`) -- the
/// live equivalent of `write_router::PresenceCheck::Absent`, used for Bridge-routed delete/merge
/// verification instead of a same-process SQLite re-read.
fn is_live_object_absent(raw: &Value) -> bool {
    raw.get("found").and_then(Value::as_bool) == Some(false)
}

/// Normalizes a `bridge_live_read` response into the same `CanonicalWriteView` shape a Local API
/// GET produces. `what` is only used for the error message on an unexpectedly-absent object (a
/// find-immediately-after-write mismatch, not a designed code path).
fn parse_live_object(raw: &Value, what: &str) -> anyhow::Result<CanonicalWriteView> {
    if is_live_object_absent(raw) {
        anyhow::bail!("{what} was not found via the live Zotero-runtime readback");
    }
    let key = raw
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("live readback response missing `key`"))?
        .to_string();
    let library_id = raw
        .get("libraryID")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow::anyhow!("live readback response missing `libraryID`"))?;
    let data = raw
        .get("data")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("live readback response missing `data`"))?;
    let item_type = data
        .get("itemType")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(CanonicalWriteView {
        key,
        library_id,
        item_type,
        data,
    })
}

/// The one renderer every write command's `Applied` case emits through, regardless of backend
/// (§3.5, review Blocker 1). Never includes a raw Local-API-internal `version` field.
fn render_write_result(
    view: &CanonicalWriteView,
    mismatches: &[write_router::FieldMismatch],
) -> Value {
    let mut payload = serde_json::json!({
        "outcome": "applied",
        "key": view.key,
        "library_id": view.library_id,
        "item_type": view.item_type,
        "data": Value::Object(view.data.clone()),
    });
    if !mismatches.is_empty() {
        if let Value::Object(map) = &mut payload {
            map.insert(
                "field_mismatches".to_string(),
                serde_json::to_value(mismatches).unwrap_or(Value::Null),
            );
        }
    }
    payload
}

/// §3.5's requested-vs-observed field diff, shared by every backend (previously Local-API-only
/// logic hidden inside `write_router::verify_write`): a Local API PATCH that partially applies,
/// or a Bridge script that silently drops a field, must surface a mismatch rather than a
/// same-looking, silent success either way.
fn diff_requested_fields(
    requested: &serde_json::Map<String, Value>,
    observed: &serde_json::Map<String, Value>,
) -> Vec<write_router::FieldMismatch> {
    let mut mismatches = Vec::new();
    for (field, requested_value) in requested {
        let observed_value = observed.get(field).cloned();
        if observed_value.as_ref() != Some(requested_value) {
            mismatches.push(write_router::FieldMismatch {
                field: field.clone(),
                requested: requested_value.clone(),
                observed: observed_value,
            });
        }
    }
    mismatches
}

/// Re-reads `path` via the Local API after a successful write, converts it to the canonical
/// write-result view, diffs `requested` against the observed fields, and emits the rendered
/// result. Shared by every Local-API-routed CRUD command.
fn render_local_api_object_after_write(
    runtime: &runtime::RuntimeContext,
    json_mode: bool,
    path: &str,
    requested: &serde_json::Map<String, Value>,
) -> anyhow::Result<i32> {
    match write_router::verify_present(runtime, path) {
        write_router::PresenceCheck::Present(summary) => {
            let view = CanonicalWriteView::from(&summary);
            let mismatches = diff_requested_fields(requested, &view.data);
            output::emit(json_mode, &render_write_result(&view, &mismatches));
            Ok(0)
        }
        other => presence_check_error(path, other),
    }
}

/// Re-reads an item live through the JS Bridge (never SQLite) after a successful Bridge write,
/// converts it to the canonical write-result view, diffs `requested` against the observed
/// fields, and emits the rendered result. The Bridge-side counterpart to
/// `render_local_api_object_after_write` -- both converge on the exact same output shape.
fn render_bridge_item_after_write(
    client: &bridge::JSBridgeClient,
    json_mode: bool,
    library_id: u32,
    key: &str,
    requested: &serde_json::Map<String, Value>,
) -> anyhow::Result<i32> {
    let raw = bridge_live_read(client, LIVE_ITEM_READBACK_JS, library_id, key)?;
    let view = parse_live_object(&raw, "item")?;
    let mismatches = diff_requested_fields(requested, &view.data);
    output::emit(json_mode, &render_write_result(&view, &mismatches));
    Ok(0)
}

/// Collection counterpart to `render_bridge_item_after_write`.
fn render_bridge_collection_after_write(
    client: &bridge::JSBridgeClient,
    json_mode: bool,
    library_id: u32,
    key: &str,
    requested: &serde_json::Map<String, Value>,
) -> anyhow::Result<i32> {
    let raw = bridge_live_read(client, LIVE_COLLECTION_READBACK_JS, library_id, key)?;
    let view = parse_live_object(&raw, "collection")?;
    let mismatches = diff_requested_fields(requested, &view.data);
    output::emit(json_mode, &render_write_result(&view, &mismatches));
    Ok(0)
}

/// Maps every non-`Present` `PresenceCheck` (encountered while fetching an object's current
/// state immediately before a write -- never a post-write check, which callers handle inline)
/// to a plain domain error. Only ever called with the non-`Present` variants in practice.
fn presence_check_error(path: &str, check: write_router::PresenceCheck) -> anyhow::Result<i32> {
    match check {
        write_router::PresenceCheck::Present(_) => {
            unreachable!("presence_check_error is only called for the non-Present variants")
        }
        write_router::PresenceCheck::Absent => {
            Err(error::DomainError::new(format!("Local API object not found: {path}")).into())
        }
        write_router::PresenceCheck::Unexpected { status, detail } => Err(error::DomainError::new(
            format!("Local API returned unexpected HTTP {status} reading {path}: {detail}"),
        )
        .into()),
        write_router::PresenceCheck::TransportError { detail } => Err(error::DomainError::new(
            format!("Local API transport error reading {path}: {detail}"),
        )
        .into()),
    }
}

/// Extracts a Zotero item/collection array field (e.g. `data.collections`) as a plain string
/// list, for `write_router::union_replace`/`difference_replace`'s full-array-replace helpers.
fn extract_string_array(data: &serde_json::Map<String, Value>, field: &str) -> Vec<String> {
    data.get(field)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Computes the full replacement `data.tags` array (Zotero tag objects, `{"tag": "...", ...}`,
/// not plain strings -- unlike `collections`) for `item tag --add/--remove`'s full-array-replace.
/// Preserves every field on an untouched existing tag entry (e.g. an automatic tag's `type`);
/// only appends a bare `{"tag": name}` for a genuinely new tag.
fn compute_tag_array(current: &Value, add: &[String], remove: &[String]) -> Vec<Value> {
    let mut result: Vec<Value> = current.as_array().cloned().unwrap_or_default();
    if !remove.is_empty() {
        result.retain(|entry| {
            let name = entry.get("tag").and_then(Value::as_str).unwrap_or("");
            !remove.iter().any(|r| r == name)
        });
    }
    for name in add {
        let exists = result
            .iter()
            .any(|entry| entry.get("tag").and_then(Value::as_str) == Some(name.as_str()));
        if !exists {
            result.push(serde_json::json!({ "tag": name }));
        }
    }
    result
}

fn library_id_u32(library_id: i64) -> anyhow::Result<u32> {
    u32::try_from(library_id).map_err(|_| {
        anyhow::Error::from(error::DomainError::new(format!(
            "library id {library_id} is out of range for the JS Bridge"
        )))
    })
}

fn item_update_command(
    runtime: &runtime::RuntimeContext,
    session: &session::SessionState,
    json_mode: bool,
    item_key: &str,
    fields: &[(String, String)],
) -> anyhow::Result<i32> {
    if fields.is_empty() {
        return Err(error::DomainError::new("At least one --field key=value is required").into());
    }
    let client = runtime.bridge_client();
    let prefer = target::Prefer::for_runtime(runtime);
    let item = target::resolve_item(runtime, &client, Some(item_key), session, prefer)?;
    let mut body = serde_json::Map::new();
    for (key, value) in fields {
        body.insert(key.clone(), Value::String(value.clone()));
    }

    if runtime.local_api_writes_available {
        let scope = item.local_api_scope()?;
        let path = format!("{scope}/items/{}", item.key);
        let current = match write_router::verify_present(runtime, &path) {
            write_router::PresenceCheck::Present(summary) => summary,
            other => return presence_check_error(&path, other),
        };
        let outcome = write_router::patch_item(
            runtime,
            &path,
            &item.key,
            &Value::Object(body.clone()),
            current.version,
        )?;
        if let Some((code, payload)) = write_outcome_failure(&outcome) {
            output::emit(json_mode, &payload);
            return Ok(code);
        }
        return render_local_api_object_after_write(runtime, json_mode, &path, &body);
    }

    let library_id = library_id_u32(item.library_id)?;
    let mut fields_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for (key, value) in fields {
        fields_map.insert(key.clone(), value.clone());
    }
    let outcome = client.item_update(library_id, &item.key, &fields_map)?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }
    render_bridge_item_after_write(&client, json_mode, library_id, &item.key, &body)
}

/// `_persist_selected_collection()` (`zotero_cli.py:781-786`): overwrites `current_library` and
/// `current_collection` with the Connector's raw `libraryID`/`id` fields verbatim (a JSON
/// number, or `null` if either key is absent from the selection response), then saves. Shared by
/// `collection use-selected` and `session use-selected` -- the only state mutation either command
/// performs, and it is CLI-owned session state, never Zotero library data.
fn persist_selected_collection(
    selected: &Value,
    session: session::SessionState,
) -> anyhow::Result<session::SessionState> {
    let mut state = session;
    state.current_library = Some(selected.get("libraryID").cloned().unwrap_or(Value::Null));
    state.current_collection = Some(selected.get("id").cloned().unwrap_or(Value::Null));
    session::save_session_state(&state)?;
    Ok(state)
}

/// `_split_export_refs()` (`zotero_cli.py:1958-1962`).
fn split_export_refs(items: &str) -> anyhow::Result<Vec<String>> {
    let refs: Vec<String> = items
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect();
    if refs.is_empty() {
        return Err(
            error::DomainError::new("--items must contain at least one item key or ID.").into(),
        );
    }
    Ok(refs)
}

/// `export_bib_command()` (`zotero_cli.py:1912-1955`): exports real Zotero items to a standalone
/// BibTeX/BibLaTeX file. The only command in this slice that writes to local disk -- always at
/// the caller-supplied `--output` path, never elsewhere, and it always overwrites (Python's
/// `Path.write_text` truncates unconditionally; there is no separate overwrite guard to port).
fn export_bib_command(
    runtime: &runtime::RuntimeContext,
    session: &session::SessionState,
    json_mode: bool,
    items: Option<&str>,
    collection_ref: Option<&str>,
    fmt: &str,
    output: &str,
) -> anyhow::Result<i32> {
    // `bool(items) == bool(collection_ref)` (`zotero_cli.py:1914`): Python's truthiness treats an
    // empty string the same as `None`, so an empty `--items ""` / `--collection ""` is filtered
    // here too rather than just checked for `Option::is_some()`.
    let items = items.filter(|value| !value.is_empty());
    let collection_ref = collection_ref.filter(|value| !value.is_empty());
    if items.is_some() == collection_ref.is_some() {
        return Err(error::DomainError::new("Pass exactly one of --items or --collection.").into());
    }

    let (refs, source) = if let Some(items) = items {
        let refs = split_export_refs(items)?;
        let source = serde_json::json!({"type": "items", "refs": refs});
        (refs, source)
    } else {
        let collection_ref = collection_ref.expect("exactly one of items/collection_ref is Some");
        let collection = catalog::get_collection(runtime, Some(collection_ref), session)?;
        let refs: Vec<String> = catalog::collection_items(runtime, Some(collection_ref), session)?
            .into_iter()
            .filter(|item| {
                item.type_name != "attachment"
                    && item.type_name != "note"
                    && item.type_name != "annotation"
            })
            .map(|item| item.key)
            .collect();
        let source = serde_json::json!({
            "type": "collection",
            "collection": serde_json::to_value(&collection)?,
        });
        (refs, source)
    };

    if refs.is_empty() {
        return Err(error::DomainError::new("No exportable Zotero items found.").into());
    }

    let mut exported = Vec::with_capacity(refs.len());
    for item_ref in &refs {
        exported.push(rendering::export_item(
            runtime,
            Some(item_ref.as_str()),
            fmt,
            session,
        )?);
    }

    let output_path = paths::expand_user_path(output);
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    // `"\n\n".join(entry["content"].strip() for entry in exported if entry.get("content"))`
    // then `content + ("\n" if content else "")` (`zotero_cli.py:1942-1943`).
    let content = exported
        .iter()
        .filter(|entry| !entry.content.is_empty())
        .map(|entry| entry.content.trim())
        .collect::<Vec<_>>()
        .join("\n\n");
    let final_content = if content.is_empty() {
        content
    } else {
        format!("{content}\n")
    };
    std::fs::write(&output_path, final_content)?;

    let payload = serde_json::json!({
        "action": "export-bib",
        "format": fmt,
        "output": output_path.to_string_lossy(),
        "item_count": exported.len(),
        "items": exported
            .iter()
            .map(|entry| serde_json::json!({"itemKey": entry.item_key, "libraryID": entry.library_id}))
            .collect::<Vec<_>>(),
        "source": source,
    });
    output::emit(json_mode, &payload);
    Ok(0)
}

fn item_tag_command(
    runtime: &runtime::RuntimeContext,
    session: &session::SessionState,
    json_mode: bool,
    item_key: &str,
    add: &[String],
    remove: &[String],
) -> anyhow::Result<i32> {
    if add.is_empty() && remove.is_empty() {
        return Err(
            error::DomainError::new("At least one --add or --remove tag is required").into(),
        );
    }
    let client = runtime.bridge_client();
    let prefer = target::Prefer::for_runtime(runtime);
    let item = target::resolve_item(runtime, &client, Some(item_key), session, prefer)?;

    if runtime.local_api_writes_available {
        let scope = item.local_api_scope()?;
        let path = format!("{scope}/items/{}", item.key);
        let current = match write_router::verify_present(runtime, &path) {
            write_router::PresenceCheck::Present(summary) => summary,
            other => return presence_check_error(&path, other),
        };
        let empty = Value::Array(Vec::new());
        let current_tags = current.data.get("tags").unwrap_or(&empty);
        let new_tags = compute_tag_array(current_tags, add, remove);
        let mut body = serde_json::Map::new();
        body.insert("tags".to_string(), Value::Array(new_tags));
        let outcome = write_router::patch_item(
            runtime,
            &path,
            &item.key,
            &Value::Object(body.clone()),
            current.version,
        )?;
        if let Some((code, payload)) = write_outcome_failure(&outcome) {
            output::emit(json_mode, &payload);
            return Ok(code);
        }
        return render_local_api_object_after_write(runtime, json_mode, &path, &body);
    }

    let library_id = library_id_u32(item.library_id)?;
    let pre_raw = bridge_live_read(&client, LIVE_ITEM_READBACK_JS, library_id, &item.key)?;
    let pre_view = parse_live_object(&pre_raw, "item")?;
    let empty = Value::Array(Vec::new());
    let current_tags = pre_view.data.get("tags").unwrap_or(&empty);
    let new_tags = compute_tag_array(current_tags, add, remove);
    let mut body = serde_json::Map::new();
    body.insert("tags".to_string(), Value::Array(new_tags));

    let outcome = client.item_tag(library_id, &item.key, add, remove)?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }
    render_bridge_item_after_write(&client, json_mode, library_id, &item.key, &body)
}

fn item_delete_command(
    runtime: &runtime::RuntimeContext,
    session: &session::SessionState,
    json_mode: bool,
    item_key: &str,
    confirm: bool,
) -> anyhow::Result<i32> {
    if !confirm {
        return Err(error::DomainError::new("Refusing to delete without --confirm").into());
    }
    let client = runtime.bridge_client();
    let prefer = target::Prefer::for_runtime(runtime);
    let item = target::resolve_item(runtime, &client, Some(item_key), session, prefer)?;

    if runtime.local_api_writes_available {
        let scope = item.local_api_scope()?;
        let path = format!("{scope}/items/{}", item.key);
        let current = match write_router::verify_present(runtime, &path) {
            write_router::PresenceCheck::Present(summary) => summary,
            other => return presence_check_error(&path, other),
        };
        let outcome = write_router::delete_item(runtime, &path, &item.key, current.version)?;
        if let Some((code, payload)) = write_outcome_failure(&outcome) {
            output::emit(json_mode, &payload);
            return Ok(code);
        }
        output::emit(
            json_mode,
            &serde_json::json!({ "outcome": "applied", "deleted_key": item.key }),
        );
        return Ok(0);
    }

    let library_id = library_id_u32(item.library_id)?;
    let outcome = client.item_delete(library_id, &item.key)?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }
    // Live Zotero-runtime absence check (never SQLite -- same staleness hazard as the Local API
    // path's `verify_absent`, generalized to Bridge-routed deletes per review Blocker 2).
    let raw = bridge_live_read(&client, LIVE_ITEM_READBACK_JS, library_id, &item.key)?;
    if !is_live_object_absent(&raw) {
        output::emit(
            json_mode,
            &serde_json::json!({
                "outcome": "conflict",
                "detail": format!(
                    "Bridge reported the item deleted but the live Zotero runtime still resolves {}",
                    item.key
                ),
                "needs_human_action": false,
            }),
        );
        return Ok(1);
    }
    output::emit(
        json_mode,
        &serde_json::json!({ "outcome": "applied", "deleted_key": item.key }),
    );
    Ok(0)
}

fn item_attach_command(
    runtime: &runtime::RuntimeContext,
    session: &session::SessionState,
    json_mode: bool,
    item_key: &str,
    pdf_path: &str,
) -> anyhow::Result<i32> {
    // Always JS Bridge: the Local API's file-upload protocol (§3.6 row 50, "VERIFY IN SLICE 3")
    // was never implemented -- no `write_router` primitive exists for it -- so this command has
    // exactly one committed backend regardless of `local_api_writes_available`.
    let client = runtime.bridge_client();
    let item = target::resolve_item(
        runtime,
        &client,
        Some(item_key),
        session,
        target::Prefer::Bridge,
    )?;
    let library_id = library_id_u32(item.library_id)?;
    let outcome = client.item_attach(library_id, &item.key, std::path::Path::new(pdf_path))?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }
    render_bridge_item_after_write(
        &client,
        json_mode,
        library_id,
        &item.key,
        &serde_json::Map::new(),
    )
}

fn item_add_to_collection_command(
    runtime: &runtime::RuntimeContext,
    session: &session::SessionState,
    json_mode: bool,
    item_ref: &str,
    collection_ref: &str,
) -> anyhow::Result<i32> {
    let client = runtime.bridge_client();
    let prefer = target::Prefer::for_runtime(runtime);
    let item = target::resolve_item(runtime, &client, Some(item_ref), session, prefer)?;
    let collection =
        target::resolve_collection(runtime, &client, Some(collection_ref), session, prefer)?;
    if collection.library_id != item.library_id {
        return Err(
            error::DomainError::new("Item and collection must belong to the same library").into(),
        );
    }

    if runtime.local_api_writes_available {
        let scope = item.local_api_scope()?;
        let path = format!("{scope}/items/{}", item.key);
        let current = match write_router::verify_present(runtime, &path) {
            write_router::PresenceCheck::Present(summary) => summary,
            other => return presence_check_error(&path, other),
        };
        let current_collections = extract_string_array(&current.data, "collections");
        let new_collections = write_router::union_replace(
            &current_collections,
            std::slice::from_ref(&collection.key),
        );
        let mut body = serde_json::Map::new();
        body.insert(
            "collections".to_string(),
            serde_json::to_value(&new_collections)?,
        );
        let outcome = write_router::patch_item(
            runtime,
            &path,
            &item.key,
            &Value::Object(body.clone()),
            current.version,
        )?;
        if let Some((code, payload)) = write_outcome_failure(&outcome) {
            output::emit(json_mode, &payload);
            return Ok(code);
        }
        return render_local_api_object_after_write(runtime, json_mode, &path, &body);
    }

    let library_id = library_id_u32(item.library_id)?;
    let pre_raw = bridge_live_read(&client, LIVE_ITEM_READBACK_JS, library_id, &item.key)?;
    let pre_view = parse_live_object(&pre_raw, "item")?;
    let current_collections = extract_string_array(&pre_view.data, "collections");
    let new_collections =
        write_router::union_replace(&current_collections, std::slice::from_ref(&collection.key));
    let mut body = serde_json::Map::new();
    body.insert(
        "collections".to_string(),
        serde_json::to_value(&new_collections)?,
    );

    let outcome = client.item_add_to_collection(library_id, &item.key, &collection.key)?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }
    render_bridge_item_after_write(&client, json_mode, library_id, &item.key, &body)
}

fn item_move_to_collection_command(
    runtime: &runtime::RuntimeContext,
    session: &session::SessionState,
    json_mode: bool,
    item_ref: &str,
    collection_ref: &str,
    from: &[String],
    all_other_collections: bool,
) -> anyhow::Result<i32> {
    let client = runtime.bridge_client();
    let prefer = target::Prefer::for_runtime(runtime);
    let item = target::resolve_item(runtime, &client, Some(item_ref), session, prefer)?;
    let collection =
        target::resolve_collection(runtime, &client, Some(collection_ref), session, prefer)?;
    if collection.library_id != item.library_id {
        return Err(
            error::DomainError::new("Item and collection must belong to the same library").into(),
        );
    }

    if runtime.local_api_writes_available {
        let scope = item.local_api_scope()?;
        let path = format!("{scope}/items/{}", item.key);
        let current = match write_router::verify_present(runtime, &path) {
            write_router::PresenceCheck::Present(summary) => summary,
            other => return presence_check_error(&path, other),
        };
        let current_collections = extract_string_array(&current.data, "collections");
        let from_keys = resolve_from_keys(runtime, &client, session, from, prefer)?;
        let new_collections = compute_move_to_collection_set(
            &current_collections,
            &collection.key,
            &from_keys,
            all_other_collections,
        );
        let mut body = serde_json::Map::new();
        body.insert(
            "collections".to_string(),
            serde_json::to_value(&new_collections)?,
        );
        let outcome = write_router::patch_item(
            runtime,
            &path,
            &item.key,
            &Value::Object(body.clone()),
            current.version,
        )?;
        if let Some((code, payload)) = write_outcome_failure(&outcome) {
            output::emit(json_mode, &payload);
            return Ok(code);
        }
        return render_local_api_object_after_write(runtime, json_mode, &path, &body);
    }

    // The Bridge's `item_move_to_collection` primitive is a single add+remove transaction with
    // at most one source collection (§3.6 row 68's original upstream shape) -- it cannot express
    // `--all-other-collections` or more than one `--from`. Rather than silently dropping extra
    // sources, this is a documented, explicit failure.
    if all_other_collections || from.len() > 1 {
        return Err(error::DomainError::new(
            "Local API writes are unavailable on this Zotero instance, and the JS Bridge \
             fallback for `item move-to-collection` supports at most one --from source and \
             does not support --all-other-collections",
        )
        .into());
    }
    let from_key = match from.first() {
        Some(source_ref) => Some(
            target::resolve_collection(runtime, &client, Some(source_ref), session, prefer)?.key,
        ),
        None => None,
    };
    let library_id = library_id_u32(item.library_id)?;
    let pre_raw = bridge_live_read(&client, LIVE_ITEM_READBACK_JS, library_id, &item.key)?;
    let pre_view = parse_live_object(&pre_raw, "item")?;
    let current_collections = extract_string_array(&pre_view.data, "collections");
    let from_keys: Vec<String> = from_key.iter().cloned().collect();
    let new_collections =
        compute_move_to_collection_set(&current_collections, &collection.key, &from_keys, false);
    let mut body = serde_json::Map::new();
    body.insert(
        "collections".to_string(),
        serde_json::to_value(&new_collections)?,
    );

    let outcome = client.item_move_to_collection(
        library_id,
        &item.key,
        &collection.key,
        from_key.as_deref(),
    )?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }
    render_bridge_item_after_write(&client, json_mode, library_id, &item.key, &body)
}

/// Resolves each `--from` reference to its collection key -- callers may pass a name, not
/// necessarily a key.
fn resolve_from_keys(
    runtime: &runtime::RuntimeContext,
    client: &bridge::JSBridgeClient,
    session: &session::SessionState,
    from: &[String],
    prefer: target::Prefer,
) -> anyhow::Result<Vec<String>> {
    from.iter()
        .map(|source_ref| {
            Ok(target::resolve_collection(runtime, client, Some(source_ref), session, prefer)?.key)
        })
        .collect()
}

/// Computes `item move-to-collection`'s full replacement `collections` array: add `target`, then
/// either strip every other membership (`--all-other-collections`) or strip only the named
/// `from_keys` -- matching the Python contract's own three-way behavior (no flag: additive move
/// alongside existing memberships; `--from`: remove only those sources; `--all-other-collections`:
/// the item ends up in `target` alone).
fn compute_move_to_collection_set(
    current: &[String],
    target: &str,
    from_keys: &[String],
    all_other_collections: bool,
) -> Vec<String> {
    if all_other_collections {
        return vec![target.to_string()];
    }
    let target_key = target.to_string();
    if from_keys.is_empty() {
        return write_router::union_replace(current, std::slice::from_ref(&target_key));
    }
    let mut replaced = write_router::union_replace(current, std::slice::from_ref(&target_key));
    replaced = write_router::difference_replace(&replaced, from_keys);
    if !replaced.contains(&target_key) {
        replaced.push(target_key);
    }
    replaced
}

fn item_merge_command(
    runtime: &runtime::RuntimeContext,
    session: &session::SessionState,
    json_mode: bool,
    keep_key: &str,
    merge_keys: &[String],
    dry_run: bool,
) -> anyhow::Result<i32> {
    // `merge_items()`'s top-of-function self-filter (`hygiene.py:410`): silently drop any merge
    // key equal to the keep key (plain string equality on the raw CLI args, not resolved-item
    // equality), by literal string match -- before either the preview or the confirm path runs.
    let merge_keys: Vec<String> = merge_keys
        .iter()
        .filter(|k| !k.is_empty() && k.as_str() != keep_key)
        .cloned()
        .collect();
    if keep_key.is_empty() || merge_keys.is_empty() {
        let payload = serde_json::json!({
            "action": "item_merge",
            "ok": false,
            "status": "error",
            "code": "INVALID_ARGS",
            "error": "keep key and at least one other key are required",
        });
        output::emit(json_mode, &payload);
        return Ok(1);
    }

    if dry_run {
        let payload = hygiene::merge_preview(runtime, session, keep_key, &merge_keys)?;
        let ok = payload
            .get("ok")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let status = payload
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        output::emit(json_mode, &payload);
        // `exit_code_for()` (`results.py:40-47`): ok=false -> 1; else status in the failure set -> 1.
        let exit_code =
            if !ok || matches!(status, "partial_success" | "error" | "failed" | "timeout") {
                1
            } else {
                0
            };
        return Ok(exit_code);
    }

    let client = runtime.bridge_client();
    // `item merge --confirm` is Bridge-committed: `Zotero.Items.merge()` has no Local API
    // equivalent, so the targets are resolved through the very Bridge that performs the merge.
    let prefer = target::Prefer::Bridge;
    let keep_item = target::resolve_item(runtime, &client, Some(keep_key), session, prefer)?;
    let mut resolved_merge_keys = Vec::with_capacity(merge_keys.len());
    for key in &merge_keys {
        let merged_item = target::resolve_item(runtime, &client, Some(key), session, prefer)?;
        if merged_item.library_id != keep_item.library_id {
            return Err(error::DomainError::new(
                "All merged items must belong to the same library as the target item",
            )
            .into());
        }
        resolved_merge_keys.push(merged_item.key);
    }

    let library_id = library_id_u32(keep_item.library_id)?;
    let outcome = client.item_merge(library_id, &keep_item.key, &resolved_merge_keys)?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }

    // §3.5's merge sub-rule, verified live through the JS Bridge (never SQLite, which cannot be
    // trusted to see a write made moments earlier by this same process -- review Blocker 2): the
    // survivor must resolve, and every merged-away key must no longer resolve.
    let survivor_raw =
        bridge_live_read(&client, LIVE_ITEM_READBACK_JS, library_id, &keep_item.key)?;
    if is_live_object_absent(&survivor_raw) {
        return Err(error::DomainError::new(format!(
            "merge reported success but the survivor {} no longer resolves through the live \
             Zotero runtime",
            keep_item.key
        ))
        .into());
    }
    let mut still_present = Vec::new();
    for key in &resolved_merge_keys {
        let raw = bridge_live_read(&client, LIVE_ITEM_READBACK_JS, library_id, key)?;
        if !is_live_object_absent(&raw) {
            still_present.push(key.clone());
        }
    }
    if !still_present.is_empty() {
        output::emit(
            json_mode,
            &serde_json::json!({
                "outcome": "conflict",
                "detail": format!(
                    "merge reported success but {} merged-away key(s) still resolve through the \
                     live Zotero runtime: {:?}",
                    still_present.len(),
                    still_present
                ),
                "needs_human_action": false,
            }),
        );
        return Ok(1);
    }

    let view = parse_live_object(&survivor_raw, "item")?;
    output::emit(json_mode, &render_write_result(&view, &[]));
    Ok(0)
}

/// `collection create`'s duplicate-name lookup, read live through the Local API instead of
/// SQLite. Matches on the same two facts the SQLite version did -- exact name and same parent --
/// expressed in the Local API's own terms (`data.name` / `data.parentCollection`, where a
/// top-level collection reports `false` rather than a key).
fn find_existing_collection(
    runtime: &runtime::RuntimeContext,
    scope: &str,
    name: &str,
    parent_key: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let payload = http::local_api_get_json(
        runtime.environment.port,
        &format!("{scope}/collections"),
        &[("format", "json".to_string())],
        std::time::Duration::from_secs(10),
    )?;
    let Some(entries) = payload.as_array() else {
        return Ok(None);
    };
    for entry in entries {
        let data = entry.get("data");
        let entry_name = data.and_then(|d| d.get("name")).and_then(Value::as_str);
        if entry_name != Some(name) {
            continue;
        }
        let entry_parent = data
            .and_then(|d| d.get("parentCollection"))
            .and_then(Value::as_str);
        if entry_parent == parent_key {
            if let Some(key) = entry.get("key").and_then(Value::as_str) {
                return Ok(Some(key.to_string()));
            }
        }
    }
    Ok(None)
}

fn collection_create_command(
    runtime: &runtime::RuntimeContext,
    session: &session::SessionState,
    json_mode: bool,
    name: &str,
    parent: Option<&str>,
) -> anyhow::Result<i32> {
    let client = runtime.bridge_client();
    let prefer = target::Prefer::for_runtime(runtime);
    let library = target::resolve_default_library(runtime, &client, session, prefer)?;
    let library_id = library.library_id;
    let parent_collection = match parent {
        Some(p) => Some(target::resolve_collection(
            runtime,
            &client,
            Some(p),
            session,
            prefer,
        )?),
        None => None,
    };
    if let Some(parent) = &parent_collection {
        if parent.library_id != library_id {
            return Err(error::DomainError::new(
                "Parent collection must belong to the current library",
            )
            .into());
        }
    }

    if runtime.local_api_writes_available {
        let scope = library.local_api_scope()?;
        // §3.3's duplicate-write protection for this non-idempotent POST: if a collection with
        // the same name already exists under the same parent, treat the create as already done
        // rather than risk a second POST on a caller retry after an ambiguous outcome. Read
        // through the Local API rather than SQLite -- a running Zotero holds the WAL lock, and
        // a stale snapshot is exactly the wrong input to a duplicate check.
        if let Some(found) = find_existing_collection(
            runtime,
            &scope,
            name,
            parent_collection.as_ref().map(|p| p.key.as_str()),
        )? {
            output::emit(
                json_mode,
                &serde_json::json!({
                    "outcome": "applied",
                    "key": found,
                    "already_existed": true,
                }),
            );
            return Ok(0);
        }

        let path = format!("{scope}/collections");
        let mut body = serde_json::Map::new();
        body.insert("name".to_string(), Value::String(name.to_string()));
        if let Some(parent) = &parent_collection {
            body.insert(
                "parentCollection".to_string(),
                Value::String(parent.key.clone()),
            );
        }
        let (outcome, _raw) =
            write_router::post_create(runtime, &path, &Value::Object(body.clone()))?;
        if let Some((code, payload)) = write_outcome_failure(&outcome) {
            output::emit(json_mode, &payload);
            return Ok(code);
        }
        let WriteOutcome::Applied { affected_key } = outcome else {
            unreachable!("write_outcome_failure returns None only for Applied");
        };
        let item_path = format!("{scope}/collections/{affected_key}");
        return render_local_api_object_after_write(runtime, json_mode, &item_path, &body);
    }

    let bridge_library_id = library_id_u32(library_id)?;
    let outcome = client.collection_create(
        bridge_library_id,
        name,
        parent_collection.as_ref().map(|p| p.key.as_str()),
    )?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }
    let WriteOutcome::Applied { affected_key } = outcome else {
        unreachable!("write_outcome_failure returns None only for Applied");
    };
    render_bridge_collection_after_write(
        &client,
        json_mode,
        bridge_library_id,
        &affected_key,
        &serde_json::Map::new(),
    )
}

fn collection_rename_command(
    runtime: &runtime::RuntimeContext,
    session: &session::SessionState,
    json_mode: bool,
    collection_key: &str,
    name: Option<&str>,
    parent: Option<&str>,
) -> anyhow::Result<i32> {
    if name.is_none() && parent.is_none() {
        return Err(
            error::DomainError::new("No changes specified (use --name or --parent)").into(),
        );
    }
    let client = runtime.bridge_client();
    let prefer = target::Prefer::for_runtime(runtime);
    let collection =
        target::resolve_collection(runtime, &client, Some(collection_key), session, prefer)?;
    let parent_collection = match parent {
        Some(p) => Some(target::resolve_collection(
            runtime,
            &client,
            Some(p),
            session,
            prefer,
        )?),
        None => None,
    };
    let mut body = serde_json::Map::new();
    if let Some(name) = name {
        body.insert("name".to_string(), Value::String(name.to_string()));
    }
    if let Some(parent) = &parent_collection {
        body.insert(
            "parentCollection".to_string(),
            Value::String(parent.key.clone()),
        );
    }

    if runtime.local_api_writes_available {
        let scope = collection.local_api_scope()?;
        let path = format!("{scope}/collections/{}", collection.key);
        let current = match write_router::verify_present(runtime, &path) {
            write_router::PresenceCheck::Present(summary) => summary,
            other => return presence_check_error(&path, other),
        };
        let outcome = write_router::patch_item(
            runtime,
            &path,
            &collection.key,
            &Value::Object(body.clone()),
            current.version,
        )?;
        if let Some((code, payload)) = write_outcome_failure(&outcome) {
            output::emit(json_mode, &payload);
            return Ok(code);
        }
        return render_local_api_object_after_write(runtime, json_mode, &path, &body);
    }

    let library_id = library_id_u32(collection.library_id)?;
    let outcome = client.collection_rename(
        library_id,
        &collection.key,
        name,
        parent_collection.as_ref().map(|p| p.key.as_str()),
    )?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }
    render_bridge_collection_after_write(&client, json_mode, library_id, &collection.key, &body)
}

fn collection_delete_command(
    runtime: &runtime::RuntimeContext,
    session: &session::SessionState,
    json_mode: bool,
    collection_key: &str,
    delete_items: bool,
    confirm: bool,
) -> anyhow::Result<i32> {
    if !confirm {
        return Err(error::DomainError::new("Refusing to delete without --confirm").into());
    }
    let client = runtime.bridge_client();
    let prefer = target::Prefer::for_runtime(runtime);
    let collection =
        target::resolve_collection(runtime, &client, Some(collection_key), session, prefer)?;

    // No Local API primitive exists for cascading item deletion -- always use the JS Bridge
    // when --delete-items is requested, regardless of local_api_writes_available.
    if runtime.local_api_writes_available && !delete_items {
        let scope = collection.local_api_scope()?;
        let path = format!("{scope}/collections/{}", collection.key);
        let current = match write_router::verify_present(runtime, &path) {
            write_router::PresenceCheck::Present(summary) => summary,
            other => return presence_check_error(&path, other),
        };
        let outcome = write_router::delete_item(runtime, &path, &collection.key, current.version)?;
        if let Some((code, payload)) = write_outcome_failure(&outcome) {
            output::emit(json_mode, &payload);
            return Ok(code);
        }
        output::emit(
            json_mode,
            &serde_json::json!({ "outcome": "applied", "deleted_key": collection.key }),
        );
        return Ok(0);
    }

    let library_id = library_id_u32(collection.library_id)?;
    let outcome = client.collection_delete(library_id, &collection.key, delete_items)?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }
    // Live Zotero-runtime absence check (never SQLite -- review Blocker 2).
    let raw = bridge_live_read(
        &client,
        LIVE_COLLECTION_READBACK_JS,
        library_id,
        &collection.key,
    )?;
    if !is_live_object_absent(&raw) {
        output::emit(
            json_mode,
            &serde_json::json!({
                "outcome": "conflict",
                "detail": format!(
                    "Bridge reported the collection deleted but the live Zotero runtime still \
                     resolves {}",
                    collection.key
                ),
                "needs_human_action": false,
            }),
        );
        return Ok(1);
    }
    output::emit(
        json_mode,
        &serde_json::json!({ "outcome": "applied", "deleted_key": collection.key }),
    );
    Ok(0)
}

fn collection_remove_item_command(
    runtime: &runtime::RuntimeContext,
    session: &session::SessionState,
    json_mode: bool,
    collection_key: &str,
    item_key: &str,
) -> anyhow::Result<i32> {
    let client = runtime.bridge_client();
    let prefer = target::Prefer::for_runtime(runtime);
    let collection =
        target::resolve_collection(runtime, &client, Some(collection_key), session, prefer)?;
    let item = target::resolve_item(runtime, &client, Some(item_key), session, prefer)?;
    if collection.library_id != item.library_id {
        return Err(
            error::DomainError::new("Item and collection must belong to the same library").into(),
        );
    }

    if runtime.local_api_writes_available {
        let scope = item.local_api_scope()?;
        let path = format!("{scope}/items/{}", item.key);
        let current = match write_router::verify_present(runtime, &path) {
            write_router::PresenceCheck::Present(summary) => summary,
            other => return presence_check_error(&path, other),
        };
        let current_collections = extract_string_array(&current.data, "collections");
        let new_collections = write_router::difference_replace(
            &current_collections,
            std::slice::from_ref(&collection.key),
        );
        let mut body = serde_json::Map::new();
        body.insert(
            "collections".to_string(),
            serde_json::to_value(&new_collections)?,
        );
        let outcome = write_router::patch_item(
            runtime,
            &path,
            &item.key,
            &Value::Object(body.clone()),
            current.version,
        )?;
        if let Some((code, payload)) = write_outcome_failure(&outcome) {
            output::emit(json_mode, &payload);
            return Ok(code);
        }
        return render_local_api_object_after_write(runtime, json_mode, &path, &body);
    }

    let library_id = library_id_u32(item.library_id)?;
    let pre_raw = bridge_live_read(&client, LIVE_ITEM_READBACK_JS, library_id, &item.key)?;
    let pre_view = parse_live_object(&pre_raw, "item")?;
    let current_collections = extract_string_array(&pre_view.data, "collections");
    let new_collections = write_router::difference_replace(
        &current_collections,
        std::slice::from_ref(&collection.key),
    );
    let mut body = serde_json::Map::new();
    body.insert(
        "collections".to_string(),
        serde_json::to_value(&new_collections)?,
    );

    let outcome = client.collection_remove_item(library_id, &item.key, &collection.key)?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }
    render_bridge_item_after_write(&client, json_mode, library_id, &item.key, &body)
}

fn js_command(
    client: &bridge::JSBridgeClient,
    json_mode: bool,
    code: &str,
    wait: u64,
) -> anyhow::Result<i32> {
    let result = client.execute_raw_js(code, wait)?;
    output::emit(json_mode, &result);
    Ok(0)
}

fn sync_command(client: &bridge::JSBridgeClient, json_mode: bool) -> anyhow::Result<i32> {
    let message = client.trigger_sync()?;
    output::emit(json_mode, &Value::String(message));
    Ok(0)
}

/// Default staging directory for the XPI plugin artifact when `--output-dir` is omitted: beside
/// `session.json`, using the same `session_state_dir()`/`CLI_ANYTHING_ZOTERO_STATE_DIR`
/// convention -- CLI-owned state, never inside the Zotero profile directory itself.
fn plugin_output_dir(output_dir: Option<&str>) -> std::path::PathBuf {
    match output_dir {
        Some(dir) => std::path::PathBuf::from(dir),
        None => session::session_state_dir().join("plugin"),
    }
}

/// Stages the bundled CLI Bridge XPI and reports exactly what the human has to do next.
///
/// The binary already carries the plugin (`plugin::build_xpi` assembles it from compiled-in
/// assets), so nothing is downloaded and no version has to be matched by hand -- but the
/// previous output said only "install manually via Zotero", with no version and no ordered
/// steps, which left a first-run user to guess.
///
/// Staging deliberately remains the whole of the CLI's role: it writes an `.xpi` to a directory
/// it owns and never touches the Zotero profile. Installation goes through Zotero's own
/// Add-ons dialog so the user's normal plugin-consent flow is not bypassed, and `app doctor`
/// verifies the result afterwards.
fn app_install_plugin_command(
    runtime: &runtime::RuntimeContext,
    json_mode: bool,
    output_dir: Option<&str>,
) -> anyhow::Result<i32> {
    let dir = plugin_output_dir(output_dir);
    let xpi_path = plugin::stage_xpi(&dir)?;
    let display_path = xpi_path.to_string_lossy().into_owned();
    let profile_dir = runtime.environment.profile_dir.as_deref();
    let installed_version = paths::installed_plugin_version(profile_dir);
    let bundled_version = paths::bundled_plugin_version();
    let already_installed = paths::plugin_installed(profile_dir);

    // Ordered, literal steps -- including the exact path to select -- rather than one prose
    // sentence the caller has to parse. Structured so an agent can surface them verbatim.
    let install_steps = vec![
        "Open Zotero.".to_string(),
        "Go to Tools → Plugins.".to_string(),
        "Click the gear icon → Install Add-on From File…".to_string(),
        format!("Select: {display_path}"),
        "Restart Zotero.".to_string(),
        "Verify with: zotero-cli app doctor".to_string(),
    ];

    let message = if already_installed && installed_version == bundled_version {
        format!(
            "CLI Bridge {} is already installed. The bundled copy has been staged at {} in case \
             you need to reinstall it.",
            bundled_version.as_deref().unwrap_or("(unknown)"),
            display_path
        )
    } else if already_installed {
        format!(
            "CLI Bridge {} is installed; {} is bundled with this CLI. Install the staged copy to \
             upgrade, then restart Zotero.",
            installed_version.as_deref().unwrap_or("(unknown)"),
            bundled_version.as_deref().unwrap_or("(unknown)")
        )
    } else {
        "Bundled CLI Bridge staged. Install it through Zotero's Add-ons dialog (steps below), \
         then restart Zotero."
            .to_string()
    };

    output::emit(
        json_mode,
        &serde_json::json!({
            "action": "app_install_plugin",
            "ok": true,
            "status": "success",
            "staged_xpi_path": display_path,
            "bundled_version": bundled_version,
            "installed_version": installed_version,
            "already_installed": already_installed,
            "install_steps": install_steps,
            "message": message,
        }),
    );
    Ok(0)
}

fn app_plugin_status_command(
    runtime: &runtime::RuntimeContext,
    json_mode: bool,
    output_dir: Option<&str>,
) -> anyhow::Result<i32> {
    let dir = plugin_output_dir(output_dir);
    let report = plugin::plugin_status(Some(&dir), runtime.environment.port);
    output::emit(json_mode, &serde_json::to_value(report)?);
    Ok(0)
}

fn app_uninstall_plugin_command(json_mode: bool, output_dir: Option<&str>) -> anyhow::Result<i32> {
    let dir = plugin_output_dir(output_dir);
    let removed = plugin::remove_staged_xpi(&dir)?;
    output::emit(
        json_mode,
        &serde_json::json!({
            "removed_staged_xpi": removed,
            "message": "This only removes the staged .xpi artifact; it does not uninstall an \
                        extension already installed in Zotero's profile.",
        }),
    );
    Ok(0)
}

/// §3.4a's "explicit, deliberate authorize step": the one command allowed to trigger
/// `POST /api/local/authorize` (which blocks on Zotero's human consent dialog). Never called
/// automatically from any other write command's happy path.
fn app_authorize_local_api_command(
    runtime: &runtime::RuntimeContext,
    json_mode: bool,
    app_name: &str,
) -> anyhow::Result<i32> {
    let outcome = write_router::authorize_interactive(runtime, app_name)?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }
    output::emit(
        json_mode,
        &serde_json::json!({
            "outcome": "applied",
            "message": "Local API write credential issued and stored.",
        }),
    );
    Ok(0)
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

#[cfg(test)]
mod write_command_helper_tests {
    use super::*;

    #[test]
    fn diff_requested_fields_reports_only_mismatching_fields() {
        let mut requested = serde_json::Map::new();
        requested.insert("title".to_string(), Value::String("New Title".to_string()));
        requested.insert("date".to_string(), Value::String("2026".to_string()));

        let mut observed = serde_json::Map::new();
        observed.insert("title".to_string(), Value::String("New Title".to_string()));
        observed.insert("date".to_string(), Value::String("2020".to_string()));

        let mismatches = diff_requested_fields(&requested, &observed);
        assert_eq!(mismatches.len(), 1);
        assert_eq!(mismatches[0].field, "date");
        assert_eq!(mismatches[0].requested, Value::String("2026".to_string()));
        assert_eq!(
            mismatches[0].observed,
            Some(Value::String("2020".to_string()))
        );
    }

    #[test]
    fn diff_requested_fields_reports_a_missing_field_as_none_observed() {
        let mut requested = serde_json::Map::new();
        requested.insert("title".to_string(), Value::String("X".to_string()));
        let observed = serde_json::Map::new();

        let mismatches = diff_requested_fields(&requested, &observed);
        assert_eq!(mismatches.len(), 1);
        assert_eq!(mismatches[0].observed, None);
    }

    #[test]
    fn compute_tag_array_add_preserves_unrelated_existing_tags() {
        let current = serde_json::json!([{"tag": "existing"}, {"tag": "automatic", "type": 1}]);
        let result = compute_tag_array(&current, &["new-tag".to_string()], &[]);
        assert_eq!(
            result,
            vec![
                serde_json::json!({"tag": "existing"}),
                serde_json::json!({"tag": "automatic", "type": 1}),
                serde_json::json!({"tag": "new-tag"}),
            ]
        );
    }

    #[test]
    fn compute_tag_array_remove_only_removes_the_named_tags() {
        let current = serde_json::json!([
            {"tag": "keep-me"},
            {"tag": "remove-me"},
            {"tag": "also-keep", "type": 1},
        ]);
        let result = compute_tag_array(&current, &[], &["remove-me".to_string()]);
        assert_eq!(
            result,
            vec![
                serde_json::json!({"tag": "keep-me"}),
                serde_json::json!({"tag": "also-keep", "type": 1}),
            ]
        );
    }

    #[test]
    fn compute_tag_array_add_does_not_duplicate_an_existing_tag() {
        let current = serde_json::json!([{"tag": "dup"}]);
        let result = compute_tag_array(&current, &["dup".to_string()], &[]);
        assert_eq!(result, vec![serde_json::json!({"tag": "dup"})]);
    }

    #[test]
    fn extract_string_array_reads_a_plain_string_array_field() {
        let mut data = serde_json::Map::new();
        data.insert("collections".to_string(), serde_json::json!(["AAA", "BBB"]));
        assert_eq!(
            extract_string_array(&data, "collections"),
            vec!["AAA".to_string(), "BBB".to_string()]
        );
    }

    #[test]
    fn extract_string_array_defaults_to_empty_when_field_missing() {
        let data = serde_json::Map::new();
        assert_eq!(
            extract_string_array(&data, "collections"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn compute_move_to_collection_set_all_other_collections_leaves_only_the_target() {
        let current = vec!["EXISTC1".to_string(), "EXISTC2".to_string()];
        let result = compute_move_to_collection_set(&current, "TARGET1", &[], true);
        assert_eq!(result, vec!["TARGET1".to_string()]);
    }

    #[test]
    fn compute_move_to_collection_set_from_removes_only_named_sources() {
        let current = vec!["EXISTC1".to_string(), "EXISTC2".to_string()];
        let result =
            compute_move_to_collection_set(&current, "TARGET1", &["EXISTC1".to_string()], false);
        assert_eq!(result, vec!["EXISTC2".to_string(), "TARGET1".to_string()]);
    }

    #[test]
    fn compute_move_to_collection_set_with_no_from_is_purely_additive() {
        let current = vec!["EXISTC1".to_string()];
        let result = compute_move_to_collection_set(&current, "TARGET1", &[], false);
        assert_eq!(result, vec!["EXISTC1".to_string(), "TARGET1".to_string()]);
    }

    #[test]
    fn is_live_object_absent_recognizes_the_not_found_envelope() {
        assert!(is_live_object_absent(&serde_json::json!({"found": false})));
        assert!(!is_live_object_absent(
            &serde_json::json!({"found": true, "key": "X"})
        ));
    }

    #[test]
    fn parse_live_object_extracts_the_canonical_view() {
        let raw = serde_json::json!({
            "found": true,
            "key": "ITEM0001",
            "libraryID": 1,
            "data": {"itemType": "document", "title": "X"},
        });
        let view = parse_live_object(&raw, "item").unwrap();
        assert_eq!(view.key, "ITEM0001");
        assert_eq!(view.library_id, 1);
        assert_eq!(view.item_type, "document");
    }

    #[test]
    fn parse_live_object_errors_when_absent() {
        let raw = serde_json::json!({"found": false});
        assert!(parse_live_object(&raw, "item").is_err());
    }

    #[test]
    fn render_write_result_never_includes_a_version_key() {
        let view = CanonicalWriteView {
            key: "ITEM0001".to_string(),
            library_id: 1,
            item_type: "document".to_string(),
            data: serde_json::Map::new(),
        };
        let rendered = render_write_result(&view, &[]);
        assert!(rendered.get("version").is_none());
        assert_eq!(rendered["outcome"], "applied");
    }
}

#[cfg(test)]
mod phase7_dispatch_helper_tests {
    use super::*;

    #[test]
    fn exit_code_for_maps_ok_false_to_one() {
        let payload = serde_json::json!({"ok": false, "status": "error"});
        assert_eq!(exit_code_for(&payload), 1);
    }

    #[test]
    fn exit_code_for_maps_partial_success_to_one_even_when_ok_true() {
        let payload = serde_json::json!({"ok": true, "status": "partial_success"});
        assert_eq!(exit_code_for(&payload), 1);
    }

    #[test]
    fn exit_code_for_maps_success_to_zero() {
        let payload = serde_json::json!({"ok": true, "status": "success"});
        assert_eq!(exit_code_for(&payload), 0);
    }

    #[test]
    fn exit_code_for_treats_a_missing_ok_key_as_not_false() {
        // Mirrors Python's `payload.get("ok") is False`: a missing key is `None`, not `False`.
        let payload = serde_json::json!({"status": "success"});
        assert_eq!(exit_code_for(&payload), 0);
    }

    #[test]
    fn exit_code_for_treats_a_missing_status_as_success() {
        let payload = serde_json::json!({"ok": true});
        assert_eq!(exit_code_for(&payload), 0);
    }

    #[test]
    fn unwrap_transport_envelope_passes_through_a_transport_failure() {
        let result = serde_json::json!({"ok": false, "data": null, "error": "boom"});
        let (payload, code) = unwrap_transport_envelope(result.clone(), true);
        assert_eq!(payload, result);
        assert_eq!(code, 1);
    }

    #[test]
    fn unwrap_transport_envelope_unwraps_data_on_success() {
        let result = serde_json::json!({"ok": true, "data": {"checked": 3}, "error": null});
        let (payload, code) = unwrap_transport_envelope(result, true);
        assert_eq!(payload, serde_json::json!({"checked": 3}));
        assert_eq!(code, 0);
    }

    #[test]
    fn unwrap_transport_envelope_treats_a_nested_ok_false_data_object_as_failure() {
        let result = serde_json::json!({"ok": true, "data": {"ok": false, "error": "nested"}});
        let (payload, code) = unwrap_transport_envelope(result, false);
        assert_eq!(payload, serde_json::json!({"ok": false, "error": "nested"}));
        assert_eq!(code, 1);
    }

    #[test]
    fn unwrap_transport_envelope_require_data_rejects_a_null_data_success() {
        let result = serde_json::json!({"ok": true, "data": null});
        let (payload, code) = unwrap_transport_envelope(result, true);
        assert_eq!(payload["code"], "EMPTY_RESULT");
        assert_eq!(code, 1);
    }

    #[test]
    fn resolve_bool_flag_prefers_the_explicit_positive_flag() {
        assert!(resolve_bool_flag(true, false, false));
    }

    #[test]
    fn resolve_bool_flag_prefers_the_explicit_negative_flag() {
        assert!(!resolve_bool_flag(false, true, true));
    }

    #[test]
    fn resolve_bool_flag_falls_back_to_the_default_when_neither_flag_is_set() {
        assert!(resolve_bool_flag(false, false, true));
        assert!(!resolve_bool_flag(false, false, false));
    }
}
