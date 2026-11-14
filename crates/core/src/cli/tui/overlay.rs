//! Modal overlays: permission approval, agent questions and the model picker
//! (help, the session picker and `/login` live in their own modules).

use super::composer::printable_char;
use super::diff::preview_diff;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;
use std::time::{Duration, Instant};

pub enum Overlay {
    Approval(Approval),
    Question(QuestionFlow),
    ModelPicker(ModelPicker),
    /// Reasoning effort levels (`/thinking`).
    Effort(OptionPicker),
    /// Permission modes (`/permissions`).
    Mode(OptionPicker),
    Help(super::help::HelpView),
    Sessions(super::sessions::SessionPicker),
    Login(super::login::LoginFlow),
}

// ---------------------------------------------------------------------
// Model picker (`/model` without an argument, like pi's model selector)
// ---------------------------------------------------------------------

/// Searchable list of every `provider/model`; Enter switches to the
/// highlighted one.
pub struct ModelPicker {
    pub all: Vec<super::popup::PopupItem>,
    pub query: String,
    pub selected: usize,
    /// `provider/model` in use, marked in the list.
    pub current: Option<String>,
}

/// What a key did in the picker.
#[derive(Debug, PartialEq, Eq)]
pub enum PickerOutcome {
    Pick(String),
    Close,
}

impl ModelPicker {
    pub fn new(all: Vec<super::popup::PopupItem>, current: Option<String>) -> Self {
        let selected = current
            .as_deref()
            .and_then(|current| all.iter().position(|item| item.value == current))
            .unwrap_or(0);
        Self { all, query: String::new(), selected, current }
    }

