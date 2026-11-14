//! Multi-line prompt editor with wrapping, word editing and history recall.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

/// The text a key press types, if it types any. Plain and Shift characters
/// are text; Ctrl or Alt alone are shortcuts. Windows reports AltGr as
/// Ctrl+Alt, so there that combination is text too (Turkish Q: `@ \ { [ ] } |`).
pub fn printable_char(key: &KeyEvent) -> Option<char> {
    printable_char_on(key, cfg!(windows))
}

fn printable_char_on(key: &KeyEvent, altgr_is_ctrl_alt: bool) -> Option<char> {
    let KeyCode::Char(ch) = key.code else {
        return None;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match (ctrl, alt) {
        (false, false) => Some(ch),
        (true, true) if altgr_is_ctrl_alt && !ch.is_control() => Some(ch),
        _ => None,
    }
}

/// One wrapped, on-screen row of the composer (byte offsets into the text).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisualLine {
    pub start: usize,
    pub end: usize,
}

pub struct Composer {
    text: String,
    /// Byte offset of the cursor, always on a char boundary.
    cursor: usize,
    /// Wrap width in columns (set by the renderer).
    width: usize,
    history: Vec<String>,
    history_pos: Option<usize>,
    draft: String,
}

impl Default for Composer {
    fn default() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            width: 80,
            history: Vec::new(),
            history_pos: None,
            draft: String::new(),
        }
    }
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

impl Composer {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn set_width(&mut self, width: usize) {
        self.width = width.max(2);
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.history_pos = None;
    }

    // ---- insertion -----------------------------------------------------

