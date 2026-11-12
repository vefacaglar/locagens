//! `lcgn` — the Locagens CLI binary logic (runs the shared Rust core
//! in-process; no HTTP server, no spawned `pi` process).

pub mod args;
pub mod auth;
pub mod models;
pub mod render;
pub mod repl;
pub mod runner;

use args::{Cli, Command};
use clap::Parser as _;
use render::enable_windows_vt;
use runner::{TurnOptions, TurnTarget};
use std::io::Write;
use std::sync::Arc;

/// Process entry point; returns the exit code.
pub fn main_entry() -> i32 {
    enable_windows_vt();
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("lcgn: failed to start the async runtime: {err}");
            return 2;
        }
    };
    runtime.block_on(run(cli))
}

async fn run(cli: Cli) -> i32 {
    let core = match crate::build_core() {
        Ok(core) => core,
        Err(err) => {
            eprintln!("lcgn: {err}");
            return 2;
        }
    };
    let state = Arc::new(core.into_app_state(Arc::new(
        crate::services::processes::ProcessManager::new(),
    )));

    // Subcommands first: auth / sessions.
    if let Some(command) = cli.command.clone() {
        return match command {
            Command::Auth { action } => auth::auth_command(&state.pi, action).await,
            Command::Sessions { json } => {
                let project = match cli.project_dir() {
                    Ok(project) => project,
                    Err(message) => {
                        eprintln!("lcgn: {message}");
                        return 2;
                    }
                };
                auth::sessions_command(&state, &project, json)
            }
        };
    }

    // --list-models
    if let Some(search) = cli.list_models.clone() {
        let table = models::list_models_table(&state.pi, &search).await;
        print!("{table}");
        let _ = std::io::stdout().flush();
        return 0;
    }

    let project = match cli.project_dir() {
        Ok(project) => project,
        Err(message) => {
            eprintln!("lcgn: {message}");
            return 2;
        }
    };

    let json_mode = cli.json_mode();
    let model = if cli.model.is_some() || cli.provider.is_some() {
        match models::resolve_model(&state.pi, cli.model.as_deref(), cli.provider.as_deref()).await {
            Ok(model) => Some(model),
            Err(message) => {
                eprintln!("lcgn: {message}");
                return 2;
            }
        }
    } else {
        None
    };

    let opts = TurnOptions {
        project_path: project.clone(),
        model: model.clone(),
        thinking: cli.thinking_level(),
        mode: cli.effective_mode(),
        bypass: cli.approve,
        json: json_mode,
        show_thinking: cli.show_thinking(),
        interactive: false,
        no_session: cli.no_session,
        system_prompt: cli.system_prompt.clone(),
        append_system_prompt: cli.append_system_prompt.clone(),
    };

    // Interactive REPL when there is a TTY and no -p / --mode json.
    let interactive = !cli.print && !json_mode && args::stdin_is_tty();
    if interactive {
        let initial_session = match resolve_initial_session(&cli, &state, &project).await {
            Ok(session) => session,
            Err(code) => return code,
        };
        return repl::repl(
            state,
            repl::ReplConfig {
                project,
                model,
                thinking: cli.thinking_level(),
                mode: cli.effective_mode(),
                bypass: cli.approve,
                show_thinking: cli.show_thinking(),
                no_session: cli.no_session,
                system_prompt: cli.system_prompt.clone(),
                append_system_prompt: cli.append_system_prompt.clone(),
                initial_session,
            },
        )
        .await;
    }

    // One-shot run (with -p, --mode json, or piped/non-tty stdin).
    if cli.resume {
        eprintln!("lcgn: --resume needs an interactive terminal.");
        return 2;
    }
    let session = match resolve_initial_session(&cli, &state, &project).await {
        Ok(session) => session,
        Err(code) => return code,
    };
    let target = match session.clone() {
        Some(id) => TurnTarget::Continue(id),
        None => TurnTarget::New,
    };
    let stdin = args::read_piped_stdin();
    let task = match args::assemble_prompt(&cli.messages, &project, stdin.as_deref()) {
        Ok(task) => task,
        Err(message) => {
            eprintln!("lcgn: {message}");
            return 2;
        }
    };
    runner::run_turn(&state, target, &opts, task, None).await.0
}

/// Resolves `-c` / `--session` / `-r` into the session id to continue
/// (or `None` for a fresh run).
async fn resolve_initial_session(
    cli: &Cli,
    state: &Arc<crate::state::AppState>,
    project: &std::path::Path,
) -> Result<Option<String>, i32> {
    let fail = |message: String| -> i32 {
        eprintln!("lcgn: {message}");
        2
    };
    if let Some(partial) = cli.session.as_deref().filter(|s| !s.trim().is_empty()) {
        return match auth::resolve_session(state, partial.trim(), project) {
            Ok(run) => Ok(Some(run.id)),
            Err(message) => Err(fail(message)),
        };
    }
    if cli.cont {
        return match auth::latest_session(state, project) {
            Some(run) => Ok(Some(run.id)),
            None => Err(fail(format!(
                "No previous session for {}. Start one by passing a message.",
                project.display()
            ))),
        };
    }
    if cli.resume {
        return match auth::pick_session(state, project).await {
            Some(run) => Ok(Some(run.id)),
            None => Err(0),
        };
    }
    Ok(None)
}
