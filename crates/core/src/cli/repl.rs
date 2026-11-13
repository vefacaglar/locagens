//! Interactive `lcgn` REPL: line editor, slash commands, Ctrl+C cancel.

use super::models::{list_models_table, resolve_model, ResolvedModel};
use super::runner::{run_turn, TurnOptions, TurnTarget};
use crate::cli::auth;
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
                match slot.take() {
                    Some(run_id) => {
                        drop(slot);
                        eprintln!();
                        eprintln!("lcgn: cancelling {run_id}…");
                        let _ = std::io::stderr().flush();
                        let _ = crate::api::runs::cancel_run(&state, &run_id).await;
                    }
                    None => std::process::exit(0),
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
                if trimmed.starts_with('/') {
                    match handle_slash(
                        &state,
                        trimmed,
                        &config,
                        &mut model,
                        &mut thinking,
                        &mut mode,
                        &mut session,
                    )
                    .await
                    {
                        SlashResult::Continue => continue,
                        SlashResult::Exit => break,
                    }
                }

                let (attachment_tokens, rest): (Vec<&str>, Vec<&str>) = trimmed
                    .split_whitespace()
                    .partition(|token| token.starts_with('@'));
                let mut tokens: Vec<String> = attachment_tokens.iter().map(|t| t.to_string()).collect();
                tokens.extend(rest.iter().map(|token| token.to_string()));
                let task = match super::args::assemble_prompt(&tokens, &config.project, None) {
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
                // The Ctrl+C watcher handles cancel/exit.
                continue;
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

enum SlashResult {
    Continue,
    Exit,
}

#[allow(clippy::too_many_arguments)]
async fn handle_slash(
    state: &Arc<AppState>,
    command: &str,
    config: &ReplConfig,
    model: &mut Option<ResolvedModel>,
    thinking: &mut Option<String>,
    mode: &mut String,
    session: &mut Option<String>,
) -> SlashResult {
    let mut parts = command.split_whitespace();
    let head = parts.next().unwrap_or("").to_lowercase();
    let rest: Vec<&str> = parts.collect();
    match (head.as_str(), rest.as_slice()) {
        ("/exit", _) | ("/quit", _) => SlashResult::Exit,
        ("/help", _) => {
            eprintln!(
                "Commands:\n\
                 /model [provider/id]  show or switch models\n\
                 /thinking <level>     reasoning effort (off|minimal|low|medium|high|xhigh|max)\n\
                 /plan                 switch to plan mode\n\
                 /build                switch to build mode (edits applied directly)\n\
                 /new                  start a fresh session on the next message\n\
                 /fork                 fork the current session into an independent copy\n\
                 /undo                 undo the last turn (git snapshot restore)\n\
                 /diff                 show the current working-tree diff\n\
                 /mcp                  list configured MCP servers and their status\n\
                 /skills               list discovered $skills for this project\n\
                 /sessions             list sessions for this directory\n\
                 /usage                usage/cost of the current session\n\
                 /help                 this help\n\
                 /exit                 leave lcgn (Ctrl+D also exits)"
            );
            SlashResult::Continue
        }
        ("/fork", _) => {
            match session.as_deref() {
                Some(run_id) => match crate::api::runs::fork_run(state, run_id, None).await {
                    Ok(forked) => {
                        eprintln!("lcgn: forked run {run_id} -> {}", forked.id);
                        *session = Some(forked.id);
                    }
                    Err(err) => eprintln!("lcgn: fork failed: {err}"),
                },
                None => {
                    eprintln!("lcgn: no active session yet — send a message first.");
                }
            }
            SlashResult::Continue
        }
        ("/undo", _) => {
            let project = config.project.to_string_lossy().to_string();
            match crate::api::projects::undo_last_turn(state, &project, session.as_deref()).await {
                Ok(snapshot) => {
                    eprintln!(
                        "lcgn: restored snapshot {} (HEAD {}).",
                        snapshot.id, snapshot.head_sha
                    );
                }
                Err(err) => eprintln!("lcgn: undo failed: {err}"),
            }
            SlashResult::Continue
        }
        ("/diff", _) => {
            let project = config.project.to_string_lossy().to_string();
            match crate::api::projects::git_diff(state, &project, false).await {
                Ok(diff) if diff.trim().is_empty() => {
                    eprintln!("lcgn: working tree clean.");
                }
                Ok(diff) => {
                    println!("{diff}");
                    let _ = std::io::stdout().flush();
                }
                Err(err) => eprintln!("lcgn: diff failed: {err}"),
            }
            SlashResult::Continue
        }
        ("/mcp", _) => {
            let project = config.project.to_string_lossy().to_string();
            match crate::api::mcp::list_mcp_servers(state, Some(&project)).await {
                Ok(response) if response.servers.is_empty() => {
                    eprintln!("lcgn: no MCP servers configured.");
                }
                Ok(response) => {
                    for server in response.servers {
                        eprintln!(
                            "lcgn: {} — {} ({})",
                            server.config.name, server.config.transport, server.status
                        );
                    }
                }
                Err(err) => eprintln!("lcgn: mcp list failed: {err}"),
            }
            SlashResult::Continue
        }
        ("/skills", _) => {
            let project = config.project.to_string_lossy().to_string();
            match crate::api::skills::list_skills(state, Some(&project)).await {
                Ok(response) if response.skills.is_empty() => {
                    eprintln!("lcgn: no skills discovered.");
                }
                Ok(response) => {
                    for skill in response.skills {
                        eprintln!(
                            "lcgn: ${} — {} [{}]",
                            skill.name, skill.description, skill.source
                        );
                    }
                }
                Err(err) => eprintln!("lcgn: skills list failed: {err}"),
            }
            SlashResult::Continue
        }
        ("/model", []) => {
            let table = list_models_table(&state.pi, "").await;
            print!("{table}");
            let _ = std::io::stdout().flush();
            SlashResult::Continue
        }
        ("/model", [target]) => {
            match resolve_model(&state.pi, Some(target), None).await {
                Ok(resolved) => {
                    if state.pi.has_api_key(&resolved.provider_id)
                        || resolved.provider_id == "ollama"
                    {
                        *model = Some(resolved.clone());
                        eprintln!("lcgn: model set to {}.", resolved.label());
                    } else {
                        eprintln!(
                            "lcgn: provider \"{}\" is not authenticated. Run `lcgn auth login {}`.",
                            resolved.provider_id, resolved.provider_id
                        );
                    }
                }
                Err(message) => eprintln!("lcgn: {message}"),
            }
            SlashResult::Continue
        }
        ("/thinking", [level]) => {
            match level.trim().to_lowercase().as_str() {
                "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => {
                    let level = level.trim().to_lowercase();
                    let shown = level.clone();
                    *thinking = if level == "off" { None } else { Some(level) };
                    eprintln!("lcgn: thinking set to {shown}.");
                }
                _ => eprintln!("lcgn: unknown thinking level \"{level}\"."),
            }
            SlashResult::Continue
        }
        ("/thinking", []) => {
            eprintln!(
                "lcgn: thinking is currently {}.",
                thinking.as_deref().unwrap_or("off")
            );
            SlashResult::Continue
        }
        ("/plan", _) => {
            *mode = "plan".to_string();
            eprintln!("lcgn: mode set to plan.");
            SlashResult::Continue
        }
        ("/build", _) => {
            *mode = "accept_edits".to_string();
            eprintln!("lcgn: mode set to build.");
            SlashResult::Continue
        }
        ("/new", _) => {
            *session = None;
            eprintln!("lcgn: next message starts a new session.");
            SlashResult::Continue
        }
        ("/sessions", _) => {
            auth::sessions_command(state, &config.project, false);
            SlashResult::Continue
        }
        ("/usage", _) => match session.as_deref() {
            Some(run_id) => {
                let entries = state.db.list_usage(Some(run_id));
                if entries.is_empty() {
                    eprintln!("lcgn: no usage recorded for this session yet.");
                } else {
                    let input: i64 = entries.iter().map(|e| e.input_tokens).sum();
                    let output: i64 = entries.iter().map(|e| e.output_tokens).sum();
                    let cost: f64 = entries.iter().map(|e| e.cost).sum();
                    let calls = entries.len();
                    eprintln!("lcgn: {calls} call(s) · {input} in / {output} out · ${cost:.4}");
                }
                SlashResult::Continue
            }
            None => {
                eprintln!("lcgn: no active session yet.");
                SlashResult::Continue
            }
        },
        (other, _) => {
            eprintln!("lcgn: unknown command \"{other}\". Type /help.");
            SlashResult::Continue
        }
    }
}