    pub fn insert_char(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
        self.history_pos = None;
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    /// Inserts pasted text (CRLF normalized, tabs expanded).
    pub fn insert_str(&mut self, pasted: &str) {
        let cleaned = pasted.replace("\r\n", "\n").replace('\r', "\n").replace('\t', "    ");
        self.text.insert_str(self.cursor, &cleaned);
        self.cursor += cleaned.len();
        self.history_pos = None;
    }

    /// Replaces `start..cursor` with `replacement` (popup completion).
    pub fn replace_range_to_cursor(&mut self, start: usize, replacement: &str) {
        let start = start.min(self.cursor);
        self.text.replace_range(start..self.cursor, replacement);
        self.history_pos = None;
        self.cursor = start + replacement.len();
    }

    // ---- deletion ------------------------------------------------------

    pub fn backspace(&mut self) {
        if let Some(previous) = self.prev_boundary(self.cursor) {
            self.text.replace_range(previous..self.cursor, "");
            self.history_pos = None;
            self.cursor = previous;
        }
    }

    pub fn delete_forward(&mut self) {
        if let Some(next) = self.next_boundary(self.cursor) {
            self.text.replace_range(self.cursor..next, "");
            self.history_pos = None;
        }
    }

    pub fn delete_word_back(&mut self) {
        let start = self.word_start_before(self.cursor);
        self.text.replace_range(start..self.cursor, "");
        self.history_pos = None;
        self.cursor = start;
    }

    pub fn delete_word_forward(&mut self) {
        let end = self.word_end_after(self.cursor);
        self.text.replace_range(self.cursor..end, "");
        self.history_pos = None;
    }

    /// Ctrl+U: delete from the start of the logical line to the cursor.
    pub fn kill_to_line_start(&mut self) {
        let start = self.line_start(self.cursor);
        self.text.replace_range(start..self.cursor, "");
        self.history_pos = None;
        self.cursor = start;
    }

    /// Ctrl+K: delete from the cursor to the end of the logical line.
    pub fn kill_to_line_end(&mut self) {
        let end = self.line_end(self.cursor);
        if end == self.cursor {
            self.delete_forward();
        } else {
            self.text.replace_range(self.cursor..end, "");
            self.history_pos = None;
        }
    }

    // ---- movement ------------------------------------------------------

    pub fn move_left(&mut self) {
        if let Some(previous) = self.prev_boundary(self.cursor) {
            self.cursor = previous;
        }
    }

    pub fn move_right(&mut self) {
        if let Some(next) = self.next_boundary(self.cursor) {
            self.cursor = next;
        }
    }

    pub fn move_word_left(&mut self) {
        self.cursor = self.word_start_before(self.cursor);
    }

    pub fn move_word_right(&mut self) {
        self.cursor = self.word_end_after(self.cursor);
    }

    pub fn move_home(&mut self) {
        self.cursor = self.line_start(self.cursor);
    }

    pub fn move_end(&mut self) {
        self.cursor = self.line_end(self.cursor);
    }

    /// Moves up one wrapped row; `false` when already on the first row.
    pub fn move_up(&mut self) -> bool {
        let rows = self.visual_lines();
        let (row, col) = self.row_col(&rows);
        if row == 0 {
            return false;
        }
        self.cursor = self.offset_at(rows[row - 1], col);
        true
    }

    /// Moves down one wrapped row; `false` when already on the last row.
    pub fn move_down(&mut self) -> bool {
        let rows = self.visual_lines();
        let (row, col) = self.row_col(&rows);
        if row + 1 >= rows.len() {
            return false;
        }
        self.cursor = self.offset_at(rows[row + 1], col);
        true
    }

    // ---- history -------------------------------------------------------

    /// Up-arrow on the first row: recall an older prompt.
    pub fn history_prev(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let next = match self.history_pos {
            None => {
                self.draft = self.text.clone();
                self.history.len() - 1
            }
            Some(0) => return false,
            Some(pos) => pos - 1,
        };
        self.history_pos = Some(next);
        self.text = self.history[next].clone();
        self.cursor = self.text.len();
        true
    }

    /// Down-arrow on the last row: recall a newer prompt (or the draft).
    pub fn history_next(&mut self) -> bool {
        let Some(pos) = self.history_pos else {
            return false;
        };
        if pos + 1 < self.history.len() {
            self.history_pos = Some(pos + 1);
            self.text = self.history[pos + 1].clone();
        } else {
            self.history_pos = None;
            self.text = std::mem::take(&mut self.draft);
        }
        self.cursor = self.text.len();
        true
    }

    /// Older prompts first; seeded at start and extended by submissions.
    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// Seeds the recall history (oldest first), e.g. from `history.jsonl`.
    pub fn seed_history(&mut self, prompts: Vec<String>) {
        self.history = prompts;
        self.history_pos = None;
    }

    /// Replaces the text and puts the cursor at the end.
    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
        self.cursor = self.text.len();
        self.history_pos = None;
    }

    /// Appends `block` after the current text, separated by a blank line.
    pub fn append_block(&mut self, block: &str) {
        if !self.text.trim().is_empty() {
            self.text.push_str("\n\n");
        }
        self.text.push_str(block);
        self.cursor = self.text.len();
        self.history_pos = None;
    }

    /// Takes the trimmed text, records it in history and clears the editor.
    pub fn take_submission(&mut self) -> String {
        let submitted = self.text.trim().to_string();
        if !submitted.is_empty() && self.history.last() != Some(&submitted) {
            self.history.push(submitted.clone());
        }
        self.text.clear();
        self.cursor = 0;
        self.history_pos = None;
        self.draft.clear();
        submitted
    }

    // ---- layout --------------------------------------------------------

    /// The wrapped rows for the current width.
    pub fn visual_lines(&self) -> Vec<VisualLine> {
        let mut rows = Vec::new();
        let mut line_start = 0;
        for logical in self.text.split('\n') {
            let line_end = line_start + logical.len();
            let mut row_start = line_start;
            let mut used = 0;
            for (offset, ch) in logical.char_indices() {
                let w = UnicodeWidthChar::width(ch).unwrap_or(0);
                if used + w > self.width && used > 0 {
                    rows.push(VisualLine {
                        start: row_start,
                        end: line_start + offset,
                    });
                    row_start = line_start + offset;
                    used = 0;
                }
                used += w;
            }
            rows.push(VisualLine {
                start: row_start,
                end: line_end,
            });
            line_start = line_end + 1;
        }
        rows
    }

