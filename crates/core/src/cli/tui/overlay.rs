//! Modal overlays: permission approval and agent questions.

use super::diff::preview_diff;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;

pub enum Overlay {
    Approval(Approval),
    Question(QuestionFlow),
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
}

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
        let title = match action {
            "command" => "Run this command?".to_string(),
            "fetch" => "Fetch this URL?".to_string(),
            "move" => "Move this file?".to_string(),
            "create" => "Create this file?".to_string(),
            "edit" => "Edit this file?".to_string(),
            "delete" => "Delete this file?".to_string(),
            "mkdir" => "Create this directory?".to_string(),
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
            "move" => details.push(format!("{path} -> {}", str_field(&preview, "destPath"))),
            "create" | "edit" | "delete" | "mkdir" => details.push(path.to_string()),
            _ => {
                if let Some(call) = event.pointer("/toolCall/function/arguments").and_then(Value::as_str) {
                    details.push(call.chars().take(400).collect());
                }
            }
        }
        Self {
            title,
            tool_name,
            details,
            diff: Self::diff_for(&preview, action, path),
            scroll: 0,
        }
    }

    fn diff_for(preview: &Value, action: &str, path: &str) -> Option<String> {
        let old = preview.get("oldContent").and_then(Value::as_str);
        let new = preview.get("newContent").and_then(Value::as_str);
        let diff = match action {
            "edit" if old.is_some() && new.is_some() => preview_diff(path, old, new),
            "create" => preview_diff(path, None, new),
            "delete" => preview_diff(path, old, None),
            _ => return None,
        };
        (!diff.is_empty()).then_some(diff)
    }

    /// Handles a key; returns the permission decision when one was made.
    pub fn on_key(&mut self, key: KeyEvent) -> Option<&'static str> {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return (key.code == KeyCode::Char('c')).then_some("deny");
        }
        match key.code {
            KeyCode::Char('o' | 'y') => Some("allow_once"),
            KeyCode::Char('r') => Some("allow_run"),
            KeyCode::Char('p') => Some("allow_project"),
            KeyCode::Char('a') => Some("allow_always"),
            KeyCode::Char('d' | 'n') | KeyCode::Esc => Some("deny"),
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
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => self.note.push(ch),
            _ => {}
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
        assert_eq!(approval.on_key(press(KeyCode::Char('o'))), Some("allow_once"));
        assert_eq!(approval.on_key(press(KeyCode::Char('y'))), Some("allow_once"));
        assert_eq!(approval.on_key(press(KeyCode::Char('r'))), Some("allow_run"));
        assert_eq!(approval.on_key(press(KeyCode::Char('p'))), Some("allow_project"));
        assert_eq!(approval.on_key(press(KeyCode::Char('a'))), Some("allow_always"));
        assert_eq!(approval.on_key(press(KeyCode::Char('d'))), Some("deny"));
        assert_eq!(approval.on_key(press(KeyCode::Char('n'))), Some("deny"));
        assert_eq!(approval.on_key(press(KeyCode::Esc)), Some("deny"));
        assert_eq!(approval.on_key(press(KeyCode::Down)), None);
        assert_eq!(approval.scroll, 1);
        assert_eq!(approval.on_key(press(KeyCode::Char('x'))), None);
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
