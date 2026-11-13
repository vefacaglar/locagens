//! Markdown -> styled, width-wrapped ratatui lines (pulldown-cmark).

use super::highlight::highlight_code;
use super::wrap::{display_width, sanitize, truncate_to_width, wrap_with_prefix};
use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Renders markdown source into lines no wider than `width` columns.
pub fn render_markdown(source: &str, width: usize) -> Vec<Line<'static>> {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let mut renderer = Renderer::new(width.max(8));
    for event in Parser::new_ext(source, options) {
        renderer.handle(event);
    }
    renderer.finish()
}

/// An enclosing block that contributes a line prefix.
enum Context {
    Quote,
    Item { marker: String, marker_used: bool },
}

struct CodeBuffer {
    lang: String,
    text: String,
}

#[derive(Default)]
struct TableBuffer {
    alignments: Vec<Alignment>,
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    in_head: bool,
}

struct Renderer {
    width: usize,
    lines: Vec<Line<'static>>,
    inline: Vec<Span<'static>>,
    styles: Vec<Style>,
    contexts: Vec<Context>,
    /// One entry per open list: `Some(next number)` for ordered lists.
    lists: Vec<Option<u64>>,
    code: Option<CodeBuffer>,
    table: Option<TableBuffer>,
    link_url: Option<String>,
}

impl Renderer {
    fn new(width: usize) -> Self {
        Self {
            width,
            lines: Vec::new(),
            inline: Vec::new(),
            styles: vec![Style::default()],
            contexts: Vec::new(),
            lists: Vec::new(),
            code: None,
            table: None,
            link_url: None,
        }
    }

    fn style(&self) -> Style {
        *self.styles.last().unwrap_or(&Style::default())
    }

    fn push_style(&mut self, patch: impl FnOnce(Style) -> Style) {
        let next = patch(self.style());
        self.styles.push(next);
    }

