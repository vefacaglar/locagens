//! Mouse, paste and key handling, popups.

use super::*;

impl App {
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

    pub(super) fn on_ctrl_c(&mut self) -> Vec<Action> {
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

    pub(super) fn on_escape(&mut self) -> Vec<Action> {
        if self.popup.take().is_some() {
            self.popup_dismissed_for = Some(self.composer.text().to_string());
            return Vec::new();
        }
        if self.busy {
            return vec![Action::Cancel];
        }
        Vec::new()
    }

    pub(super) fn on_overlay_key(&mut self, key: KeyEvent) -> Vec<Action> {
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
    pub(super) fn on_popup_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
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
    pub(super) fn accept_popup(&mut self, run_if_complete: bool) -> Vec<Action> {
        let Some(popup) = self.popup.take() else {
            return Vec::new();
        };
        let Some(item) = popup.selected_item().cloned() else {
            return Vec::new();
        };
        let run_now = run_if_complete
            && match popup.kind {
                PopupKind::Slash => command_takes_no_args(&item.value),
                PopupKind::Review => true,
                PopupKind::File => false,
            };
        self.composer
            .replace_range_to_cursor(popup.token_start, &completion_text(popup.kind, &item));
        if run_now {
            return self.submit_composer();
        }
        let mut actions = Vec::new();
        // `/review ` keeps the popup open to pick the target.
        let opens_review = popup.kind == PopupKind::Slash && item.value == "/review";
        if !opens_review {
            self.popup_dismissed_for = Some(self.composer.text().to_string());
        }
        self.refresh_popup(&mut actions);
        actions
    }

    pub(super) fn on_edit_key(&mut self, key: KeyEvent) -> Vec<Action> {
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
    pub(super) fn submit_or_continue_line(&mut self) -> Vec<Action> {
        let text = self.composer.text();
        if text[..self.composer.cursor()].ends_with('\\') {
            self.composer.backspace();
            self.composer.insert_newline();
            return Vec::new();
        }
        self.submit_composer()
    }

    pub(super) fn submit_composer(&mut self) -> Vec<Action> {
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

    pub(super) fn refresh_popup(&mut self, actions: &mut Vec<Action>) {
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
                self.popup = (!items.is_empty()).then_some(Popup {
                    kind: PopupKind::Slash,
                    items,
                    selected: 0,
                    query: trigger.query,
                    token_start: trigger.start,
                });
            }
            PopupKind::Review => {
                let items = review_items(&self.review_targets, &trigger.query);
                self.popup = (!items.is_empty()).then_some(Popup {
                    kind: PopupKind::Review,
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

}
