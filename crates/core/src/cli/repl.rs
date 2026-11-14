//! Interactive `lcgn` REPL: line editor, slash commands, Ctrl+C cancel.

use super::commands::{self, CommandSession, Output};
use crate::commands::SlashCommand;
use super::models::ResolvedModel;
use super::runner::{run_turn, TurnOptions, TurnTarget};
use crate::state::AppState;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

/// REPL configuration carried over from the CLI flags.
pub struct ReplConfig {
    pub project: PathBuf,
    pub model: Option<ResolvedModel>,
    pub thinking: Option<String>,
    pub mode: String,
    pub bypass: bool,
    pub show_thinking: bool,
    pub no_session: bool,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub initial_session: Option<String>,
}

/// The run id currently executing, so the Ctrl+C watcher can cancel it
/// instead of exiting.
pub type ActiveRun = Arc<Mutex<Option<String>>>;

pub async fn repl(state: Arc<AppState>, config: ReplConfig) -> i32 {
    let mut rl = match DefaultEditor::new() {
        Ok(editor) => editor,
        Err(err) => {
            eprintln!("lcgn: line editor unavailable: {err}");
            return 2;
        }
    };

    let mut session: Option<String> = config.initial_session.clone();
    let mut model = config.model.clone();
    let mut thinking = config.thinking.clone();
    let mut mode = config.mode.clone();

    // Ctrl+C cancels the active run; at an idle prompt it exits.
    let active_run: ActiveRun = Arc::new(Mutex::new(None));
    {
        let active_run = active_run.clone();
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                if tokio::signal::ctrl_c().await.is_err() {
                    return;
                }
                let mut slot = active_run.lock().await;
                // Idle: rustyline reports Ctrl+C itself (Interrupted) and the
                // REPL exits cleanly, so nothing to do without an active run.
                if let Some(run_id) = slot.take() {
                    drop(slot);
                    eprintln!();
                    eprintln!("lcgn: cancelling {run_id}…");
                    let _ = std::io::stderr().flush();
                    let _ = crate::api::runs::cancel_run(&state, &run_id).await;
                }
            }
        });
    }

    eprintln!(
        "lcgn — Locagens CLI · mode: {}{} · type /help for commands",
        mode,
        model
            .as_ref()
            .map(|m| format!(" · model: {}", m.label()))
            .unwrap_or_default()
    );

    loop {
        match rl.readline("» ") {
            Ok(line) => {
                let _ = rl.add_history_entry(line.trim());
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Some(command) = SlashCommand::parse(trimmed) {
                    let mut current = CommandSession {
                        model: model.clone(),
                        thinking: thinking.clone(),
                        mode: mode.clone(),
                        bypass: config.bypass,
                        session: session.clone(),
                        busy: false,
                    };
                    let result = commands::execute(&state, &config.project, command, &mut current).await;
                    model = current.model;
                    thinking = current.thinking;
                    mode = current.mode;
                    session = current.session;
                    print_outputs(&result.outputs);
                    if let Some(target) = result.review.as_deref() {
                        run_review_command(&state, &config, model.as_ref(), thinking.clone(), target, &active_run).await;
                    }
                    if result.exit {
                        break;
                    }
                    continue;
                }

                // Same prompt rules as the TUI: `@file` mentions of existing
                // project files are attached, the text stays verbatim.
                let task = match super::prompt::build_prompt(trimmed, &config.project) {
                    Ok(task) => task,
                    Err(message) => {
                        eprintln!("lcgn: {message}");
                        continue;
                    }
                };

                let opts = TurnOptions {
                    project_path: config.project.clone(),
                    model: model.clone(),
                    thinking: thinking.clone(),
                    mode: mode.clone(),
                    bypass: config.bypass,
                    json: false,
                    show_thinking: config.show_thinking,
                    interactive: true,
                    no_session: config.no_session,
                    system_prompt: config.system_prompt.clone(),
                    append_system_prompt: config.append_system_prompt.clone(),
                };
                let target = match &session {
                    Some(id) => TurnTarget::Continue(id.clone()),
                    None => TurnTarget::New,
                };
                let (_code, run_id) =
                    run_turn(&state, target, &opts, task, Some(active_run.clone())).await;
                if let Some(run_id) = run_id {
                    if config.no_session {
                        session = None;
                    } else {
                        session = Some(run_id);
                    }
                }
                eprintln!();
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl+C at an idle prompt exits (a run in progress is
                // cancelled by the watcher above).
                break;
            }
            Err(ReadlineError::Eof) => break,
            Err(err) => {
                eprintln!("lcgn: {err}");
                break;
            }
        }
    }
    0
}

/// Prints command outputs the REPL way: notices and errors on stderr, tables
/// and diffs on stdout.
fn print_outputs(outputs: &[Output]) {
    for output in outputs {
        match output {
            Output::Notice(text) => eprintln!("lcgn: {text}"),
            Output::Error(text) => eprintln!("lcgn: {text}"),
            Output::Table(text) => {
                print!("{text}");
                let _ = std::io::stdout().flush();
            }
            Output::Diff(text) => {
                println!("{text}");
                let _ = std::io::stdout().flush();
            }
        }
    }
}

/// `/review [target]`: streams a read-only review of the working tree.
async fn run_review_command(
    state: &Arc<AppState>,
    config: &ReplConfig,
    model: Option<&ResolvedModel>,
    thinking: Option<String>,
    argument: &str,
    active_run: &ActiveRun,
) {
    let target = match super::review::target_from_slash(argument) {
        Ok(target) => target,
        Err(message) => return eprintln!("lcgn: {message}"),
    };
    match model {
        Some(model) => {
            super::review::run_review(state, &config.project, model, thinking, target, false, Some(active_run.clone()))
                .await;
            eprintln!();
        }
        None => eprintln!("lcgn: no model selected. Use /model provider/id first."),
    }
}
