//! Performs the UI's [`Action`]s against the core API: starting turns,
//! answering prompts, cancelling, slash commands and file search.

use super::app::{context_left_percent, Action, App, UsageInfo};
use super::popup::PopupItem;
use super::transcript::CellKind;
use crate::api::runs as runs_api;
use crate::api::runs::{ContinueRunPayload, CreateRunPayload};
use crate::cli::commands::{self, ensure_authenticated, CommandSession, Output};
use crate::commands::SlashCommand;
use crate::cli::events::{Pumped, RunEvents};
use crate::cli::prompt::build_prompt;
use crate::orchestrator::RunHandle;
use crate::state::AppState;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::broadcast::Receiver;
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
    rx: Option<RunEvents>,
    handle: Option<Arc<RunHandle>>,
    config: DriverConfig,
    files_tx: UnboundedSender<FileResults>,
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
    pub async fn recv_run(&mut self) -> Option<Pumped> {
        match self.rx.as_mut() {
            Some(events) => Some(events.next().await),
            None => std::future::pending().await,
        }
    }

    pub async fn on_run_recv(&mut self, received: Pumped) {
        match received {
            Pumped::Event(event) => {
                self.app.on_run_event(&event);
                let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
                if matches!(kind, "round_started" | "run_completed" | "run_failed") {
                    self.refresh_usage();
                }
            }
            // The receiver lagged: re-apply the persisted messages so the
            // transcript catches up; pending prompts and a terminal event
            // follow as ordinary events (QA-046).
            Pumped::Resynced(resync) => {
                let values: Vec<Value> = resync.messages.iter().filter_map(|m| serde_json::to_value(m).ok()).collect();
                self.app.load_history(&values);
                self.refresh_usage();
            }
            Pumped::Closed => {
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
                let _ = runs_api::delete_run(&self.state, &id).await;
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
            if let Err(message) = ensure_authenticated(&self.state, model) {
                return self.app.push_error(message);
            }
        }
        let started = match self.app.session.clone() {
            None => self.create_run(prompt).await,
            Some(id) => self.continue_run(&id, prompt).await,
        };
        match started {
            Ok((run_id, rx, handle)) => {
                self.rx = Some(RunEvents::new(self.state.clone(), run_id.clone(), rx));
                self.app.session = Some(run_id);
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
            no_session: self.config.no_session,
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
        let totals = crate::api::runs::UsageTotals::of(&entries);
        let last_main = entries
            .iter()
            .rev()
            .find(|e| e.role() == crate::db::MAIN_AGENT_ROLE);
        let context_left = last_main.and_then(|entry| {
            let window = self
                .state
                .pi
                .model_info(&entry.provider_id, &entry.model)?
                .context_window?;
            Some(context_left_percent(entry.input_tokens, window))
        });
        self.app.set_usage(UsageInfo {
            total_tokens: totals.input_tokens + totals.output_tokens,
            cost: totals.cost,
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

    /// Runs a slash command through the shared executor and applies the
    /// result to the UI; only help text, review streaming and quitting are
    /// TUI specific.
    async fn slash(&mut self, line: &str) {
        let Some(command) = SlashCommand::parse(line) else { return };
        if command == SlashCommand::Help {
            return self.app.push_notice(App::help_text());
        }
        let mut session = CommandSession {
            model: self.app.model.clone(),
            thinking: self.app.thinking.clone(),
            mode: self.app.mode.clone(),
            bypass: self.app.bypass,
            session: self.app.session.clone(),
            busy: self.app.busy,
        };
        let project = self.app.project.clone();
        let result = commands::execute(&self.state, &project, command, &mut session).await;

        self.app.model = session.model;
        self.app.thinking = session.thinking;
        if self.app.mode != session.mode {
            self.app.mode = session.mode;
            self.app.dirty = true;
        }
        self.app.session = session.session;
        if result.reset_view {
            self.app.title = None;
            self.app.cells.clear();
            self.app.set_usage(UsageInfo::default());
            self.app.viewport.follow = true;
        }
        for output in result.outputs {
            match output {
                Output::Notice(text) => self.app.push_notice(text),
                Output::Error(text) => self.app.push_error(text),
                Output::Table(text) => self.app.push_notice(text.trim_end().to_string()),
                Output::Diff(diff) => self.app.push(CellKind::Diff { title: "Working tree diff".into(), diff }),
            }
        }
        if let Some(target) = result.review {
            self.start_review(&target).await;
        }
        if result.exit {
            self.quit().await;
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
        if let Err(message) = ensure_authenticated(&self.state, &model) {
            return self.app.push_error(message);
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
                self.rx = Some(RunEvents::new(self.state.clone(), started.run.id.clone(), started.events));
                self.app.session = Some(started.run.id);
                self.handle = Some(started.handle);
                self.app.begin_turn();
            }
            Err(err) => self.app.push_error(err.message),
        }
    }
}
