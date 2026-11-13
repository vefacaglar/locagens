//! Drawing: header, transcript, status line, composer, popup, footer and
//! the modal overlays.

use super::app::{format_elapsed, App};
use super::diff::diff_lines;
use super::overlay::{Approval, Overlay, QuestionFlow};
use super::popup::{Popup, PopupKind};
use super::wrap::{truncate_to_width, wrap_text};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const MAX_COMPOSER_ROWS: usize = 8;
const MAX_POPUP_ROWS: usize = 8;

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let inner_width = area.width.saturating_sub(2) as usize;
    app.composer.set_width(inner_width.saturating_sub(1));
    let rows = app.composer.visual_lines().len().clamp(1, MAX_COMPOSER_ROWS) as u16;
    let [header, transcript, status, composer, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(rows + 2),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(f, app, header);
    draw_transcript(f, app, transcript);
    draw_status(f, app, status);
    draw_composer(f, app, composer);
    draw_footer(f, app, footer);
    if app.overlay.is_none() {
        if let Some(popup) = &app.popup {
            draw_popup(f, popup, composer, transcript);
        }
    }
    draw_overlay(f, app, area);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let project = app.project.display().to_string();
    let line = Line::from(vec![
        Span::styled(" lcgn ", Style::default().fg(Color::Black).bg(Color::Cyan)),
        Span::styled(format!(" {project}"), dim()),
        Span::raw("  "),
        Span::styled(app.display_title(), Style::default().add_modifier(Modifier::BOLD)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn welcome_lines() -> Vec<Line<'static>> {
    let title = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    vec![
        Line::default(),
        Line::from(Span::styled("  Locagens", title)),
        Line::from(Span::styled("  Type a task and press Enter. The agent works inside this folder.", dim())),
        Line::default(),
        Line::from(Span::styled("  /  commands    @  mention a file    Shift+Tab  plan/build", dim())),
        Line::from(Span::styled("  Esc  interrupt    Ctrl+O  show reasoning    Ctrl+D  quit", dim())),
    ]
}

fn draw_transcript(f: &mut Frame, app: &mut App, area: Rect) {
    app.viewport.height = area.height as usize;
    if app.cells.is_empty() {
        app.viewport.total = 0;
        f.render_widget(Paragraph::new(welcome_lines()), area);
        return;
    }
    let width = (area.width as usize).saturating_sub(1).max(10);
    let expanded = app.show_reasoning;
    let mut total = 0;
    for cell in app.cells.iter_mut() {
        total += cell.lines(width, expanded).len();
    }
    app.viewport.total = total;
    let top = app.viewport.resolve_top();
    let bottom = top + area.height as usize;

    let mut visible: Vec<Line<'static>> = Vec::with_capacity(area.height as usize);
    let mut offset = 0;
    for cell in app.cells.iter_mut() {
        let lines = cell.lines(width, expanded);
        let (start, end) = (offset, offset + lines.len());
        offset = end;
        if end <= top {
            continue;
        }
        if start >= bottom {
            break;
        }
        let from = top.saturating_sub(start);
        let to = lines.len().min(bottom - start);
        visible.extend_from_slice(&lines[from..to]);
    }
    f.render_widget(Paragraph::new(visible), area);

    if !app.viewport.follow && area.height > 0 {
        let label = " ↓ End: jump to latest ";
        let w = label.chars().count() as u16;
        if area.width > w {
            let hint = Rect::new(area.right() - w, area.bottom() - 1, w, 1);
            f.render_widget(
                Paragraph::new(Span::styled(label, Style::default().add_modifier(Modifier::REVERSED))),
                hint,
            );
        }
    }
}

/// "Working" text with a bright spot sweeping across it.
fn shimmer_spans(text: &str, tick: usize) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let span_len = chars.len() + 4;
    let center = (tick % span_len) as isize - 2;
    chars
        .iter()
        .enumerate()
        .map(|(index, ch)| {
            let distance = (index as isize - center).abs();
            let style = match distance {
                0 => Style::default().add_modifier(Modifier::BOLD),
                1 => Style::default(),
                _ => dim(),
            };
            Span::styled(ch.to_string(), style)
        })
        .collect()
}

fn status_line(app: &App) -> Line<'static> {
    match &app.overlay {
        Some(Overlay::Approval(_)) => {
            return Line::from(Span::styled(
                "⏸ Waiting for your approval",
                Style::default().fg(Color::Yellow),
            ));
        }
        Some(Overlay::Question(_)) => {
            return Line::from(Span::styled(
                "⏸ Waiting for your answer",
                Style::default().fg(Color::Yellow),
            ));
        }
        None => {}
    }
    if app.quit_armed() {
        return Line::from(Span::styled(
            "Press Ctrl+C again to quit",
            Style::default().fg(Color::Yellow),
        ));
    }
    if !app.busy {
        return Line::default();
    }
    let frame = SPINNER_FRAMES[app.spinner % SPINNER_FRAMES.len()];
    let label = app
        .patch_progress
        .clone()
        .or_else(|| app.running_tool())
        .unwrap_or_else(|| "Working".to_string());
    let mut spans = vec![Span::styled(format!("{frame} "), Style::default().fg(Color::Cyan))];
    spans.extend(shimmer_spans(&label, app.spinner));
    let elapsed = app.elapsed().map(format_elapsed).unwrap_or_default();
    spans.push(Span::styled(format!("  {elapsed} · Esc to interrupt"), dim()));
    Line::from(spans)
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    f.render_widget(Paragraph::new(status_line(app)), area);
}

fn draw_composer(f: &mut Frame, app: &App, area: Rect) {
    let border = if app.busy { Color::Yellow } else { Color::Cyan };
    let title = if app.busy {
        " Message · Enter queues while running "
    } else {
        " Message "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.composer.is_empty() {
        let placeholder = Span::styled("Ask anything…  (/ for commands, @ to mention files)", dim());
        f.render_widget(Paragraph::new(placeholder), inner);
    } else {
        let text = app.composer.text();
        let rows = app.composer.visual_lines();
        let (cursor_row, _) = app.composer.cursor_row_col();
        let height = inner.height as usize;
        let first = cursor_row.saturating_sub(height.saturating_sub(1));
        let lines: Vec<Line> = rows
            .iter()
            .skip(first)
            .take(height)
            .map(|row| Line::from(text[row.start..row.end].to_string()))
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }
    if app.overlay.is_none() {
        let (row, col) = app.composer.cursor_row_col();
        let height = inner.height as usize;
        let first = row.saturating_sub(height.saturating_sub(1));
        let x = inner.x + (col as u16).min(inner.width.saturating_sub(1));
        let y = inner.y + (row - first) as u16;
        f.set_cursor_position((x, y));
    }
}

fn format_tokens(tokens: i64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1e6)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1e3)
    } else {
        tokens.to_string()
    }
}

