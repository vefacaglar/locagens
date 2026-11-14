//! The `/command` and `@file` completion popups above the composer.

use crate::commands::{self, CommandSpec, Frontend};

/// The `/` popup offers the shared command table ([`crate::commands`]):
/// CLI commands matching `query` (without the slash), prefix matches first.
pub fn filter_commands(query: &str) -> Vec<&'static CommandSpec> {
    commands::matching(query, Frontend::Cli)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupKind {
    Slash,
    File,
    /// Targets of `/review ` (uncommitted changes, branches, commits).
    Review,
    /// `provider/model` ids for `/model `.
    Model,
    /// Reasoning levels for `/thinking `.
    Thinking,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PopupItem {
    /// What gets inserted (`/model`, `src/main.rs`).
    pub value: String,
    /// What is shown in the list.
    pub label: String,
    pub detail: String,
}

pub struct Popup {
    pub kind: PopupKind,
    pub items: Vec<PopupItem>,
    pub selected: usize,
    pub query: String,
    /// Byte offset in the composer where the triggering token starts.
    pub token_start: usize,
}

impl Popup {
    pub fn selected_item(&self) -> Option<&PopupItem> {
        self.items.get(self.selected)
    }

    pub fn select_next(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + 1) % self.items.len();
        }
    }

    pub fn select_previous(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + self.items.len() - 1) % self.items.len();
        }
    }
}

pub fn slash_items(query: &str) -> Vec<PopupItem> {
    filter_commands(query)
        .into_iter()
        .map(|command| PopupItem {
            value: format!("/{}", command.name),
            label: command.usage(),
            detail: command.description.to_string(),
        })
        .collect()
}

/// Review targets matching `query` (case-insensitive substring of the label).
pub fn review_items(targets: &[PopupItem], query: &str) -> Vec<PopupItem> {
    let query = query.trim().to_lowercase();
    targets
        .iter()
        .filter(|item| item.label.to_lowercase().contains(&query))
        .cloned()
        .collect()
}

pub fn file_items(paths: &[String]) -> Vec<PopupItem> {
    paths
        .iter()
        .map(|path| PopupItem {
            value: path.clone(),
            label: path.clone(),
            detail: String::new(),
        })
        .collect()
}

/// Choices matching `query` (case-insensitive): prefix matches of the value
/// (or of its part after `provider/`) first, then substring matches; capped
/// so a long catalog stays quick to scan.
pub fn argument_items(choices: &[PopupItem], query: &str) -> Vec<PopupItem> {
    const MAX_ITEMS: usize = 50;
    let query = query.trim().to_lowercase();
    let starts = |item: &PopupItem| {
        let value = item.value.to_lowercase();
        value.starts_with(&query) || value.split_once('/').is_some_and(|(_, model)| model.starts_with(&query))
    };
    let (mut prefix, rest): (Vec<&PopupItem>, Vec<&PopupItem>) = choices.iter().partition(|item| starts(item));
    prefix.extend(rest.into_iter().filter(|item| {
        item.value.to_lowercase().contains(&query) || item.detail.to_lowercase().contains(&query)
    }));
    prefix.into_iter().take(MAX_ITEMS).cloned().collect()
}

/// `/thinking` levels as popup items.
pub fn thinking_choices() -> Vec<PopupItem> {
    crate::cli::commands::THINKING_LEVELS
        .iter()
        .map(|level| PopupItem { value: (*level).to_string(), label: (*level).to_string(), detail: String::new() })
        .collect()
}

/// A completion trigger found before the cursor.
#[derive(Debug, PartialEq, Eq)]
pub struct Trigger {
    pub kind: PopupKind,
    pub start: usize,
    pub query: String,
}

/// Detects `/cmd` (only at the very start, before any whitespace) or an
/// `@file` token ending at `cursor`.
pub fn detect_trigger(text: &str, cursor: usize) -> Option<Trigger> {
    let before = &text[..cursor];
    for (prefix, kind) in [("/model ", PopupKind::Model), ("/thinking ", PopupKind::Thinking)] {
        if let Some(rest) = before.strip_prefix(prefix) {
            if !rest.contains(char::is_whitespace) {
                return Some(Trigger { kind, start: prefix.len(), query: rest.to_string() });
            }
        }
    }
    if let Some(rest) = before.strip_prefix("/review ") {
        if !rest.contains('\n') {
            return Some(Trigger {
                kind: PopupKind::Review,
                start: "/review ".len(),
                query: rest.to_string(),
            });
        }
    }
    if let Some(rest) = before.strip_prefix('/') {
        if !rest.contains(char::is_whitespace) {
            return Some(Trigger {
                kind: PopupKind::Slash,
                start: 0,
                query: rest.to_string(),
            });
        }
    }
    let token_start = before
        .rfind(char::is_whitespace)
        .map(|i| i + before[i..].chars().next().map_or(1, char::len_utf8))
        .unwrap_or(0);
    let token = &before[token_start..];
    token.strip_prefix('@').map(|query| Trigger {
        kind: PopupKind::File,
        start: token_start,
        query: query.to_string(),
    })
}

/// The text to insert for a chosen popup item.
pub fn completion_text(kind: PopupKind, item: &PopupItem) -> String {
    match kind {
        PopupKind::Slash => format!("{} ", item.value),
        PopupKind::File => format!("@{} ", item.value),
        PopupKind::Review | PopupKind::Model | PopupKind::Thinking => item.value.clone(),
    }
}

