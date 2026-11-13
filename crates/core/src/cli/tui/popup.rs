//! The `/command` and `@file` completion popups above the composer.

/// A slash command offered by the `/` popup (same set as the REPL).
pub struct SlashCommand {
    pub name: &'static str,
    pub args: &'static str,
    pub description: &'static str,
}

pub const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand { name: "model", args: "[provider/id]", description: "show or switch models" },
    SlashCommand { name: "thinking", args: "<level>", description: "reasoning effort (off..max)" },
    SlashCommand { name: "plan", args: "", description: "switch to plan mode (read-only)" },
    SlashCommand { name: "build", args: "", description: "switch to build mode (edits applied)" },
    SlashCommand { name: "new", args: "", description: "start a fresh session on the next message" },
    SlashCommand { name: "fork", args: "", description: "fork the current session" },
    SlashCommand { name: "undo", args: "", description: "undo the last turn (git snapshot)" },
    SlashCommand { name: "diff", args: "", description: "show the working-tree diff" },
    SlashCommand { name: "mcp", args: "", description: "list MCP servers and status" },
    SlashCommand { name: "skills", args: "", description: "list discovered skills" },
    SlashCommand { name: "sessions", args: "", description: "list sessions for this directory" },
    SlashCommand { name: "usage", args: "", description: "usage and cost of this session" },
    SlashCommand { name: "help", args: "", description: "show commands and key bindings" },
    SlashCommand { name: "exit", args: "", description: "leave lcgn" },
];

/// Commands matching `query` (without the slash): prefix matches first,
/// then substring matches, each in table order.
pub fn filter_commands(query: &str) -> Vec<&'static SlashCommand> {
    let query = query.trim().to_lowercase();
    let prefix = SLASH_COMMANDS.iter().filter(|c| c.name.starts_with(&query));
    let contains = SLASH_COMMANDS
        .iter()
        .filter(|c| !c.name.starts_with(&query) && c.name.contains(&query));
    prefix.chain(contains).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupKind {
    Slash,
    File,
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
            label: format!("/{} {}", command.name, command.args).trim_end().to_string(),
            detail: command.description.to_string(),
        })
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
    }
}

/// Whether a slash command takes no arguments (so Enter can run it).
pub fn command_takes_no_args(value: &str) -> bool {
    SLASH_COMMANDS
        .iter()
        .find(|command| format!("/{}", command.name) == value)
        .is_some_and(|command| command.args.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_filter_prefers_prefix_matches() {
        let names: Vec<&str> = filter_commands("m").iter().map(|c| c.name).collect();
        assert_eq!(names[0], "model");
        assert!(names.contains(&"mcp"));
        // "skills" contains no 'm'; "undo" neither; "new" neither.
        assert!(!names.contains(&"skills"));
        let all = filter_commands("");
        assert_eq!(all.len(), SLASH_COMMANDS.len());
    }

    #[test]
    fn slash_filter_falls_back_to_substring() {
        let names: Vec<&str> = filter_commands("ess").iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["sessions"]);
        assert!(filter_commands("zzz").is_empty());
    }

    #[test]
    fn detects_slash_only_at_start() {
        let trigger = detect_trigger("/mo", 3).unwrap();
        assert_eq!(trigger.kind, PopupKind::Slash);
        assert_eq!(trigger.query, "mo");
        assert!(detect_trigger("/model gpt", 10).is_none());
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
