//! TUI state: the run-event reducer, key handling and scrolling.
//!
//! Everything here is synchronous and terminal-free so it can be unit
//! tested; the async work (API calls) is requested through [`Action`]s and
//! performed by the driver.

use super::composer::Composer;
use super::overlay::{Approval, Overlay, QuestionAnswer, QuestionFlow};
use super::popup::{
    command_takes_no_args, completion_text, detect_trigger, file_items, slash_items, Popup,
    PopupKind,
};
use super::transcript::{tool_status_label, Cell, CellKind, ToolResult};
use crate::cli::models::ResolvedModel;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};
use serde_json::Value;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The Ctrl+C-twice-to-quit window.
const QUIT_WINDOW: Duration = Duration::from_secs(2);
const WHEEL_LINES: usize = 3;
/// Spinner advances every this many redraw ticks.
const SPINNER_TICKS: u32 = 3;

/// Work the driver must do on behalf of the UI.
pub enum Action {
    Submit(String),
    Queue(String),
    Slash(String),
    Decide(&'static str),
    Answer(QuestionAnswer),
    Cancel,
    Quit,
    SearchFiles(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Finish {
    Completed,
    Cancelled,
    Failed,
}

#[derive(Default)]
pub struct UsageInfo {
    pub total_tokens: i64,
    pub cost: f64,
    /// Percent of the model's context window still free.
    pub context_left: Option<u8>,
}

/// Percent of the context window left after `last_input` tokens.
pub fn context_left_percent(last_input: i64, window: i64) -> u8 {
    if window <= 0 {
        return 100;
    }
    let used = (last_input.max(0) * 100 / window).clamp(0, 100);
    (100 - used) as u8
}

#[derive(Default)]
pub struct Viewport {
    pub top: usize,
    pub follow: bool,
    pub total: usize,
    pub height: usize,
}

impl Viewport {
    fn max_top(&self) -> usize {
        self.total.saturating_sub(self.height)
    }

    pub fn scroll_up(&mut self, lines: usize) {
        if self.follow {
            self.top = self.max_top();
            self.follow = false;
        }
        self.top = self.top.saturating_sub(lines);
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.top = (self.top + lines).min(self.max_top());
        if self.top >= self.max_top() {
            self.follow = true;
        }
    }

    /// The first visible line for the current content size.
    pub fn resolve_top(&mut self) -> usize {
        if self.follow {
            self.top = self.max_top();
        } else {
            self.top = self.top.min(self.max_top());
        }
        self.top
    }
}

pub struct App {
    pub project: PathBuf,
    pub model: Option<ResolvedModel>,
    pub thinking: Option<String>,
    pub mode: String,
    pub bypass: bool,
    pub session: Option<String>,
    pub title: Option<String>,
    pub cells: Vec<Cell>,
    pub composer: Composer,
    pub popup: Option<Popup>,
    pub overlay: Option<Overlay>,
    pub busy: bool,
    pub busy_since: Option<Instant>,
    pub spinner: usize,
    pub show_reasoning: bool,
    pub viewport: Viewport,
    pub quit_armed_at: Option<Instant>,
    pub should_quit: bool,
    pub usage: UsageInfo,
    pub dirty: bool,
    /// Set when a run reached a terminal state; the driver drops its
    /// event receiver and refreshes usage.
    pub run_finished: bool,
    turn_diff: Option<(String, u64)>,
    queued_texts: VecDeque<String>,
    popup_dismissed_for: Option<String>,
    last_file_query: Option<String>,
    spinner_ticks: u32,
}

fn agent_tag(message: &Value) -> Option<String> {
    let role = message.get("agentRole").and_then(Value::as_str).filter(|s| !s.is_empty());
    let name = message.get("agentName").and_then(Value::as_str).filter(|s| !s.is_empty());
    match (role, name) {
        (Some(role), Some(name)) => Some(format!("{role} {name}")),
        (Some(role), None) => Some(role.to_string()),
        (None, Some(name)) => Some(name.to_string()),
        (None, None) => None,
    }
}

pub fn format_elapsed(duration: Duration) -> String {
    let secs = duration.as_secs();
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60),
    }
}

impl App {
    pub fn new(
        project: PathBuf,
        model: Option<ResolvedModel>,
        thinking: Option<String>,
        mode: String,
        bypass: bool,
        session: Option<String>,
    ) -> Self {
        Self {
            project,
            model,
            thinking,
            mode,
            bypass,
            session,
            title: None,
            cells: Vec::new(),
            composer: Composer::default(),
            popup: None,
            overlay: None,
            busy: false,
            busy_since: None,
            spinner: 0,
            show_reasoning: false,
            viewport: Viewport {
                follow: true,
                ..Viewport::default()
            },
            quit_armed_at: None,
            should_quit: false,
            usage: UsageInfo::default(),
            dirty: true,
            run_finished: false,
            turn_diff: None,
            queued_texts: VecDeque::new(),
            popup_dismissed_for: None,
            last_file_query: None,
            spinner_ticks: 0,
        }
    }

    // ---- small queries -------------------------------------------------

    pub fn mode_label(&self) -> &'static str {
        match self.mode.as_str() {
            "plan" => "Plan",
            "chat" => "Chat",
            "auto" => "Full Access",
            _ => "Build",
        }
    }

    pub fn quit_armed(&self) -> bool {
        self.quit_armed_at.is_some_and(|at| at.elapsed() < QUIT_WINDOW)
    }

    pub fn elapsed(&self) -> Option<Duration> {
        self.busy_since.map(|since| since.elapsed())
    }

