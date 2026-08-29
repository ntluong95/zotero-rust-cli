pub mod bridge;
pub mod catalog;
pub mod cli;
pub mod credentials;
pub mod db;
pub mod docx;
pub mod error;
pub mod http;
pub mod output;
pub mod paths;
pub mod plugin;
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
        Commands::Item(ItemCommands::Update { item_key, fields }) => {
            let runtime = build_runtime();
            item_update_command(&runtime, &session, json_mode, &item_key, &fields)
        }
        Commands::Item(ItemCommands::Tag {
            item_key,
            add,
            remove,
        }) => {
            let runtime = build_runtime();
            item_tag_command(&runtime, &session, json_mode, &item_key, &add, &remove)
        }
        Commands::Item(ItemCommands::Delete { item_key, confirm }) => {
            let runtime = build_runtime();
            item_delete_command(&runtime, &session, json_mode, &item_key, confirm)
        }
        Commands::Item(ItemCommands::Attach { item_key, pdf_path }) => {
            let runtime = build_runtime();
            item_attach_command(&runtime, &session, json_mode, &item_key, &pdf_path)
        }
        Commands::Item(ItemCommands::AddToCollection {
            item_ref,
            collection_ref,
        }) => {
            let runtime = build_runtime();
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
            let runtime = build_runtime();
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
        Commands::Item(ItemCommands::Duplicates { limit }) => {
            let runtime = build_runtime();
            item_duplicates_command(&runtime, &session, json_mode, limit)
        }
        Commands::Item(ItemCommands::Merge {
            keep_key,
            merge_keys,
            confirm,
        }) => {
            let runtime = build_runtime();
            item_merge_command(
                &runtime,
                &session,
                json_mode,
                &keep_key,
                &merge_keys,
                confirm,
            )
        }
        Commands::Collection(CollectionCommands::Create { name, parent }) => {
            let runtime = build_runtime();
            collection_create_command(&runtime, &session, json_mode, &name, parent.as_deref())
        }
        Commands::Collection(CollectionCommands::Rename {
            collection_key,
            name,
            parent,
        }) => {
            let runtime = build_runtime();
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
            let runtime = build_runtime();
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
            let runtime = build_runtime();
            collection_remove_item_command(
                &runtime,
                &session,
                json_mode,
                &collection_key,
                &item_key,
            )
        }
        Commands::App(AppCommands::InstallPlugin { output_dir }) => {
            app_install_plugin_command(json_mode, output_dir.as_deref())
        }
        Commands::App(AppCommands::PluginStatus { output_dir }) => {
            let runtime = build_runtime();
            app_plugin_status_command(&runtime, json_mode, output_dir.as_deref())
        }
        Commands::App(AppCommands::UninstallPlugin { output_dir }) => {
            app_uninstall_plugin_command(json_mode, output_dir.as_deref())
        }
        Commands::App(AppCommands::AuthorizeLocalApi { app_name }) => {
            let runtime = build_runtime();
            app_authorize_local_api_command(&runtime, json_mode, &app_name)
        }
        Commands::Js { code, wait } => js_command(json_mode, &code, wait),
        Commands::Sync => sync_command(json_mode),
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

/// §3.5's compatibility renderer for a Local-API-routed write: a deliberately narrow view built
/// from `write_router::LocalApiItemSummary`, never the raw Local API response. Excludes
/// `LocalApiItemSummary.version` on purpose -- the standing backend-identity denylist (§3.5/
/// Testing Strategy) forbids a raw Local API `version` key from ever reaching stdout JSON.
fn render_local_api_write_result(
    summary: &write_router::LocalApiItemSummary,
    mismatches: &[write_router::FieldMismatch],
) -> Value {
    let mut payload = serde_json::json!({
        "outcome": "applied",
        "key": summary.key,
        "library_id": summary.library_id,
        "item_type": summary.item_type,
        "data": Value::Object(summary.data.clone()),
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
    let item = catalog::get_item(runtime, Some(item_key), session)?;

    if runtime.local_api_writes_available {
        let scope = catalog::local_api_scope(runtime, item.library_id)?;
        let path = format!("{scope}/items/{}", item.key);
        let current = match write_router::verify_present(runtime, &path) {
            write_router::PresenceCheck::Present(summary) => summary,
            other => return presence_check_error(&path, other),
        };
        let mut body = serde_json::Map::new();
        for (key, value) in fields {
            body.insert(key.clone(), Value::String(value.clone()));
        }
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
        let (summary, mismatches) = write_router::verify_write(runtime, &path, &body)?;
        output::emit(
            json_mode,
            &render_local_api_write_result(&summary, &mismatches),
        );
        return Ok(0);
    }

    let library_id = library_id_u32(item.library_id)?;
    let mut fields_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for (key, value) in fields {
        fields_map.insert(key.clone(), value.clone());
    }
    let client = bridge::JSBridgeClient::with_default_port();
    let outcome = client.item_update(library_id, &item.key, &fields_map)?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }
    let refreshed = catalog::get_item(runtime, Some(&item.key), session)?;
    output::emit(json_mode, &serde_json::to_value(refreshed)?);
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
    let item = catalog::get_item(runtime, Some(item_key), session)?;

    if runtime.local_api_writes_available {
        let scope = catalog::local_api_scope(runtime, item.library_id)?;
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
        let (summary, mismatches) = write_router::verify_write(runtime, &path, &body)?;
        output::emit(
            json_mode,
            &render_local_api_write_result(&summary, &mismatches),
        );
        return Ok(0);
    }

    let library_id = library_id_u32(item.library_id)?;
    let client = bridge::JSBridgeClient::with_default_port();
    let outcome = client.item_tag(library_id, &item.key, add, remove)?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }
    let refreshed = catalog::get_item(runtime, Some(&item.key), session)?;
    output::emit(json_mode, &serde_json::to_value(refreshed)?);
    Ok(0)
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
    let item = catalog::get_item(runtime, Some(item_key), session)?;

    if runtime.local_api_writes_available {
        let scope = catalog::local_api_scope(runtime, item.library_id)?;
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
    let client = bridge::JSBridgeClient::with_default_port();
    let outcome = client.item_delete(library_id, &item.key)?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }
    // Bridge-routed delete has no Local-API-grade absence verification available: a SQLite
    // re-read immediately after a same-process write is exactly the staleness hazard
    // `write_router.rs` documents extensively, so it is not attempted here as proof of
    // deletion -- the Bridge's own "DELETED:"-prefixed confirmation is the only signal used.
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
    let item = catalog::get_item(runtime, Some(item_key), session)?;
    let library_id = library_id_u32(item.library_id)?;
    let client = bridge::JSBridgeClient::with_default_port();
    let outcome = client.item_attach(library_id, &item.key, std::path::Path::new(pdf_path))?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }
    let refreshed = catalog::get_item(runtime, Some(&item.key), session)?;
    output::emit(json_mode, &serde_json::to_value(refreshed)?);
    Ok(0)
}

