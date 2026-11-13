//! Performs the UI's [`Action`]s against the core API: starting turns,
//! answering prompts, cancelling, slash commands and file search.

use super::app::{context_left_percent, Action, App, UsageInfo};
use super::popup::PopupItem;
use super::transcript::CellKind;
use crate::api::runs as runs_api;
use crate::api::runs::{ContinueRunPayload, CreateRunPayload};
use crate::cli::args::assemble_prompt;
use crate::cli::auth;
use crate::cli::models::{list_models_table, resolve_model};
use crate::orchestrator::RunHandle;
use crate::state::AppState;
use serde_json::Value;
use std::path::{Component, Path};
use std::sync::Arc;
use tokio::sync::broadcast::{error::RecvError, Receiver};
use tokio::sync::mpsc::UnboundedSender;

/// Session options that never change while the TUI runs.
pub struct DriverConfig {
    pub no_session: bool,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
}

/// File search results: `(query, matching paths)`.
pub type FileResults = (String, Vec<String>);

pub struct Driver {
    pub state: Arc<AppState>,
    pub app: App,
    rx: Option<Receiver<Value>>,
    handle: Option<Arc<RunHandle>>,
    config: DriverConfig,
    files_tx: UnboundedSender<FileResults>,
}

/// Turns composer text into the run prompt: `@path` mentions of existing
/// project files are attached (like the REPL), the text is kept verbatim.
pub fn build_prompt(text: &str, project: &Path) -> Result<String, String> {
    let mut messages: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let Some(relative) = word.strip_prefix('@') else {
            continue;
        };
        let relative = relative.trim_end_matches([',', '.', ';', ':', '!', '?', ')']);
        let inside_project = Path::new(relative)
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
        let mention = format!("@{relative}");
        if !relative.is_empty()
            && inside_project
            && project.join(relative).is_file()
            && !messages.contains(&mention)
        {
            messages.push(mention);
        }
    }
    // The leading space keeps `@file ...` text from being read as an attachment.
    messages.push(format!(" {text}"));
    assemble_prompt(&messages, project, None)
}

/// The `/review ` popup entries for `project`: uncommitted changes, every
/// other local branch and the newest commits (read-only git; empty outside a
/// repository).
pub fn review_target_items(project: &Path) -> Vec<PopupItem> {
    use crate::orchestrator::review as engine;
    if !engine::is_git_repo(project) {
        return Vec::new();
    }
    let mut items = Vec::new();
    if engine::has_uncommitted_changes(project) {
        items.push(PopupItem {
            value: "uncommitted".into(),
            label: "uncommitted".into(),
            detail: "staged, unstaged and untracked changes".into(),
        });
    }
    let current = engine::current_branch(project);
    for branch in engine::local_branches(project) {
        if current.as_deref() == Some(branch.as_str()) {
            continue;
        }
        items.push(PopupItem {
            value: format!("base {branch}"),
            label: format!("base {branch}"),
            detail: "changes against this branch".into(),
        });
    }
    for (sha, subject) in engine::recent_commits(project, 15) {
        let short: String = sha.chars().take(7).collect();
        items.push(PopupItem {
            value: format!("commit {sha}"),
            label: format!("commit {short}"),
            detail: subject,
        });
    }
    items
}

impl Driver {
    pub fn new(
        state: Arc<AppState>,
        mut app: App,
        config: DriverConfig,
        files_tx: UnboundedSender<FileResults>,
    ) -> Self {
        app.review_targets = review_target_items(&app.project);
        Self {
            state,
            app,
            rx: None,
            handle: None,
            config,
            files_tx,
        }
    }

    /// Next event of the active run; pending forever when idle.
    pub async fn recv_run(&mut self) -> Option<Result<Value, RecvError>> {
        match self.rx.as_mut() {
            Some(rx) => Some(rx.recv().await),
            None => std::future::pending().await,
        }
    }

    pub async fn on_run_recv(&mut self, received: Result<Value, RecvError>) {
        match received {
            Ok(event) => {
                self.app.on_run_event(&event);
                let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
                if matches!(kind, "round_started" | "run_completed" | "run_failed") {
                    self.refresh_usage();
                }
            }
            Err(RecvError::Lagged(_)) => {}
            Err(RecvError::Closed) => {
                self.app.on_run_closed();
                self.rx = None;
            }
        }
        self.settle_finished_run().await;
    }

