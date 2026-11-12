//! Executes one agent turn through the shared orchestrator and renders the
//! live event stream (used by `-p` one-shot runs and the interactive REPL).

use crate::cli::models::ResolvedModel;
use crate::cli::render::{colors_enabled, TextRenderer};
use crate::orchestrator::RunHandle;
use crate::api::runs as runs_api;
use crate::api::runs::{CreateRunPayload, ContinueRunPayload};
use crate::state::AppState;
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;

/// Per-turn options derived from the CLI flags (or REPL state).
#[derive(Clone, Debug)]
pub struct TurnOptions {
    pub project_path: PathBuf,
    /// `None` on `--continue`/`--session` without model flags keeps the
    /// session's provider/model.
    pub model: Option<ResolvedModel>,
    pub thinking: Option<String>,
    pub mode: String,
    pub bypass: bool,
    pub json: bool,
    pub show_thinking: bool,
    pub interactive: bool,
    pub no_session: bool,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
}

/// New run or continue an existing one.
#[derive(Clone, Debug)]
pub enum TurnTarget {
    New,
    Continue(String),
}

/// Runs one turn. Returns `(exit_code, run_id)`.
///
/// `active` (when given) is updated with the live run id so an interactive
/// Ctrl+C watcher can cancel the run instead of exiting the process.
pub async fn run_turn(
    state: &Arc<AppState>,
    target: TurnTarget,
    opts: &TurnOptions,
    task: String,
    active: Option<crate::cli::repl::ActiveRun>,
) -> (i32, Option<String>) {
    // Configuration check: the provider must be authenticated.
    if let Some(model) = &opts.model {
        if !state.pi.has_api_key(&model.provider_id) && model.provider_id != "ollama" {
            eprintln!(
                "lcgn: provider \"{}\" is not authenticated. Run `lcgn auth login {}` first.",
                model.provider_id, model.provider_id
            );
            return (2, None);
        }
    }

    let (run_id, mut rx, handle) = match target {
        TurnTarget::New => {
            let Some(model) = opts.model.clone() else {
                eprintln!(
                    "lcgn: no model specified. Use --model provider/model (see `lcgn --list-models`)."
                );
                return (2, None);
            };
            ensure_project(state, &opts.project_path);
            let payload = CreateRunPayload {
                task: task.clone(),
                project_path: Some(opts.project_path.display().to_string()),
                provider_id: model.provider_id.clone(),
                model: model.model.clone(),
                reasoning_effort: opts.thinking.clone(),
                mode: Some(opts.mode.clone()),
                bypass_permissions: opts.bypass,
                system_prompt: opts.system_prompt.clone(),
                append_system_prompt: opts.append_system_prompt.clone(),
                ..Default::default()
            };
            match runs_api::create_run(state, &payload).await {
                Ok(started) => (started.run.id, started.events, started.handle),
                Err(err) => {
                    eprintln!("lcgn: {}", err.message);
                    return (2, None);
                }
            }
        }
        TurnTarget::Continue(id) => {
            let payload = ContinueRunPayload {
                task: task.clone(),
                provider_id: opts.model.as_ref().map(|m| m.provider_id.clone()),
                model: opts.model.as_ref().map(|m| m.model.clone()),
                reasoning_effort: opts.thinking.clone(),
                mode: Some(opts.mode.clone()),
                bypass_permissions: opts.bypass,
                system_prompt: opts.system_prompt.clone(),
                append_system_prompt: opts.append_system_prompt.clone(),
                ..Default::default()
            };
            match runs_api::continue_run(state, &id, &payload).await {
                Ok(continued) => (id.clone(), continued.events, continued.handle),
                Err(err) => {
                    eprintln!("lcgn: {}", err.message);
                    return (2, None);
                }
            }
        }
    };

    if let Some(active) = &active {
        *active.lock().await = Some(run_id.clone());
    }

    let mut renderer = TextRenderer::new(
        std::io::stdout(),
        std::io::stderr(),
        colors_enabled() && !opts.json,
        opts.show_thinking && !opts.json,
        opts.interactive,
    );
    let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());

    let code = loop {
        let event = tokio::select! {
            ev = rx.recv() => match ev {
                Ok(event) => event,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break 1,
            },
            _ = ctrl_c.as_mut() => {
                eprintln!();
                eprintln!("lcgn: cancelled (Ctrl+C)");
                let _ = runs_api::cancel_run(state, &run_id).await;
                break 1;
            }
        };

        if opts.json {
            println!("{event}");
            let _ = std::io::stdout().flush();
        } else {
            renderer.on_event(&event);
        }

        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "run_completed" => {
                if !opts.json {
                    renderer.finalize(event.get("finalOutput").and_then(Value::as_str).unwrap_or(""));
                }
                break 0;
            }
            "run_failed" => {
                if !opts.json {
                    let message = event
                        .get("errorMessage")
                        .and_then(Value::as_str)
                        .unwrap_or("Run failed.");
                    eprintln!("lcgn: error: {message}");
                }
                break 1;
            }
            "status_changed" => {
                let status = event.get("status").and_then(Value::as_str).unwrap_or("");
                match status {
                    "cancelled" => {
                        if !opts.json {
                            eprintln!("lcgn: run cancelled.");
                        }
                        break 1;
                    }
                    "failed" => break 1,
                    _ => {}
                }
            }
            "permission_requested" => handle_permission(state, &handle, &event, opts).await,
            "question_requested" => handle_question(&handle, &event, opts).await,
            _ => {}
        }
    };

    if let Some(active) = &active {
        *active.lock().await = None;
    }
    if !opts.json {
        print_usage_line(state, &run_id);
    }
    if opts.no_session {
        state.db.delete_run(&run_id);
    }
    (code, Some(run_id))
}

