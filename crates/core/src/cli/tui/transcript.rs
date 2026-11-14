//! Transcript cells and their rendering into wrapped ratatui lines.

use super::theme;
use super::diff::{diff_lines, FileStat};
use super::markdown::StreamedMarkdown;
use super::wrap::{display_width, sanitize, truncate_to_width, wrap_text, wrap_with_prefix};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

/// How many output lines a tool cell shows before truncating.
const TOOL_OUTPUT_LINES: usize = 5;
/// A failed tool shows more, since the error is what the user needs.
const FAILED_OUTPUT_LINES: usize = 15;
/// Cap per tool cell when details are expanded (Ctrl+O).
const EXPANDED_OUTPUT_LINES: usize = 200;
/// How many diff lines a diff cell shows before truncating.
const DIFF_LINES: usize = 40;
/// Files listed in a collapsed turn summary.
const SUMMARY_FILES: usize = 8;

#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub ok: bool,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CellKind {
    User { text: String },
    Assistant { text: String },
    Reasoning { text: String },
    Tool {
        call_id: String,
        name: String,
        /// Raw JSON arguments of the call.
        arguments: String,
        result: Option<ToolResult>,
        /// Unified diff of the proposed edit, when a preview was seen.
        diff: Option<String>,
    },
    Diff { title: String, diff: String },
    Notice(String),
    Error(String),
    /// End of a turn: `✔ 1m 12s · 3 files changed +42 −7 …` plus per-file
    /// counts; the full diff shows when details are expanded.
    TurnSummary {
        headline: String,
        ok: bool,
        stats: Vec<FileStat>,
        diff: Option<String>,
    },
}

struct Cached {
    width: usize,
    expanded: bool,
    lines: Vec<Line<'static>>,
}

pub struct Cell {
    /// Id of the run message this cell came from (for in-place updates).
    pub msg_id: Option<String>,
    /// `role name` of the sub-agent that produced the cell.
    pub agent: Option<String>,
    kind: CellKind,
    cache: Option<Cached>,
    /// Assistant text: blocks already rendered while it streamed.
    markdown: StreamedMarkdown,
}

impl Cell {
    pub fn new(kind: CellKind) -> Self {
        Self {
            msg_id: None,
            agent: None,
            kind,
            cache: None,
            markdown: StreamedMarkdown::default(),
        }
    }

    pub fn from_message(msg_id: &str, agent: Option<String>, kind: CellKind) -> Self {
        Self {
            msg_id: (!msg_id.is_empty()).then(|| msg_id.to_string()),
            agent,
            kind,
            cache: None,
            markdown: StreamedMarkdown::default(),
        }
    }

    pub fn kind(&self) -> &CellKind {
        &self.kind
    }

    /// Mutates the content and drops the render cache.
    pub fn update(&mut self, change: impl FnOnce(&mut CellKind)) {
        change(&mut self.kind);
        self.cache = None;
    }

    /// Rendered lines for `width` (cached until the cell changes).
    pub fn lines(&mut self, width: usize, expanded: bool) -> &[Line<'static>] {
        let stale = self
            .cache
            .as_ref()
            .is_none_or(|c| c.width != width || c.expanded != expanded);
        if stale {
            let lines = self.render(width, expanded);
            self.cache = Some(Cached {
                width,
                expanded,
                lines,
            });
        }
        &self.cache.as_ref().expect("cache just filled").lines
    }

    fn render(&mut self, width: usize, expanded: bool) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if let Some(agent) = &self.agent {
            lines.push(Line::from(Span::styled(
                format!("[{agent}]"),
                Style::default().fg(theme::agent()),
            )));
        }
        match &self.kind {
            CellKind::User { text } => render_user(&mut lines, text, width),
            CellKind::Assistant { text } => render_assistant(&mut lines, &mut self.markdown, text, width),
            CellKind::Reasoning { text } => render_reasoning(&mut lines, text, width, expanded),
            CellKind::Tool {
                name,
                arguments,
                result,
                diff,
                ..
            } => render_tool(&mut lines, name, arguments, result.as_ref(), diff.as_deref(), width, expanded),
            CellKind::Diff { title, diff } => render_diff(&mut lines, title, diff, width, expanded),
            CellKind::Notice(text) => lines.extend(wrap_text(
                text,
                width,
                dim(),
                vec![Span::styled("· ", dim())],
                vec![Span::raw("  ")],
            )),
            CellKind::Error(text) => lines.extend(wrap_text(
                text,
                width,
                Style::default().fg(theme::err()),
                vec![Span::styled("✖ ", Style::default().fg(theme::err()))],
                vec![Span::raw("  ")],
            )),
            CellKind::TurnSummary { headline, ok, stats, diff } => {
                render_summary(&mut lines, headline, *ok, stats, diff.as_deref(), width, expanded)
            }
        }
        if !matches!(self.kind, CellKind::Reasoning { .. } | CellKind::Notice(_)) {
            lines.push(Line::default());
        }
        lines
    }
}