    fn pop_style(&mut self) {
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => {
                let style = self.style().fg(Color::Cyan);
                self.inline.push(Span::styled(sanitize(&code), style));
            }
            Event::SoftBreak => self.inline.push(Span::styled(" ", self.style())),
            Event::HardBreak => self.flush_inline(),
            Event::Rule => {
                self.flush_inline();
                let rule = "─".repeat(self.width.min(40));
                self.lines.push(Line::from(Span::styled(
                    rule,
                    Style::default().add_modifier(Modifier::DIM),
                )));
                self.blank_line();
            }
            Event::Html(html) | Event::InlineHtml(html) => self.text(&html),
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.flush_inline(),
            Tag::Heading { level, .. } => {
                self.flush_inline();
                self.push_style(|s| heading_style(s, level));
            }
            Tag::BlockQuote(_) => {
                self.flush_inline();
                self.contexts.push(Context::Quote);
            }
            Tag::CodeBlock(kind) => {
                self.flush_inline();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some(CodeBuffer {
                    lang,
                    text: String::new(),
                });
            }
            Tag::List(start) => {
                self.flush_inline();
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush_inline();
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let marker = format!("{number}. ");
                        *number += 1;
                        marker
                    }
                    _ => "• ".to_string(),
                };
                self.contexts.push(Context::Item {
                    marker,
                    marker_used: false,
                });
            }
            Tag::Table(alignments) => {
                self.flush_inline();
                self.table = Some(TableBuffer {
                    alignments,
                    ..TableBuffer::default()
                });
            }
            Tag::TableHead => {
                if let Some(table) = self.table.as_mut() {
                    table.in_head = true;
                }
            }
            Tag::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    table.current_row.clear();
                }
            }
            Tag::TableCell => self.inline.clear(),
            Tag::Emphasis => self.push_style(|s| s.add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(|s| s.add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self.push_style(|s| s.add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { dest_url, .. } => {
                self.link_url = Some(dest_url.to_string());
                self.push_style(|s| s.fg(Color::Cyan).add_modifier(Modifier::UNDERLINED));
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_inline();
                self.blank_line();
            }
            TagEnd::Heading(_) => {
                self.flush_inline();
                self.pop_style();
                self.blank_line();
            }
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                self.contexts.pop();
                self.blank_line();
            }
            TagEnd::CodeBlock => self.finish_code_block(),
            TagEnd::List(_) => {
                self.flush_inline();
                self.lists.pop();
                self.blank_line();
            }
            TagEnd::Item => {
                self.flush_inline();
                self.contexts.pop();
            }
            TagEnd::Table => self.finish_table(),
            TagEnd::TableHead => {
                if let Some(table) = self.table.as_mut() {
                    table.header = std::mem::take(&mut table.current_row);
                    table.in_head = false;
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    let row = std::mem::take(&mut table.current_row);
                    table.rows.push(row);
                }
            }
            TagEnd::TableCell => {
                let cell: String = self.inline.drain(..).map(|s| s.content.to_string()).collect();
                if let Some(table) = self.table.as_mut() {
                    table.current_row.push(cell);
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => self.pop_style(),
            TagEnd::Link => {
                self.pop_style();
                if let Some(url) = self.link_url.take() {
                    let dim = Style::default().add_modifier(Modifier::DIM);
                    self.inline.push(Span::styled(format!(" ({url})"), dim));
                }
            }
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if let Some(code) = self.code.as_mut() {
            code.text.push_str(text);
            return;
        }
        let style = self.style();
        self.inline.push(Span::styled(sanitize(text), style));
    }

    fn blank_line(&mut self) {
        let inside_item = self
            .contexts
            .iter()
            .any(|context| matches!(context, Context::Item { .. }));
        if !inside_item {
            self.lines.push(Line::default());
        }
    }

    /// The `(first, rest)` prefixes for the next block line, consuming any
    /// pending list marker.
    fn take_prefixes(&mut self) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
        let mut first: Vec<Span<'static>> = Vec::new();
        let mut rest: Vec<Span<'static>> = Vec::new();
        let quote_style = Style::default().fg(Color::Green);
        for context in self.contexts.iter_mut() {
            match context {
                Context::Quote => {
                    first.push(Span::styled("▎ ", quote_style));
                    rest.push(Span::styled("▎ ", quote_style));
                }
                Context::Item {
                    marker,
                    marker_used,
                } => {
                    let blank = " ".repeat(display_width(marker));
                    if *marker_used {
                        first.push(Span::raw(blank.clone()));
                    } else {
                        first.push(Span::styled(marker.clone(), Style::default().fg(Color::Cyan)));
                        *marker_used = true;
                    }
                    rest.push(Span::raw(blank));
                }
            }
        }
        (first, rest)
    }

    fn flush_inline(&mut self) {
        if self.inline.is_empty() || self.table.is_some() {
            return;
        }
        let body = std::mem::take(&mut self.inline);
        if body.iter().all(|span| span.content.trim().is_empty()) {
            return;
        }
        let (first, rest) = self.take_prefixes();
        let wrapped = wrap_with_prefix(&body, self.width, first, rest);
        self.lines.extend(wrapped);
    }

    fn finish_code_block(&mut self) {
        let Some(code) = self.code.take() else {
            return;
        };
        let (first, rest) = self.take_prefixes();
        let dim = Style::default().add_modifier(Modifier::DIM);
        let highlighted = highlight_code(code.text.trim_end_matches('\n'), &code.lang);
        let mut first_line = true;
        if !code.lang.is_empty() {
            let mut prefix = first.clone();
            prefix.push(Span::styled(format!("  {}", code.lang), dim));
            self.lines.push(Line::from(prefix));
            first_line = false;
        }
        for line in highlighted {
            let mut body = vec![Span::raw("  ")];
            body.extend(line.spans);
            let prefix = if first_line { first.clone() } else { rest.clone() };
            first_line = false;
            let wrapped = wrap_with_prefix(&body, self.width, prefix, rest.clone());
            self.lines.extend(wrapped);
        }
        self.blank_line();
    }

    fn finish_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        let (first, rest) = self.take_prefixes();
        let columns = table.header.len().max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
        if columns == 0 {
            return;
        }
        let widths = fit_column_widths(&table, columns, self.width.saturating_sub(display_width_of(&first)));
        let dim = Style::default().add_modifier(Modifier::DIM);
        let mut rendered: Vec<Line<'static>> = Vec::new();
        rendered.push(table_row(&table.header, &widths, &table.alignments, Some(Modifier::BOLD)));
        let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w + 2)).collect();
        rendered.push(Line::from(Span::styled(format!("├{}┤", rule.join("┼")), dim)));
        for row in &table.rows {
            rendered.push(table_row(row, &widths, &table.alignments, None));
        }
        for (index, line) in rendered.into_iter().enumerate() {
            let mut prefix = if index == 0 { first.clone() } else { rest.clone() };
            prefix.extend(line.spans);
            self.lines.push(Line::from(prefix));
        }
        self.blank_line();
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_inline();
        if self.code.is_some() {
            self.finish_code_block();
        }
        while self.lines.last().is_some_and(|line| line.spans.is_empty()) {
            self.lines.pop();
        }
        self.lines
    }
}

