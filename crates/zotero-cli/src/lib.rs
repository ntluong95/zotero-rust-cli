pub mod catalog;
pub mod cli;
pub mod db;
pub mod error;
pub mod http;
pub mod output;
pub mod paths;
pub mod runtime;
pub mod session;

use clap::{CommandFactory, Parser};
use serde_json::Value;

use cli::{AppCommands, Cli, CollectionCommands, Commands, ItemCommands};

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
    let runtime = runtime::build_runtime_context(runtime::BuildEnvironmentArgs {
        backend: &backend,
        data_dir: cli.data_dir.as_deref(),
        profile_dir: cli.profile_dir.as_deref(),
        executable: cli.executable.as_deref(),
    });
    let session = session::load_session_state();

    match command {
        Commands::App(AppCommands::Status) => {
            let payload = runtime.to_status_payload();
            output::emit(json_mode, &Value::Object(payload));
            Ok(0)
        }
        Commands::Item(ItemCommands::List { limit }) => {
            let items = catalog::list_items(&runtime, &session, Some(limit))?;
            output::emit(json_mode, &serde_json::to_value(items)?);
            Ok(0)
        }
        Commands::Item(ItemCommands::Get { item_ref }) => {
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
        Commands::Collection(CollectionCommands::List) => {
            let collections = catalog::list_collections(&runtime, &session)?;
            output::emit(json_mode, &serde_json::to_value(collections)?);
            Ok(0)
        }
    }
}
