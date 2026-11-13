//! Unified-diff rendering (+/- coloring) and diff building for previews.

use super::wrap::{sanitize, wrap_spans};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// The style for one unified-diff line.
pub fn diff_line_style(line: &str) -> Style {
    if line.starts_with("+++") || line.starts_with("---") {
        Style::default().add_modifier(Modifier::BOLD)
    } else if line.starts_with("@@") {
        Style::default().fg(Color::Cyan)
    } else if line.starts_with('+') {
        Style::default().fg(Color::Green)
    } else if line.starts_with('-') {
        Style::default().fg(Color::Red)
    } else if line.starts_with("diff ") || line.starts_with("index ") {
        Style::default().add_modifier(Modifier::DIM)
    } else {
        Style::default()
    }
}

/// Colored, width-wrapped lines for a unified diff. Continuation lines are
/// indented so they stay visibly part of the same change.
pub fn diff_lines(diff: &str, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for raw in sanitize(diff).lines() {
        let style = diff_line_style(raw);
        for (index, spans) in wrap_spans(&[Span::styled(raw.to_string(), style)], width)
            .into_iter()
            .enumerate()
        {
            let mut spans = spans;
            if index > 0 {
                spans.insert(0, Span::styled("  ", style));
            }
            out.push(Line::from(spans));
        }
    }
    out
}

/// Builds the diff shown in the approval overlay and edit cells.
/// `None` content means the file does not exist on that side.
pub fn preview_diff(path: &str, old: Option<&str>, new: Option<&str>) -> String {
    let a_label = if old.is_some() {
        format!("a/{path}")
    } else {
        "/dev/null".to_string()
    };
    let b_label = if new.is_some() {
        format!("b/{path}")
    } else {
        "/dev/null".to_string()
    };
    locagens_apply_patch::unified_diff(
        &a_label,
        &b_label,
        old.unwrap_or_default(),
        new.unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_added_and_removed_lines() {
        let lines = diff_lines("--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n ctx\n", 40);
        assert_eq!(lines.len(), 6);
        assert_eq!(lines[3].spans[0].style.fg, Some(Color::Red));
        assert_eq!(lines[4].spans[0].style.fg, Some(Color::Green));
        assert_eq!(lines[2].spans[0].style.fg, Some(Color::Cyan));
        assert_eq!(lines[5].spans[0].style.fg, None);
        // File headers are not colored like changes.
        assert_eq!(lines[0].spans[0].style.fg, None);
    }

    #[test]
    fn preview_diff_marks_changes() {
        let diff = preview_diff("src/a.rs", Some("one\ntwo\n"), Some("one\nthree\n"));
        assert!(diff.contains("-two"), "got: {diff}");
        assert!(diff.contains("+three"), "got: {diff}");
    }

    #[test]
    fn preview_diff_for_new_file_is_all_additions() {
        let diff = preview_diff("new.txt", None, Some("hello\n"));
        assert!(diff.contains("+hello"), "got: {diff}");
        assert!(diff.contains("/dev/null"));
    }
}
