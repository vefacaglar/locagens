//! Drawing of the help, session picker and `/login` overlays.

use super::help::{command_rows, HelpView, KEY_GROUPS, TIPS};
use super::login::{InputPurpose, LoginFlow, Stage};
use super::sessions::SessionPicker;
use super::theme;
use super::ui::centered;
use super::wrap::truncate_to_width;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

fn dim() -> Style {
    theme::dim()
}

fn frame(f: &mut Frame, rect: Rect, title: &str) -> Rect {
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::accent()))
        .title(title.to_string());
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    inner
}

// ---- help ----------------------------------------------------------------

fn command_lines() -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        "Commands",
        Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD),
    ))];
    for (usage, description) in command_rows() {
        lines.push(Line::from(vec![
            Span::styled(format!("{usage:<18}"), Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(description, dim()),
        ]));
    }
    lines
}

fn key_lines() -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        "Keys",
        Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD),
    ))];
    for (group, keys) in KEY_GROUPS {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled((*group).to_string(), Style::default().add_modifier(Modifier::BOLD))));
        for (key, what) in *keys {
            lines.push(Line::from(vec![
                Span::styled(format!("{key:<19}"), Style::default().fg(theme::accent())),
                Span::styled((*what).to_string(), dim()),
            ]));
        }
    }
    lines.push(Line::default());
    lines.extend(TIPS.iter().map(|tip| Line::from(Span::styled((*tip).to_string(), dim()))));
    lines
}

pub fn draw_help(f: &mut Frame, help: &mut HelpView, area: Rect) {
    let width = area.width.saturating_sub(2).min(120);
    let height = area.height.saturating_sub(2).max(5);
    let rect = centered(area, width, height);
    let inner = frame(f, rect, " Help ");
    let [body, keys] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    let wide = body.width >= 96;
    let (commands, bindings) = (command_lines(), key_lines());
    let lines_total = if wide { commands.len().max(bindings.len()) } else { commands.len() + bindings.len() + 1 };
    help.scroll = help.scroll.min(lines_total.saturating_sub(body.height as usize));
    if wide {
        let [left, right] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(body);
        f.render_widget(Paragraph::new(commands).scroll((help.scroll as u16, 0)), left);
        f.render_widget(Paragraph::new(bindings).scroll((help.scroll as u16, 0)), right);
    } else {
        let mut all = commands;
        all.push(Line::default());
        all.extend(bindings);
        f.render_widget(Paragraph::new(all).scroll((help.scroll as u16, 0)), body);
    }
    f.render_widget(Paragraph::new(Span::styled("↑↓ scroll · Esc or ? close", dim())), keys);
}

// ---- session picker -------------------------------------------------------

pub fn draw_sessions(f: &mut Frame, picker: &SessionPicker, area: Rect) {
    let width = area.width.saturating_sub(4).min(110);
    let height = area.height.saturating_sub(4).clamp(6, 26);
    let rect = centered(area, width, height);
    let inner = frame(f, rect, " Resume a session ");
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
    let first = picker.selected.saturating_sub(rows.saturating_sub(1));
    let lines: Vec<Line<'static>> = if visible.is_empty() {
        vec![Line::from(Span::styled("No matching sessions", dim()))]
    } else {
        visible
            .iter()
            .enumerate()
            .skip(first)
            .take(rows)
            .map(|(index, row)| {
                let selected = index == picker.selected;
                let marker = if selected { "› " } else { "  " };
                let detail_width = row.detail.chars().count() + 2;
                let title_width = (list.width as usize).saturating_sub(detail_width + 4).max(8);
                let title = truncate_to_width(&row.title, title_width);
                let used = 2 + title.chars().count();
                let pad = (list.width as usize).saturating_sub(used + row.detail.chars().count()).max(1);
                let style = if selected {
                    Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let current = picker.current.as_deref() == Some(row.id.as_str());
                let mut spans = vec![Span::styled(format!("{marker}{title}"), style)];
                spans.push(Span::raw(" ".repeat(pad)));
                spans.push(Span::styled(row.detail.clone(), dim()));
                if current {
                    spans.push(Span::styled(" (open)", dim()));
                }
                Line::from(spans)
            })
            .collect()
    };
    f.render_widget(Paragraph::new(lines), list);
    let noun = if picker.total() == 1 { "session" } else { "sessions" };
    f.render_widget(
        Paragraph::new(Span::styled(
            format!("{} {noun} in this folder · type to search · Enter open · Esc close", picker.total()),
            dim(),
        )),
        keys,
    );
}

// ---- login ----------------------------------------------------------------