/// Whether a slash command takes no arguments (so Enter can run it).
pub fn command_takes_no_args(value: &str) -> bool {
    /// Optional argument, useful bare: Enter in the popup runs them.
    const RUN_BARE: [&str; 2] = ["login", "copy"];
    value.starts_with('/')
        && commands::find(value)
            .is_some_and(|command| command.args.is_empty() || RUN_BARE.contains(&command.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(value: &str, detail: &str) -> PopupItem {
        PopupItem { value: value.into(), label: value.into(), detail: detail.into() }
    }

    #[test]
    fn model_arguments_trigger_a_filtered_model_list() {
        let trigger = detect_trigger("/model gl", 9).unwrap();
        assert_eq!((trigger.kind, trigger.start, trigger.query.as_str()), (PopupKind::Model, 7, "gl"));
        assert_eq!(detect_trigger("/thinking hi", 12).unwrap().kind, PopupKind::Thinking);
        assert!(detect_trigger("/model a b", 10).is_none(), "one argument only");

        let choices = vec![
            choice("xai/grok-4.7", "xAI"),
            choice("opencode-go/glm-5.3", "OpenCode Go"),
            choice("opencode-go/kimi-k3", "OpenCode Go"),
        ];
        let values = |query: &str| argument_items(&choices, query).into_iter().map(|item| item.value).collect::<Vec<_>>();
        assert_eq!(values("gl"), vec!["opencode-go/glm-5.3"], "matches the model part after the provider");
        assert_eq!(values("x")[0], "xai/grok-4.7", "prefix matches first");
        assert_eq!(values("opencode").len(), 2);
        assert_eq!(values("").len(), 3);
        assert!(values("zzz").is_empty());
        assert_eq!(completion_text(PopupKind::Model, &choices[0]), "xai/grok-4.7");
    }

    #[test]
    fn slash_filter_prefers_prefix_matches() {
        let names: Vec<&str> = filter_commands("m").iter().map(|c| c.name).collect();
        assert_eq!(names[0], "model");
        assert!(names.contains(&"mcp"));
        // "skills" contains no 'm'; "undo" neither; "new" neither.
        assert!(!names.contains(&"skills"));
        let all = filter_commands("");
        assert_eq!(all.len(), commands::commands_for(Frontend::Cli).count());
    }

    #[test]
    fn slash_filter_falls_back_to_substring() {
        let names: Vec<&str> = filter_commands("sum").iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["resume"]);
        assert!(filter_commands("zzz").is_empty());
    }

    #[test]
    fn detects_slash_only_at_start() {
        let trigger = detect_trigger("/mo", 3).unwrap();
        assert_eq!(trigger.kind, PopupKind::Slash);
        assert_eq!(trigger.query, "mo");
        // A command with its argument is no longer a command query (`/model `
        // and `/thinking ` open their own argument popups instead).
        assert!(detect_trigger("/mcp list", 9).is_none());
        assert_eq!(detect_trigger("/model gpt", 10).unwrap().kind, PopupKind::Model);
        assert!(detect_trigger("hi /mo", 6).is_none());
    }

    #[test]
    fn detects_file_mentions_mid_text() {
        let text = "look at @src/ma";
        let trigger = detect_trigger(text, text.len()).unwrap();
        assert_eq!(trigger.kind, PopupKind::File);
        assert_eq!(trigger.start, 8);
        assert_eq!(trigger.query, "src/ma");
        assert!(detect_trigger("email a@b.com", 13).is_none());
        let multiline = "first\n@rea";
        assert_eq!(detect_trigger(multiline, multiline.len()).unwrap().query, "rea");
    }

    #[test]
    fn review_arguments_trigger_the_target_popup() {
        let trigger = detect_trigger("/review ba", 10).unwrap();
        assert_eq!(trigger.kind, PopupKind::Review);
        assert_eq!(trigger.start, 8);
        assert_eq!(trigger.query, "ba");
        // Without the space it is still the command popup.
        assert_eq!(detect_trigger("/review", 7).unwrap().kind, PopupKind::Slash);
        let targets = vec![
            PopupItem { value: "uncommitted".into(), label: "uncommitted".into(), detail: String::new() },
            PopupItem { value: "base main".into(), label: "base main".into(), detail: String::new() },
        ];
        assert_eq!(review_items(&targets, "").len(), 2);
        assert_eq!(review_items(&targets, "BASE")[0].value, "base main");
        assert!(review_items(&targets, "fix the parser").is_empty());
        assert_eq!(completion_text(PopupKind::Review, &targets[1]), "base main");
    }

    #[test]
    fn selection_wraps_around() {
        let mut popup = Popup {
            kind: PopupKind::Slash,
            items: slash_items("p"),
            selected: 0,
            query: "p".into(),
            token_start: 0,
        };
        let count = popup.items.len();
        popup.select_previous();
        assert_eq!(popup.selected, count - 1);
        popup.select_next();
        assert_eq!(popup.selected, 0);
    }

    #[test]
    fn completion_text_and_argument_detection() {
        let item = &slash_items("plan")[0];
        assert_eq!(completion_text(PopupKind::Slash, item), "/plan ");
        assert!(command_takes_no_args("/plan"));
        assert!(!command_takes_no_args("/model"));
        let file = &file_items(&["a/b.rs".to_string()])[0];
        assert_eq!(completion_text(PopupKind::File, file), "@a/b.rs ");
    }
}
