//! The `/help` overlay (also opened by `?` on an empty composer): commands
//! from the shared table next to the key bindings.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Scroll position of the help overlay.
#[derive(Default)]
pub struct HelpView {
    pub scroll: usize,
}

/// Key bindings, grouped for the overlay and `docs/CLI.md`.
pub const KEY_GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "Send and edit",
        &[
            ("Enter", "send (queues while a run is going)"),
            ("Shift+Enter", "newline (also Ctrl+J, Alt+Enter, trailing \\)"),
            ("Up / Down", "prompt history"),
            ("Ctrl+R", "search prompt history"),
            ("@", "mention a file"),
            ("/", "commands"),
            ("?", "this help (empty composer)"),
        ],
    ),
    (
        "Navigate",
        &[
            ("PageUp / PageDown", "scroll the transcript (wheel works too)"),
            ("End", "jump to the latest output"),
            ("Ctrl+O", "show details: reasoning, full output, diffs"),
        ],
    ),
    (
        "Run control",
        &[
            ("Esc", "interrupt the run"),
            ("Shift+Tab", "cycle mode (Build, Plan, Full Access)"),
            ("Up (running)", "edit the newest queued message"),
            ("Ctrl+C", "clear, interrupt, then quit"),
            ("Ctrl+D", "quit (empty composer)"),
        ],
    ),
];

/// Extra lines shown under the key groups.
pub const TIPS: &[&str] = &[
    "Select text: hold Shift while dragging. /copy copies the last answer.",
    "Screen reader: run `lcgn --no-tui`; [tui] animations = false calms the UI.",
];

/// `(usage, description)` of every command the CLI offers.
pub fn command_rows() -> Vec<(String, String)> {
    crate::commands::commands_for(crate::commands::Frontend::Cli)
        .map(|spec| (spec.usage(), spec.description.to_string()))
        .collect()
}

impl HelpView {
    /// Returns `true` when the overlay should close.
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc | KeyCode::Char('?' | 'q') | KeyCode::Enter => return true,
            KeyCode::Char('c') if ctrl => return true,
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll += 1,
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::PageDown => self.scroll += 10,
            KeyCode::Home => self.scroll = 0,
            _ => {}
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn lists_every_cli_command_with_its_description() {
        let rows = command_rows();
        assert!(rows.iter().any(|(usage, text)| usage == "/resume" && text.contains("session")));
        assert_eq!(rows.len(), crate::commands::COMMANDS.len());
    }

    #[test]
    fn scrolls_and_closes() {
        let mut view = HelpView::default();
        assert!(!view.on_key(press(KeyCode::Down)));
        assert_eq!(view.scroll, 1);
        assert!(!view.on_key(press(KeyCode::Up)));
        assert_eq!(view.scroll, 0);
        assert!(view.on_key(press(KeyCode::Esc)));
        assert!(view.on_key(press(KeyCode::Char('?'))));
    }
}
