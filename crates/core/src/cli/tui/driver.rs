//! Performs the UI's [`Action`]s against the core API: starting turns,
//! answering prompts, cancelling, slash commands and file search.

use super::app::{Action, App, UsageInfo};
use super::login::{LoginAction, LoginFlow, LoginRow, Stage, WaitInfo};
use super::overlay::{ModelPicker, Overlay};
use super::popup::PopupItem;
use super::sessions::SessionPicker;
use super::transcript::CellKind;
use crate::api::providers as provider_api;
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
use tokio::sync::mpsc::{Sender, UnboundedSender};

/// Session options that never change while the TUI runs.
pub struct DriverConfig {
    pub no_session: bool,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
}

/// File search results: `(query, matching paths)`.
pub type FileResults = (String, Vec<String>);

/// Progress of an OAuth sign-in running on a background task.
pub enum LoginEvent {
    Started(Result<Box<provider_api::OAuthLoginStart>, String>),
    Finished(Result<(), String>),
}

pub struct Driver {
    pub state: Arc<AppState>,
    pub app: App,
    rx: Option<RunEvents>,
    handle: Option<Arc<RunHandle>>,
    config: DriverConfig,
    files_tx: UnboundedSender<FileResults>,
    login_tx: UnboundedSender<LoginEvent>,
    /// Tells the event loop to reload model choices (a list was refreshed).
    models_tx: Option<Sender<()>>,
    login_task: Option<tokio::task::JoinHandle<()>>,
    login_start: Option<provider_api::OAuthLoginStart>,
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

/// How long quitting waits for a cancelled run to wind down.
const QUIT_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

impl Driver {
    pub fn new(
        state: Arc<AppState>,
        mut app: App,
        config: DriverConfig,
        files_tx: UnboundedSender<FileResults>,
        login_tx: UnboundedSender<LoginEvent>,
    ) -> Self {
        app.review_targets = review_target_items(&app.project);
        app.branch = crate::orchestrator::review::current_branch(&app.project);
        let mut driver = Self {
            state,
            app,
            rx: None,
            handle: None,
            config,
            files_tx,
            login_tx,
            models_tx: None,
            login_task: None,
            login_start: None,
        };
        driver.seed_prompt_history();
        driver
    }

    pub fn set_models_notifier(&mut self, tx: Sender<()>) {
        self.models_tx = Some(tx);
    }

    /// Earlier prompts of this folder's sessions feed the Up arrow and Ctrl+R.
    fn seed_prompt_history(&mut self) {
        let project = self.app.project.display().to_string();
        let ids: std::collections::HashSet<String> = crate::cli::auth::sessions_for(&self.state, Some(&project))
            .into_iter()
            .map(|run| run.id)
            .collect();
        let prompts = self.state.db.prompt_history(&ids, 100);
        self.app.seed_history(prompts);
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
        self.app.branch = crate::orchestrator::review::current_branch(&self.app.project);
        if self.app.should_quit {
            return;
        }
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
            Action::Resume(id) => self.resume_session(&id).await,
            Action::Login(action) => self.login_action(action).await,
            Action::Withdraw => self.withdraw_queued().await,
        }
    }

    /// `Up` while a run is going: take the newest queued message back.
    async fn withdraw_queued(&mut self) {
        let (Some(handle), Some(content)) = (self.handle.clone(), self.app.last_queued_content().map(str::to_string))
        else {
            return;
        };
        if crate::api::queue::withdraw_queued_message(&handle, &content).await {
            self.app.unqueue_last();
        } else {
            self.app.flash("The run already picked that message up.");
        }
    }