fn item_add_to_collection_command(
    runtime: &runtime::RuntimeContext,
    session: &session::SessionState,
    json_mode: bool,
    item_ref: &str,
    collection_ref: &str,
) -> anyhow::Result<i32> {
    let item = catalog::get_item(runtime, Some(item_ref), session)?;
    let collection = catalog::get_collection(runtime, Some(collection_ref), session)?;
    if collection.library_id != item.library_id {
        return Err(
            error::DomainError::new("Item and collection must belong to the same library").into(),
        );
    }

    if runtime.local_api_writes_available {
        let scope = catalog::local_api_scope(runtime, item.library_id)?;
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
        let (summary, mismatches) = write_router::verify_write(runtime, &path, &body)?;
        output::emit(
            json_mode,
            &render_local_api_write_result(&summary, &mismatches),
        );
        return Ok(0);
    }

    let library_id = library_id_u32(item.library_id)?;
    let client = bridge::JSBridgeClient::with_default_port();
    let outcome = client.item_add_to_collection(library_id, &item.key, &collection.key)?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }
    let refreshed = catalog::get_item(runtime, Some(&item.key), session)?;
    output::emit(json_mode, &serde_json::to_value(refreshed)?);
    Ok(0)
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
    let item = catalog::get_item(runtime, Some(item_ref), session)?;
    let collection = catalog::get_collection(runtime, Some(collection_ref), session)?;
    if collection.library_id != item.library_id {
        return Err(
            error::DomainError::new("Item and collection must belong to the same library").into(),
        );
    }

    if runtime.local_api_writes_available {
        let scope = catalog::local_api_scope(runtime, item.library_id)?;
        let path = format!("{scope}/items/{}", item.key);
        let current = match write_router::verify_present(runtime, &path) {
            write_router::PresenceCheck::Present(summary) => summary,
            other => return presence_check_error(&path, other),
        };
        let current_collections = extract_string_array(&current.data, "collections");
        let new_collections = if all_other_collections {
            vec![collection.key.clone()]
        } else if !from.is_empty() {
            let mut from_keys = Vec::with_capacity(from.len());
            for source_ref in from {
                from_keys.push(catalog::get_collection(runtime, Some(source_ref), session)?.key);
            }
            let mut replaced = write_router::union_replace(
                &current_collections,
                std::slice::from_ref(&collection.key),
            );
            replaced = write_router::difference_replace(&replaced, &from_keys);
            if !replaced.contains(&collection.key) {
                replaced.push(collection.key.clone());
            }
            replaced
        } else {
            write_router::union_replace(&current_collections, std::slice::from_ref(&collection.key))
        };
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
        let (summary, mismatches) = write_router::verify_write(runtime, &path, &body)?;
        output::emit(
            json_mode,
            &render_local_api_write_result(&summary, &mismatches),
        );
        return Ok(0);
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
        Some(source_ref) => Some(catalog::get_collection(runtime, Some(source_ref), session)?.key),
        None => None,
    };
    let library_id = library_id_u32(item.library_id)?;
    let client = bridge::JSBridgeClient::with_default_port();
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
    let refreshed = catalog::get_item(runtime, Some(&item.key), session)?;
    output::emit(json_mode, &serde_json::to_value(refreshed)?);
    Ok(0)
}

