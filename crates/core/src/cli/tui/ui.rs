//! Drawing: header, transcript, status line, composer, popup, footer and
//! the modal overlays.

use super::theme;
use super::app::{compact_tokens, format_elapsed, App};
use super::diff::diff_lines;
use super::overlay::{Approval, ModelPicker, Overlay, QuestionFlow};
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
const MAX_QUEUE_ROWS: usize = 3;

fn dim() -> Style {
    theme::dim()
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let inner_width = area.width.saturating_sub(2) as usize;
    app.composer.set_width(inner_width.saturating_sub(1));
    let rows = if app.history_search.is_some() {
        2
    } else {
        app.composer.visual_lines().len().clamp(1, MAX_COMPOSER_ROWS) as u16
    };
    let queue_rows = queue_strip_rows(app);
    let [header, transcript, status, queue, composer, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(queue_rows),
        Constraint::Length(rows + 2),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(f, app, header);
    draw_transcript(f, app, transcript);
    draw_status(f, app, status);
    draw_queue(f, app, queue);
    draw_composer(f, app, composer);
    draw_footer(f, app, footer);
    if app.overlay.is_none() {
        if let Some(popup) = &app.popup {
            draw_popup(f, popup, composer, transcript);
        }
    }
    draw_overlay(f, app, area);
}

/// `~/repos/app` instead of the full home path.
pub fn shorten_home(path: &str, home: Option<&std::path::Path>) -> String {
    let Some(home) = home.map(|h| h.display().to_string()).filter(|h| !h.is_empty()) else {
        return path.to_string();
    };
    match path.strip_prefix(home.as_str()) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with(['/', '\\']) => format!("~{rest}"),
        _ => path.to_string(),
    }
}

