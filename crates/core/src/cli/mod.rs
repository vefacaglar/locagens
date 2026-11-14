//! `lcgn` — the Locagens CLI binary logic (runs the shared Rust core
//! in-process; no HTTP server, no spawned `pi` process).

pub mod args;
pub mod auth;
pub mod commands;
pub mod events;
pub mod logging;
pub mod mcp;
pub mod models;
pub mod prompt;
pub mod render;
pub mod repl;
pub mod review;
pub mod runner;
pub mod trust;
pub mod tui;

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
    if let Err(message) = cli.validate() {
        eprintln!("lcgn: {message}");
        return 2;
    }
    logging::init();
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
    runtime.block_on(async {
        let code = run(cli).await;
        // Politely end remote MCP sessions (DELETE Mcp-Session-Id).
        crate::services::mcp::shutdown_sessions().await;
        code
    })
}

/// Loads config.toml (+ `--profile`), installs it process-wide and fills
/// unset flags from it. Errors name the offending file.
fn load_effective_config(cli: &mut Cli) -> Result<locagens_config::config_toml::Config, String> {
    use locagens_config::config_toml::Config;
    let config = Config::load_from_files(&crate::paths::config_files())
        .and_then(|config| config.with_profile(cli.profile.as_deref()))
        .map_err(|err| err.to_string())?;
    crate::config_runtime::install(config.clone());
    cli.apply_config_defaults(&config);
    Ok(config)
}

/// `lcgn config [show]`: the effective config with secrets masked.
fn config_command(config: &locagens_config::config_toml::Config) -> i32 {
    let files: Vec<String> = crate::paths::config_files()
        .iter()
        .filter(|path| path.is_file())
        .map(|path| path.display().to_string())
        .collect();
    println!("# Effective config (profile: {})", config.profile.as_deref().unwrap_or("none"));
    if files.is_empty() {
        println!("# No config.toml found; showing built-in defaults.");
    }
    for file in &files {
        println!("# source: {file}");
    }
    println!("{}", config.to_masked_toml());
    0
}

async fn run(mut cli: Cli) -> i32 {
    let config = match load_effective_config(&mut cli) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("lcgn: {message}");
            return 2;
        }
    };
    // `lcgn config` only prints the file chain: no store, no legacy import.
    if matches!(cli.command, Some(Command::Config { .. })) {
        return config_command(&config);
    }
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
            Command::Auth { action } => auth::auth_command(&state, action).await,
            Command::Mcp { action } => {
                if let (true, Ok(dir)) = (cli.trust, cli.project_dir()) {
                    crate::services::project_trust::trust_for_session(&dir);
                }
                let project = cli.project_dir().ok().map(|p| p.to_string_lossy().to_string());
                mcp::mcp_command(&state, action, project.as_deref()).await
            }
            Command::Config { .. } => config_command(&config),
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
            Command::Review(args) => return review_main(&state, &cli, &args).await,
            Command::Trust { path, revoke, status } => trust::trust_command(&cli, path.as_deref(), revoke, status),
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
    // Interactive TUI (or the plain REPL with --no-tui) when there is a TTY
    // and no -p / --mode json.
    let interactive = !cli.print && !json_mode && args::stdin_is_tty();
    let model = if cli.model.is_some() || cli.provider.is_some() {
        match models::resolve_model(&state.pi, cli.model.as_deref(), cli.provider.as_deref()).await {
            Ok(model) => Some(model),
            // A stale config.toml default (e.g. Codex's own `model` in
            // ~/.codex/config.toml) must not keep the interactive UI from
            // opening: start without a model and let the user pick one.
            Err(message) if cli.model_from_config && interactive => {
                eprintln!("lcgn: config.toml default model unavailable: {message} Pick one with /model.");
                None
            }
            Err(message) if cli.model_from_config => {
                eprintln!(
                    "lcgn: config.toml default model unavailable: {message} Pass --model provider/model or set `model` in ~/.locagens/config.toml."
                );
                return 2;
            }
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

    // Project hooks / MCP / plugins / rules / roles load only for a trusted
    // folder: ask once on a terminal, otherwise print a one-line notice.
    trust::ensure_project_trust(&project, cli.trust, interactive);
    if interactive {
        // In the full-screen UI `-r` opens the /resume picker instead of the
        // numbered stderr prompt.
        let defer_resume = cli.use_tui();
        let initial_session = match resolve_initial_session(&cli, &state, &project, defer_resume).await {
            Ok(session) => session,
            Err(code) => return code,
        };
        let open_resume = defer_resume && cli.resume && initial_session.is_none();
        let config = repl::ReplConfig {
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
            open_resume,
        };
        if cli.use_tui() {
            return tui::run(state, config).await;
        }
        return repl::repl(state, config).await;
    }

    // One-shot run (with -p, --mode json, or piped/non-tty stdin).
    if cli.resume {
        eprintln!("lcgn: --resume needs an interactive terminal.");
        return 2;
    }
    let session = match resolve_initial_session(&cli, &state, &project, false).await {
        Ok(session) => session,
        Err(code) => return code,
    };
    let target = match session.clone() {
        Some(id) => TurnTarget::Continue(id),
        None => TurnTarget::New,
    };
    let stdin = if cli.wants_stdin() { args::read_piped_stdin() } else { None };
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
    defer_resume: bool,
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
    if cli.resume && defer_resume {
        return Ok(None);
    }
    if cli.resume {
        return match auth::pick_session(state, project).await {
            Some(run) => Ok(Some(run.id)),
            None => Err(0),
        };
    }
    Ok(None)
}

/// `lcgn review`: subcommand flags win over the global ones.
async fn review_main(state: &Arc<crate::state::AppState>, cli: &Cli, args: &args::ReviewArgs) -> i32 {
    let cwd = args.cwd.clone().or_else(|| cli.cwd.clone());
    let project = match cwd.as_deref().filter(|dir| !dir.trim().is_empty()) {
        Some(dir) => crate::util::canonical_project_path(dir),
        None => std::env::current_dir().map_err(|err| format!("Cannot resolve the current directory: {err}")),
    };
    let project = match project {
        Ok(project) => project,
        Err(message) => {
            eprintln!("lcgn: {message}");
            return 2;
        }
    };
    trust::ensure_project_trust(&project, cli.trust, false);
    let provider_flag = args.provider.as_deref().or(cli.provider.as_deref());
    // The global `cli.model` may have been filled from config.toml for the
    // configured provider: do not pair it with a different review provider.
    let config_model = crate::config_runtime::current().model.clone();
    let inherited_model = cli.model.is_some() && cli.model == config_model;
    let model_flag = args.model.as_deref().or(if args.provider.is_some()
        && inherited_model
        && args.provider.as_deref() != cli.provider.as_deref()
    {
        None
    } else {
        cli.model.as_deref()
    });
    let model = if model_flag.is_some() || provider_flag.is_some() {
        match models::resolve_model(&state.pi, model_flag, provider_flag).await {
            Ok(model) => Some(model),
            Err(message) => {
                eprintln!("lcgn: {message}");
                return 2;
            }
        }
    } else {
        None
    };
    let thinking = args
        .thinking
        .as_deref()
        .or(cli.thinking.as_deref())
        .filter(|level| !level.is_empty() && *level != "off")
        .map(str::to_string);
    let json_mode = args.mode.as_deref().or(cli.mode.as_deref()) == Some("json");
    review::review_command(state, &project, model, thinking, args, json_mode).await
}