    /// The models matching the search text (all of them when it is empty).
    pub fn visible(&self) -> Vec<super::popup::PopupItem> {
        if self.query.trim().is_empty() {
            return self.all.clone();
        }
        super::popup::argument_items(&self.all, &self.query)
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Option<PickerOutcome> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let count = self.visible().len();
        if let Some(ch) = printable_char(&key) {
            self.query.push(ch);
            self.selected = 0;
            return None;
        }
        match key.code {
            KeyCode::Esc => return Some(PickerOutcome::Close),
            KeyCode::Char('c') if ctrl => return Some(PickerOutcome::Close),
            KeyCode::Enter => {
                return self.visible().get(self.selected).map(|item| PickerOutcome::Pick(item.value.clone()));
            }
            KeyCode::Up => self.selected = self.selected.checked_sub(1).unwrap_or(count.saturating_sub(1)),
            KeyCode::Char('p') if ctrl => self.selected = self.selected.checked_sub(1).unwrap_or(count.saturating_sub(1)),
            KeyCode::Down => self.selected = if self.selected + 1 >= count { 0 } else { self.selected + 1 },
            KeyCode::Char('n') if ctrl => self.selected = if self.selected + 1 >= count { 0 } else { self.selected + 1 },
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(10),
            KeyCode::PageDown => self.selected = (self.selected + 10).min(count.saturating_sub(1)),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = count.saturating_sub(1),
            KeyCode::Backspace => {
                self.query.pop();
                self.selected = 0;
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod model_picker_tests {
    use super::*;
    use crate::cli::tui::popup::PopupItem;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn item(value: &str) -> PopupItem {
        PopupItem { value: value.into(), label: value.into(), detail: String::new() }
    }

    #[test]
    fn typing_filters_and_enter_picks_the_highlighted_model() {
        let all = vec![item("xai/grok-4.7"), item("opencode-go/deepseek-v4.1-flash"), item("opencode-go/glm-5.3")];
        let mut picker = ModelPicker::new(all, Some("opencode-go/glm-5.3".into()));
        assert_eq!(picker.selected, 2, "starts on the current model");
        for ch in "deep".chars() {
            assert_eq!(picker.on_key(press(KeyCode::Char(ch))), None);
        }
        assert_eq!(picker.visible().len(), 1);
        assert_eq!(
            picker.on_key(press(KeyCode::Enter)),
            Some(PickerOutcome::Pick("opencode-go/deepseek-v4.1-flash".into()))
        );
        picker.on_key(press(KeyCode::Backspace));
        picker.query.clear();
        picker.on_key(press(KeyCode::Up));
        assert_eq!(picker.selected, 2, "wraps to the end");
        assert_eq!(picker.on_key(press(KeyCode::Esc)), Some(PickerOutcome::Close));
    }
}

// ---------------------------------------------------------------------
// Option picker (`/thinking` and `/permissions` without an argument)
// ---------------------------------------------------------------------

/// A short fixed list; Enter picks the highlighted option.
pub struct OptionPicker {
    pub title: &'static str,
    /// `value` is what gets applied, `label` and `detail` are shown.
    pub options: Vec<super::popup::PopupItem>,
    pub selected: usize,
    /// Value in use, marked in the list.
    pub current: String,
}

impl OptionPicker {
    pub fn new(title: &'static str, options: Vec<super::popup::PopupItem>, current: &str) -> Self {
        let selected = options.iter().position(|option| option.value == current).unwrap_or(0);
        Self { title, options, selected, current: current.to_string() }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Option<PickerOutcome> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let count = self.options.len();
        match key.code {
            KeyCode::Esc => return Some(PickerOutcome::Close),
            KeyCode::Char('c') if ctrl => return Some(PickerOutcome::Close),
            KeyCode::Enter => {
                return self.options.get(self.selected).map(|option| PickerOutcome::Pick(option.value.clone()));
            }
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.checked_sub(1).unwrap_or(count.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.selected = if self.selected + 1 >= count { 0 } else { self.selected + 1 },
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = count.saturating_sub(1),
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod option_picker_tests {
    use super::*;
    use crate::cli::tui::popup::PopupItem;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn picker() -> OptionPicker {
        let options = ["low", "medium", "high"]
            .map(|value| PopupItem { value: value.into(), label: value.into(), detail: String::new() })
            .to_vec();
        OptionPicker::new("Reasoning effort", options, "medium")
    }

    #[test]
    fn starts_on_the_current_option_and_wraps() {
        let mut picker = picker();
        assert_eq!(picker.selected, 1);
        picker.on_key(press(KeyCode::Down));
        picker.on_key(press(KeyCode::Down));
        assert_eq!(picker.selected, 0, "wraps to the top");
        picker.on_key(press(KeyCode::Up));
        assert_eq!(picker.selected, 2, "wraps to the bottom");
    }

    #[test]
    fn enter_picks_and_escape_closes() {
        let mut picker = picker();
        picker.on_key(press(KeyCode::Down));
        assert_eq!(picker.on_key(press(KeyCode::Enter)), Some(PickerOutcome::Pick("high".into())));
        assert_eq!(picker.on_key(press(KeyCode::Esc)), Some(PickerOutcome::Close));
        assert_eq!(picker.on_key(press(KeyCode::Char('x'))), None);
    }
}

// ---------------------------------------------------------------------
// Approval
// ---------------------------------------------------------------------

pub struct Approval {
    pub title: String,
    pub tool_name: String,
    /// Plain description lines (command, path, url, domains).
    pub details: Vec<String>,
    /// Unified diff of the proposed change, when there is one.
    pub diff: Option<String>,
    pub scroll: usize,
    /// Keys that grant access are ignored until this instant, so text typed
    /// before the prompt appeared cannot answer it.
    pub armed_at: Instant,
    /// A standing grant (project / always) waits here for a second press.
    pub pending_grant: Option<&'static str>,
    /// Why the prompt appeared (`preview.reason.text`).
    pub reason: Option<String>,
    /// Only "once" and "deny" apply (dangerous command, protected path).
    pub one_time_only: bool,
    /// What a standing grant would cover (`preview.grantLabel`).
    pub grant_label: Option<String>,
}

/// How long a new approval prompt ignores grant keys.
const APPROVAL_ARM_DELAY: Duration = Duration::from_millis(600);

fn str_field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

impl Approval {
    /// Builds the overlay from a `permission_requested` event.
    pub fn from_event(event: &Value) -> Self {
        let tool_name = event
            .pointer("/toolCall/function/name")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_string();
        let preview = event.get("preview").cloned().unwrap_or(Value::Null);
        let action = str_field(&preview, "action");
        let path = str_field(&preview, "path");
        let files: Vec<String> = preview
            .get("files")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let host = preview
            .get("networkDomains")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let title = match action {
            "command" => "Run this command?".to_string(),
            "fetch" => "Fetch this URL?".to_string(),
            "web_search" => "Search the web?".to_string(),
            "move" => "Move this file?".to_string(),
            "create" => "Create this file?".to_string(),
            "edit" => "Edit this file?".to_string(),
            "delete" => "Delete this file?".to_string(),
            "mkdir" => "Create this directory?".to_string(),
            "patch" => {
                let noun = if files.len() == 1 { "file" } else { "files" };
                format!("Apply this patch? ({} {noun})", files.len())
            }
            "network" => format!("Allow network access to {host}?"),
            _ => format!("Allow {tool_name}?"),
        };
        let mut details = Vec::new();
        match action {
            "command" => {
                details.push(format!("$ {}", str_field(&preview, "command")));
                let domains: Vec<&str> = preview
                    .get("networkDomains")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                if !domains.is_empty() {
                    details.push(format!("network: {}", domains.join(", ")));
                }
            }
            "fetch" => details.push(format!("GET {}", str_field(&preview, "url"))),
            "web_search" => details.push(format!("query: \"{}\"", str_field(&preview, "query"))),
            "move" => details.push(format!("{path} -> {}", str_field(&preview, "destPath"))),
            "create" | "edit" | "delete" | "mkdir" => details.push(path.to_string()),
            "patch" => details.extend(files.iter().map(|file| format!("  {file}"))),
            "network" => details.push(str_field(&preview, "url").to_string()),
            _ => {
                if let Some(call) = event.pointer("/toolCall/function/arguments").and_then(Value::as_str) {
                    details.push(format!("Arguments: {}", call.chars().take(400).collect::<String>()));
                }
            }
        }
        Self {
            title,
            tool_name,
            details,
            diff: Self::diff_for(&preview, action, path),
            scroll: 0,
            armed_at: Instant::now() + APPROVAL_ARM_DELAY,
            pending_grant: None,
            reason: preview
                .pointer("/reason/text")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .map(str::to_string),
            one_time_only: preview.get("oneTimeOnly").and_then(Value::as_bool).unwrap_or(false),
            grant_label: preview
                .get("grantLabel")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .map(str::to_string),
        }
    }

    fn diff_for(preview: &Value, action: &str, path: &str) -> Option<String> {
        let old = preview.get("oldContent").and_then(Value::as_str);
        let new = preview.get("newContent").and_then(Value::as_str);
        if action == "patch" {
            return preview
                .get("diff")
                .and_then(Value::as_str)
                .filter(|diff| !diff.trim().is_empty())
                .map(str::to_string);
        }
        let diff = match action {
            "edit" if old.is_some() && new.is_some() => preview_diff(path, old, new),
            "create" => preview_diff(path, None, new),
            "delete" => preview_diff(path, old, None),
            _ => return None,
        };
        (!diff.is_empty()).then_some(diff)
    }

    /// Standing grants persist, so they need the same key pressed twice.
    fn confirm_grant(&mut self, pending: Option<&'static str>, grant: &'static str) -> Option<&'static str> {
        if pending == Some(grant) {
            return Some(grant);
        }
        self.pending_grant = Some(grant);
        None
    }

    /// Handles a key; returns the permission decision when one was made.
    pub fn on_key(&mut self, key: KeyEvent) -> Option<&'static str> {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return (key.code == KeyCode::Char('c')).then_some("deny");
        }
        let armed = Instant::now() >= self.armed_at;
        let pending = self.pending_grant.take();
        match key.code {
            KeyCode::Char('d' | 'n') | KeyCode::Esc => Some("deny"),
            // Granting keys do nothing until the prompt has been visible a moment.
            KeyCode::Char('o' | 'y') if armed => Some("allow_once"),
            KeyCode::Char('r' | 'p' | 'a') if self.one_time_only => None,
            KeyCode::Char('r') if armed => Some("allow_run"),
            KeyCode::Char('p') if armed => self.confirm_grant(pending, "allow_project"),
            KeyCode::Char('a') if armed => self.confirm_grant(pending, "allow_always"),
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll += 1;
                None
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                None
            }
            KeyCode::PageDown => {
                self.scroll += 10;
                None
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------
// Questions
// ---------------------------------------------------------------------

pub struct QuestionItem {
    pub header: String,
    pub text: String,
    pub options: Vec<String>,
    pub multi: bool,
}

/// What the user answered: per-question selections plus free-text notes.
pub struct QuestionAnswer {
    pub selections: Vec<Vec<String>>,
    pub notes: Vec<String>,
}

pub struct QuestionFlow {
    pub items: Vec<QuestionItem>,
    pub index: usize,
    pub cursor: usize,
    pub picked: Vec<bool>,
    pub note: String,
    pub note_focus: bool,
    answers: Vec<Vec<String>>,
}

fn option_label(option: &Value) -> String {
    match option {
        Value::String(label) => label.clone(),
        other => other
            .get("label")
            .and_then(Value::as_str)
            .or_else(|| other.get("value").and_then(Value::as_str))
            .unwrap_or("option")
            .to_string(),
    }
}

impl QuestionFlow {
    /// Builds the overlay from a `question_requested` event.
    pub fn from_event(event: &Value) -> Self {
        let items: Vec<QuestionItem> = event
            .get("questions")
            .and_then(Value::as_array)
            .map(|questions| {
                questions
                    .iter()
                    .map(|question| QuestionItem {
                        header: str_field(question, "header").to_string(),
                        text: question
                            .get("question")
                            .and_then(Value::as_str)
                            .or_else(|| question.get("title").and_then(Value::as_str))
                            .unwrap_or("Question")
                            .to_string(),
                        options: question
                            .get("options")
                            .and_then(Value::as_array)
                            .map(|o| o.iter().map(option_label).collect())
                            .unwrap_or_default(),
                        multi: question
                            .get("multiSelect")
                            .or_else(|| question.get("multi_select"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut flow = Self {
            items,
            index: 0,
            cursor: 0,
            picked: Vec::new(),
            note: String::new(),
            note_focus: false,
            answers: Vec::new(),
        };
        flow.reset_question();
        flow
    }

    pub fn current(&self) -> Option<&QuestionItem> {
        self.items.get(self.index)
    }

    fn reset_question(&mut self) {
        let options = self.current().map_or(0, |item| item.options.len());
        self.cursor = 0;
        self.picked = vec![false; options];
        self.note_focus = options == 0;
    }

    fn finish(&mut self, skipped: bool) -> QuestionAnswer {
        let mut notes = Vec::new();
        if !self.note.trim().is_empty() {
            notes.push(self.note.trim().to_string());
        }
        if skipped {
            notes.push("(The user skipped the remaining questions.)".to_string());
        }
        QuestionAnswer {
            selections: std::mem::take(&mut self.answers),
            notes,
        }
    }

    fn confirm_current(&mut self) -> Option<QuestionAnswer> {
        let item = self.current()?;
        let mut chosen: Vec<String> = item
            .options
            .iter()
            .zip(&self.picked)
            .filter(|(_, picked)| **picked)
            .map(|(label, _)| label.clone())
            .collect();
        if chosen.is_empty() {
            if let Some(label) = item.options.get(self.cursor) {
                chosen.push(label.clone());
            }
        }
        self.answers.push(chosen);
        self.index += 1;
        if self.index >= self.items.len() {
            return Some(self.finish(false));
        }
        self.reset_question();
        None
    }

    fn toggle_cursor(&mut self) {
        let multi = self.current().is_some_and(|item| item.multi);
        if multi {
            if let Some(slot) = self.picked.get_mut(self.cursor) {
                *slot = !*slot;
            }
        } else {
            let was = self.picked.get(self.cursor).copied().unwrap_or(false);
            self.picked.iter_mut().for_each(|slot| *slot = false);
            if let Some(slot) = self.picked.get_mut(self.cursor) {
                *slot = !was;
            }
        }
    }

    /// Handles a key; returns the answer once every question is done.
    pub fn on_key(&mut self, key: KeyEvent) -> Option<QuestionAnswer> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(self.finish(true));
        }
        if self.note_focus {
            return self.on_note_key(key);
        }
        let count = self.picked.len();
        match key.code {
            KeyCode::Esc => return Some(self.finish(true)),
            KeyCode::Up | KeyCode::Char('k') if count > 0 => {
                self.cursor = (self.cursor + count - 1) % count;
            }
            KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                self.cursor = (self.cursor + 1) % count;
            }
            KeyCode::Char(' ') => self.toggle_cursor(),
            KeyCode::Tab => self.note_focus = true,
            KeyCode::Enter => return self.confirm_current(),
            _ => {}
        }
        None
    }

    fn on_note_key(&mut self, key: KeyEvent) -> Option<QuestionAnswer> {
        let has_options = !self.picked.is_empty();
        match key.code {
            KeyCode::Esc | KeyCode::Tab if has_options => self.note_focus = false,
            KeyCode::Esc => return Some(self.finish(true)),
            KeyCode::Enter if has_options => self.note_focus = false,
            KeyCode::Enter => return self.confirm_current(),
            KeyCode::Backspace => {
                self.note.pop();
            }
            _ => {
                if let Some(ch) = printable_char(&key) {
                    self.note.push(ch);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn permission_event(preview: Value) -> Value {
        json!({
            "type": "permission_requested",
            "toolCall": { "id": "c1", "function": { "name": "run_command", "arguments": "{}" } },
            "preview": preview,
        })
    }

    #[test]
    fn approval_for_command_lists_command_and_domains() {
        let approval = Approval::from_event(&permission_event(json!({
            "action": "command",
            "command": "curl example.com",
            "networkDomains": ["example.com"],
        })));
        assert_eq!(approval.title, "Run this command?");
        assert_eq!(approval.details, vec!["$ curl example.com", "network: example.com"]);
        assert!(approval.diff.is_none());
    }

    #[test]
    fn approval_for_edit_builds_a_diff() {
        let approval = Approval::from_event(&permission_event(json!({
            "action": "edit",
            "path": "a.txt",
            "oldContent": "one\ntwo\n",
            "newContent": "one\n2\n",
        })));
        assert_eq!(approval.title, "Edit this file?");
        let diff = approval.diff.unwrap();
        assert!(diff.contains("-two") && diff.contains("+2"));
    }

    #[test]
    fn approval_keys_map_to_decisions() {
        let mut approval = Approval::from_event(&permission_event(json!({"action": "command", "command": "ls"})));
        approval.armed_at = Instant::now();
        assert_eq!(approval.on_key(press(KeyCode::Char('o'))), Some("allow_once"));
        assert_eq!(approval.on_key(press(KeyCode::Char('y'))), Some("allow_once"));
        assert_eq!(approval.on_key(press(KeyCode::Char('r'))), Some("allow_run"));
        assert_eq!(approval.on_key(press(KeyCode::Char('p'))), None, "standing grants need a second press");
        assert_eq!(approval.on_key(press(KeyCode::Char('p'))), Some("allow_project"));
        assert_eq!(approval.on_key(press(KeyCode::Char('a'))), None);
        assert_eq!(approval.on_key(press(KeyCode::Char('a'))), Some("allow_always"));
        assert_eq!(approval.on_key(press(KeyCode::Char('d'))), Some("deny"));
        assert_eq!(approval.on_key(press(KeyCode::Char('n'))), Some("deny"));
        assert_eq!(approval.on_key(press(KeyCode::Esc)), Some("deny"));
        assert_eq!(approval.on_key(press(KeyCode::Down)), None);
        assert_eq!(approval.scroll, 1);
        assert_eq!(approval.on_key(press(KeyCode::Char('x'))), None);
    }

    #[test]
    fn typed_ahead_keys_do_not_answer_a_fresh_prompt() {
        let mut approval = Approval::from_event(&permission_event(json!({"action": "command", "command": "ls"})));
        for ch in ['o', 'r', 'p', 'a', 'y'] {
            assert_eq!(approval.on_key(press(KeyCode::Char(ch))), None, "{ch} must be ignored while arming");
        }
        assert_eq!(approval.pending_grant, None);
        // Denying is always allowed.
        assert_eq!(approval.on_key(press(KeyCode::Esc)), Some("deny"));
    }

    #[test]
    fn standing_grant_is_cancelled_by_another_key() {
        let mut approval = Approval::from_event(&permission_event(json!({"action": "command", "command": "ls"})));
        approval.armed_at = Instant::now();
        assert_eq!(approval.on_key(press(KeyCode::Char('a'))), None);
        assert_eq!(approval.on_key(press(KeyCode::Char('x'))), None);
        assert_eq!(approval.on_key(press(KeyCode::Char('a'))), None, "pending grant was reset");
        assert_eq!(approval.on_key(press(KeyCode::Char('p'))), None);
        assert_eq!(approval.on_key(press(KeyCode::Char('a'))), None, "different grant restarts");
    }

    fn question_event() -> Value {
        json!({
            "type": "question_requested",
            "questions": [
                { "question": "Pick a color", "header": "Color", "options": ["red", {"label": "blue"}] },
                { "title": "Pick toppings", "multiSelect": true, "options": [{"value": "ham"}, "egg", "cheese"] },
            ]
        })
    }

    #[test]
    fn question_flow_single_and_multi_select() {
        let mut flow = QuestionFlow::from_event(&question_event());
        assert_eq!(flow.current().unwrap().text, "Pick a color");
        flow.on_key(press(KeyCode::Down));
        assert!(flow.on_key(press(KeyCode::Enter)).is_none());
        // Multi-select: toggle ham and cheese.
        flow.on_key(press(KeyCode::Char(' ')));
        flow.on_key(press(KeyCode::Down));
        flow.on_key(press(KeyCode::Down));
        flow.on_key(press(KeyCode::Char(' ')));
        let answer = flow.on_key(press(KeyCode::Enter)).expect("finished");
        assert_eq!(
            answer.selections,
            vec![vec!["blue".to_string()], vec!["ham".to_string(), "cheese".to_string()]]
        );
        assert!(answer.notes.is_empty());
    }

    #[test]
    fn question_enter_without_toggle_picks_cursor_and_note_is_returned() {
        let event = json!({"questions": [{"question": "Q", "options": ["a", "b"]}]});
        let mut flow = QuestionFlow::from_event(&event);
        flow.on_key(press(KeyCode::Tab));
        for ch in "hi".chars() {
            flow.on_key(press(KeyCode::Char(ch)));
        }
        flow.on_key(press(KeyCode::Enter)); // leave note focus
        let answer = flow.on_key(press(KeyCode::Enter)).expect("finished");
        assert_eq!(answer.selections, vec![vec!["a".to_string()]]);
        assert_eq!(answer.notes, vec!["hi".to_string()]);
    }

    #[test]
    fn escape_skips_questions() {
        let mut flow = QuestionFlow::from_event(&question_event());
        let answer = flow.on_key(press(KeyCode::Esc)).expect("skipped");
        assert!(answer.selections.is_empty());
        assert_eq!(answer.notes.len(), 1);
    }
}

#[cfg(test)]
mod approval_ux_tests {
    use super::*;
    use serde_json::json;

    fn event(preview: Value) -> Value {
        json!({"type": "permission_requested",
            "toolCall": {"id": "c1", "function": {"name": "apply_patch", "arguments": "{}"}},
            "preview": preview})
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn patch_shows_files_and_the_ready_diff() {
        let approval = Approval::from_event(&event(json!({
            "action": "patch", "files": ["src/a.rs", "b.rs"], "diff": "--- a/src/a.rs\n+++ b/src/a.rs\n+x\n"})));
        assert_eq!(approval.title, "Apply this patch? (2 files)");
        assert_eq!(approval.details, vec!["  src/a.rs", "  b.rs"]);
        assert!(approval.diff.unwrap().contains("+x"));
    }

    #[test]
    fn network_names_the_host_and_unknown_tools_label_arguments() {
        let approval = Approval::from_event(&event(json!({
            "action": "network", "url": "api.github.com:443", "networkDomains": ["api.github.com"]})));
        assert_eq!(approval.title, "Allow network access to api.github.com?");
        assert_eq!(approval.details, vec!["api.github.com:443"]);
        let other = Approval::from_event(&event(json!({"action": "mystery"})));
        assert!(other.details[0].starts_with("Arguments: "));
    }

    #[test]
    fn web_search_shows_the_query() {
        let approval = Approval::from_event(&event(json!({"action": "web_search", "query": "rust serde"})));
        assert_eq!(approval.title, "Search the web?");
        assert_eq!(approval.details, vec!["query: \"rust serde\""]);
    }

    #[test]
    fn reason_grant_label_and_one_time_only_come_from_the_preview() {
        let mut approval = Approval::from_event(&event(json!({
            "action": "command", "command": "rm -rf x",
            "reason": {"code": "dangerous_command", "text": "This command can delete data."},
            "oneTimeOnly": true, "grantLabel": "rm *"})));
        approval.armed_at = Instant::now();
        assert_eq!(approval.reason.as_deref(), Some("This command can delete data."));
        assert_eq!(approval.grant_label.as_deref(), Some("rm *"));
        for key in ['r', 'p', 'a'] {
            assert_eq!(approval.on_key(press(KeyCode::Char(key))), None);
            assert_eq!(approval.on_key(press(KeyCode::Char(key))), None, "no grant even when pressed twice");
        }
        assert_eq!(approval.on_key(press(KeyCode::Char('y'))), Some("allow_once"));
    }
}