    /// Drops per-run state once the app saw a terminal event, then sends
    /// any message that was queued but never folded into the run.
    async fn settle_finished_run(&mut self) {
        if !self.app.run_finished {
            return;
        }
        self.app.run_finished = false;
        self.rx = None;
        self.handle = None;
        if self.config.no_session {
            if let Some(id) = self.app.session.take() {
                self.state.db.delete_run(&id);
            }
        }
        self.refresh_usage();
        self.app.review_targets = review_target_items(&self.app.project);
        if let Some(prompt) = self.app.take_leftover_queued() {
            self.start_turn(prompt).await;
        }
    }

    pub async fn perform(&mut self, action: Action) {
        match action {
            Action::Submit(text) => self.submit(&text).await,
            Action::Queue(text) => self.queue(&text).await,
            Action::Slash(text) => self.slash(&text).await,
            Action::Decide(decision) => {
                if let Some(handle) = &self.handle {
                    runs_api::apply_permission_decision(&self.state, handle, decision).await;
                }
            }
            Action::Answer(answer) => {
                if let Some(handle) = &self.handle {
                    runs_api::answer_question_core(handle, answer.selections, answer.notes).await;
                }
            }
            Action::Cancel => self.cancel().await,
            Action::Quit => self.quit().await,
            Action::SearchFiles(query) => self.search_files(query),
        }
    }

    async fn quit(&mut self) {
        if self.app.busy {
            self.cancel().await;
        }
        self.app.should_quit = true;
    }

    async fn cancel(&mut self) {
        if let Some(id) = self.app.session.clone() {
            let _ = runs_api::cancel_run(&self.state, &id).await;
        }
    }

    fn search_files(&self, query: String) {
        let state = self.state.clone();
        let project = self.app.project.display().to_string();
        let tx = self.files_tx.clone();
        tokio::spawn(async move {
            let files = crate::api::projects::list_files(&state, &project, Some(&query))
                .await
                .unwrap_or_default();
            let _ = tx.send((query, files));
        });
    }

    // ---- turns -----------------------------------------------------------

    async fn submit(&mut self, text: &str) {
        match build_prompt(text, &self.app.project) {
            Ok(prompt) => self.start_turn(prompt).await,
            Err(message) => self.app.push_error(message),
        }
    }

    async fn queue(&mut self, text: &str) {
        let Some(run_id) = self.app.session.clone() else {
            self.app.push_error("No active run to queue the message on.");
            return;
        };
        let prompt = match build_prompt(text, &self.app.project) {
            Ok(prompt) => prompt,
            Err(message) => return self.app.push_error(message),
        };
        match runs_api::queue_message(&self.state, &run_id, &prompt).await {
            Ok(()) => self.app.note_queued(text, &prompt),
            Err(err) => self.app.push_error(err.message),
        }
    }

    async fn start_turn(&mut self, prompt: String) {
        if let Some(model) = &self.app.model {
            if !self.state.pi.has_api_key(&model.provider_id) && model.provider_id != "ollama" {
                let message = format!(
                    "Provider \"{0}\" is not authenticated. Run `lcgn auth login {0}`.",
                    model.provider_id
                );
                return self.app.push_error(message);
            }
        }
        let started = match self.app.session.clone() {
            None => self.create_run(prompt).await,
            Some(id) => self.continue_run(&id, prompt).await,
        };
        match started {
            Ok((run_id, rx, handle)) => {
                self.app.session = Some(run_id);
                self.rx = Some(rx);
                self.handle = Some(handle);
                self.app.begin_turn();
            }
            Err(message) => self.app.push_error(message),
        }
    }

    async fn create_run(
        &self,
        prompt: String,
    ) -> Result<(String, Receiver<Value>, Arc<RunHandle>), String> {
        let model = self.app.model.clone().ok_or_else(|| {
            "No model selected. Use /model provider/id (type /model to list models).".to_string()
        })?;
        crate::cli::runner::ensure_project(&self.state, &self.app.project);
        let payload = CreateRunPayload {
            task: prompt,
            project_path: Some(self.app.project.display().to_string()),
            provider_id: model.provider_id,
            model: model.model,
            reasoning_effort: self.app.thinking.clone(),
            mode: Some(self.app.mode.clone()),
            bypass_permissions: self.app.bypass,
            system_prompt: self.config.system_prompt.clone(),
            append_system_prompt: self.config.append_system_prompt.clone(),
            ..Default::default()
        };
        match runs_api::create_run(&self.state, &payload).await {
            Ok(started) => Ok((started.run.id, started.events, started.handle)),
            Err(err) => Err(err.message),
        }
    }