fn footer_left(app: &App) -> Vec<Span<'static>> {
    let model = app
        .model
        .as_ref()
        .map(|m| m.label())
        .unwrap_or_else(|| "no model - use /model".to_string());
    let thinking = app.thinking.as_deref().unwrap_or("off");
    let mut parts = vec![
        Span::styled(format!(" {model}"), Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(format!(" · think:{thinking}"), dim()),
        Span::styled(format!(" · {}", app.mode_label()), Style::default().fg(Color::Cyan)),
    ];
    if let Some(session) = &app.session {
        let short: String = session.chars().filter(|c| c.is_alphanumeric()).take(8).collect();
        parts.push(Span::styled(format!(" · #{short}"), dim()));
    }
    if app.usage.total_tokens > 0 {
        parts.push(Span::styled(format!(" · {} tok", format_tokens(app.usage.total_tokens)), dim()));
        if app.usage.cost > 0.0 {
            parts.push(Span::styled(format!(" · ${:.4}", app.usage.cost), dim()));
        }
    }
    if let Some(left) = app.usage.context_left {
        let color = if left < 15 { Color::Red } else { Color::Reset };
        parts.push(Span::styled(format!(" · {left}% context left"), Style::default().fg(color).add_modifier(Modifier::DIM)));
    }
    parts
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let left = footer_left(app);
    let left_width: usize = left.iter().map(|s| s.content.chars().count()).sum();
    f.render_widget(Paragraph::new(Line::from(left)), area);
    let hints = "Enter send · Ctrl+J newline · / · @ ";
    let hint_width = hints.chars().count();
    if area.width as usize > left_width + hint_width + 2 {
        let rect = Rect::new(area.right() - hint_width as u16, area.y, hint_width as u16, 1);
        f.render_widget(Paragraph::new(Span::styled(hints, dim())).alignment(Alignment::Right), rect);
    }
}

