//! Unified-diff rendering (+/- coloring) and diff building for previews.

use super::theme;
use super::wrap::{sanitize, wrap_spans};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// The style for one unified-diff line.
pub fn diff_line_style(line: &str) -> Style {
    if line.starts_with("+++") || line.starts_with("---") {
        Style::default().add_modifier(Modifier::BOLD)
    } else if line.starts_with("@@") {
        Style::default().fg(theme::accent())
    } else if line.starts_with('+') {
        Style::default().fg(theme::ok())
    } else if line.starts_with('-') {
        Style::default().fg(theme::err())
    } else if line.starts_with("diff ") || line.starts_with("index ") {
        theme::dim()
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

/// Lines added and removed in one file of a unified diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileStat {
    pub path: String,
    pub added: usize,
    pub removed: usize,
}

/// Per-file `+added -removed` counts of a unified diff (git or plain).
pub fn file_stats(diff: &str) -> Vec<FileStat> {
    let mut stats: Vec<FileStat> = Vec::new();
    let mut old_path = String::new();
    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("--- ") {
            old_path = path.trim().trim_start_matches("a/").to_string();
        } else if let Some(path) = line.strip_prefix("+++ ") {
            let path = path.trim().trim_start_matches("b/");
            let path = if path == "/dev/null" { old_path.clone() } else { path.to_string() };
            stats.push(FileStat { path, added: 0, removed: 0 });
        } else if let Some(current) = stats.last_mut() {
            if line.starts_with('+') {
                current.added += 1;
            } else if line.starts_with('-') {
                current.removed += 1;
            }
        }
    }
    stats
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
        assert_eq!(lines[3].spans[0].style.fg, Some(theme::err()));
        assert_eq!(lines[4].spans[0].style.fg, Some(theme::ok()));
        assert_eq!(lines[2].spans[0].style.fg, Some(theme::accent()));
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

    #[test]
    fn file_stats_count_lines_per_file() {
        let diff = "diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1,2 @@\n-old\n+new\n+more\n--- a/gone.rs\n+++ /dev/null\n-bye\n";
        assert_eq!(
            file_stats(diff),
            vec![
                FileStat { path: "x.rs".into(), added: 2, removed: 1 },
                FileStat { path: "gone.rs".into(), added: 0, removed: 1 },
            ]
        );
        assert!(file_stats("").is_empty());
    }
}