    /// Label of the tool currently running, if any.
    pub fn running_tool(&self) -> Option<String> {
        self.cells.iter().rev().take(12).find_map(|cell| match cell.kind() {
            CellKind::Tool {
                name,
                arguments,
                result: None,
                ..
            } => Some(tool_status_label(name, arguments)),
            _ => None,
        })
    }

    pub fn push(&mut self, kind: CellKind) {
        self.cells.push(Cell::new(kind));
        self.dirty = true;
    }

    pub fn push_notice(&mut self, text: impl Into<String>) {
        self.push(CellKind::Notice(text.into()));
    }

    pub fn push_error(&mut self, text: impl Into<String>) {
        self.push(CellKind::Error(text.into()));
    }

    pub fn set_usage(&mut self, usage: UsageInfo) {
        self.usage = usage;
        self.dirty = true;
    }

    /// Mode switch used by `/plan`, `/build` and Shift+Tab.
    pub fn set_build_mode(&mut self, plan: bool) {
        self.mode = if plan {
            "plan".to_string()
        } else if self.bypass {
            "auto".to_string()
        } else {
            "accept_edits".to_string()
        };
        self.dirty = true;
    }

    // ---- redraw tick -----------------------------------------------------

    /// Called on every redraw tick; animates only while busy.
    pub fn tick(&mut self) {
        if self.busy {
            self.spinner_ticks += 1;
            if self.spinner_ticks >= SPINNER_TICKS {
                self.spinner_ticks = 0;
                self.spinner = self.spinner.wrapping_add(1);
                self.dirty = true;
            }
        }
        if self.quit_armed_at.is_some() && !self.quit_armed() {
            self.quit_armed_at = None;
            self.dirty = true;
        }
    }

    // ---- turn lifecycle (driver callbacks) ---------------------------------

    pub fn begin_turn(&mut self) {
        self.busy = true;
        self.busy_since = Some(Instant::now());
        self.turn_diff = None;
        self.viewport.follow = true;
        self.dirty = true;
    }

    /// Records a mid-run message the driver queued on the active run.
    pub fn note_queued(&mut self, display: &str, queued_content: &str) {
        self.queued_texts.push_back(queued_content.to_string());
        self.push(CellKind::User {
            text: display.to_string(),
            queued: true,
        });
    }

    /// After a clean finish: the oldest message that was queued but never
    /// folded into the run, ready to be sent as a new turn.
    pub fn take_leftover_queued(&mut self) -> Option<String> {
        let text = self.queued_texts.pop_front()?;
        // The new turn's user event re-creates the cell, so drop the
        // "queued" placeholder.
        if let Some(index) = self
            .cells
            .iter()
            .position(|c| matches!(c.kind(), CellKind::User { queued: true, .. }))
        {
            self.cells.remove(index);
        }
        Some(text)
    }

    fn clear_first_queued_flag(&mut self) {
        if let Some(cell) = self
            .cells
            .iter_mut()
            .find(|c| matches!(c.kind(), CellKind::User { queued: true, .. }))
        {
            cell.update(|kind| {
                if let CellKind::User { queued, .. } = kind {
                    *queued = false;
                }
            });
            self.dirty = true;
        }
    }

    fn finish(&mut self, outcome: Finish) {
        if !self.busy {
            return;
        }
        let elapsed = self.elapsed().map(format_elapsed).unwrap_or_default();
        self.busy = false;
        self.busy_since = None;
        self.overlay = None;
        self.run_finished = true;
        if outcome != Finish::Completed {
            self.interrupt_pending_tools();
            self.discard_queued();
        }
        if let Some((diff, files)) = self.turn_diff.take() {
            let noun = if files == 1 { "file" } else { "files" };
            self.push(CellKind::Diff {
                title: format!("Changes: {files} {noun}"),
                diff,
            });
        }
        match outcome {
            Finish::Completed => self.push(CellKind::Completed(format!("done · {elapsed}"))),
            Finish::Cancelled => self.push_notice("Run cancelled."),
            Finish::Failed => {}
        }
        self.dirty = true;
    }

    fn interrupt_pending_tools(&mut self) {
        for cell in self.cells.iter_mut() {
            if matches!(cell.kind(), CellKind::Tool { result: None, .. }) {
                cell.update(|kind| {
                    if let CellKind::Tool { result, .. } = kind {
                        *result = Some(ToolResult {
                            ok: false,
                            output: "interrupted".to_string(),
                        });
                    }
                });
            }
        }
    }

    fn discard_queued(&mut self) {
        let count = self.queued_texts.len();
        if count == 0 {
            return;
        }
        self.queued_texts.clear();
        for cell in self.cells.iter_mut() {
            cell.update(|kind| {
                if let CellKind::User { queued, .. } = kind {
                    *queued = false;
                }
            });
        }
        self.push_notice(format!("{count} queued message(s) were not delivered."));
    }

    /// The event channel closed without a terminal event.
    pub fn on_run_closed(&mut self) {
        self.finish(Finish::Failed);
    }

    // ---- run-event reducer ---------------------------------------------------