fn item_duplicates_command(
    runtime: &runtime::RuntimeContext,
    session: &session::SessionState,
    json_mode: bool,
    limit: usize,
) -> anyhow::Result<i32> {
    let library_id = library_id_u32(catalog::default_library(runtime, session)?)?;
    let client = bridge::JSBridgeClient::with_default_port();
    let result = client.find_duplicates(library_id, limit)?;
    output::emit(json_mode, &result);
    Ok(0)
}

fn item_merge_command(
    runtime: &runtime::RuntimeContext,
    session: &session::SessionState,
    json_mode: bool,
    keep_key: &str,
    merge_keys: &[String],
    confirm: bool,
) -> anyhow::Result<i32> {
    if merge_keys.is_empty() {
        return Err(error::DomainError::new("At least one merge key is required").into());
    }
    if !confirm {
        return Err(error::DomainError::new("Refusing to merge without --confirm").into());
    }
    let keep_item = catalog::get_item(runtime, Some(keep_key), session)?;
    let mut resolved_merge_keys = Vec::with_capacity(merge_keys.len());
    for key in merge_keys {
        let merged_item = catalog::get_item(runtime, Some(key), session)?;
        if merged_item.library_id != keep_item.library_id {
            return Err(error::DomainError::new(
                "All merged items must belong to the same library as the target item",
            )
            .into());
        }
        resolved_merge_keys.push(merged_item.key);
    }

    let library_id = library_id_u32(keep_item.library_id)?;
    let client = bridge::JSBridgeClient::with_default_port();
    let outcome = client.item_merge(library_id, &keep_item.key, &resolved_merge_keys)?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }

    // §3.5's merge sub-rule: assert the survivor is present and every merged-away key is gone
    // or redirects, not just that the survivor's JSON renders correctly.
    let survivor = catalog::get_item(runtime, Some(&keep_item.key), session)?;
    let still_present: Vec<String> = resolved_merge_keys
        .iter()
        .filter(|key| catalog::get_item(runtime, Some(key), session).is_ok())
        .cloned()
        .collect();
    if !still_present.is_empty() {
        output::emit(
            json_mode,
            &serde_json::json!({
                "outcome": "conflict",
                "detail": format!(
                    "merge reported success but {} merged-away key(s) are still present: {:?}",
                    still_present.len(),
                    still_present
                ),
                "needs_human_action": false,
            }),
        );
        return Ok(1);
    }

    output::emit(json_mode, &serde_json::to_value(survivor)?);
    Ok(0)
}