fn header_location(app: &App) -> String {
    let project = shorten_home(&app.project.display().to_string(), app.home.as_deref());
    match &app.branch {
        Some(branch) => format!("{project} ({branch})"),
        None => project,
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let title = app.display_title();
    // Badge, spaces and at least part of the title; the path gets the rest.
    let budget = (area.width as usize).saturating_sub(10 + title.chars().count().min(30));
    let line = Line::from(vec![
        Span::styled(" lcgn ", theme::badge()),
        Span::styled(format!(" {}", keep_path_tail(&header_location(app), budget)), dim()),
        Span::raw("  "),
        Span::styled(title, Style::default().add_modifier(Modifier::BOLD)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

/// A path cut from the left to `max` characters (`…\work\demo`), so the
/// folder name at its end stays visible.
fn keep_path_tail(path: &str, max: usize) -> String {
    let count = path.chars().count();
    if count <= max {
        return path.to_string();
    }
    let keep = max.saturating_sub(1);
    let tail: String = path.chars().skip(count - keep).collect();
    format!("…{tail}")
}

fn welcome_lines(app: &App) -> Vec<Line<'static>> {
    let title = Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD);
    if app.no_providers {
        return vec![
            Line::default(),
            Line::from(Span::styled("  Locagens", title)),
            Line::default(),
            Line::from("  No model provider is connected yet."),
            Line::from("  Press Enter to connect one, or type /login."),
            Line::default(),
            Line::from(Span::styled("  Credentials from Codex (~/.codex) and pi (~/.pi/agent) are used", dim())),
            Line::from(Span::styled("  automatically when present.", dim())),
        ];
    }
    vec![
        Line::default(),
        Line::from(Span::styled("  Locagens", title)),
        Line::from(Span::styled("  Type a task and press Enter. The agent works inside this folder.", dim())),
        Line::default(),
        Line::from(Span::styled("  /  commands    @  mention a file    Shift+Tab  switch mode", dim())),
        Line::from(Span::styled("  Esc  interrupt    Ctrl+O  show details    Ctrl+D  quit    ?  help", dim())),
    ]
}

fn draw_transcript(f: &mut Frame, app: &mut App, area: Rect) {
    app.viewport.height = area.height as usize;
    if app.cells.is_empty() {
        app.viewport.total = 0;
        f.render_widget(Paragraph::new(welcome_lines(app)), area);
        return;
    }
    let width = (area.width as usize).saturating_sub(1).max(10);
    let expanded = app.details;
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
    let waiting = |text: &'static str| Line::from(Span::styled(text, Style::default().fg(theme::warn())));
    match &app.overlay {
        Some(Overlay::Approval(_)) => return waiting("⏸ Waiting for your approval"),
        Some(Overlay::Question(_)) => return waiting("⏸ Waiting for your answer"),
        Some(Overlay::ModelPicker(_)) => {
            return Line::from(Span::styled("Select a model", Style::default().fg(theme::accent())));
        }
        Some(Overlay::Effort(_)) => {
            return Line::from(Span::styled("Select reasoning effort", Style::default().fg(theme::accent())));
        }
        Some(Overlay::Mode(_)) => {
            return Line::from(Span::styled("Select permissions", Style::default().fg(theme::accent())));
        }
        Some(Overlay::Sessions(_)) => {
            return Line::from(Span::styled("Resume a session", Style::default().fg(theme::accent())));
        }
        Some(Overlay::Login(_)) => {
            return Line::from(Span::styled("Connect a provider", Style::default().fg(theme::accent())));
        }
        Some(Overlay::Help(_)) => {
            return Line::from(Span::styled("Help", Style::default().fg(theme::accent())));
        }
        None => {}
    }
    if app.quit_armed() {
        return Line::from(Span::styled(
            "Press Ctrl+C again to quit",
            Style::default().fg(theme::warn()),
        ));
    }
    let flash = app.flash_text().map(str::to_string);
    if !app.busy {
        return match flash {
            Some(text) => Line::from(Span::styled(text, dim())),
            None => Line::default(),
        };
    }
    let label = app
        .patch_progress
        .clone()
        .or_else(|| app.running_tool())
        .unwrap_or_else(|| "Working".to_string());
    let mut spans = if app.reduced_motion {
        // No spinner and no shimmer: a static ellipsis and the plain label.
        vec![Span::styled("… ", Style::default().fg(theme::accent())), Span::raw(label)]
    } else {
        let frame = SPINNER_FRAMES[app.spinner % SPINNER_FRAMES.len()];
        let mut spans = vec![Span::styled(format!("{frame} "), Style::default().fg(theme::accent()))];
        spans.extend(shimmer_spans(&label, app.spinner));
        spans
    };
    let elapsed = app.elapsed().map(format_elapsed).unwrap_or_default();
    spans.push(Span::styled(format!("  {elapsed} · Esc to interrupt"), dim()));
    if let Some(text) = flash {
        spans.push(Span::styled(format!("  · {text}"), dim()));
    }
    Line::from(spans)
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    f.render_widget(Paragraph::new(status_line(app)), area);
}

/// Rows of the "Queued" strip above the composer (0 when nothing waits).
fn queue_strip_rows(app: &App) -> u16 {
    let count = app.queued_displays().len();
    match count {
        0 => 0,
        1..=MAX_QUEUE_ROWS => 1 + count as u16,
        _ => 2 + MAX_QUEUE_ROWS as u16,
    }
}

fn draw_queue(f: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 {
        return;
    }
    let queued = app.queued_displays();
    let mut lines = vec![Line::from(Span::styled(
        " Queued · sent after the current step · Up edits the newest",
        dim(),
    ))];
    let width = area.width.saturating_sub(5) as usize;
    for text in queued.iter().take(MAX_QUEUE_ROWS) {
        let first = text.lines().next().unwrap_or("");
        lines.push(Line::from(vec![
            Span::styled("  › ", dim()),
            Span::raw(truncate_to_width(first, width)),
        ]));
    }
    if queued.len() > MAX_QUEUE_ROWS {
        lines.push(Line::from(Span::styled(
            format!("  … +{} more", queued.len() - MAX_QUEUE_ROWS),
            dim(),
        )));
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// Composer border colour and title for the mode (UX-012): Build keeps the
/// accent, Plan is blue, Full Access is red. Running turns the border
/// yellow only in Build.
pub fn composer_frame(app: &App) -> (String, Color) {
    if app.history_search.is_some() {
        return (" History search · Enter use · Esc cancel ".to_string(), theme::accent());
    }
    let (label, color, build) = match app.mode.as_str() {
        "plan" => (" Plan · read-only", theme::plan(), false),
        "auto" => (" Full Access · no permission prompts", theme::err(), false),
        "chat" => (" Chat · no tools", theme::accent(), false),
        _ => (" Message", theme::accent(), true),
    };
    let (title, color) = match (app.busy, build) {
        (true, true) => (format!("{label} · Enter queues while running"), theme::warn()),
        (true, false) => (format!("{label} · Enter queues"), color),
        (false, _) => (label.to_string(), color),
    };
    (format!("{title} "), color)
}

fn draw_history_search(f: &mut Frame, app: &App, inner: Rect) {
    let Some(search) = &app.history_search else { return };
    let matches = search.matches(app.composer.history());
    let mut lines = vec![Line::from(vec![
        Span::styled("search: ", dim()),
        Span::raw(search.query.clone()),
        Span::styled("▏", Style::default().fg(theme::accent())),
    ])];
    let width = inner.width.saturating_sub(12) as usize;
    lines.push(match matches.get(search.selected) {
        Some(prompt) => {
            let first = prompt.lines().next().unwrap_or("");
            Line::from(vec![
                Span::styled("› ", Style::default().fg(theme::accent())),
                Span::styled(truncate_to_width(first, width), Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {}/{}", search.selected + 1, matches.len()), dim()),
            ])
        }
        None => Line::from(Span::styled("no matching prompt", dim())),
    });
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_composer(f: &mut Frame, app: &App, area: Rect) {
    let (title, border) = composer_frame(app);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.history_search.is_some() {
        draw_history_search(f, app, inner);
        return;
    }
    if app.composer.is_empty() {
        let placeholder = Span::styled("Ask anything…  / commands · @ files · ? help", dim());
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

/// `$0.01`, or `<$0.01` for a tiny non-zero cost.
fn format_cost(cost: f64) -> String {
    if cost > 0.0 && cost < 0.005 {
        "<$0.01".to_string()
    } else {
        format!("${cost:.2}")
    }
}

fn mode_color(app: &App) -> Color {
    match app.mode.as_str() {
        "plan" => theme::plan(),
        "auto" => theme::err(),
        _ => theme::accent(),
    }
}

fn footer_left(app: &App) -> Vec<Span<'static>> {
    let model = app
        .model
        .as_ref()
        .map(|m| m.label())
        .unwrap_or_else(|| "no model · /login or /model".to_string());
    let thinking = app.thinking.as_deref().unwrap_or("off");
    vec![
        Span::styled(format!(" {model}"), Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(format!(" · {thinking}"), dim()),
        Span::styled(format!(" · {}", app.mode_label()), Style::default().fg(mode_color(app))),
    ]
}

fn footer_right(app: &App) -> Vec<Span<'static>> {
    let mut parts = Vec::new();
    match (app.usage.context, app.usage.context_used()) {
        (_, Some(used)) if used > 85 => parts.push(Span::styled(
            format!("{used}% context used · /new starts fresh "),
            Style::default().fg(theme::err()),
        )),
        (Some((used, window)), _) => {
            let (used, window) = (compact_tokens(used), compact_tokens(window));
            parts.push(Span::styled(format!("{used}/{window} context"), dim()));
            if app.usage.cost > 0.0 {
                parts.push(Span::styled(format!(" · {} ", format_cost(app.usage.cost)), dim()));
            } else {
                parts.push(Span::raw(" "));
            }
        }
        _ if app.usage.cost > 0.0 => {
            parts.push(Span::styled(format!("{} ", format_cost(app.usage.cost)), dim()));
        }
        _ => {}
    }
    parts
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let left = footer_left(app);
    let left_width: usize = left.iter().map(|s| s.content.chars().count()).sum();
    f.render_widget(Paragraph::new(Line::from(left)), area);
    let right = footer_right(app);
    let right_width: usize = right.iter().map(|s| s.content.chars().count()).sum();
    if right_width > 0 && area.width as usize > left_width + right_width + 2 {
        let rect = Rect::new(area.right() - right_width as u16, area.y, right_width as u16, 1);
        f.render_widget(Paragraph::new(Line::from(right)).alignment(Alignment::Right), rect);
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
        PopupKind::Model => " Models · Enter switch · Tab insert ",
        PopupKind::Thinking => " Reasoning level · Enter set ",
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
            let mut spans = vec![Span::styled(label.clone(), Style::default().fg(theme::accent()))];
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

pub(super) fn centered(area: Rect, width: u16, height: u16) -> Rect {
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
        Some(Overlay::ModelPicker(picker)) => draw_model_picker(f, picker, area),
        Some(Overlay::Effort(picker) | Overlay::Mode(picker)) => super::ui_overlays::draw_option_picker(f, picker, area),
        Some(Overlay::Help(help)) => super::ui_overlays::draw_help(f, help, area),
        Some(Overlay::Sessions(picker)) => super::ui_overlays::draw_sessions(f, picker, area),
        Some(Overlay::Login(flow)) => super::ui_overlays::draw_login(f, flow, area),
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
    let height = (body.len() as u16 + 8).max(10).min(area.height.saturating_sub(2).max(3));
    let rect = centered(area, width, height);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::warn()))
        .title(format!(" Permission required · {} ", approval.tool_name));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let [title, content, keys] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .areas(inner);

    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(approval.title.clone(), Style::default().add_modifier(Modifier::BOLD))),
            Line::from(Span::styled(approval.reason.clone().unwrap_or_default(), dim())),
        ]),
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
    let mut legend = vec![approval_legend(approval)];
    if let (false, Some(label)) = (approval.one_time_only, &approval.grant_label) {
        legend.push(Line::from(Span::styled(format!("p / a cover: {label}"), dim())));
    }
    f.render_widget(Paragraph::new(legend), keys);
}

/// The key legend of the approval overlay. Standing grants (project,
/// always) need the key twice; after the first press the line says so.
pub fn approval_legend(approval: &Approval) -> Line<'static> {
    match approval.pending_grant {
        Some("allow_project") => {
            return Line::from(Span::styled(
                "press p again to allow in this project (d to deny)",
                Style::default().fg(theme::warn()),
            ));
        }
        Some("allow_always") => {
            return Line::from(Span::styled(
                "press a again to always allow (d to deny)",
                Style::default().fg(theme::warn()),
            ));
        }
        _ => {}
    }
    let key = Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD);
    if approval.one_time_only {
        return Line::from(vec![
            Span::styled("y", key),
            Span::raw(" once  "),
            Span::styled("d", key),
            Span::raw(" deny  "),
            Span::styled("↑↓", key),
            Span::styled(" scroll", dim()),
        ]);
    }
    Line::from(vec![
        Span::styled("y", key),
        Span::raw(" once  "),
        Span::styled("r", key),
        Span::raw(" this run  "),
        Span::styled("p×2", key),
        Span::raw(" project  "),
        Span::styled("a×2", key),
        Span::raw(" always  "),
        Span::styled("d", key),
        Span::raw(" deny  "),
        Span::styled("↑↓", key),
        Span::styled(" scroll", dim()),
    ])
}

fn question_lines(flow: &QuestionFlow, width: usize) -> Vec<Line<'static>> {
    let Some(item) = flow.current() else {
        return Vec::new();
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    if !item.header.is_empty() {
        lines.push(Line::from(Span::styled(
            item.header.clone(),
            Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD),
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
            Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)
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

fn draw_model_picker(f: &mut Frame, picker: &ModelPicker, area: Rect) {
    let width = area.width.saturating_sub(4).min(80);
    let height = area.height.saturating_sub(4).clamp(6, 24);
    let rect = centered(area, width, height);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::accent()))
        .title(" Select model ");
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let [search, list, keys] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Search: ", dim()),
            Span::raw(picker.query.clone()),
            Span::styled("▏", Style::default().fg(theme::accent())),
        ])),
        search,
    );
    let visible = picker.visible();
    let rows = list.height as usize;
    // Keep the highlighted row in view.
    let first = picker.selected.saturating_sub(rows.saturating_sub(1));
    let lines: Vec<Line<'static>> = if visible.is_empty() {
        vec![Line::from(Span::styled("No matching models", dim()))]
    } else {
        visible
            .iter()
            .enumerate()
            .skip(first)
            .take(rows)
            .map(|(index, item)| {
                let active = picker.current.as_deref() == Some(item.value.as_str());
                let marker = if index == picker.selected { "› " } else { "  " };
                let style = if index == picker.selected {
                    Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let mut spans = vec![Span::styled(format!("{marker}{}", item.value), style)];
                if active {
                    spans.push(Span::styled("  (current)", dim()));
                }
                if !item.detail.is_empty() {
                    spans.push(Span::styled(format!("  {}", item.detail), dim()));
                }
                Line::from(spans)
            })
            .collect()
    };
    f.render_widget(Paragraph::new(lines), list);
    f.render_widget(
        Paragraph::new(Span::styled(
            format!("{} models · type to search · ↑↓ move · Enter switch · Esc close", visible.len()),
            dim(),
        )),
        keys,
    );
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
        .border_style(Style::default().fg(theme::accent()))
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
        assert!(screen.contains("acme/turbo-1 · high · Build"), "{screen}");
        assert!(!screen.contains("#abcdef12"), "the session hash is gone from the footer");
        assert!(screen.contains("C:/work/demo"));
        assert!(screen.contains("? help"));
        assert!(screen.contains("Locagens"));
    }

    #[test]
    fn smoke_renders_transcript_messages_and_status() {
        let mut app = app();
        app.push(CellKind::User { text: "refactor the parser".into() });
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

    /// Renders `app` at a normal and a small size; prints both frames (see
    /// them with `--nocapture`) and returns the normal one.
    fn render_both(app: &mut App, name: &str) -> String {
        let normal = render(app, 90, 26);
        let small = render(app, 50, 14);
        println!("== {name} 90x26
{normal}
== {name} 50x14
{small}");
        normal
    }

    #[test]
    fn smoke_renders_the_model_picker() {
        use crate::cli::tui::popup::PopupItem;
        let mut app = app();
        let item = |value: &str| PopupItem { value: value.into(), label: value.into(), detail: String::new() };
        let all = vec![item("acme/turbo-1"), item("acme/turbo-2"), item("other/long-model-name-preview")];
        app.overlay = Some(Overlay::ModelPicker(crate::cli::tui::overlay::ModelPicker::new(all, Some("acme/turbo-1".into()))));
        let screen = render_both(&mut app, "model picker");
        assert!(screen.contains("acme/turbo-2"), "{screen}");
        assert!(screen.contains("other/long-model-name-preview"), "{screen}");
    }

    #[test]
    fn smoke_renders_the_effort_picker() {
        let mut app = app();
        app.open_effort_picker();
        let screen = render_both(&mut app, "effort picker");
        assert!(screen.contains("Reasoning effort"), "{screen}");
        assert!(screen.contains("xhigh"), "{screen}");
        assert!(screen.contains("high  (current)"), "{screen}");
        let small = render(&mut app, 50, 14);
        assert!(small.contains("Reasoning effort") && small.contains("(current)"), "{small}");
    }

    #[test]
    fn smoke_renders_the_mode_picker() {
        let mut app = app();
        app.open_mode_picker();
        let screen = render_both(&mut app, "mode picker");
        assert!(screen.contains("Ask permissions"), "{screen}");
        assert!(screen.contains("Full Access"), "{screen}");
        assert!(screen.contains("Build  (current)"), "{screen}");
        let small = render(&mut app, 50, 14);
        assert!(small.contains("Permissions") && small.contains("Full Access"), "{small}");
    }

    #[test]
    fn footer_keeps_model_effort_and_mode_in_step() {
        let mut app = app();
        let footer = |app: &App| footer_left(app).iter().map(|s| s.content.to_string()).collect::<String>();
        assert!(footer(&app).contains("acme/turbo-1 · high · Build"), "{}", footer(&app));
        app.cycle_mode();
        app.thinking = None;
        assert!(footer(&app).contains("acme/turbo-1 · off · Plan"), "{}", footer(&app));
        app.mode = "ask_permissions".into();
        assert!(footer(&app).contains("· Ask"), "{}", footer(&app));
    }

    #[test]
    fn smoke_renders_the_help_overlay() {
        let mut app = app();
        app.overlay = Some(Overlay::Help(crate::cli::tui::help::HelpView::default()));
        let screen = render_both(&mut app, "help");
        assert!(screen.contains("Commands"), "{screen}");
        // Long usages keep a gap before their description.
        assert!(screen.contains("/model [provider/id]  show or switch models"), "{screen}");
        // A 100-column terminal stacks the sections instead of cutting the
        // command descriptions at a second column.
        let medium = render(&mut app, 100, 30);
        assert!(medium.contains("reasoning effort (off..max)"), "{medium}");
    }

    #[test]
    fn smoke_renders_the_sessions_picker() {
        use crate::cli::tui::sessions::SessionPicker;
        let mut app = app();
        let run = |id: &str, title: &str| crate::db::Run {
            id: id.into(),
            title: title.into(),
            task: title.into(),
            model: "turbo-1".into(),
            status: "done".into(),
            updated_at: "2026-11-14T10:00:00Z".into(),
            ..Default::default()
        };
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-14T12:00:00Z").unwrap().with_timezone(&chrono::Utc);
        let runs = vec![run("r1", "Fix the parser"), run("r2", "Add a login flow for the very long provider name")];
        app.overlay = Some(Overlay::Sessions(SessionPicker::new(&runs, Some("r1".into()), now)));
        let screen = render_both(&mut app, "sessions");
        assert!(screen.contains("Fix the parser"), "{screen}");
    }

    #[test]
    fn smoke_renders_the_login_overlay() {
        use crate::cli::tui::login::{LoginFlow, LoginRow};
        let mut app = app();
        let row = |id: &str, name: &str, method: &str, status: &str| LoginRow {
            id: id.into(),
            name: name.into(),
            method: method.into(),
            status: status.into(),
            has_oauth: method.starts_with("Sign in"),
            keyless: false,
        };
        let rows = vec![
            row("openai", "OpenAI", "Sign in with ChatGPT", ""),
            row("opencode-go", "OpenCode Go", "API key", "✔ connected"),
        ];
        app.overlay = Some(Overlay::Login(LoginFlow::new(rows)));
        let screen = render_both(&mut app, "login");
        assert!(screen.contains("OpenCode Go"), "{screen}");
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

#[cfg(test)]
mod ux_tests {
    use super::*;
    use crate::cli::tui::wrap::line_text;
    use std::path::PathBuf;

    fn app(mode: &str) -> App {
        App::new(PathBuf::from("/p"), None, None, mode.into(), false, None)
    }

    #[test]
    fn composer_frame_names_the_mode() {
        let (title, color) = composer_frame(&app("accept_edits"));
        assert_eq!((title.as_str(), color), (" Message ", theme::accent()));
        let (title, color) = composer_frame(&app("plan"));
        assert_eq!((title.as_str(), color), (" Plan · read-only ", theme::plan()));
        let (title, color) = composer_frame(&app("auto"));
        assert_eq!((title.as_str(), color), (" Full Access · no permission prompts ", theme::err()));
        let mut busy = app("accept_edits");
        busy.busy = true;
        assert_eq!(composer_frame(&busy).1, theme::warn());
        let mut busy_auto = app("auto");
        busy_auto.busy = true;
        assert_eq!(composer_frame(&busy_auto).1, theme::err(), "yellow only in Build");
    }

    #[test]
    fn home_is_shortened_in_the_header() {
        let home = std::path::Path::new("/home/me");
        assert_eq!(shorten_home("/home/me/repos/app", Some(home)), "~/repos/app");
        assert_eq!(shorten_home("/home/me", Some(home)), "~");
        assert_eq!(shorten_home("/home/mex/app", Some(home)), "/home/mex/app");
        assert_eq!(shorten_home("/srv/app", None), "/srv/app");
    }

    #[test]
    fn long_header_paths_keep_their_folder_name() {
        assert_eq!(keep_path_tail("C:/work/demo", 20), "C:/work/demo");
        assert_eq!(keep_path_tail("C:/very/long/path/to/demo", 10), "…h/to/demo");
    }

    #[test]
    fn approval_legend_announces_the_second_press() {
        let event = serde_json::json!({"toolCall": {"function": {"name": "run_command"}}, "preview": {"action": "command", "command": "ls"}});
        let mut approval = Approval::from_event(&event);
        assert!(line_text(&approval_legend(&approval)).contains("a×2 always"));
        assert!(line_text(&approval_legend(&approval)).contains("p×2 project"));
        approval.pending_grant = Some("allow_always");
        assert_eq!(line_text(&approval_legend(&approval)), "press a again to always allow (d to deny)");
        approval.pending_grant = None;
        approval.one_time_only = true;
        assert!(!line_text(&approval_legend(&approval)).contains("always"));
    }

    #[test]
    fn footer_shows_context_and_cost_and_warns_when_low() {
        let mut app = app("accept_edits");
        app.usage = crate::cli::tui::app::UsageInfo { cost: 0.0123, context: Some((6_400, 1_000_000)) };
        let right: String = footer_right(&app).iter().map(|s| s.content.to_string()).collect();
        assert_eq!(right, "6.4k/1M context · $0.01 ");
        app.usage.context = Some((112_000, 128_000));
        let right: String = footer_right(&app).iter().map(|s| s.content.to_string()).collect();
        assert_eq!(right, "87% context used · /new starts fresh ");
    }

    #[test]
    fn welcome_offers_login_without_a_provider_and_status_flashes() {
        let mut app = app("accept_edits");
        app.no_providers = true;
        let text: String = welcome_lines(&app).iter().map(line_text).collect::<Vec<_>>().join("|");
        assert!(text.contains("Press Enter to connect one, or type /login."));
        app.flash("Mode set to plan");
        assert_eq!(line_text(&status_line(&app)), "Mode set to plan");
    }

    #[test]
    fn queue_strip_grows_with_messages() {
        let mut app = app("accept_edits");
        assert_eq!(queue_strip_rows(&app), 0);
        app.note_queued("a", "a");
        assert_eq!(queue_strip_rows(&app), 2);
        for text in ["b", "c", "d"] {
            app.note_queued(text, text);
        }
        assert_eq!(queue_strip_rows(&app), 5);
    }

    #[test]
    fn reduced_motion_status_is_static() {
        let mut app = app("accept_edits");
        app.busy = true;
        app.reduced_motion = true;
        let line = line_text(&status_line(&app));
        assert!(line.starts_with("… Working"), "{line}");
    }
}