    pub fn on_run_event(&mut self, event: &Value) {
        self.dirty = true;
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "status_changed" => {
                if event.get("status").and_then(Value::as_str) == Some("cancelled") {
                    self.finish(Finish::Cancelled);
                }
            }
            "message_created" | "message_updated" => {
                if let Some(message) = event.get("message") {
                    self.on_message(message);
                }
            }
            "permission_requested" => self.open_approval(event),
            "question_requested" => {
                self.overlay = Some(Overlay::Question(QuestionFlow::from_event(event)));
            }
            "run_title_changed" => {
                self.title = event.get("title").and_then(Value::as_str).map(str::to_string);
            }
            "plan_updated" => self.on_plan(event),
            "turn_diff_updated" => {
                let diff = event.get("diff").and_then(Value::as_str).unwrap_or("");
                let files = event.get("filesChanged").and_then(Value::as_u64).unwrap_or(0);
                if !diff.is_empty() {
                    self.turn_diff = Some((diff.to_string(), files));
                }
            }
            "context_compacted" => {
                let before = event.get("messagesBefore").and_then(Value::as_u64).unwrap_or(0);
                let after = event.get("messagesAfter").and_then(Value::as_u64).unwrap_or(0);
                self.push_notice(format!("context compacted ({before} -> {after} messages)"));
            }
            "agent_spawned" => {
                let name = event.get("name").and_then(Value::as_str).unwrap_or("agent");
                let role = event.get("role").and_then(Value::as_str).unwrap_or("explorer");
                self.push_notice(format!("spawned {role} agent \"{name}\""));
            }
            "run_completed" => self.finish(Finish::Completed),
            "run_failed" => {
                let message = event
                    .get("errorMessage")
                    .and_then(Value::as_str)
                    .unwrap_or("Run failed.");
                self.push_error(message.to_string());
                self.finish(Finish::Failed);
            }
            _ => {}
        }
    }

    fn open_approval(&mut self, event: &Value) {
        let approval = Approval::from_event(event);
        let call_id = event.pointer("/toolCall/id").and_then(Value::as_str).unwrap_or("");
        if let Some(diff) = approval.diff.clone() {
            if let Some(cell) = self.cells.iter_mut().rev().find(|c| {
                matches!(c.kind(), CellKind::Tool { call_id: id, .. } if id == call_id)
            }) {
                cell.update(|kind| {
                    if let CellKind::Tool { diff: slot, .. } = kind {
                        *slot = Some(diff);
                    }
                });
            }
        }
        self.overlay = Some(Overlay::Approval(approval));
    }

    fn on_plan(&mut self, event: &Value) {
        let title = event.pointer("/plan/title").and_then(Value::as_str).unwrap_or("Plan");
        let body = event.pointer("/plan/body").and_then(Value::as_str).unwrap_or("");
        let text = format!("# {title}\n\n{body}");
        let existing = self
            .cells
            .iter_mut()
            .rev()
            .find(|c| c.msg_id.as_deref() == Some("__plan__"));
        match existing {
            Some(cell) => cell.update(|kind| *kind = CellKind::Assistant { text }),
            None => self
                .cells
                .push(Cell::from_message("__plan__", None, CellKind::Assistant { text })),
        }
    }

    fn on_message(&mut self, message: &Value) {
        let id = message.get("id").and_then(Value::as_str).unwrap_or("");
        let agent = agent_tag(message);
        match message.get("role").and_then(Value::as_str).unwrap_or("") {
            "assistant" => self.on_assistant_message(id, agent, message),
            "tool" => self.on_tool_message(message),
            "user" => self.on_user_message(id, message),
            _ => {}
        }
    }

    fn on_assistant_message(&mut self, id: &str, agent: Option<String>, message: &Value) {
        let reasoning = message.get("reasoningContent").and_then(Value::as_str).unwrap_or("");
        let content = message.get("content").and_then(Value::as_str).unwrap_or("");
        if !reasoning.trim().is_empty() {
            self.upsert_text(id, &agent, reasoning, true);
        }
        if !content.trim().is_empty() {
            self.upsert_text(id, &agent, content, false);
        }
        let calls = message
            .get("rawResponse")
            .and_then(Value::as_str)
            .and_then(|raw| serde_json::from_str::<Vec<Value>>(raw).ok())
            .unwrap_or_default();
        for call in calls {
            self.upsert_tool_call(id, &agent, &call);
        }
    }

    /// Creates or updates the reasoning/assistant text cell of a message.
    fn upsert_text(&mut self, id: &str, agent: &Option<String>, text: &str, reasoning: bool) {
        let is_target = |kind: &CellKind| match kind {
            CellKind::Reasoning { .. } => reasoning,
            CellKind::Assistant { .. } => !reasoning,
            _ => false,
        };
        let position = self
            .cells
            .iter()
            .rposition(|c| c.msg_id.as_deref() == Some(id) && is_target(c.kind()));
        let new_kind = || {
            if reasoning {
                CellKind::Reasoning { text: text.to_string() }
            } else {
                CellKind::Assistant { text: text.to_string() }
            }
        };
        match position {
            Some(index) => {
                let unchanged = matches!(self.cells[index].kind(),
                    CellKind::Reasoning { text: t } | CellKind::Assistant { text: t } if t == text);
                if !unchanged {
                    self.cells[index].update(|kind| *kind = new_kind());
                }
            }
            None => {
                let cell = Cell::from_message(id, agent.clone(), new_kind());
                // Reasoning always precedes the answer text of the same message.
                let before_answer = reasoning
                    .then(|| {
                        self.cells.iter().rposition(|c| {
                            c.msg_id.as_deref() == Some(id)
                                && matches!(c.kind(), CellKind::Assistant { .. })
                        })
                    })
                    .flatten();
                match before_answer {
                    Some(index) => self.cells.insert(index, cell),
                    None => self.cells.push(cell),
                }
            }
        }
    }

    fn upsert_tool_call(&mut self, msg_id: &str, agent: &Option<String>, call: &Value) {
        let call_id = call.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let name = call.pointer("/function/name").and_then(Value::as_str).unwrap_or("tool");
        let arguments = call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .unwrap_or("{}")
            .to_string();
        let existing = self.cells.iter_mut().rev().find(|c| {
            matches!(c.kind(), CellKind::Tool { call_id: id, .. } if *id == call_id)
        });
        match existing {
            Some(cell) => cell.update(|kind| {
                if let CellKind::Tool { arguments: slot, name: n, .. } = kind {
                    *slot = arguments;
                    *n = name.to_string();
                }
            }),
            None => self.cells.push(Cell::from_message(
                msg_id,
                agent.clone(),
                CellKind::Tool {
                    call_id,
                    name: name.to_string(),
                    arguments,
                    result: None,
                    diff: None,
                },
            )),
        }
    }

    fn on_tool_message(&mut self, message: &Value) {
        let call_id = message.get("rawResponse").and_then(Value::as_str).unwrap_or("");
        let content = message.get("content").and_then(Value::as_str).unwrap_or("");
        let result = ToolResult {
            ok: !(content.starts_with("Tool error")
                || content.starts_with("The user denied permission")),
            output: content.to_string(),
        };
        let existing = self.cells.iter_mut().rev().find(|c| {
            matches!(c.kind(), CellKind::Tool { call_id: id, .. } if id == call_id)
        });
        match existing {
            Some(cell) => cell.update(|kind| {
                if let CellKind::Tool { result: slot, .. } = kind {
                    *slot = Some(result);
                }
            }),
            None => self.push(CellKind::Tool {
                call_id: call_id.to_string(),
                name: "tool".to_string(),
                arguments: "{}".to_string(),
                result: Some(result),
                diff: None,
            }),
        }
    }

    fn on_user_message(&mut self, id: &str, message: &Value) {
        let content = message.get("content").and_then(Value::as_str).unwrap_or("");
        if content.starts_with("<environment_context>") {
            return;
        }
        if self.queued_texts.pop_front().is_some() {
            self.clear_first_queued_flag();
            return;
        }
        let seen = !id.is_empty()
            && self
                .cells
                .iter()
                .any(|c| c.msg_id.as_deref() == Some(id) && matches!(c.kind(), CellKind::User { .. }));
        if !seen {
            self.cells.push(Cell::from_message(
                id,
                None,
                CellKind::User {
                    text: content.to_string(),
                    queued: false,
                },
            ));
        }
    }

    /// Replays persisted messages when resuming a session.
    pub fn load_history(&mut self, messages: &[Value]) {
        for message in messages {
            self.on_message(message);
        }
        self.viewport.follow = true;
        self.dirty = true;
    }

    // ---- scrolling -------------------------------------------------------

    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        let delta = match mouse.kind {
            MouseEventKind::ScrollUp => -1isize,
            MouseEventKind::ScrollDown => 1,
            _ => return,
        };
        if let Some(Overlay::Approval(approval)) = self.overlay.as_mut() {
            approval.scroll = if delta < 0 {
                approval.scroll.saturating_sub(WHEEL_LINES)
            } else {
                approval.scroll + WHEEL_LINES
            };
        } else if delta < 0 {
            self.viewport.scroll_up(WHEEL_LINES);
        } else {
            self.viewport.scroll_down(WHEEL_LINES);
        }
        self.dirty = true;
    }

    pub fn on_paste(&mut self, text: &str) -> Vec<Action> {
        if self.overlay.is_some() {
            return Vec::new();
        }
        self.composer.insert_str(text);
        self.dirty = true;
        let mut actions = Vec::new();
        self.refresh_popup(&mut actions);
        actions
    }

    // ---- keys -------------------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if key.kind == KeyEventKind::Release {
            return Vec::new();
        }
        self.dirty = true;
        if self.overlay.is_some() {
            return self.on_overlay_key(key);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c') if ctrl => return self.on_ctrl_c(),
            KeyCode::Char('d') if ctrl && self.composer.is_empty() => return vec![Action::Quit],
            KeyCode::Char('o') if ctrl => {
                self.show_reasoning = !self.show_reasoning;
                return Vec::new();
            }
            KeyCode::PageUp => {
                self.viewport.scroll_up(self.viewport.height.saturating_sub(2).max(1));
                return Vec::new();
            }
            KeyCode::PageDown => {
                self.viewport.scroll_down(self.viewport.height.saturating_sub(2).max(1));
                return Vec::new();
            }
            KeyCode::End if ctrl || self.composer.is_empty() => {
                self.viewport.follow = true;
                return Vec::new();
            }
            KeyCode::BackTab => {
                let plan = self.mode != "plan";
                self.set_build_mode(plan);
                return Vec::new();
            }
            KeyCode::Esc => return self.on_escape(),
            _ => {}
        }
        if self.popup.is_some() {
            if let Some(actions) = self.on_popup_key(key) {
                return actions;
            }
        }
        self.on_edit_key(key)
    }

    fn on_ctrl_c(&mut self) -> Vec<Action> {
        if !self.composer.is_empty() {
            self.composer.clear();
            self.popup = None;
            return Vec::new();
        }
        if self.busy {
            return vec![Action::Cancel];
        }
        if self.quit_armed() {
            return vec![Action::Quit];
        }
        self.quit_armed_at = Some(Instant::now());
        Vec::new()
    }

    fn on_escape(&mut self) -> Vec<Action> {
        if self.popup.take().is_some() {
            self.popup_dismissed_for = Some(self.composer.text().to_string());
            return Vec::new();
        }
        if self.busy {
            return vec![Action::Cancel];
        }
        Vec::new()
    }

    fn on_overlay_key(&mut self, key: KeyEvent) -> Vec<Action> {
        match self.overlay.as_mut() {
            Some(Overlay::Approval(approval)) => match approval.on_key(key) {
                Some(decision) => {
                    self.overlay = None;
                    vec![Action::Decide(decision)]
                }
                None => Vec::new(),
            },
            Some(Overlay::Question(flow)) => match flow.on_key(key) {
                Some(answer) => {
                    self.overlay = None;
                    vec![Action::Answer(answer)]
                }
                None => Vec::new(),
            },
            None => Vec::new(),
        }
    }

    /// Popup navigation; `None` means the key should edit the composer.
    fn on_popup_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let popup = self.popup.as_mut()?;
        if popup.items.is_empty() {
            return None;
        }
        match key.code {
            KeyCode::Up => popup.select_previous(),
            KeyCode::Down => popup.select_next(),
            KeyCode::Char('p') if ctrl => popup.select_previous(),
            KeyCode::Char('n') if ctrl => popup.select_next(),
            KeyCode::Tab => return Some(self.accept_popup(false)),
            KeyCode::Enter if !key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
                return Some(self.accept_popup(true));
            }
            _ => return None,
        }
        Some(Vec::new())
    }

    /// Inserts the selected completion; Enter on an argument-less slash
    /// command also runs it.
    fn accept_popup(&mut self, run_if_complete: bool) -> Vec<Action> {
        let Some(popup) = self.popup.take() else {
            return Vec::new();
        };
        let Some(item) = popup.selected_item().cloned() else {
            return Vec::new();
        };
        let run_now = run_if_complete
            && popup.kind == PopupKind::Slash
            && command_takes_no_args(&item.value);
        self.composer
            .replace_range_to_cursor(popup.token_start, &completion_text(popup.kind, &item));
        if run_now {
            return self.submit_composer();
        }
        let mut actions = Vec::new();
        self.popup_dismissed_for = Some(self.composer.text().to_string());
        self.refresh_popup(&mut actions);
        actions
    }

    fn on_edit_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let c = &mut self.composer;
        match key.code {
            KeyCode::Enter if shift || alt || ctrl => c.insert_newline(),
            KeyCode::Enter => return self.submit_or_continue_line(),
            KeyCode::Char('j') if ctrl => c.insert_newline(),
            KeyCode::Char('a') if ctrl => c.move_home(),
            KeyCode::Char('e') if ctrl => c.move_end(),
            KeyCode::Char('b') if ctrl => c.move_left(),
            KeyCode::Char('f') if ctrl => c.move_right(),
            KeyCode::Char('w') if ctrl => c.delete_word_back(),
            KeyCode::Char('u') if ctrl => c.kill_to_line_start(),
            KeyCode::Char('k') if ctrl => c.kill_to_line_end(),
            KeyCode::Char('h') if ctrl => c.backspace(),
            KeyCode::Char('d') if alt => c.delete_word_forward(),
            KeyCode::Char('b') if alt => c.move_word_left(),
            KeyCode::Char('f') if alt => c.move_word_right(),
            KeyCode::Backspace if ctrl || alt => c.delete_word_back(),
            KeyCode::Backspace => c.backspace(),
            KeyCode::Delete if ctrl || alt => c.delete_word_forward(),
            KeyCode::Delete => c.delete_forward(),
            KeyCode::Left if ctrl || alt => c.move_word_left(),
            KeyCode::Right if ctrl || alt => c.move_word_right(),
            KeyCode::Left => c.move_left(),
            KeyCode::Right => c.move_right(),
            KeyCode::Home => c.move_home(),
            KeyCode::End => c.move_end(),
            KeyCode::Up => {
                if !c.move_up() {
                    c.history_prev();
                }
            }
            KeyCode::Down => {
                if !c.move_down() {
                    c.history_next();
                }
            }
            KeyCode::Char(ch) if !ctrl && !alt => c.insert_char(ch),
            _ => return Vec::new(),
        }
        let mut actions = Vec::new();
        self.refresh_popup(&mut actions);
        actions
    }

    /// Enter: a trailing backslash continues the line, otherwise submit.
    fn submit_or_continue_line(&mut self) -> Vec<Action> {
        let text = self.composer.text();
        if text[..self.composer.cursor()].ends_with('\\') {
            self.composer.backspace();
            self.composer.insert_newline();
            return Vec::new();
        }
        self.submit_composer()
    }

    fn submit_composer(&mut self) -> Vec<Action> {
        self.popup = None;
        self.popup_dismissed_for = None;
        let text = self.composer.take_submission();
        if text.is_empty() {
            return Vec::new();
        }
        if is_slash_command(&text) {
            return vec![Action::Slash(text)];
        }
        if self.busy {
            vec![Action::Queue(text)]
        } else {
            vec![Action::Submit(text)]
        }
    }

    // ---- popups -------------------------------------------------------------

    fn refresh_popup(&mut self, actions: &mut Vec<Action>) {
        let text = self.composer.text().to_string();
        if self.popup_dismissed_for.as_deref() == Some(text.as_str()) {
            self.popup = None;
            return;
        }
        self.popup_dismissed_for = None;
        let Some(trigger) = detect_trigger(&text, self.composer.cursor()) else {
            self.popup = None;
            self.last_file_query = None;
            return;
        };
        match trigger.kind {
            PopupKind::Slash => {
                let items = slash_items(&trigger.query);
                self.popup = (!items.is_empty()).then(|| Popup {
                    kind: PopupKind::Slash,
                    items,
                    selected: 0,
                    query: trigger.query,
                    token_start: trigger.start,
                });
            }
            PopupKind::File => {
                let changed = self.last_file_query.as_deref() != Some(trigger.query.as_str());
                if changed {
                    self.last_file_query = Some(trigger.query.clone());
                    actions.push(Action::SearchFiles(trigger.query.clone()));
                }
                match self.popup.as_mut() {
                    Some(popup) if popup.kind == PopupKind::File => {
                        popup.query = trigger.query;
                        popup.token_start = trigger.start;
                    }
                    _ => {
                        self.popup = Some(Popup {
                            kind: PopupKind::File,
                            items: Vec::new(),
                            selected: 0,
                            query: trigger.query,
                            token_start: trigger.start,
                        })
                    }
                }
            }
        }
    }

    /// File search results for `query`; ignored when the query moved on.
    pub fn set_file_matches(&mut self, query: &str, paths: &[String]) {
        if let Some(popup) = self.popup.as_mut() {
            if popup.kind == PopupKind::File && popup.query == query {
                popup.items = file_items(paths);
                popup.selected = 0;
                self.dirty = true;
            }
        }
    }

    // ---- misc ---------------------------------------------------------------

    pub fn help_text() -> String {
        "Commands: /model /thinking /plan /build /new /fork /undo /diff /mcp /skills /sessions /usage /exit\n\
         Keys: Enter send · Shift+Enter, Ctrl+J, Alt+Enter or trailing \\ newline · Up/Down history\n\
         Esc interrupt · Ctrl+C clear/interrupt/quit · Ctrl+D quit · Ctrl+O reasoning · Shift+Tab plan/build\n\
         PageUp/PageDown/wheel scroll · End follow · @ file mention · / commands"
            .to_string()
    }

    /// Plain-text title for the header line.
    pub fn display_title(&self) -> String {
        self.title
            .as_deref()
            .map(|t| t.lines().next().unwrap_or("").trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "new session".to_string())
    }

    /// First user prompt text shown for attachments (test helper).
    #[cfg(test)]
    pub fn last_user_text(&self) -> Option<String> {
        self.cells.iter().rev().find_map(|c| match c.kind() {
            CellKind::User { text, .. } => Some(text.clone()),
            _ => None,
        })
    }
}

