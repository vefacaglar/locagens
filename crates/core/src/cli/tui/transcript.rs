//! Transcript cells and their rendering into wrapped ratatui lines.

use super::diff::diff_lines;
use super::markdown::render_markdown;
use super::wrap::{sanitize, truncate_to_width, wrap_text, wrap_with_prefix};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

/// How many output lines a tool cell shows before truncating.
const TOOL_OUTPUT_LINES: usize = 5;
/// How many diff lines a diff cell shows before truncating.
const DIFF_LINES: usize = 40;

#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub ok: bool,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CellKind {
    User { text: String, queued: bool },
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
    Completed(String),
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
}

impl Cell {
    pub fn new(kind: CellKind) -> Self {
        Self {
            msg_id: None,
            agent: None,
            kind,
            cache: None,
        }
    }

    pub fn from_message(msg_id: &str, agent: Option<String>, kind: CellKind) -> Self {
        Self {
            msg_id: (!msg_id.is_empty()).then(|| msg_id.to_string()),
            agent,
            kind,
            cache: None,
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

    fn render(&self, width: usize, expanded: bool) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if let Some(agent) = &self.agent {
            lines.push(Line::from(Span::styled(
                format!("[{agent}]"),
                Style::default().fg(Color::Magenta),
            )));
        }
        match &self.kind {
            CellKind::User { text, queued } => render_user(&mut lines, text, *queued, width),
            CellKind::Assistant { text } => render_assistant(&mut lines, text, width),
            CellKind::Reasoning { text } => render_reasoning(&mut lines, text, width, expanded),
            CellKind::Tool {
                name,
                arguments,
                result,
                diff,
                ..
            } => render_tool(&mut lines, name, arguments, result.as_ref(), diff.as_deref(), width),
            CellKind::Diff { title, diff } => render_diff(&mut lines, title, diff, width),
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
                Style::default().fg(Color::Red),
                vec![Span::styled("✖ ", Style::default().fg(Color::Red))],
                vec![Span::raw("  ")],
            )),
            CellKind::Completed(text) => lines.push(Line::from(Span::styled(
                format!("── {text} ──"),
                dim(),
            ))),
        }
        if !matches!(self.kind, CellKind::Reasoning { .. } | CellKind::Notice(_)) {
            lines.push(Line::default());
        }
        lines
    }
}

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

fn render_user(lines: &mut Vec<Line<'static>>, text: &str, queued: bool, width: usize) {
    let marker = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let mut body = wrap_text(
        &summarize_attachments(text),
        width,
        Style::default().add_modifier(Modifier::BOLD),
        vec![Span::styled("› ", marker)],
        vec![Span::raw("  ")],
    );
    if queued {
        if let Some(last) = body.last_mut() {
            last.spans.push(Span::styled("  (queued)", Style::default().fg(Color::Yellow)));
        }
    }
    lines.extend(body);
}

fn render_assistant(lines: &mut Vec<Line<'static>>, text: &str, width: usize) {
    let mut body = render_markdown(text, width.saturating_sub(2));
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
) {
    let (marker, marker_style) = match result {
        None => ("⋯", Style::default().fg(Color::Yellow)),
        Some(r) if r.ok => ("✔", Style::default().fg(Color::Green)),
        Some(_) => ("✖", Style::default().fg(Color::Red)),
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
        push_limited_diff(lines, diff, width.saturating_sub(4), "  ");
    }
    if let Some(result) = result {
        push_tool_output(lines, name, result, width);
    }
}

fn push_limited_diff(lines: &mut Vec<Line<'static>>, diff: &str, width: usize, indent: &str) {
    let rendered = diff_lines(diff, width);
    let total = rendered.len();
    for mut line in rendered.into_iter().take(DIFF_LINES) {
        line.spans.insert(0, Span::raw(indent.to_string()));
        lines.push(line);
    }
    if total > DIFF_LINES {
        lines.push(Line::from(Span::styled(
            format!("{indent}… +{} more diff lines", total - DIFF_LINES),
            dim(),
        )));
    }
}

fn push_tool_output(lines: &mut Vec<Line<'static>>, name: &str, result: &ToolResult, width: usize) {
    let quiet = matches!(
        name,
        "read_file" | "list_directory" | "search_files" | "set_chat_title" | "update_plan" | "remember"
    );
    if result.ok && quiet {
        return;
    }
    let style = if result.ok { dim() } else { Style::default().fg(Color::Red) };
    let output = sanitize(result.output.trim_end());
    if output.is_empty() {
        return;
    }
    let all: Vec<&str> = output.lines().collect();
    for line in all.iter().take(TOOL_OUTPUT_LINES) {
        let text = truncate_to_width(line, width.saturating_sub(4));
        lines.push(Line::from(vec![
            Span::styled("  │ ", dim()),
            Span::styled(text, style),
        ]));
    }
    if all.len() > TOOL_OUTPUT_LINES {
        lines.push(Line::from(Span::styled(
            format!("  … +{} lines", all.len() - TOOL_OUTPUT_LINES),
            dim(),
        )));
    }
}

fn render_diff(lines: &mut Vec<Line<'static>>, title: &str, diff: &str, width: usize) {
    lines.push(Line::from(vec![
        Span::styled("• ", dim()),
        Span::styled(title.to_string(), Style::default().add_modifier(Modifier::BOLD)),
    ]));
    push_limited_diff(lines, diff, width.saturating_sub(2), "  ");
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
        "spawn_agents" | "spawn_agent" => ("Spawned agents".into(), String::new()),
        "wait_agent" | "send_input" | "close_agent" | "list_agents" => {
            (name.replace('_', " "), first(&["agent_id", "id"]))
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
    fn user_cell_shows_prompt_and_queue_marker() {
        let mut cell = Cell::new(CellKind::User {
            text: "fix the bug".into(),
            queued: true,
        });
        let out = texts(&mut cell, 40, false);
        assert_eq!(out[0], "› fix the bug  (queued)");
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
        assert_eq!(added.spans.last().unwrap().style.fg, Some(Color::Green));
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
