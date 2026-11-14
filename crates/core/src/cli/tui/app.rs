//! TUI state: the run-event reducer, key handling and scrolling.
//!
//! Everything here is synchronous and terminal-free so it can be unit
//! tested; the async work (API calls) is requested through [`Action`]s and
//! performed by the driver.

use super::composer::{printable_char, Composer};
use super::login::LoginAction;
use super::overlay::{Approval, Overlay, QuestionAnswer, QuestionFlow};
use super::popup::{
    command_takes_no_args, completion_text, detect_trigger, file_items, review_items, slash_items, Popup,
    PopupKind,
};
use super::transcript::{tool_status_label, Cell, CellKind, ToolResult};
use crate::cli::models::ResolvedModel;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};
use serde_json::Value;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

mod events;
mod input;
mod turn;
#[cfg(test)]
mod tests;


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
    /// Load a session picked in `/resume`.
    Resume(String),
    /// A step of the `/login` overlay.
    Login(LoginAction),
    /// Pull the newest queued message back off the active run.
    Withdraw,
}

/// A queued mid-run message: what the user typed and what the run got.
struct Queued {
    display: String,
    content: String,
}

/// Ctrl+R prompt-history search state.
#[derive(Default)]
pub struct HistorySearch {
    pub query: String,
    pub selected: usize,
}

impl HistorySearch {
    /// Prompts containing every word of the query, newest first.
    pub fn matches<'a>(&self, history: &'a [String]) -> Vec<&'a String> {
        let words: Vec<String> = self.query.split_whitespace().map(str::to_lowercase).collect();
        history
            .iter()
            .rev()
            .filter(|prompt| {
                let lower = prompt.to_lowercase();
                words.iter().all(|word| lower.contains(word.as_str()))
            })
            .collect()
    }
}

/// How long a status-line flash stays.
const FLASH_TTL: Duration = Duration::from_secs(2);
/// A tick count that updates the elapsed time every ~5 s with reduced motion.
const REDUCED_MOTION_TICKS: u32 = 150;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Finish {
    Completed,
    Cancelled,
    Failed,
}

#[derive(Default)]
pub struct UsageInfo {
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
    /// Targets offered after `/review ` (filled by the driver from git).
    pub review_targets: Vec<super::popup::PopupItem>,
    /// `provider/model` ids offered by the `/model ` popup.
    pub model_choices: Vec<super::popup::PopupItem>,
    pub overlay: Option<Overlay>,
    pub busy: bool,
    pub busy_since: Option<Instant>,
    pub spinner: usize,
    pub details: bool,
    pub viewport: Viewport,
    pub quit_armed_at: Option<Instant>,
    pub should_quit: bool,
    pub usage: UsageInfo,
    pub dirty: bool,
    /// Set when a run reached a terminal state; the driver drops its
    /// event receiver and refreshes usage.
    pub run_finished: bool,
    turn_diff: Option<(String, u64)>,
    /// Status label while the model streams an `apply_patch` call.
    pub patch_progress: Option<String>,
    queued: VecDeque<Queued>,
    /// Short confirmation shown in the status line (setting changes, copy).
    flash: Option<(String, Instant, Duration)>,
    /// `/auto` or `--approve` was used: Shift+Tab may land on Full Access.
    pub auto_used: bool,
    /// Current git branch of the project, for the header.
    pub branch: Option<String>,
    pub home: Option<PathBuf>,
    /// No provider has credentials yet: the welcome text offers `/login`.
    pub no_providers: bool,
    /// `[tui] animations = false` / `LCGN_REDUCED_MOTION`.
    pub reduced_motion: bool,
    drag_hint_shown: bool,
    pub history_search: Option<HistorySearch>,
    popup_dismissed_for: Option<String>,
    last_file_query: Option<String>,
    spinner_ticks: u32,
}

/// `Writing changes · src/a.rs +3 -1 · b.rs +2 -0` from a `patch_progress` event.
fn patch_progress_label(event: &Value) -> String {
    let mut label = "Writing changes".to_string();
    for file in event.get("files").and_then(Value::as_array).into_iter().flatten() {
        let path = file.get("path").and_then(Value::as_str).unwrap_or("?");
        let count = |key: &str| file.get(key).and_then(Value::as_u64).unwrap_or(0);
        label.push_str(&format!(" · {path} +{} -{}", count("added"), count("removed")));
    }
    label
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
            review_targets: Vec::new(),
            model_choices: Vec::new(),
            overlay: None,
            busy: false,
            busy_since: None,
            spinner: 0,
            details: false,
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
            patch_progress: None,
            queued: VecDeque::new(),
            flash: None,
            auto_used: bypass,
            branch: None,
            home: dirs::home_dir(),
            no_providers: false,
            reduced_motion: false,
            drag_hint_shown: false,
            history_search: None,
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

    /// Shift+Tab: Build -> Plan -> (Full Access, once it was asked for) ->
    /// Build. With `--approve` Build means Full Access, so it alternates
    /// between Plan and Full Access.
    pub fn cycle_mode(&mut self) {
        let next = match self.mode.as_str() {
            "plan" if self.bypass || self.auto_used => "auto",
            "plan" => "accept_edits",
            "auto" if self.bypass => "plan",
            "auto" => "accept_edits",
            _ if self.bypass => "plan",
            _ => "plan",
        };
        self.mode = next.to_string();
        self.dirty = true;
    }

    // ---- status-line flash -----------------------------------------------------

    /// Shows `text` in the status line for two seconds.
    pub fn flash(&mut self, text: impl Into<String>) {
        self.flash_for(text, FLASH_TTL);
    }

    pub fn flash_for(&mut self, text: impl Into<String>, ttl: Duration) {
        self.flash = Some((text.into(), Instant::now(), ttl));
        self.dirty = true;
    }

    pub fn flash_text(&self) -> Option<&str> {
        self.flash
            .as_ref()
            .filter(|(_, since, ttl)| since.elapsed() < *ttl)
            .map(|(text, _, _)| text.as_str())
    }

    // ---- misc ---------------------------------------------------------------

    pub fn open_help(&mut self) {
        self.overlay = Some(Overlay::Help(super::help::HelpView::default()));
        self.dirty = true;
    }

    /// Seeds the Up-arrow history with earlier prompts (oldest first).
    pub fn seed_history(&mut self, prompts: Vec<String>) {
        self.composer.seed_history(prompts);
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