fn draw_popup(f: &mut Frame, popup: &Popup, composer: Rect, transcript: Rect) {
    let rows = popup.items.len().clamp(1, MAX_POPUP_ROWS) as u16;
    let height = (rows + 2).min(transcript.height + 1);
    let y = composer.y.saturating_sub(height).max(transcript.y);
    let area = Rect::new(composer.x, y, composer.width, composer.y - y);
    if area.height < 3 {
        return;
    }
    let title = match popup.kind {
        PopupKind::Slash => " Commands · Tab complete ",
        PopupKind::File => " Files · Tab insert ",
        PopupKind::Review => " Review target · Enter run ",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(dim())
        .title(title);
    f.render_widget(Clear, area);
    if popup.items.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(" searching…", dim())).block(block),
            area,
        );
        return;
    }
    let width = area.width.saturating_sub(4) as usize;
    let items: Vec<ListItem> = popup
        .items
        .iter()
        .map(|item| {
            let label = truncate_to_width(&item.label, width);
            let mut spans = vec![Span::styled(label.clone(), Style::default().fg(Color::Cyan))];
            let used = label.chars().count() + 2;
            if !item.detail.is_empty() && used + 4 < width {
                spans.push(Span::styled(
                    format!("  {}", truncate_to_width(&item.detail, width - used)),
                    dim(),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_selected(Some(popup.selected));
    f.render_stateful_widget(list, area, &mut state);
}

// ---- overlays -------------------------------------------------------------

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn draw_overlay(f: &mut Frame, app: &mut App, area: Rect) {
    match app.overlay.as_mut() {
        Some(Overlay::Approval(approval)) => draw_approval(f, approval, area),
        Some(Overlay::Question(flow)) => draw_question(f, flow, area),
        None => {}
    }
}

fn approval_body(approval: &Approval, width: usize) -> Vec<Line<'static>> {
    let mut body: Vec<Line<'static>> = Vec::new();
    for detail in &approval.details {
        body.extend(wrap_text(detail, width, Style::default(), Vec::new(), Vec::new()));
    }
    if let Some(diff) = &approval.diff {
        body.push(Line::default());
        body.extend(diff_lines(diff, width));
    }
    body
}

fn draw_approval(f: &mut Frame, approval: &mut Approval, area: Rect) {
    let width = area.width.saturating_sub(4).min(110);
    let inner_width = width.saturating_sub(4) as usize;
    let body = approval_body(approval, inner_width.max(10));
    let height = (body.len() as u16 + 6).max(9).min(area.height.saturating_sub(2).max(3));
    let rect = centered(area, width, height);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow))
        .title(format!(" Permission required · {} ", approval.tool_name));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let [title, content, keys] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    f.render_widget(
        Paragraph::new(Span::styled(
            approval.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        title,
    );
    let max_scroll = body.len().saturating_sub(content.height as usize);
    approval.scroll = approval.scroll.min(max_scroll);
    let visible: Vec<Line<'static>> = body
        .into_iter()
        .skip(approval.scroll)
        .take(content.height as usize)
        .collect();
    f.render_widget(Paragraph::new(visible), content);
    let key = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("o", key),
            Span::raw(" once  "),
            Span::styled("r", key),
            Span::raw(" this run  "),
            Span::styled("p", key),
            Span::raw(" project  "),
            Span::styled("a", key),
            Span::raw(" always  "),
            Span::styled("d", key),
            Span::raw(" deny  "),
            Span::styled("↑↓", key),
            Span::styled(" scroll", dim()),
        ])),
        keys,
    );
}

fn question_lines(flow: &QuestionFlow, width: usize) -> Vec<Line<'static>> {
    let Some(item) = flow.current() else {
        return Vec::new();
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    if !item.header.is_empty() {
        lines.push(Line::from(Span::styled(
            item.header.clone(),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )));
    }
    lines.extend(wrap_text(
        &item.text,
        width,
        Style::default().add_modifier(Modifier::BOLD),
        Vec::new(),
        Vec::new(),
    ));
    lines.push(Line::default());
    for (index, option) in item.options.iter().enumerate() {
        let picked = flow.picked.get(index).copied().unwrap_or(false);
        let mark = match (item.multi, picked) {
            (true, true) => "[x]",
            (true, false) => "[ ]",
            (false, true) => "(•)",
            (false, false) => "( )",
        };
        let here = index == flow.cursor && !flow.note_focus;
        let style = if here {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(
            format!("{} {mark} {option}", if here { "›" } else { " " }),
            style,
        )));
    }
    lines.push(Line::default());
    let caret = if flow.note_focus { "▌" } else { "" };
    lines.push(Line::from(vec![
        Span::styled("Note: ", dim()),
        Span::raw(format!("{}{caret}", flow.note)),
    ]));
    lines
}