pub fn draw_login(f: &mut Frame, flow: &LoginFlow, area: Rect) {
    match &flow.stage {
        Stage::Pick { query, selected, error } => draw_login_pick(f, flow, query, *selected, error.as_deref(), area),
        Stage::Input { purpose, provider, text } => {
            let (title, prompt, shown, footer) = match purpose {
                InputPurpose::ApiKey => (
                    format!(" {} API key ", provider.name),
                    "Paste your API key. It is stored in ~/.locagens/auth.json and sent only to the provider.",
                    "•".repeat(text.chars().count()),
                    "Enter save · Esc back",
                ),
                InputPurpose::Domain => (
                    format!(" {} ", provider.name),
                    "GitHub Enterprise URL or domain (leave blank for github.com).",
                    text.clone(),
                    "Enter continue · Esc back",
                ),
            };
            let rect = centered(area, area.width.saturating_sub(4).min(80), 8);
            let inner = frame(f, rect, &title);
            let lines = vec![
                Line::from(Span::styled(prompt, dim())),
                Line::default(),
                Line::from(vec![
                    Span::styled("> ", Style::default().fg(theme::accent())),
                    Span::raw(shown),
                    Span::styled("▏", Style::default().fg(theme::accent())),
                ]),
                Line::default(),
                Line::from(Span::styled(footer, dim())),
            ];
            f.render_widget(Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }), inner);
        }
        Stage::Waiting { provider, info } => {
            let rect = centered(area, area.width.saturating_sub(4).min(86), 12);
            let inner = frame(f, rect, &format!(" Sign in to {} ", provider.name));
            let mut lines: Vec<Line<'static>> = Vec::new();
            match info {
                None => lines.push(Line::from(Span::styled("Starting sign-in…", dim()))),
                Some(info) => {
                    if let Some(code) = &info.code {
                        lines.push(Line::from(Span::styled("Enter this code:", dim())));
                        lines.push(Line::from(Span::styled(
                            format!("  {code}"),
                            Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD),
                        )));
                        lines.push(Line::from(Span::styled("at:", dim())));
                    } else {
                        lines.push(Line::from(Span::styled("Open this URL and sign in:", dim())));
                    }
                    lines.push(Line::from(format!("  {}", info.url)));
                    lines.push(Line::default());
                    lines.push(Line::from(Span::styled("Waiting for approval…", Style::default().fg(theme::warn()))));
                    if let Some(error) = &info.error {
                        lines.push(Line::from(Span::styled(error.clone(), Style::default().fg(theme::err()))));
                    }
                    if info.accepts_paste {
                        lines.push(Line::from(vec![
                            Span::styled("Or paste the redirect URL: ", dim()),
                            Span::raw(truncate_to_width(&info.paste, 40)),
                            Span::styled("▏", Style::default().fg(theme::accent())),
                        ]));
                    }
                }
            }
            lines.push(Line::default());
            let footer = match info {
                Some(info) if info.code.is_some() => "Enter open browser · c copy code · Esc cancel",
                _ => "Enter open browser · Esc cancel",
            };
            lines.push(Line::from(Span::styled(footer, dim())));
            f.render_widget(Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }), inner);
        }
    }
}

fn draw_login_pick(f: &mut Frame, flow: &LoginFlow, query: &str, selected: usize, error: Option<&str>, area: Rect) {
    let width = area.width.saturating_sub(4).min(90);
    let height = area.height.saturating_sub(4).clamp(7, 22);
    let rect = centered(area, width, height);
    let inner = frame(f, rect, " Connect a provider ");
    let [search, list, note, keys] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Search: ", dim()),
            Span::raw(query.to_string()),
            Span::styled("▏", Style::default().fg(theme::accent())),
        ])),
        search,
    );
    let visible = flow.visible();
    let rows = list.height as usize;
    let first = selected.saturating_sub(rows.saturating_sub(1));
    let lines: Vec<Line<'static>> = if visible.is_empty() {
        vec![Line::from(Span::styled("No matching providers", dim()))]
    } else {
        visible
            .iter()
            .enumerate()
            .skip(first)
            .take(rows)
            .map(|(index, row)| {
                let here = index == selected;
                let style = if here {
                    Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let marker = if here { "› " } else { "  " };
                let mut spans = vec![
                    Span::styled(format!("{marker}{:<22}", truncate_to_width(&row.name, 22)), style),
                    Span::styled(format!("{:<28}", truncate_to_width(&row.method, 28)), dim()),
                ];
                if !row.status.is_empty() {
                    spans.push(Span::styled(row.status.clone(), Style::default().fg(theme::ok())));
                }
                Line::from(spans)
            })
            .collect()
    };
    f.render_widget(Paragraph::new(lines), list);
    if let Some(error) = error {
        f.render_widget(
            Paragraph::new(Span::styled(truncate_to_width(error, inner.width as usize), Style::default().fg(theme::err()))),
            note,
        );
    }
    f.render_widget(
        Paragraph::new(Span::styled(
            "type to search · Enter connect · Tab use an API key instead · Esc close",
            dim(),
        )),
        keys,
    );
}