    async fn continue_run(
        &self,
        id: &str,
        prompt: String,
    ) -> Result<(String, Receiver<Value>, Arc<RunHandle>), String> {
        let payload = ContinueRunPayload {
            task: prompt,
            provider_id: self.app.model.as_ref().map(|m| m.provider_id.clone()),
            model: self.app.model.as_ref().map(|m| m.model.clone()),
            reasoning_effort: self.app.thinking.clone(),
            mode: Some(self.app.mode.clone()),
            bypass_permissions: self.app.bypass,
            system_prompt: self.config.system_prompt.clone(),
            append_system_prompt: self.config.append_system_prompt.clone(),
            ..Default::default()
        };
        match runs_api::continue_run(&self.state, id, &payload).await {
            Ok(continued) => Ok((id.to_string(), continued.events, continued.handle)),
            Err(err) => Err(err.message),
        }
    }

    // ---- usage -------------------------------------------------------------

    pub fn refresh_usage(&mut self) {
        let Some(run_id) = self.app.session.clone() else {
            self.app.set_usage(UsageInfo::default());
            return;
        };
        let entries = self.state.db.list_usage(Some(&run_id));
        let total_tokens = entries.iter().map(|e| e.input_tokens + e.output_tokens).sum();
        let cost = entries.iter().map(|e| e.cost).sum();
        let last_main = entries
            .iter()
            .rev()
            .find(|e| e.agent_role.as_deref().is_none_or(|role| role == "planner"));
        let context_left = last_main.and_then(|entry| {
            let window = self
                .state
                .pi
                .model_info(&entry.provider_id, &entry.model)?
                .context_window?;
            Some(context_left_percent(entry.input_tokens, window))
        });
        self.app.set_usage(UsageInfo {
            total_tokens,
            cost,
            context_left,
        });
    }

    // ---- resume --------------------------------------------------------------

    /// Loads the persisted transcript and run details of `run_id`.
    pub async fn load_session(&mut self, run_id: &str) {
        if let Ok(run) = runs_api::get_run(&self.state, run_id).await {
            self.app.title = Some(run.title.clone()).filter(|t| !t.trim().is_empty());
            if self.app.model.is_none() {
                self.app.model = Some(crate::cli::models::ResolvedModel {
                    provider_id: run.provider_id,
                    model: run.model,
                });
            }
        }
        if let Ok(messages) = runs_api::list_messages(&self.state, run_id).await {
            let values: Vec<Value> = messages
                .iter()
                .filter_map(|m| serde_json::to_value(m).ok())
                .collect();
            self.app.load_history(&values);
        }
        self.refresh_usage();
    }

    // ---- slash commands ----------------------------------------------------------

    async fn slash(&mut self, command: &str) {
        let mut parts = command.split_whitespace();
        let head = parts.next().unwrap_or("").to_lowercase();
        let rest: Vec<&str> = parts.collect();
        let project = self.app.project.display().to_string();
        match (head.as_str(), rest.as_slice()) {
            ("/exit" | "/quit", _) => self.quit().await,
            ("/help", _) => self.app.push_notice(App::help_text()),
            ("/plan", _) => {
                self.app.set_build_mode(true);
                self.app.push_notice("Mode set to plan (read-only).");
            }
            ("/build", _) => {
                self.app.set_build_mode(false);
                self.app.push_notice(format!("Mode set to {}.", self.app.mode_label().to_lowercase()));
            }
            ("/thinking", []) => {
                let level = self.app.thinking.clone().unwrap_or_else(|| "off".into());
                self.app.push_notice(format!("Thinking is {level}."));
            }
            ("/thinking", [level]) => self.set_thinking(level),
            ("/model", []) => {
                let table = list_models_table(&self.state.pi, "").await;
                self.app.push_notice(table.trim_end().to_string());
            }
            ("/model", [target]) => self.set_model(target).await,
            ("/diff", _) => self.show_diff(&project).await,
            ("/review", _) if self.app.busy => {
                self.app.push_notice("Wait for the current run to finish (or press Esc to interrupt).");
            }
            ("/review", _) => self.start_review(command.trim_start_matches("/review").trim()).await,
            ("/mcp", _) => self.show_mcp(&project).await,
            ("/skills", _) => self.show_skills(&project).await,
            ("/sessions", _) => self.show_sessions(&project),
            ("/usage", _) => self.show_usage(),
            ("/new" | "/fork" | "/undo", _) if self.app.busy => {
                self.app.push_notice("Wait for the current run to finish (or press Esc to interrupt).");
            }
            ("/new", _) => self.new_session(),
            ("/fork", _) => self.fork_session().await,
            ("/undo", _) => self.undo(&project).await,
            (other, _) => self.app.push_error(format!("Unknown command \"{other}\". Type /help.")),
        }
    }