fn draw_question(f: &mut Frame, flow: &QuestionFlow, area: Rect) {
    let width = area.width.saturating_sub(4).min(90);
    let body = question_lines(flow, width.saturating_sub(4) as usize);
    let height = (body.len() as u16 + 4).max(8).min(area.height.saturating_sub(2).max(3));
    let rect = centered(area, width, height);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(format!(" Question {}/{} ", flow.index + 1, flow.items.len().max(1)));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let [content, keys] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    f.render_widget(Paragraph::new(body), content);
    f.render_widget(
        Paragraph::new(Span::styled(
            "↑↓ move · Space select · Enter confirm · Tab note · Esc skip",
            dim(),
        )),
        keys,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::models::ResolvedModel;
    use crate::cli::tui::transcript::CellKind;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use serde_json::json;
    use std::path::PathBuf;

    fn app() -> App {
        App::new(
            PathBuf::from("C:/work/demo"),
            Some(ResolvedModel {
                provider_id: "acme".into(),
                model: "turbo-1".into(),
            }),
            Some("high".into()),
            "accept_edits".into(),
            false,
            Some("abcdef123456".into()),
        )
    }

    fn render(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn smoke_renders_footer_header_and_welcome() {
        let mut app = app();
        let screen = render(&mut app, 100, 24);
        assert!(screen.contains("acme/turbo-1"), "{screen}");
        assert!(screen.contains("think:high"));
        assert!(screen.contains("Build"));
        assert!(screen.contains("#abcdef12"));
        assert!(screen.contains("C:/work/demo"));
        assert!(screen.contains("Ask anything"));
        assert!(screen.contains("Locagens"));
    }

    #[test]
    fn smoke_renders_transcript_messages_and_status() {
        let mut app = app();
        app.push(CellKind::User { text: "refactor the parser".into(), queued: false });
        app.on_run_event(&json!({"type": "message_updated", "message": {
            "id": "m1", "role": "assistant", "content": "Sure, **done**."}}));
        app.begin_turn();
        let screen = render(&mut app, 100, 24);
        assert!(screen.contains("› refactor the parser"), "{screen}");
        assert!(screen.contains("• Sure, done."), "{screen}");
        assert!(screen.contains("Working"));
        assert!(screen.contains("Esc to interrupt"));
    }

    #[test]
    fn smoke_renders_the_approval_overlay_with_diff() {
        let mut app = app();
        app.on_run_event(&json!({"type": "permission_requested",
            "toolCall": {"id": "c1", "function": {"name": "write_file", "arguments": "{}"}},
            "preview": {"action": "edit", "path": "src/a.rs", "oldContent": "x\ny\n", "newContent": "x\nz\n"}}));
        let screen = render(&mut app, 100, 30);
        assert!(screen.contains("Edit this file?"), "{screen}");
        assert!(screen.contains("Permission required"));
        assert!(screen.contains("+z"));
        assert!(screen.contains("once"));
        assert!(screen.contains("Waiting for your approval"));
    }

    #[test]
    fn smoke_renders_the_question_overlay() {
        let mut app = app();
        app.on_run_event(&json!({"type": "question_requested",
            "questions": [{"question": "Which database?", "options": ["sqlite", "postgres"]}]}));
        let screen = render(&mut app, 100, 30);
        assert!(screen.contains("Which database?"), "{screen}");
        assert!(screen.contains("sqlite"));
    }

    #[test]
    fn smoke_renders_slash_popup() {
        let mut app = app();
        for ch in "/mo".chars() {
            app.on_key(ratatui_key(ch));
        }
        let screen = render(&mut app, 100, 24);
        assert!(screen.contains("/model"), "{screen}");
        assert!(screen.contains("show or switch models"));
    }

    fn ratatui_key(ch: char) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char(ch),
            crossterm::event::KeyModifiers::NONE,
        )
    }

    #[test]
    fn renders_in_a_tiny_terminal_without_panicking() {
        let mut app = app();
        app.push(CellKind::Assistant { text: "text ".repeat(50) });
        app.on_run_event(&json!({"type": "permission_requested",
            "toolCall": {"id": "c", "function": {"name": "run_command", "arguments": "{}"}},
            "preview": {"action": "command", "command": "ls"}}));
        let _ = render(&mut app, 20, 8);
        let _ = render(&mut app, 3, 3);
    }
}