/// Registers the project folder in `projects.json` when unknown.
fn ensure_project(state: &AppState, path: &Path) {
    let project_path = path.display().to_string();
    if state.db.get_project(&project_path).is_none() {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| project_path.clone());
        state.db.create_project(&project_path, &name);
    }
}

/// Final one-line usage/cost summary on stderr.
fn print_usage_line(state: &AppState, run_id: &str) {
    let entries = state.db.list_usage(Some(run_id));
    if entries.is_empty() {
        return;
    }
    let input: i64 = entries.iter().map(|e| e.input_tokens).sum();
    let output: i64 = entries.iter().map(|e| e.output_tokens).sum();
    let cache_read: i64 = entries.iter().map(|e| e.cache_read_tokens).sum();
    let cost: f64 = entries.iter().map(|e| e.cost).sum();
    eprintln!(
        "tokens: {input} in / {output} out · cache read: {cache_read} · cost: ${cost:.4}"
    );
}

async fn handle_permission(
    state: &Arc<AppState>,
    handle: &Arc<RunHandle>,
    event: &Value,
    opts: &TurnOptions,
) {
    let tool_call = event.get("toolCall").cloned().unwrap_or(Value::Null);
    let name = tool_call
        .pointer("/function/name")
        .and_then(Value::as_str)
        .unwrap_or("tool")
        .to_string();
    if !opts.interactive {
        eprintln!(
            "lcgn: permission required for `{name}` — denied (non-interactive run). Use --approve or run without -p to approve interactively."
        );
        runs_api::apply_permission_decision(state, handle, "deny").await;
        return;
    }

    eprintln!();
    eprintln!("Permission required: {name}");
    if let Some(preview) = event.get("preview").filter(|p| !p.is_null()) {
        print_preview(preview);
    }
    eprint!("[o]nce, [p]roject, [a]lways, [r]un, [d]eny? ");
    let _ = std::io::stderr().flush();
    let decision = tokio::task::spawn_blocking(read_permission_choice)
        .await
        .unwrap_or_else(|_| "deny".to_string());
    let decision = match decision.as_str() {
        "o" => "allow_once",
        "p" => "allow_project",
        "a" => "allow_always",
        "r" => "allow_run",
        _ => "deny",
    };
    runs_api::apply_permission_decision(state, handle, decision).await;
}