fn collection_create_command(
    runtime: &runtime::RuntimeContext,
    session: &session::SessionState,
    json_mode: bool,
    name: &str,
    parent: Option<&str>,
) -> anyhow::Result<i32> {
    let library_id = catalog::default_library(runtime, session)?;
    let parent_collection = match parent {
        Some(p) => Some(catalog::get_collection(runtime, Some(p), session)?),
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
        // §3.3's duplicate-write protection for this non-idempotent POST: if a collection with
        // the same name already exists under the same parent, treat the create as already done
        // rather than risk a second POST on a caller retry after an ambiguous outcome.
        let existing = catalog::list_collections(runtime, session)?;
        let parent_id = parent_collection.as_ref().map(|p| p.collection_id);
        if let Some(found) = existing
            .iter()
            .find(|c| c.collection_name == name && c.parent_collection_id == parent_id)
        {
            output::emit(
                json_mode,
                &serde_json::json!({
                    "outcome": "applied",
                    "key": found.key,
                    "already_existed": true,
                }),
            );
            return Ok(0);
        }

        let scope = catalog::local_api_scope(runtime, library_id)?;
        let path = format!("{scope}/collections");
        let mut body = serde_json::Map::new();
        body.insert("name".to_string(), Value::String(name.to_string()));
        if let Some(parent) = &parent_collection {
            body.insert(
                "parentCollection".to_string(),
                Value::String(parent.key.clone()),
            );
        }
        let (outcome, _raw) = write_router::post_create(runtime, &path, &Value::Object(body))?;
        if let Some((code, payload)) = write_outcome_failure(&outcome) {
            output::emit(json_mode, &payload);
            return Ok(code);
        }
        let WriteOutcome::Applied { affected_key } = outcome else {
            unreachable!("write_outcome_failure returns None only for Applied");
        };
        let item_path = format!("{scope}/collections/{affected_key}");
        return match write_router::verify_present(runtime, &item_path) {
            write_router::PresenceCheck::Present(summary) => {
                output::emit(json_mode, &render_local_api_write_result(&summary, &[]));
                Ok(0)
            }
            other => presence_check_error(&item_path, other),
        };
    }

    let bridge_library_id = library_id_u32(library_id)?;
    let client = bridge::JSBridgeClient::with_default_port();
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
    let created = catalog::get_collection(runtime, Some(&affected_key), session)?;
    output::emit(json_mode, &serde_json::to_value(created)?);
    Ok(0)
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
    let collection = catalog::get_collection(runtime, Some(collection_key), session)?;
    let parent_collection = match parent {
        Some(p) => Some(catalog::get_collection(runtime, Some(p), session)?),
        None => None,
    };

    if runtime.local_api_writes_available {
        let scope = catalog::local_api_scope(runtime, collection.library_id)?;
        let path = format!("{scope}/collections/{}", collection.key);
        let current = match write_router::verify_present(runtime, &path) {
            write_router::PresenceCheck::Present(summary) => summary,
            other => return presence_check_error(&path, other),
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
        let (summary, mismatches) = write_router::verify_write(runtime, &path, &body)?;
        output::emit(
            json_mode,
            &render_local_api_write_result(&summary, &mismatches),
        );
        return Ok(0);
    }

    let library_id = library_id_u32(collection.library_id)?;
    let client = bridge::JSBridgeClient::with_default_port();
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
    let refreshed = catalog::get_collection(runtime, Some(&collection.key), session)?;
    output::emit(json_mode, &serde_json::to_value(refreshed)?);
    Ok(0)
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
    let collection = catalog::get_collection(runtime, Some(collection_key), session)?;

    // No Local API primitive exists for cascading item deletion -- always use the JS Bridge
    // when --delete-items is requested, regardless of local_api_writes_available.
    if runtime.local_api_writes_available && !delete_items {
        let scope = catalog::local_api_scope(runtime, collection.library_id)?;
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
    let client = bridge::JSBridgeClient::with_default_port();
    let outcome = client.collection_delete(library_id, &collection.key, delete_items)?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
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
    let collection = catalog::get_collection(runtime, Some(collection_key), session)?;
    let item = catalog::get_item(runtime, Some(item_key), session)?;
    if collection.library_id != item.library_id {
        return Err(
            error::DomainError::new("Item and collection must belong to the same library").into(),
        );
    }

    if runtime.local_api_writes_available {
        let scope = catalog::local_api_scope(runtime, item.library_id)?;
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
        let (summary, mismatches) = write_router::verify_write(runtime, &path, &body)?;
        output::emit(
            json_mode,
            &render_local_api_write_result(&summary, &mismatches),
        );
        return Ok(0);
    }

    let library_id = library_id_u32(item.library_id)?;
    let client = bridge::JSBridgeClient::with_default_port();
    let outcome = client.collection_remove_item(library_id, &item.key, &collection.key)?;
    if let Some((code, payload)) = write_outcome_failure(&outcome) {
        output::emit(json_mode, &payload);
        return Ok(code);
    }
    let refreshed = catalog::get_item(runtime, Some(&item.key), session)?;
    output::emit(json_mode, &serde_json::to_value(refreshed)?);
    Ok(0)
}

fn js_command(json_mode: bool, code: &str, wait: u64) -> anyhow::Result<i32> {
    let client = bridge::JSBridgeClient::with_default_port();
    let result = client.execute_raw_js(code, wait)?;
    output::emit(json_mode, &result);
    Ok(0)
}

fn sync_command(json_mode: bool) -> anyhow::Result<i32> {
    let client = bridge::JSBridgeClient::with_default_port();
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

fn app_install_plugin_command(json_mode: bool, output_dir: Option<&str>) -> anyhow::Result<i32> {
    let dir = plugin_output_dir(output_dir);
    let xpi_path = plugin::stage_xpi(&dir)?;
    output::emit(
        json_mode,
        &serde_json::json!({
            "staged_xpi_path": xpi_path.to_string_lossy(),
            "message": "XPI staged. Install manually via Zotero: Tools > Plugins/Add-ons > \
                        Install Add-on From File, then restart Zotero.",
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
