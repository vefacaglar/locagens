//! `lcgn review` and the `/review` REPL command: starts a read-only review
//! run, waits for it and prints the findings.

use crate::api::review::{self as review_api, StartReviewPayload};
use crate::api::runs as runs_api;
use crate::cli::args::ReviewArgs;
use crate::cli::commands::ensure_authenticated;
use crate::cli::events::{Pumped, RunEvents};
use crate::cli::models::ResolvedModel;
use crate::state::AppState;
use locagens_protocol::review::{render_review_text, ReviewPayload, ReviewTarget};
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

/// Picks the review target from `lcgn review` flags: `--base`, `--commit`,
/// `--uncommitted` or a custom prompt; no flag and no prompt reviews the
/// uncommitted changes.
pub fn target_from_args(args: &ReviewArgs) -> Result<ReviewTarget, String> {
    let prompt = args.prompt.join(" ").trim().to_string();
    if let Some(branch) = args.base.as_deref() {
        return Ok(ReviewTarget::BaseBranch { branch: branch.trim().to_string() });
    }
    if let Some(sha) = args.commit.as_deref() {
        return Ok(ReviewTarget::Commit { sha: sha.trim().to_string(), title: None });
    }
    if !args.uncommitted && !prompt.is_empty() {
        return Ok(ReviewTarget::Custom { instructions: prompt });
    }
    Ok(ReviewTarget::Uncommitted)
}

/// Parses the argument of the REPL/TUI `/review` command:
/// `` (uncommitted), `base <branch>`, `commit <sha>`, `uncommitted`, or free text.
pub fn target_from_slash(rest: &str) -> Result<ReviewTarget, String> {
    let rest = rest.trim();
    if rest.is_empty() || rest == "uncommitted" {
        return Ok(ReviewTarget::Uncommitted);
    }
    let (head, tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let tail = tail.trim();
    match head {
        "base" if tail.is_empty() => Err("Usage: /review base <branch>".to_string()),
        "base" => Ok(ReviewTarget::BaseBranch { branch: tail.to_string() }),
        "commit" if tail.is_empty() => Err("Usage: /review commit <sha>".to_string()),
        "commit" => Ok(ReviewTarget::Commit { sha: tail.to_string(), title: None }),
        _ => Ok(ReviewTarget::Custom { instructions: rest.to_string() }),
    }
}

/// Runs one review to completion. Text mode prints the findings to stdout
/// (progress on stderr); JSON mode prints one event per line followed by a
/// final `review_completed` line. Returns `(exit_code, run_id)`.
pub async fn run_review(
    state: &Arc<AppState>,
    project: &Path,
    model: &ResolvedModel,
    thinking: Option<String>,
    target: ReviewTarget,
    json_mode: bool,
    active: Option<crate::cli::repl::ActiveRun>,
) -> (i32, Option<String>) {
    if let Err(message) = ensure_authenticated(state, model).await {
        eprintln!("lcgn: {message}");
        return (2, None);
    }
    crate::cli::runner::ensure_project(state, project);
    let payload = StartReviewPayload {
        target: target.clone(),
        project_path: Some(project.display().to_string()),
        provider_id: model.provider_id.clone(),
        model: model.model.clone(),
        reasoning_effort: thinking,
    };
    let started = match review_api::start_review(state, &payload).await {
        Ok(started) => started,
        Err(err) => {
            eprintln!("lcgn: {}", err.message);
            return (2, None);
        }
    };
    let run_id = started.run.id.clone();
    let handle = started.handle.clone();
    let mut events = RunEvents::new(state.clone(), run_id.clone(), started.events);
    if let Some(active) = &active {
        *active.lock().await = Some(run_id.clone());
    }
    if !json_mode {
        eprintln!("Reviewing {} with {}…", target.hint(), model.label());
    }
    let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());

    let code = loop {
        let event = tokio::select! {
            pumped = events.next() => match pumped {
                Pumped::Event(event) => event,
                // Lagged: re-synced from the store, replayed events follow.
                Pumped::Resynced(_) => continue,
                Pumped::Closed => break 1,
            },
            _ = ctrl_c.as_mut() => {
                eprintln!("\nlcgn: review cancelled (Ctrl+C)");
                let _ = runs_api::cancel_run(state, &run_id).await;
                break 1;
            }
        };
        if json_mode {
            println!("{event}");
            let _ = std::io::stdout().flush();
        }
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "run_completed" => break 0,
            "run_failed" => {
                let message = event.get("errorMessage").and_then(Value::as_str).unwrap_or("Run failed.");
                eprintln!("lcgn: error: {message}");
                break 1;
            }
            "status_changed" => {
                if matches!(event.get("status").and_then(Value::as_str), Some("cancelled" | "failed")) {
                    break 1;
                }
            }
            "message_created" | "message_updated" if !json_mode => {
                print_tool_progress(&event);
            }
            // Chat mode has no gated tools, but answer defensively so a run can never hang.
            "permission_requested" => {
                let _ = runs_api::apply_permission_decision(state, &handle, "deny").await;
            }
            "question_requested" => {
                runs_api::answer_question_core(
                    &handle,
                    Vec::new(),
                    vec!["(Answered automatically: review runs are non-interactive.)".to_string()],
                )
                .await;
            }
            _ => {}
        }
    };

    if let Some(active) = &active {
        *active.lock().await = None;
    }
    if code == 0 {
        let review = final_review(state, &run_id);
        if json_mode {
            let line = json!({ "type": "review_completed", "runId": run_id, "review": review });
            println!("{line}");
        } else if let Some(review) = review {
            println!("{}", render_review_text(&review.output));
        }
        let _ = std::io::stdout().flush();
    }
    (code, Some(run_id))
}