    fn set_thinking(&mut self, level: &str) {
        let level = level.to_lowercase();
        if !["off", "minimal", "low", "medium", "high", "xhigh", "max"].contains(&level.as_str()) {
            return self.app.push_error(format!("Unknown thinking level \"{level}\"."));
        }
        self.app.thinking = (level != "off").then(|| level.clone());
        self.app.push_notice(format!("Thinking set to {level}."));
    }

    async fn set_model(&mut self, target: &str) {
        match resolve_model(&self.state.pi, Some(target), None).await {
            Ok(resolved) => {
                if self.state.pi.has_api_key(&resolved.provider_id) || resolved.provider_id == "ollama" {
                    self.app.push_notice(format!("Model set to {}.", resolved.label()));
                    self.app.model = Some(resolved);
                } else {
                    self.app.push_error(format!(
                        "Provider \"{0}\" is not authenticated. Run `lcgn auth login {0}`.",
                        resolved.provider_id
                    ));
                }
            }
            Err(message) => {
                let table = list_models_table(&self.state.pi, target).await;
                self.app.push_error(message);
                if !table.trim().is_empty() {
                    self.app.push_notice(table.trim_end().to_string());
                }
            }
        }
    }

    async fn show_diff(&mut self, project: &str) {
        match crate::api::projects::git_diff(&self.state, project, false).await {
            Ok(diff) if diff.trim().is_empty() => self.app.push_notice("Working tree clean."),
            Ok(diff) => self.app.push(CellKind::Diff {
                title: "Working tree diff".into(),
                diff,
            }),
            Err(err) => self.app.push_error(format!("diff failed: {err}")),
        }
    }

    /// `/review [target]`: starts a read-only review run and switches the
    /// transcript to it (the review becomes the current session, so
    /// follow-up messages continue it).
    async fn start_review(&mut self, argument: &str) {
        let target = match crate::cli::review::target_from_slash(argument) {
            Ok(target) => target,
            Err(message) => return self.app.push_error(message),
        };
        let Some(model) = self.app.model.clone() else {
            return self.app.push_error("No model selected. Use /model provider/id (type /model to list models).");
        };
        if !self.state.pi.has_api_key(&model.provider_id) && model.provider_id != "ollama" {
            return self.app.push_error(format!(
                "Provider \"{0}\" is not authenticated. Run `lcgn auth login {0}`.",
                model.provider_id
            ));
        }
        crate::cli::runner::ensure_project(&self.state, &self.app.project);
        let payload = crate::api::review::StartReviewPayload {
            target: target.clone(),
            project_path: Some(self.app.project.display().to_string()),
            provider_id: model.provider_id,
            model: model.model,
            reasoning_effort: self.app.thinking.clone(),
        };
        match crate::api::review::start_review(&self.state, &payload).await {
            Ok(started) => {
                self.app.title = None;
                self.app.cells.clear();
                self.app.set_usage(UsageInfo::default());
                self.app.viewport.follow = true;
                self.app.push_notice(format!("Reviewing {} (read-only).", target.hint()));
                self.app.session = Some(started.run.id);
                self.rx = Some(started.events);
                self.handle = Some(started.handle);
                self.app.begin_turn();
            }
            Err(err) => self.app.push_error(err.message),
        }
    }

    async fn show_mcp(&mut self, project: &str) {
        match crate::api::mcp::list_mcp_servers(&self.state, Some(project)).await {
            Ok(response) if response.servers.is_empty() => self.app.push_notice("No MCP servers configured."),
            Ok(response) => {
                let lines: Vec<String> = response
                    .servers
                    .iter()
                    .map(|s| format!("{} — {} ({})", s.config.name, s.config.transport, s.status))
                    .collect();
                self.app.push_notice(lines.join("\n"));
            }
            Err(err) => self.app.push_error(format!("mcp list failed: {err}")),
        }
    }