fn print_preview(preview: &Value) {
    let action = preview.get("action").and_then(Value::as_str).unwrap_or("");
    let path = preview.get("path").and_then(Value::as_str).unwrap_or("");
    match action {
        "command" => {
            let command = preview.get("command").and_then(Value::as_str).unwrap_or("");
            eprintln!("  command: {command}");
            if let Some(domains) = preview.get("networkDomains").and_then(Value::as_array) {
                let list: Vec<&str> = domains.iter().filter_map(Value::as_str).collect();
                if !list.is_empty() {
                    eprintln!("  network: {}", list.join(", "));
                }
            }
        }
        "fetch" => {
            let url = preview.get("url").and_then(Value::as_str).unwrap_or("");
            eprintln!("  url: {url}");
        }
        "move" => {
            let dest = preview.get("destPath").and_then(Value::as_str).unwrap_or("");
            eprintln!("  move: {path} -> {dest}");
        }
        "create" | "edit" | "delete" | "mkdir" => {
            eprintln!("  {action}: {path}");
            if let Some(new_content) = preview.get("newContent").and_then(Value::as_str) {
                for line in new_content.lines().take(12) {
                    eprintln!("  + {line}");
                }
                let total = new_content.lines().count();
                if total > 12 {
                    eprintln!("  … ({total} lines)");
                }
            }
        }
        _ => {
            if let Ok(compact) = serde_json::to_string(preview) {
                let mut cut: String = compact.chars().take(400).collect();
                if compact.chars().count() > 400 {
                    cut.push('…');
                }
                eprintln!("  {cut}");
            }
        }
    }
}

/// Reads the permission choice from stdin until a valid answer or EOF.
fn read_permission_choice() -> String {
    let mut line = String::new();
    loop {
        line.clear();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) => return "deny".to_string(), // EOF
            Ok(_) => {}
            Err(_) => return "deny".to_string(),
        }
        let answer = line.trim().to_lowercase();
        let first = answer.chars().next();
        match first {
            Some('o') if answer.len() == 1 || answer.starts_with("once") => return "o".to_string(),
            Some('p') if answer.len() == 1 || answer.starts_with("project") => {
                return "p".to_string()
            }
            Some('a') if answer.len() == 1 || answer.starts_with("always") => {
                return "a".to_string()
            }
            Some('r') if answer.len() == 1 || answer.starts_with("run") => return "r".to_string(),
            Some('d') if answer.len() == 1 || answer.starts_with("deny") => return "d".to_string(),
            _ => {
                eprint!("Answer with o, p, a, r or d: ");
                let _ = std::io::stderr().flush();
            }
        }
    }
}

async fn handle_question(handle: &Arc<RunHandle>, event: &Value, opts: &TurnOptions) {
    let questions = event.get("questions").cloned().unwrap_or(Value::Null);

    if !opts.interactive {
        eprintln!("lcgn: question skipped (non-interactive run).");
        runs_api::answer_question_core(
            handle,
            Vec::new(),
            vec!["(Answered automatically: non-interactive run.)".to_string()],
        )
        .await;
        return;
    }

    let mut selections: Vec<Vec<String>> = Vec::new();
    if let Some(items) = questions.as_array() {
        for question in items {
            let text = question
                .get("question")
                .and_then(Value::as_str)
                .or_else(|| question.get("title").and_then(Value::as_str))
                .unwrap_or("Question");
            eprintln!();
            eprintln!("Question: {text}");
            let options = question.get("options").and_then(Value::as_array).cloned().unwrap_or_default();
            let labels: Vec<String> = options
                .iter()
                .map(|option| match option {
                    Value::String(label) => label.clone(),
                    other => other
                        .get("label")
                        .and_then(Value::as_str)
                        .or_else(|| other.get("value").and_then(Value::as_str))
                        .unwrap_or("option")
                        .to_string(),
                })
                .collect();
            for (index, label) in labels.iter().enumerate() {
                eprintln!("  {}. {label}", index + 1);
            }
            eprint!("Choose (number(s), comma-separated): ");
            let _ = std::io::stderr().flush();
            let choice = tokio::task::spawn_blocking(read_line_stdin)
                .await
                .unwrap_or_default();
            let mut picked: Vec<String> = Vec::new();
            for token in choice.split(',') {
                if let Ok(number) = token.trim().parse::<usize>() {
                    if number >= 1 && number <= labels.len() {
                        picked.push(labels[number - 1].clone());
                    }
                }
            }
            if picked.is_empty() && !labels.is_empty() {
                picked.push(labels[0].clone());
            }
            selections.push(picked);
        }
    }
    runs_api::answer_question_core(handle, selections, Vec::new()).await;
}

fn read_line_stdin() -> String {
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(_) => line,
        Err(_) => String::new(),
    }
}