/// `/word` where the word is a plain command name (not a path like `/etc/x`).
fn is_slash_command(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('/') else {
        return false;
    };
    let word = rest.split_whitespace().next().unwrap_or("");
    !word.is_empty() && word.chars().all(|c| c.is_ascii_alphabetic())
        || text.trim() == "/"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn app() -> App {
        App::new(
            PathBuf::from("C:/proj"),
            Some(ResolvedModel {
                provider_id: "prov".into(),
                model: "model-x".into(),
            }),
            None,
            "accept_edits".into(),
            false,
            None,
        )
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
    }

    fn type_text(app: &mut App, text: &str) -> Vec<Action> {
        let mut actions = Vec::new();
        for ch in text.chars() {
            actions.extend(app.on_key(press(KeyCode::Char(ch))));
        }
        actions
    }

    fn assistant(id: &str, content: &str) -> Value {
        json!({"type": "message_updated", "message": {"id": id, "role": "assistant", "content": content}})
    }

    #[test]
    fn created_then_updated_replaces_assistant_content() {
        let mut app = app();
        app.on_run_event(&json!({"type": "message_created",
            "message": {"id": "m1", "role": "assistant", "content": ""}}));
        assert!(app.cells.is_empty());
        app.on_run_event(&assistant("m1", "Hel"));
        app.on_run_event(&assistant("m1", "Hello world"));
        assert_eq!(app.cells.len(), 1);
        assert_eq!(
            app.cells[0].kind(),
            &CellKind::Assistant { text: "Hello world".into() }
        );
    }

    #[test]
    fn reasoning_cell_is_placed_before_the_answer() {
        let mut app = app();
        app.on_run_event(&assistant("m1", "answer"));
        app.on_run_event(&json!({"type": "message_updated",
            "message": {"id": "m1", "role": "assistant", "content": "answer", "reasoningContent": "hmm"}}));
        assert!(matches!(app.cells[0].kind(), CellKind::Reasoning { .. }));
        assert!(matches!(app.cells[1].kind(), CellKind::Assistant { .. }));
    }

    #[test]
    fn tool_call_cell_from_raw_response_gets_its_result() {
        let mut app = app();
        let calls = json!([{"id": "c1", "function": {"name": "run_command", "arguments": "{\"command\":\"ls\"}"}}]);
        app.on_run_event(&json!({"type": "message_updated", "message": {
            "id": "m1", "role": "assistant", "content": "", "rawResponse": calls.to_string()}}));
        assert_eq!(app.running_tool().as_deref(), Some("Ran ls"));
        app.on_run_event(&json!({"type": "message_created", "message": {
            "id": "m2", "role": "tool", "content": "a\nb", "rawResponse": "c1"}}));
        match app.cells[0].kind() {
            CellKind::Tool { name, result: Some(result), .. } => {
                assert_eq!(name, "run_command");
                assert!(result.ok);
                assert_eq!(result.output, "a\nb");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(app.running_tool().is_none());
        app.on_run_event(&json!({"type": "message_created", "message": {
            "id": "m3", "role": "tool", "content": "Tool error: boom", "rawResponse": "c-unknown"}}));
        assert!(matches!(app.cells[1].kind(), CellKind::Tool { result: Some(r), .. } if !r.ok));
    }

    #[test]
    fn sub_agent_messages_are_tagged() {
        let mut app = app();
        app.on_run_event(&json!({"type": "message_updated", "message": {
            "id": "s1", "role": "assistant", "content": "found it",
            "agentRole": "explorer", "agentName": "scout"}}));
        assert_eq!(app.cells[0].agent.as_deref(), Some("explorer scout"));
    }

    #[test]
    fn permission_requested_opens_overlay_and_decision_closes_it() {
        let mut app = app();
        app.begin_turn();
        app.on_run_event(&json!({"type": "permission_requested", "toolCall":
            {"id": "c1", "function": {"name": "run_command", "arguments": "{}"}},
            "preview": {"action": "command", "command": "rm x"}}));
        assert!(matches!(app.overlay, Some(Overlay::Approval(_))));
        let actions = app.on_key(press(KeyCode::Char('r')));
        assert!(matches!(actions.as_slice(), [Action::Decide("allow_run")]));
        assert!(app.overlay.is_none());
    }

    #[test]
    fn question_requested_opens_overlay_and_answers() {
        let mut app = app();
        app.on_run_event(&json!({"type": "question_requested",
            "questions": [{"question": "Which?", "options": ["a", "b"]}]}));
        assert!(matches!(app.overlay, Some(Overlay::Question(_))));
        let actions = app.on_key(press(KeyCode::Enter));
        match actions.as_slice() {
            [Action::Answer(answer)] => assert_eq!(answer.selections, vec![vec!["a".to_string()]]),
            _ => panic!("expected an answer"),
        }
    }

    #[test]
    fn run_completed_clears_busy_and_adds_diff_and_done_line() {
        let mut app = app();
        app.begin_turn();
        app.on_run_event(&json!({"type": "turn_diff_updated", "filesChanged": 2, "diff": "-a\n+b\n"}));
        app.on_run_event(&json!({"type": "run_completed", "finalOutput": "ok"}));
        assert!(!app.busy);
        assert!(app.run_finished);
        assert!(matches!(app.cells.first().map(Cell::kind), Some(CellKind::Diff { title, .. }) if title == "Changes: 2 files"));
        assert!(matches!(app.cells.last().map(Cell::kind), Some(CellKind::Completed(_))));
    }

    #[test]
    fn run_failed_shows_error_even_after_failed_status() {
        let mut app = app();
        app.begin_turn();
        app.on_run_event(&json!({"type": "status_changed", "status": "failed"}));
        app.on_run_event(&json!({"type": "run_failed", "errorMessage": "boom"}));
        assert!(!app.busy);
        assert!(app.cells.iter().any(|c| matches!(c.kind(), CellKind::Error(m) if m == "boom")));
    }

    #[test]
    fn cancel_marks_pending_tools_interrupted() {
        let mut app = app();
        app.begin_turn();
        let calls = json!([{"id": "c1", "function": {"name": "run_command", "arguments": "{}"}}]);
        app.on_run_event(&json!({"type": "message_updated", "message": {
            "id": "m1", "role": "assistant", "content": "", "rawResponse": calls.to_string()}}));
        app.on_run_event(&json!({"type": "status_changed", "status": "cancelled"}));
        assert!(!app.busy);
        assert!(matches!(app.cells[0].kind(), CellKind::Tool { result: Some(r), .. } if !r.ok));
    }

    #[test]
    fn user_events_dedupe_and_hide_environment_context() {
        let mut app = app();
        let user = json!({"type": "message_created", "message": {"id": "u1", "role": "user", "content": "hi"}});
        app.on_run_event(&user);
        app.on_run_event(&user);
        app.on_run_event(&json!({"type": "message_created", "message": {
            "id": "u2", "role": "user", "content": "<environment_context>x</environment_context>"}}));
        assert_eq!(app.cells.len(), 1);
        assert_eq!(app.last_user_text().as_deref(), Some("hi"));
    }

    #[test]
    fn queued_message_is_unmarked_when_the_run_folds_it() {
        let mut app = app();
        app.begin_turn();
        app.note_queued("later", "later");
        assert!(matches!(app.cells[0].kind(), CellKind::User { queued: true, .. }));
        app.on_run_event(&json!({"type": "message_created", "message": {"id": "u9", "role": "user", "content": "later"}}));
        assert_eq!(app.cells.len(), 1);
        assert!(matches!(app.cells[0].kind(), CellKind::User { queued: false, .. }));
    }

    #[test]
    fn leftover_queue_is_handed_back_after_completion() {
        let mut app = app();
        app.begin_turn();
        app.note_queued("later", "later");
        app.on_run_event(&json!({"type": "run_completed", "finalOutput": ""}));
        assert_eq!(app.take_leftover_queued().as_deref(), Some("later"));
        assert!(!app.cells.iter().any(|c| matches!(c.kind(), CellKind::User { .. })));
    }

    #[test]
    fn enter_submits_or_queues_and_slash_becomes_a_command() {
        let mut app = app();
        let actions = type_text(&mut app, "hello");
        assert!(actions.is_empty());
        assert!(matches!(app.on_key(press(KeyCode::Enter)).as_slice(), [Action::Submit(t)] if t == "hello"));
        app.begin_turn();
        type_text(&mut app, "more");
        assert!(matches!(app.on_key(press(KeyCode::Enter)).as_slice(), [Action::Queue(t)] if t == "more"));
        type_text(&mut app, "/usage");
        assert!(matches!(app.on_key(press(KeyCode::Enter)).as_slice(), [Action::Slash(t)] if t == "/usage"));
        type_text(&mut app, "/etc/hosts please");
        assert!(matches!(app.on_key(press(KeyCode::Enter)).as_slice(), [Action::Queue(_)]));
    }

    #[test]
    fn newline_shortcuts_do_not_submit() {
        let mut app = app();
        type_text(&mut app, "a");
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        type_text(&mut app, "b");
        app.on_key(ctrl('j'));
        type_text(&mut app, "c");
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        type_text(&mut app, "d\\");
        assert!(app.on_key(press(KeyCode::Enter)).is_empty());
        assert_eq!(app.composer.text(), "a\nb\nc\nd\n");
    }

    #[test]
    fn release_events_are_ignored() {
        let mut app = app();
        let mut release = press(KeyCode::Char('x'));
        release.kind = KeyEventKind::Release;
        app.on_key(release);
        assert!(app.composer.is_empty());
    }

    #[test]
    fn ctrl_c_clears_then_cancels_then_arms_quit() {
        let mut app = app();
        type_text(&mut app, "draft");
        assert!(app.on_key(ctrl('c')).is_empty());
        assert!(app.composer.is_empty());
        app.begin_turn();
        assert!(matches!(app.on_key(ctrl('c')).as_slice(), [Action::Cancel]));
        app.busy = false;
        assert!(app.on_key(ctrl('c')).is_empty());
        assert!(app.quit_armed());
        assert!(matches!(app.on_key(ctrl('c')).as_slice(), [Action::Quit]));
    }

    #[test]
    fn ctrl_d_quits_only_on_empty_composer() {
        let mut app = app();
        type_text(&mut app, "x");
        assert!(app.on_key(ctrl('d')).is_empty());
        app.composer.clear();
        assert!(matches!(app.on_key(ctrl('d')).as_slice(), [Action::Quit]));
    }

    #[test]
    fn escape_cancels_a_busy_run_but_first_closes_the_popup() {
        let mut app = app();
        app.begin_turn();
        type_text(&mut app, "/mo");
        assert!(app.popup.is_some());
        assert!(app.on_key(press(KeyCode::Esc)).is_empty());
        assert!(app.popup.is_none());
        assert!(matches!(app.on_key(press(KeyCode::Esc)).as_slice(), [Action::Cancel]));
    }

    #[test]
    fn slash_popup_tab_completes_and_enter_runs_simple_commands() {
        let mut app = app();
        type_text(&mut app, "/mo");
        assert_eq!(app.popup.as_ref().unwrap().items[0].value, "/model");
        assert!(app.on_key(press(KeyCode::Tab)).is_empty());
        assert_eq!(app.composer.text(), "/model ");
        assert!(app.popup.is_none());
        app.composer.clear();
        type_text(&mut app, "/pla");
        assert!(matches!(app.on_key(press(KeyCode::Enter)).as_slice(), [Action::Slash(t)] if t == "/plan"));
    }

    #[test]
    fn file_popup_requests_a_search_and_inserts_the_mention() {
        let mut app = app();
        type_text(&mut app, "see ");
        let actions = type_text(&mut app, "@ma");
        assert!(actions.iter().any(|a| matches!(a, Action::SearchFiles(q) if q == "ma")));
        app.set_file_matches("ma", &["src/main.rs".to_string(), "README.md".to_string()]);
        app.on_key(press(KeyCode::Down));
        app.on_key(press(KeyCode::Up));
        app.on_key(press(KeyCode::Tab));
        assert_eq!(app.composer.text(), "see @src/main.rs ");
        // Stale results for an older query are ignored.
        app.set_file_matches("m", &["x".to_string()]);
        assert!(app.popup.is_none());
    }

    #[test]
    fn up_and_down_recall_history_at_the_edges() {
        let mut app = app();
        type_text(&mut app, "first");
        app.on_key(press(KeyCode::Enter));
        app.on_key(press(KeyCode::Up));
        assert_eq!(app.composer.text(), "first");
        app.on_key(press(KeyCode::Down));
        assert_eq!(app.composer.text(), "");
    }

    #[test]
    fn scrolling_leaves_follow_mode_and_end_returns() {
        let mut app = app();
        app.viewport.total = 100;
        app.viewport.height = 10;
        app.on_key(press(KeyCode::PageUp));
        assert!(!app.viewport.follow);
        assert_eq!(app.viewport.top, 82);
        app.on_key(press(KeyCode::End));
        assert!(app.viewport.follow);
        assert_eq!(app.viewport.resolve_top(), 90);
    }

    #[test]
    fn ctrl_o_toggles_reasoning_and_shift_tab_toggles_mode() {
        let mut app = app();
        app.on_key(ctrl('o'));
        assert!(app.show_reasoning);
        app.on_key(press(KeyCode::BackTab));
        assert_eq!(app.mode, "plan");
        app.on_key(press(KeyCode::BackTab));
        assert_eq!(app.mode, "accept_edits");
    }

    #[test]
    fn context_left_is_derived_from_the_window() {
        assert_eq!(context_left_percent(25_000, 100_000), 75);
        assert_eq!(context_left_percent(500_000, 100_000), 0);
        assert_eq!(context_left_percent(10, 0), 100);
    }

    #[test]
    fn elapsed_formats_compactly() {
        assert_eq!(format_elapsed(Duration::from_secs(5)), "5s");
        assert_eq!(format_elapsed(Duration::from_secs(65)), "1m 05s");
        assert_eq!(format_elapsed(Duration::from_secs(3720)), "1h 02m");
    }
}
