//! The `/resume` session picker (UX-005): a searchable list of this
//! folder's sessions, newest first.

use super::composer::printable_char;
use crate::db::Run;
use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub struct SessionRow {
    pub id: String,
    pub title: String,
    /// `2h ago · model · status`.
    pub detail: String,
    /// Lowercased title and first prompt, matched by the search.
    haystack: String,
}

pub struct SessionPicker {
    rows: Vec<SessionRow>,
    pub query: String,
    pub selected: usize,
    /// The session open now, marked in the list.
    pub current: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SessionOutcome {
    Open(String),
    Close,
}

/// `just now`, `5m ago`, `2h ago`, `3d ago`, or the date for older ones.
pub fn relative_time(timestamp: &str, now: DateTime<Utc>) -> String {
    let Ok(parsed) = DateTime::parse_from_rfc3339(timestamp) else {
        return String::new();
    };
    let secs = (now - parsed.with_timezone(&Utc)).num_seconds().max(0);
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        86_400..=2_591_999 => format!("{}d ago", secs / 86_400),
        _ => parsed.format("%Y-%m-%d").to_string(),
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

impl SessionRow {
    pub fn from_run(run: &Run, now: DateTime<Utc>) -> Self {
        let title = Some(first_line(&run.title))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| first_line(&run.task));
        let title = if title.is_empty() { "(untitled)".to_string() } else { title };
        let stamp = run.last_active_at.as_deref().unwrap_or(&run.updated_at);
        let detail = [relative_time(stamp, now), run.model.clone(), run.status.clone()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let haystack = format!("{} {}", title, first_line(&run.task)).to_lowercase();
        Self { id: run.id.clone(), title, detail, haystack }
    }
}

impl SessionPicker {
    pub fn new(runs: &[Run], current: Option<String>, now: DateTime<Utc>) -> Self {
        let rows = runs.iter().map(|run| SessionRow::from_run(run, now)).collect();
        Self { rows, query: String::new(), selected: 0, current }
    }

    pub fn total(&self) -> usize {
        self.rows.len()
    }

    /// Rows matching every word of the search text.
    pub fn visible(&self) -> Vec<&SessionRow> {
        let words: Vec<String> = self.query.split_whitespace().map(str::to_lowercase).collect();
        self.rows
            .iter()
            .filter(|row| words.iter().all(|word| row.haystack.contains(word.as_str())))
            .collect()
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Option<SessionOutcome> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let count = self.visible().len();
        if let Some(ch) = printable_char(&key) {
            self.query.push(ch);
            self.selected = 0;
            return None;
        }
        let up = |selected: usize| selected.checked_sub(1).unwrap_or(count.saturating_sub(1));
        let down = |selected: usize| if selected + 1 >= count { 0 } else { selected + 1 };
        match key.code {
            KeyCode::Esc => return Some(SessionOutcome::Close),
            KeyCode::Char('c') if ctrl => return Some(SessionOutcome::Close),
            KeyCode::Enter => {
                return self.visible().get(self.selected).map(|row| SessionOutcome::Open(row.id.clone()));
            }
            KeyCode::Up => self.selected = up(self.selected),
            KeyCode::Char('p') if ctrl => self.selected = up(self.selected),
            KeyCode::Down => self.selected = down(self.selected),
            KeyCode::Char('n') if ctrl => self.selected = down(self.selected),
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
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn run(id: &str, title: &str, task: &str) -> Run {
        Run {
            id: id.into(),
            title: title.into(),
            task: task.into(),
            model: "m1".into(),
            status: "done".into(),
            updated_at: "2026-11-14T10:00:00Z".into(),
            ..Default::default()
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-14T12:00:00Z").unwrap().with_timezone(&Utc)
    }

    #[test]
    fn relative_times() {
        let at = |s: &str| relative_time(s, now());
        assert_eq!(at("2026-11-14T11:59:30Z"), "just now");
        assert_eq!(at("2026-11-14T11:55:00Z"), "5m ago");
        assert_eq!(at("2026-11-14T10:00:00Z"), "2h ago");
        assert_eq!(at("2026-11-11T12:00:00Z"), "3d ago");
        assert_eq!(at("2026-01-01T00:00:00Z"), "2026-01-01");
        assert_eq!(at("garbage"), "");
    }

    #[test]
    fn rows_show_title_and_detail() {
        let row = SessionRow::from_run(&run("r1", "Fix parser", "please fix"), now());
        assert_eq!(row.title, "Fix parser");
        assert_eq!(row.detail, "2h ago · m1 · done");
        let untitled = SessionRow::from_run(&run("r2", "", "first prompt text"), now());
        assert_eq!(untitled.title, "first prompt text");
    }

    #[test]
    fn search_matches_title_and_first_prompt_and_enter_opens() {
        let runs = [run("r1", "Fix parser", "tokenizer bug"), run("r2", "Docs", "write readme")];
        let mut picker = SessionPicker::new(&runs, None, now());
        assert_eq!(picker.visible().len(), 2);
        for ch in "readme".chars() {
            assert_eq!(picker.on_key(press(KeyCode::Char(ch))), None);
        }
        assert_eq!(picker.visible().len(), 1);
        assert_eq!(picker.on_key(press(KeyCode::Enter)), Some(SessionOutcome::Open("r2".into())));
        picker.query.clear();
        picker.on_key(press(KeyCode::Up));
        assert_eq!(picker.selected, 1, "wraps");
        assert_eq!(picker.on_key(press(KeyCode::Esc)), Some(SessionOutcome::Close));
        picker.query = "zzz".into();
        assert_eq!(picker.on_key(press(KeyCode::Enter)), None, "nothing to open");
    }
}