/// The structured review of a finished run (the last assistant message).
pub fn final_review(state: &AppState, run_id: &str) -> Option<ReviewPayload> {
    state
        .db
        .list_messages(run_id)
        .iter()
        .rev()
        .find_map(review_api::review_of_message)
}

/// One dim stderr line per tool call the reviewer starts.
fn print_tool_progress(event: &Value) {
    let Some(message) = event.get("message") else { return };
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return;
    }
    let calls = message
        .get("rawResponse")
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str::<Vec<Value>>(raw).ok())
        .unwrap_or_default();
    for call in calls {
        let name = call.pointer("/function/name").and_then(Value::as_str).unwrap_or("tool");
        eprintln!("· {name}");
    }
}

/// Entry for `lcgn review`.
pub async fn review_command(
    state: &Arc<AppState>,
    project: &Path,
    model: Option<ResolvedModel>,
    thinking: Option<String>,
    args: &ReviewArgs,
    json_mode: bool,
) -> i32 {
    let Some(model) = model else {
        eprintln!("lcgn: no model specified. Use --model provider/model (see `lcgn --list-models`).");
        return 2;
    };
    let target = match target_from_args(args) {
        Ok(target) => target,
        Err(message) => {
            eprintln!("lcgn: {message}");
            return 2;
        }
    };
    run_review(state, project, &model, thinking, target, json_mode, None).await.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(base: Option<&str>, commit: Option<&str>, uncommitted: bool, prompt: &[&str]) -> ReviewArgs {
        ReviewArgs {
            base: base.map(str::to_string),
            commit: commit.map(str::to_string),
            uncommitted,
            prompt: prompt.iter().map(|s| s.to_string()).collect(),
            model: None,
            provider: None,
            thinking: None,
            cwd: None,
            mode: None,
        }
    }

    #[test]
    fn picks_target_from_flags() {
        assert_eq!(target_from_args(&args(None, None, false, &[])).unwrap(), ReviewTarget::Uncommitted);
        assert_eq!(target_from_args(&args(None, None, true, &[])).unwrap(), ReviewTarget::Uncommitted);
        assert_eq!(
            target_from_args(&args(Some("main"), None, false, &[])).unwrap(),
            ReviewTarget::BaseBranch { branch: "main".into() }
        );
        assert_eq!(
            target_from_args(&args(None, Some("abc123"), false, &[])).unwrap(),
            ReviewTarget::Commit { sha: "abc123".into(), title: None }
        );
        assert_eq!(
            target_from_args(&args(None, None, false, &["check", "error", "handling"])).unwrap(),
            ReviewTarget::Custom { instructions: "check error handling".into() }
        );
    }

    #[test]
    fn parses_slash_arguments() {
        assert_eq!(target_from_slash("").unwrap(), ReviewTarget::Uncommitted);
        assert_eq!(
            target_from_slash("base origin/main").unwrap(),
            ReviewTarget::BaseBranch { branch: "origin/main".into() }
        );
        assert_eq!(
            target_from_slash("commit abc").unwrap(),
            ReviewTarget::Commit { sha: "abc".into(), title: None }
        );
        assert_eq!(
            target_from_slash("look at the parser").unwrap(),
            ReviewTarget::Custom { instructions: "look at the parser".into() }
        );
        assert!(target_from_slash("base").is_err());
        assert!(target_from_slash("commit  ").is_err());
    }
}