    async fn quit(&mut self) {
        self.app.should_quit = true;
        if !self.app.busy {
            return;
        }
        self.cancel().await;
        // Let the run reach its terminal event so cleanup (the `--no-session`
        // delete, closing agents) happens before the process exits.
        let deadline = tokio::time::Instant::now() + QUIT_GRACE;
        while self.rx.is_some() {
            match tokio::time::timeout_at(deadline, self.recv_run()).await {
                Ok(Some(received)) => self.on_run_recv(received).await,
                _ => break,
            }
        }
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
            if let Err(message) = ensure_authenticated(&self.state, model).await {
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
            auto_title: true,
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
            images: Vec::new(),
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
        let context = last_main.and_then(|entry| {
            let window = self
                .state
                .pi
                .model_info(&entry.provider_id, &entry.model)?
                .context_window?;
            Some((entry.input_tokens + entry.output_tokens, window))
        });
        self.app.set_usage(UsageInfo {
            cost: totals.cost,
            context,
        });
    }

    // ---- resume --------------------------------------------------------------

    /// Loads the persisted transcript and run details of `run_id`.
    /// Fills the `/model ` popup with every model of the callable providers.
    pub async fn load_model_choices(&mut self) {
        self.reload_model_choices().await;
        self.app.no_providers = self.app.model_choices.is_empty();
        self.app.dirty = true;
    }

    async fn reload_model_choices(&mut self) {
        // Models switched off in the desktop's Settings → Providers stay hidden.
        let hidden = crate::api::providers::hidden_models(&self.state);
        self.app.model_choices = self
            .state
            .pi
            .list_providers()
            .await
            .into_iter()
            .flat_map(|provider| {
                let name = provider.display_name.clone();
                provider.models.into_iter().map(move |model| PopupItem {
                    value: format!("{}/{model}", provider.id),
                    label: format!("{}/{model}", provider.id),
                    detail: name.clone(),
                })
            })
            .filter(|item| !hidden.contains(&item.value))
            .collect();
    }

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
        match &command {
            SlashCommand::Help => return self.app.open_help(),
            SlashCommand::Sessions => return self.open_session_picker(),
            SlashCommand::Login(provider) => return self.open_login(provider.as_deref()).await,
            SlashCommand::Copy(what) => return self.copy_last_answer(what.as_deref()),
            SlashCommand::Thinking(None) => return self.app.open_effort_picker(),
            SlashCommand::Permissions(None) => return self.app.open_mode_picker(),
            _ => {}
        }
        if command == SlashCommand::Model(None) {
            // Like pi: a searchable picker instead of printing the table.
            self.load_model_choices().await;
            if self.app.model_choices.is_empty() {
                return self.app.push_error("No models available. Type /login to connect a provider.");
            }
            let current = self.app.model.as_ref().map(|model| model.label());
            self.app.overlay = Some(Overlay::ModelPicker(ModelPicker::new(self.app.model_choices.clone(), current)));
            self.app.dirty = true;
            return;
        }
        // Setting changes show in the footer; confirm them in the status line
        // instead of adding a transcript line.
        let quiet = matches!(
            command,
            SlashCommand::Plan
                | SlashCommand::Build
                | SlashCommand::Auto
                | SlashCommand::Chat
                | SlashCommand::Thinking(_)
                | SlashCommand::Permissions(_)
                | SlashCommand::Model(Some(_))
        );
        let switching_model = matches!(command, SlashCommand::Model(Some(_)));
        let previous_model = self.app.model.as_ref().map(|model| model.label());
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
            self.app.auto_used |= self.app.mode == "auto";
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
                Output::Notice(text) if quiet => self.app.flash(text),
                Output::Notice(text) => self.app.push_notice(text),
                Output::Error(text) => self.app.push_error(text),
                Output::Table(text) => self.app.push_notice(text.trim_end().to_string()),
                Output::Diff(diff) => self.app.push(CellKind::Diff { title: "Working tree diff".into(), diff }),
            }
        }
        if switching_model && self.app.model.as_ref().map(|model| model.label()) != previous_model {
            self.offer_effort_picker();
        }
        if let Some(target) = result.review {
            self.start_review(&target).await;
        }
        if result.exit {
            self.quit().await;
        }
    }

    /// After a model switch (Codex: model, then effort): models with
    /// adjustable reasoning get the effort picker, others skip it.
    fn offer_effort_picker(&mut self) {
        let Some(model) = self.app.model.as_ref() else { return };
        let adjustable = self.state.pi.model_info(&model.provider_id, &model.model).and_then(|info| info.reasoning);
        if adjustable == Some(true) {
            self.app.open_effort_picker();
        }
    }

    // ---- /resume ---------------------------------------------------------------

    fn open_session_picker(&mut self) {
        if self.app.busy {
            return self.app.push_notice(commands::BUSY_NOTICE);
        }
        let project = self.app.project.display().to_string();
        let runs = crate::cli::auth::sessions_for(&self.state, Some(&project));
        if runs.is_empty() {
            return self.app.push_notice("No sessions for this folder yet.");
        }
        let picker = SessionPicker::new(&runs, self.app.session.clone(), chrono::Utc::now());
        self.app.overlay = Some(Overlay::Sessions(picker));
        self.app.dirty = true;
    }

    /// Opens the session picker at start (`lcgn -r`).
    pub fn open_resume_picker(&mut self) {
        self.open_session_picker();
    }

    async fn resume_session(&mut self, run_id: &str) {
        if self.app.busy {
            return self.app.push_notice(commands::BUSY_NOTICE);
        }
        self.app.title = None;
        self.app.cells.clear();
        self.app.set_usage(UsageInfo::default());
        self.app.viewport.follow = true;
        self.app.session = Some(run_id.to_string());
        self.load_session(run_id).await;
    }

    // ---- /copy -------------------------------------------------------------------

    fn copy_last_answer(&mut self, what: Option<&str>) {
        let answer = self.app.cells.iter().rev().find_map(|cell| match cell.kind() {
            CellKind::Assistant { text } if cell.msg_id.as_deref() != Some("__plan__") => Some(text.clone()),
            _ => None,
        });
        let Some(answer) = answer else {
            return self.app.flash("Nothing to copy yet.");
        };
        let text = match what.map(str::to_lowercase).as_deref() {
            None => answer,
            Some("code") => match super::clipboard::last_fenced_block(&answer) {
                Some(code) => code,
                None => return self.app.flash("The last answer has no code block."),
            },
            Some(other) => return self.app.push_error(format!("Unknown /copy option \"{other}\". Use /copy or /copy code.")),
        };
        let label = super::clipboard::line_count_label(&text);
        if super::clipboard::copy_text(&text) {
            self.app.flash(format!("Copied {label}."));
        } else {
            self.app.flash("Could not reach the clipboard.");
        }
    }

    // ---- /login ------------------------------------------------------------------

    fn login_rows(&self) -> Vec<LoginRow> {
        self.state.pi.login_providers().iter().map(LoginRow::from_provider).collect()
    }

    async fn open_login(&mut self, provider: Option<&str>) {
        let rows = self.login_rows();
        let flow = match provider.map(str::trim).filter(|p| !p.is_empty()) {
            None => LoginFlow::new(rows),
            Some(id) => match LoginFlow::for_provider(rows, id) {
                Some(flow) => flow,
                None => return self.app.push_error(format!("Unknown provider \"{id}\". Type /login to pick one.")),
            },
        };
        let begin = flow.begin_action();
        self.app.overlay = Some(Overlay::Login(flow));
        self.app.dirty = true;
        if let Some(action) = begin {
            self.login_action(action).await;
        }
    }

    pub async fn login_action(&mut self, action: LoginAction) {
        match action {
            LoginAction::Close => self.app.overlay = None,
            LoginAction::Cancel => {
                self.stop_login_task();
                self.app.overlay = None;
                self.app.flash("Sign-in cancelled.");
            }
            LoginAction::Note(text) => {
                self.app.overlay = None;
                self.app.flash(text);
            }
            LoginAction::BeginOauth { provider, domain } => self.begin_oauth(provider, domain),
            LoginAction::SaveKey { provider, key } => self.save_api_key(&provider, key).await,
            LoginAction::OpenBrowser => {
                if let Some(start) = &self.login_start {
                    let note = match provider_api::open_verification_url(start.url_to_open()) {
                        Ok(()) => "Opened in your browser.".to_string(),
                        Err(err) => format!("Could not open the browser: {err}"),
                    };
                    self.app.flash(note);
                }
            }
            LoginAction::CopyCode => {
                let code = self.login_start.as_ref().and_then(|start| start.user_code.clone());
                if let Some(code) = code {
                    let copied = super::clipboard::copy_text(&code);
                    self.app.flash(if copied { "Code copied." } else { "Could not reach the clipboard." });
                }
            }
            LoginAction::SubmitRedirect(url) => self.submit_redirect(&url).await,
        }
        self.app.dirty = true;
    }

    fn stop_login_task(&mut self) {
        if let Some(task) = self.login_task.take() {
            task.abort();
        }
        self.login_start = None;
    }

    /// Starts the sign-in on a background task: it reports the code or URL
    /// first, then the outcome, through `login_tx`.
    fn begin_oauth(&mut self, provider: String, domain: Option<String>) {
        self.stop_login_task();
        let state = self.state.clone();
        let tx = self.login_tx.clone();
        self.login_task = Some(tokio::spawn(async move {
            let start = provider_api::start_oauth_login_with_domain(&state, &provider, domain.as_deref()).await;
            let start = match start {
                Ok(start) => start,
                Err(err) => {
                    let _ = tx.send(LoginEvent::Started(Err(err.to_string())));
                    return;
                }
            };
            let _ = tx.send(LoginEvent::Started(Ok(Box::new(start.clone()))));
            let outcome = provider_api::finish_oauth_login(&state, &start).await.map_err(|err| err.to_string());
            let _ = tx.send(LoginEvent::Finished(outcome));
        }));
    }

    /// An OAuth task event reached the event loop.
    pub async fn on_login_event(&mut self, event: LoginEvent) {
        let waiting = matches!(&self.app.overlay, Some(Overlay::Login(flow)) if matches!(flow.stage, Stage::Waiting { .. }));
        if !waiting {
            return; // cancelled meanwhile
        }
        match event {
            LoginEvent::Started(Ok(start)) => {
                let info = WaitInfo {
                    code: start.user_code.clone(),
                    url: start.url_to_open().to_string(),
                    accepts_paste: start.user_code.is_none(),
                    paste: String::new(),
                    error: None,
                };
                self.login_start = Some(*start);
                if let Some(Overlay::Login(flow)) = self.app.overlay.as_mut() {
                    flow.set_started(info);
                }
            }
            LoginEvent::Started(Err(message)) | LoginEvent::Finished(Err(message)) => {
                self.stop_login_task();
                if let Some(Overlay::Login(flow)) = self.app.overlay.as_mut() {
                    flow.fail(message);
                }
            }
            LoginEvent::Finished(Ok(())) => {
                let provider = self.login_start.take().map(|start| start.provider_id);
                self.login_task = None;
                self.after_login(provider.as_deref().unwrap_or("")).await;
            }
        }
        self.app.dirty = true;
    }

    async fn submit_redirect(&mut self, pasted: &str) {
        let Some(start) = self.login_start.clone() else { return };
        match provider_api::submit_oauth_redirect(&self.state, &start, pasted).await {
            Ok(()) => {
                self.stop_login_task();
                self.after_login(&start.provider_id).await;
            }
            Err(err) => {
                if let Some(Overlay::Login(flow)) = self.app.overlay.as_mut() {
                    flow.set_wait_error(err.to_string());
                }
            }
        }
    }

    /// Stores a pasted API key (the same call as `lcgn auth login`).
    async fn save_api_key(&mut self, provider: &str, key: String) {
        let input = crate::pi_providers::SetPiAuthInput {
            provider_id: provider.to_string(),
            action: "set".to_string(),
            api_key: Some(key),
        };
        match self.state.pi.set_pi_auth(&input) {
            Ok(()) => self.after_login(provider).await,
            Err(err) => {
                if let Some(Overlay::Login(flow)) = self.app.overlay.as_mut() {
                    flow.fail(err.to_string());
                }
            }
        }
    }

    /// A provider is connected: reload the models and open the picker on it.
    async fn after_login(&mut self, provider: &str) {
        self.app.overlay = None;
        self.load_model_choices().await;
        self.refresh_model_lists_in_background();
        let prefix = format!("{provider}/");
        if !self.app.model_choices.iter().any(|choice| choice.value.starts_with(&prefix)) {
            return self.app.flash(format!("Connected {provider}. Its models are still loading; try /model in a moment."));
        }
        let current = self.app.model.as_ref().map(|model| model.label());
        let mut picker = ModelPicker::new(self.app.model_choices.clone(), current);
        picker.query = prefix;
        picker.selected = 0;
        self.app.overlay = Some(Overlay::ModelPicker(picker));
        self.app.dirty = true;
    }

    fn refresh_model_lists_in_background(&self) {
        let state = self.state.clone();
        let tx = self.models_tx.clone();
        tokio::spawn(async move {
            if provider_api::refresh_model_lists(&state).await > 0 {
                if let Some(tx) = tx {
                    let _ = tx.send(()).await;
                }
            }
        });
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
        if let Err(message) = ensure_authenticated(&self.state, &model).await {
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