fn display_width_of(spans: &[Span<'static>]) -> usize {
    spans.iter().map(|s| display_width(&s.content)).sum()
}

fn heading_style(base: Style, level: HeadingLevel) -> Style {
    match level {
        HeadingLevel::H1 => base.add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        HeadingLevel::H2 => base.add_modifier(Modifier::BOLD),
        _ => base.add_modifier(Modifier::BOLD | Modifier::ITALIC),
    }
}

/// Column widths: natural widths, shrunk (widest first) to fit `available`.
fn fit_column_widths(table: &TableBuffer, columns: usize, available: usize) -> Vec<usize> {
    let mut widths = vec![3usize; columns];
    for row in std::iter::once(&table.header).chain(table.rows.iter()) {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(display_width(cell));
        }
    }
    let overhead = 3 * columns + 1;
    while widths.iter().sum::<usize>() + overhead > available {
        let (widest, value) = widths
            .iter()
            .enumerate()
            .max_by_key(|(_, w)| **w)
            .map(|(i, w)| (i, *w))
            .unwrap_or((0, 0));
        if value <= 3 {
            break;
        }
        widths[widest] -= 1;
    }
    widths
}

fn table_row(
    cells: &[String],
    widths: &[usize],
    alignments: &[Alignment],
    modifier: Option<Modifier>,
) -> Line<'static> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let cell_style = modifier.map(|m| Style::default().add_modifier(m)).unwrap_or_default();
    let mut spans = vec![Span::styled("│", dim)];
    for (index, width) in widths.iter().enumerate() {
        let text = truncate_to_width(cells.get(index).map(String::as_str).unwrap_or(""), *width);
        let pad = width.saturating_sub(display_width(&text));
        let (left, right) = match alignments.get(index) {
            Some(Alignment::Right) => (pad, 0),
            Some(Alignment::Center) => (pad / 2, pad - pad / 2),
            _ => (0, pad),
        };
        spans.push(Span::styled(
            format!(" {}{}{} ", " ".repeat(left), text, " ".repeat(right)),
            cell_style,
        ));
        spans.push(Span::styled("│", dim));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::tui::wrap::line_text;

    fn rendered(source: &str, width: usize) -> Vec<String> {
        render_markdown(source, width).iter().map(line_text).collect()
    }

    #[test]
    fn renders_headings_and_paragraphs() {
        let lines = render_markdown("# Title\n\nSome **bold** text", 40);
        assert_eq!(line_text(&lines[0]), "Title");
        assert!(lines[0].spans[0].style.add_modifier.contains(Modifier::BOLD));
        let bold = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content == "bold")
            .unwrap();
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn renders_lists_with_markers_and_hanging_indent() {
        let out = rendered("- first item that is long\n- second\n\n1. one\n2. two", 14);
        assert_eq!(out[0], "• first item");
        assert_eq!(out[1], "  that is long");
        assert!(out.contains(&"• second".to_string()));
        assert!(out.contains(&"1. one".to_string()));
        assert!(out.contains(&"2. two".to_string()));
    }

    #[test]
    fn inline_code_is_styled() {
        let lines = render_markdown("run `cargo test` now", 40);
        for word in ["cargo", "test"] {
            let span = lines[0].spans.iter().find(|s| s.content == word).unwrap();
            assert_eq!(span.style.fg, Some(Color::Cyan), "{word}");
        }
        let plain = lines[0].spans.iter().find(|s| s.content == "run").unwrap();
        assert_eq!(plain.style.fg, None);
    }

    #[test]
    fn fenced_code_is_highlighted() {
        let lines = render_markdown("```rust\nfn main() {}\n```", 40);
        let text: Vec<String> = lines.iter().map(line_text).collect();
        assert!(text.iter().any(|l| l.contains("fn main() {}")), "got {text:?}");
        let colored = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .any(|s| matches!(s.style.fg, Some(Color::Rgb(..))));
        assert!(colored, "expected syntax colors");
    }

    #[test]
    fn unclosed_fence_while_streaming_still_renders() {
        let out = rendered("text\n\n```python\nprint('hi')", 40);
        assert!(out.iter().any(|l| l.contains("print('hi')")));
    }

    #[test]
    fn quotes_get_a_bar() {
        let out = rendered("> quoted", 40);
        assert_eq!(out[0], "▎ quoted");
    }

    #[test]
    fn tables_render_aligned_columns() {
        let out = rendered("| a | bb |\n|---|----|\n| 1 | 2 |", 40);
        assert!(out[0].contains("│ a   │ bb  │"), "got {out:?}");
        assert!(out[1].starts_with('├'));
        assert!(out[2].contains("│ 1   │ 2   │"), "got {out:?}");
    }

    #[test]
    fn links_show_their_target() {
        let out = rendered("[docs](https://example.com)", 60);
        assert_eq!(out[0], "docs (https://example.com)");
    }
}