    /// The cursor's `(row, display column)` in the wrapped layout.
    pub fn cursor_row_col(&self) -> (usize, usize) {
        self.row_col(&self.visual_lines())
    }

    fn row_col(&self, rows: &[VisualLine]) -> (usize, usize) {
        let row = rows
            .iter()
            .rposition(|r| r.start <= self.cursor)
            .unwrap_or(0);
        let col = self.text[rows[row].start..self.cursor]
            .chars()
            .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
            .sum();
        (row, col)
    }

    fn offset_at(&self, row: VisualLine, col: usize) -> usize {
        let mut used = 0;
        for (offset, ch) in self.text[row.start..row.end].char_indices() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + w > col {
                return row.start + offset;
            }
            used += w;
        }
        row.end
    }

    // ---- boundaries ----------------------------------------------------

    fn prev_boundary(&self, from: usize) -> Option<usize> {
        self.text[..from].char_indices().next_back().map(|(i, _)| i)
    }

    fn next_boundary(&self, from: usize) -> Option<usize> {
        self.text[from..].chars().next().map(|c| from + c.len_utf8())
    }

    fn line_start(&self, from: usize) -> usize {
        self.text[..from].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }

    fn line_end(&self, from: usize) -> usize {
        self.text[from..]
            .find('\n')
            .map(|i| from + i)
            .unwrap_or(self.text.len())
    }

    fn word_start_before(&self, from: usize) -> usize {
        let before: Vec<(usize, char)> = self.text[..from].char_indices().collect();
        let mut index = before.len();
        while index > 0 && !is_word_char(before[index - 1].1) {
            index -= 1;
        }
        while index > 0 && is_word_char(before[index - 1].1) {
            index -= 1;
        }
        before.get(index).map(|(i, _)| *i).unwrap_or(from)
    }

    fn word_end_after(&self, from: usize) -> usize {
        let mut end = from;
        let mut seen_word = false;
        for (offset, ch) in self.text[from..].char_indices() {
            if is_word_char(ch) {
                seen_word = true;
            } else if seen_word {
                return from + offset;
            }
            end = from + offset + ch.len_utf8();
        }
        end
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn seeded_history_is_recalled_and_blocks_append_after_a_blank_line() {
        let mut c = Composer::default();
        c.seed_history(vec!["one".into(), "two".into()]);
        assert!(c.history_prev());
        assert_eq!(c.text(), "two");
        c.set_text("draft");
        c.append_block("restored");
        assert_eq!(c.text(), "draft\n\nrestored");
        c.set_text("");
        c.append_block("first");
        assert_eq!(c.text(), "first");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composer_with(text: &str) -> Composer {
        let mut composer = Composer::default();
        composer.insert_str(text);
        composer
    }

    #[test]
    fn inserts_and_backspaces_unicode() {
        let mut c = Composer::default();
        for ch in "héy".chars() {
            c.insert_char(ch);
        }
        assert_eq!(c.text(), "héy");
        c.backspace();
        c.backspace();
        assert_eq!(c.text(), "h");
        c.backspace();
        c.backspace();
        assert_eq!(c.text(), "");
    }

    #[test]
    fn newline_splits_lines_and_home_end_are_per_line() {
        let mut c = composer_with("ab");
        c.insert_newline();
        c.insert_str("cd");
        assert_eq!(c.text(), "ab\ncd");
        c.move_home();
        assert_eq!(c.cursor(), 3);
        c.move_end();
        assert_eq!(c.cursor(), 5);
        assert!(c.move_up());
        assert_eq!(c.cursor(), 2);
        assert!(!c.move_up());
        assert!(c.move_down());
        assert!(!c.move_down());
    }

    #[test]
    fn word_delete_back_and_forward() {
        let mut c = composer_with("hello big world");
        c.delete_word_back();
        assert_eq!(c.text(), "hello big ");
        c.delete_word_back();
        assert_eq!(c.text(), "hello ");
        c.move_home();
        c.delete_word_forward();
        assert_eq!(c.text(), " ");
    }

    #[test]
    fn kill_line_edges() {
        let mut c = composer_with("one two");
        c.move_word_left();
        c.kill_to_line_start();
        assert_eq!(c.text(), "two");
        c.move_home();
        c.kill_to_line_end();
        assert_eq!(c.text(), "");
    }

    #[test]
    fn paste_normalizes_line_endings() {
        let mut c = Composer::default();
        c.insert_str("a\r\nb\tc");
        assert_eq!(c.text(), "a\nb    c");
    }

    #[test]
    fn wraps_long_lines_into_rows() {
        let mut c = composer_with("abcdefghij");
        c.set_width(4);
        let rows = c.visual_lines();
        assert_eq!(rows.len(), 3);
        assert_eq!(c.cursor_row_col(), (2, 2));
        assert!(c.move_up());
        assert_eq!(c.cursor_row_col(), (1, 2));
    }

    #[test]
    fn history_recall_round_trip_keeps_draft() {
        let mut c = Composer::default();
        c.insert_str("first");
        assert_eq!(c.take_submission(), "first");
        c.insert_str("second");
        c.take_submission();
        c.insert_str("draft");
        assert!(c.history_prev());
        assert_eq!(c.text(), "second");
        assert!(c.history_prev());
        assert_eq!(c.text(), "first");
        assert!(!c.history_prev());
        assert!(c.history_next());
        assert_eq!(c.text(), "second");
        assert!(c.history_next());
        assert_eq!(c.text(), "draft");
        assert!(!c.history_next());
    }

    #[test]
    fn submission_skips_empty_and_duplicate_history() {
        let mut c = Composer::default();
        c.insert_str("  ");
        assert_eq!(c.take_submission(), "");
        c.insert_str("same");
        c.take_submission();
        c.insert_str("same");
        c.take_submission();
        assert!(c.history_prev());
        assert!(!c.history_prev());
    }

    #[test]
    fn altgr_is_text_on_windows_only_and_plain_ctrl_alt_are_shortcuts() {
        let ev = |code, mods| KeyEvent::new(code, mods);
        let both = KeyModifiers::CONTROL | KeyModifiers::ALT;
        for ch in ['@', '\\', '{', '[', ']', '}', '|'] {
            assert_eq!(printable_char_on(&ev(KeyCode::Char(ch), both), true), Some(ch));
            assert_eq!(printable_char_on(&ev(KeyCode::Char(ch), both), false), None);
        }
        assert_eq!(printable_char_on(&ev(KeyCode::Char('a'), KeyModifiers::NONE), false), Some('a'));
        assert_eq!(printable_char_on(&ev(KeyCode::Char('A'), KeyModifiers::SHIFT), false), Some('A'));
        assert_eq!(printable_char_on(&ev(KeyCode::Char('a'), KeyModifiers::CONTROL), true), None);
        assert_eq!(printable_char_on(&ev(KeyCode::Char('d'), KeyModifiers::ALT), true), None);
        assert_eq!(printable_char_on(&ev(KeyCode::Enter, both), true), None);
    }

    #[test]
    fn deleting_text_leaves_history_recall() {
        let mut c = composer_with("first");
        c.take_submission();
        assert!(c.history_prev());
        c.backspace();
        assert_eq!(c.text(), "firs");
        assert!(!c.history_next(), "Down must not replace the edited entry");
        assert_eq!(c.text(), "firs");
    }

    #[test]
    fn replace_range_completes_a_token() {
        let mut c = composer_with("look at @ma");
        c.replace_range_to_cursor(8, "@src/main.rs ");
        assert_eq!(c.text(), "look at @src/main.rs ");
        assert_eq!(c.cursor(), c.text().len());
    }
}