fn dim() -> Style {
    theme::dim()
}

fn render_user(lines: &mut Vec<Line<'static>>, text: &str, width: usize) {
    let marker = Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD);
    let body = wrap_text(
        &summarize_attachments(text),
        width,
        Style::default().add_modifier(Modifier::BOLD),
        vec![Span::styled("› ", marker)],
        vec![Span::raw("  ")],
    );
    match theme::current().user_bg {
        Some(bg) => lines.extend(body.into_iter().map(|line| band(line, bg, width + 1))),
        None => lines.extend(body),
    }
}

/// Paints `line` onto a background band that fills `width` columns.
fn band(mut line: Line<'static>, bg: Color, width: usize) -> Line<'static> {
    let used: usize = line.spans.iter().map(|span| display_width(&span.content)).sum();
    for span in line.spans.iter_mut() {
        span.style = span.style.bg(bg);
    }
    if used < width {
        line.spans.push(Span::styled(" ".repeat(width - used), Style::default().bg(bg)));
    }
    line
}

fn render_assistant(lines: &mut Vec<Line<'static>>, markdown: &mut StreamedMarkdown, text: &str, width: usize) {
    let mut body = markdown.render(text, width.saturating_sub(2));
    for (index, line) in body.iter_mut().enumerate() {
        let prefix = if index == 0 {
            Span::styled("• ", dim())
        } else {
            Span::raw("  ")
        };
        line.spans.insert(0, prefix);
    }
    lines.extend(body);
}

fn render_reasoning(lines: &mut Vec<Line<'static>>, text: &str, width: usize, expanded: bool) {
    let style = dim().add_modifier(Modifier::ITALIC);
    if expanded {
        lines.extend(wrap_text(
            text.trim(),
            width,
            style,
            vec![Span::styled("▾ ", style)],
            vec![Span::styled("│ ", dim())],
        ));
    } else {
        let count = text.lines().filter(|l| !l.trim().is_empty()).count();
        lines.push(Line::from(Span::styled(
            format!("▸ Thinking… ({count} lines, Ctrl+O to expand)"),
            style,
        )));
    }
}

fn render_tool(
    lines: &mut Vec<Line<'static>>,
    name: &str,
    arguments: &str,
    result: Option<&ToolResult>,
    diff: Option<&str>,
    width: usize,
    expanded: bool,
) {
    let (marker, marker_style) = match result {
        None => ("⋯", Style::default().fg(theme::warn())),
        Some(r) if r.ok => ("✔", Style::default().fg(theme::ok())),
        Some(_) => ("✖", Style::default().fg(theme::err())),
    };
    let (verb, target) = describe_tool(name, arguments);
    let header = vec![
        Span::styled(format!("{verb} "), Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(target),
    ];
    lines.extend(wrap_with_prefix(
        &header,
        width,
        vec![Span::styled(format!("{marker} "), marker_style)],
        vec![Span::raw("  ")],
    ));
    if let Some(diff) = diff.filter(|d| !d.is_empty()) {
        push_limited_diff(lines, diff, width.saturating_sub(4), "  ", expanded);
    }
    if let Some(result) = result {
        push_tool_output(lines, name, result, width, expanded);
    }
}

fn push_limited_diff(lines: &mut Vec<Line<'static>>, diff: &str, width: usize, indent: &str, expanded: bool) {
    let rendered = diff_lines(diff, width);
    let total = rendered.len();
    let limit = if expanded { usize::MAX } else { DIFF_LINES };
    for mut line in rendered.into_iter().take(limit) {
        line.spans.insert(0, Span::raw(indent.to_string()));
        lines.push(line);
    }
    if total > limit {
        lines.push(Line::from(Span::styled(
            format!("{indent}… +{} more diff lines · Ctrl+O to expand", total - limit),
            dim(),
        )));
    }
}

fn push_tool_output(lines: &mut Vec<Line<'static>>, name: &str, result: &ToolResult, width: usize, expanded: bool) {
    let quiet = matches!(
        name,
        "read_file" | "list_directory" | "search_files" | "set_chat_title" | "update_plan" | "remember"
    );
    if result.ok && quiet {
        return;
    }
    let style = if result.ok { dim() } else { Style::default().fg(theme::err()) };
    let output = sanitize(result.output.trim_end());
    if output.is_empty() {
        return;
    }
    let all: Vec<&str> = output.lines().collect();
    let limit = match (expanded, result.ok) {
        (true, _) => EXPANDED_OUTPUT_LINES,
        (false, false) => FAILED_OUTPUT_LINES,
        (false, true) => TOOL_OUTPUT_LINES,
    };
    for line in all.iter().take(limit) {
        let text = truncate_to_width(line, width.saturating_sub(4));
        lines.push(Line::from(vec![
            Span::styled("  │ ", dim()),
            Span::styled(text, style),
        ]));
    }
    if all.len() > limit {
        let hint = if expanded { "" } else { " · Ctrl+O to expand" };
        lines.push(Line::from(Span::styled(
            format!("  … +{} lines{hint}", all.len() - limit),
            dim(),
        )));
    }
}

fn render_diff(lines: &mut Vec<Line<'static>>, title: &str, diff: &str, width: usize, expanded: bool) {
    lines.push(Line::from(vec![
        Span::styled("• ", dim()),
        Span::styled(title.to_string(), Style::default().add_modifier(Modifier::BOLD)),
    ]));
    push_limited_diff(lines, diff, width.saturating_sub(2), "  ", expanded);
}

/// `3 files changed +42 −7` (empty when nothing changed).
pub fn changes_headline(stats: &[FileStat]) -> String {
    if stats.is_empty() {
        return String::new();
    }
    let noun = if stats.len() == 1 { "file" } else { "files" };
    let added: usize = stats.iter().map(|s| s.added).sum();
    let removed: usize = stats.iter().map(|s| s.removed).sum();
    format!("{} {noun} changed +{added} −{removed}", stats.len())
}

fn render_summary(
    lines: &mut Vec<Line<'static>>,
    headline: &str,
    ok: bool,
    stats: &[FileStat],
    diff: Option<&str>,
    width: usize,
    expanded: bool,
) {
    let (glyph, color) = if ok { ("✔", theme::ok()) } else { ("■", theme::warn()) };
    let mut text = headline.to_string();
    if !stats.is_empty() {
        text.push_str(&format!(" · {} · /diff to view · /undo to revert", changes_headline(stats)));
    }
    lines.extend(wrap_with_prefix(
        &[Span::styled(text, dim())],
        width,
        vec![Span::styled(format!("{glyph} "), Style::default().fg(color))],
        vec![Span::raw("  ")],
    ));
    let shown = if expanded { stats.len() } else { stats.len().min(SUMMARY_FILES) };
    for stat in &stats[..shown] {
        lines.push(Line::from(vec![
            Span::raw(format!("  {}  ", truncate_to_width(&stat.path, width.saturating_sub(16).max(8)))),
            Span::styled(format!("+{}", stat.added), Style::default().fg(theme::ok())),
            Span::raw(" "),
            Span::styled(format!("−{}", stat.removed), Style::default().fg(theme::err())),
        ]));
    }
    if shown < stats.len() {
        lines.push(Line::from(Span::styled(
            format!("  … +{} more files · Ctrl+O to expand", stats.len() - shown),
            dim(),
        )));
    }
    match diff.filter(|d| !d.is_empty()) {
        Some(diff) if expanded => {
            lines.push(Line::default());
            push_limited_diff(lines, diff, width.saturating_sub(2), "  ", true);
        }
        Some(_) => lines.push(Line::from(Span::styled("  Ctrl+O shows the full diff", dim()))),
        None => {}
    }
}

/// `(verb, target)` for a tool call, e.g. `("Ran", "cargo test")`.
pub fn describe_tool(name: &str, arguments: &str) -> (String, String) {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let text = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let first = |keys: &[&str]| keys.iter().find_map(|key| text(key)).unwrap_or_default();
    match name {
        "run_command" | "exec_command" => ("Ran".into(), first(&["command", "cmd"])),
        "write_stdin" => ("Sent input to".into(), first(&["session_id", "chars"])),
        "read_file" | "view_image" => ("Read".into(), first(&["path"])),
        "list_directory" => ("Listed".into(), first(&["path"])),
        "search_files" => ("Searched".into(), first(&["query", "pattern", "path"])),
        "write_file" => ("Wrote".into(), first(&["path"])),
        "edit_file" => ("Edited".into(), first(&["path"])),
        "apply_patch" => ("Applied patch".into(), String::new()),
        "delete_file" => ("Deleted".into(), first(&["path"])),
        "create_directory" => ("Created dir".into(), first(&["path"])),
        "move_file" => (
            "Moved".into(),
            format!("{} -> {}", first(&["from", "path"]), first(&["to"])),
        ),
        "fetch_url" => ("Fetched".into(), first(&["url"])),
        "spawn_agents" => ("Spawned agents".into(), String::new()),
        "spawn_agent" => ("Spawned agent".into(), first(&["task_name", "name"])),
        "wait_agent" | "send_input" | "send_message" | "followup_task" | "interrupt_agent" | "close_agent"
        | "list_agents" => {
            (name.replace('_', " "), first(&["target", "agent_id", "id"]))
        }
        "load_skill" => ("Loaded skill".into(), first(&["name"])),
        "ask_user_question" | "request_user_input" => ("Asked".into(), String::new()),
        "set_chat_title" => ("Titled chat".into(), first(&["title"])),
        "update_plan" => ("Updated plan".into(), first(&["title"])),
        "remember" => ("Remembered".into(), String::new()),
        other => (other.to_string(), first(&["path", "command", "url", "query", "name", "title"])),
    }
}

/// Replaces `## Attached file: X` fenced blocks with a short `📎 X` line so
/// the transcript doesn't print whole files.
pub fn summarize_attachments(text: &str) -> String {
    let mut out = String::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(name) = line.strip_prefix("## Attached file: ") {
            out.push_str(&format!("📎 {}\n", name.trim()));
            if lines.peek().is_some_and(|next| next.starts_with("```")) {
                lines.next();
                for body in lines.by_ref() {
                    if body.starts_with("```") {
                        break;
                    }
                }
            }
            if lines.peek().is_some_and(|next| next.trim().is_empty()) {
                lines.next();
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// A one-line label for a pending tool, used by the status line.
pub fn tool_status_label(name: &str, arguments: &str) -> String {
    let (verb, target) = describe_tool(name, arguments);
    let label = format!("{verb} {target}");
    truncate_to_width(label.trim(), 48)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::tui::wrap::line_text;

    fn texts(cell: &mut Cell, width: usize, expanded: bool) -> Vec<String> {
        cell.lines(width, expanded).iter().map(line_text).collect()
    }

    #[test]
    fn user_cell_shows_prompt_on_a_full_width_band() {
        let mut cell = Cell::new(CellKind::User { text: "fix the bug".into() });
        let lines = cell.lines(40, false).to_vec();
        assert_eq!(line_text(&lines[0]).trim_end(), "› fix the bug");
        assert_eq!(line_text(&lines[0]).chars().count(), 41, "padded to the full width");
        assert!(lines[0].spans.iter().all(|s| s.style.bg.is_some()));
    }

    #[test]
    fn expanding_shows_more_output_and_failed_tools_show_fifteen_lines() {
        let output = (1..=40).map(|n| format!("line{n}")).collect::<Vec<_>>().join("\n");
        let make = |ok| {
            Cell::new(CellKind::Tool {
                call_id: "c".into(),
                name: "run_command".into(),
                arguments: r#"{"command":"cargo test"}"#.into(),
                result: Some(ToolResult { ok, output: output.clone() }),
                diff: None,
            })
        };
        let ok_lines = texts(&mut make(true), 60, false);
        assert!(ok_lines.iter().any(|l| l.contains("… +35 lines · Ctrl+O to expand")), "{ok_lines:?}");
        let failed = texts(&mut make(false), 60, false);
        assert!(failed.iter().any(|l| l.contains("line15")) && !failed.iter().any(|l| l.contains("line16")));
        let expanded = texts(&mut make(true), 60, true);
        assert!(expanded.iter().any(|l| l.contains("line40")));
        assert!(!expanded.iter().any(|l| l.contains("Ctrl+O")));
    }

    #[test]
    fn long_diffs_expand_with_ctrl_o() {
        let diff: String = (0..60).map(|n| format!("+l{n}\n")).collect();
        let mut cell = Cell::new(CellKind::Diff { title: "d".into(), diff });
        assert!(texts(&mut cell, 60, false).iter().any(|l| l.contains("more diff lines · Ctrl+O to expand")));
        assert!(texts(&mut cell, 60, true).iter().any(|l| l.contains("+l59")));
    }

    #[test]
    fn attachments_are_summarized() {
        let text = "## Attached file: a.rs\n```\nfn a() {}\n```\n\nexplain it";
        assert_eq!(summarize_attachments(text), "📎 a.rs\nexplain it");
    }

    #[test]
    fn reasoning_collapses_to_one_line() {
        let mut cell = Cell::new(CellKind::Reasoning {
            text: "thought one\nthought two".into(),
        });
        let collapsed = texts(&mut cell, 60, false);
        assert_eq!(collapsed.len(), 1);
        assert!(collapsed[0].starts_with("▸ Thinking…"));
        let expanded = texts(&mut cell, 60, true);
        assert!(expanded.iter().any(|l| l.contains("thought two")));
    }

    #[test]
    fn tool_cell_describes_command_and_output() {
        let mut cell = Cell::new(CellKind::Tool {
            call_id: "c1".into(),
            name: "run_command".into(),
            arguments: r#"{"command":"cargo test"}"#.into(),
            result: Some(ToolResult {
                ok: true,
                output: "line1\nline2\nline3\nline4\nline5\nline6\nline7".into(),
            }),
            diff: None,
        });
        let out = texts(&mut cell, 60, false);
        assert_eq!(out[0], "✔ Ran cargo test");
        assert_eq!(out[1], "  │ line1");
        assert!(out.iter().any(|l| l.contains("… +2 lines")));
    }

    #[test]
    fn failed_and_pending_tools_have_markers() {
        let make = |result| {
            Cell::new(CellKind::Tool {
                call_id: "c".into(),
                name: "edit_file".into(),
                arguments: r#"{"path":"src/a.rs"}"#.into(),
                result,
                diff: None,
            })
        };
        assert_eq!(texts(&mut make(None), 40, false)[0], "⋯ Edited src/a.rs");
        let mut failed = make(Some(ToolResult {
            ok: false,
            output: "Tool error: nope".into(),
        }));
        let out = texts(&mut failed, 40, false);
        assert_eq!(out[0], "✖ Edited src/a.rs");
        assert_eq!(out[1], "  │ Tool error: nope");
    }

    #[test]
    fn diff_cell_colors_changes() {
        let mut cell = Cell::new(CellKind::Diff {
            title: "Changes (1 file)".into(),
            diff: "--- a/x\n+++ b/x\n-old\n+new\n".into(),
        });
        let lines = cell.lines(40, false);
        let added = lines.iter().find(|l| line_text(l).trim() == "+new").unwrap();
        assert_eq!(added.spans.last().unwrap().style.fg, Some(theme::ok()));
    }

    #[test]
    fn sub_agent_cells_are_tagged() {
        let mut cell = Cell::from_message("m1", Some("explorer scout".into()), CellKind::Assistant { text: "hi".into() });
        let out = texts(&mut cell, 40, false);
        assert_eq!(out[0], "[explorer scout]");
        assert_eq!(out[1], "• hi");
    }

    #[test]
    fn describes_known_tools() {
        assert_eq!(
            describe_tool("move_file", r#"{"from":"a","to":"b"}"#),
            ("Moved".to_string(), "a -> b".to_string())
        );
        assert_eq!(describe_tool("mystery", "{}").0, "mystery");
    }
}