    async fn show_skills(&mut self, project: &str) {
        match crate::api::skills::list_skills(&self.state, Some(project)).await {
            Ok(response) if response.skills.is_empty() => self.app.push_notice("No skills discovered."),
            Ok(response) => {
                let lines: Vec<String> = response
                    .skills
                    .iter()
                    .map(|s| format!("${} — {} [{}]", s.name, s.description, s.source))
                    .collect();
                self.app.push_notice(lines.join("\n"));
            }
            Err(err) => self.app.push_error(format!("skills list failed: {err}")),
        }
    }

    fn show_sessions(&mut self, project: &str) {
        let runs = auth::sessions_for(&self.state, Some(project));
        if runs.is_empty() {
            return self.app.push_notice("No sessions for this directory yet.");
        }
        let lines: Vec<String> = runs
            .iter()
            .take(15)
            .map(|run| {
                let id: String = run.id.chars().take(20).collect();
                let title = run.title.lines().next().unwrap_or("").trim();
                format!("{id}  {:<10} {title}", run.status)
            })
            .collect();
        self.app.push_notice(lines.join("\n"));
    }

    fn show_usage(&mut self) {
        let Some(run_id) = self.app.session.clone() else {
            return self.app.push_notice("No active session yet.");
        };
        let entries = self.state.db.list_usage(Some(&run_id));
        if entries.is_empty() {
            return self.app.push_notice("No usage recorded for this session yet.");
        }
        let input: i64 = entries.iter().map(|e| e.input_tokens).sum();
        let output: i64 = entries.iter().map(|e| e.output_tokens).sum();
        let cost: f64 = entries.iter().map(|e| e.cost).sum();
        self.app.push_notice(format!(
            "{} call(s) · {input} in / {output} out · ${cost:.4}",
            entries.len()
        ));
    }

    fn new_session(&mut self) {
        self.app.session = None;
        self.app.title = None;
        self.app.cells.clear();
        self.app.set_usage(UsageInfo::default());
        self.app.viewport.follow = true;
        self.app.push_notice("Next message starts a new session.");
    }

    async fn fork_session(&mut self) {
        let Some(run_id) = self.app.session.clone() else {
            return self.app.push_notice("No active session yet - send a message first.");
        };
        match runs_api::fork_run(&self.state, &run_id, None).await {
            Ok(forked) => {
                self.app.push_notice(format!("Forked {run_id} -> {}", forked.id));
                self.app.session = Some(forked.id);
            }
            Err(err) => self.app.push_error(format!("fork failed: {err}")),
        }
    }

    async fn undo(&mut self, project: &str) {
        let session = self.app.session.clone();
        match crate::api::projects::undo_last_turn(&self.state, project, session.as_deref()).await {
            Ok(snapshot) => {
                self.app.push_notice(format!(
                    "Restored snapshot {} (HEAD {}, {} file(s)).",
                    snapshot.id,
                    snapshot.head_sha,
                    snapshot.restored.len()
                ));
                if !snapshot.skipped.is_empty() {
                    self.app.push_notice(format!(
                        "Left alone (changed after that turn): {}",
                        snapshot.skipped.join(", ")
                    ));
                }
            }
            Err(err) => self.app.push_error(format!("undo failed: {err}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lcgn-tui-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "fn a() {}").unwrap();
        dir
    }

    #[test]
    fn mentions_of_existing_files_are_attached_once() {
        let dir = project();
        let prompt = build_prompt("explain @src/a.rs, then @src/a.rs again\nplease", &dir).unwrap();
        assert!(prompt.starts_with("## Attached file: src/a.rs"));
        assert_eq!(prompt.matches("## Attached file:").count(), 1);
        assert!(prompt.contains("explain @src/a.rs, then @src/a.rs again\nplease"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_or_escaping_mentions_stay_plain_text() {
        let dir = project();
        let prompt = build_prompt("ping @alice and read @../secret.txt", &dir).unwrap();
        assert_eq!(prompt, "ping @alice and read @../secret.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_starting_with_a_mention_is_not_an_attachment_path() {
        let dir = project();
        let prompt = build_prompt("@src/a.rs explain", &dir).unwrap();
        assert!(prompt.contains("fn a() {}"));
        assert!(prompt.trim_end().ends_with("@src/a.rs explain"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
