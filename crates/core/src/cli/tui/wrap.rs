//! Width-aware wrapping of styled spans (no external wrapping crate: the
//! transcript needs styles to survive a line break).

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Replaces tabs and drops carriage returns / control characters that would
/// corrupt the terminal grid.
pub fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\t' => out.push_str("    "),
            '\n' => out.push('\n'),
            ch if ch.is_control() => {}
            ch => out.push(ch),
        }
    }
    out
}

pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// One run of text that is either all whitespace or none.
struct Token {
    text: String,
    style: Style,
    is_space: bool,
}

fn tokenize(spans: &[Span<'static>]) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    for span in spans {
        let mut current = String::new();
        let mut current_space = false;
        for ch in span.content.chars() {
            let is_space = ch == ' ';
            if !current.is_empty() && is_space != current_space {
                tokens.push(Token {
                    text: std::mem::take(&mut current),
                    style: span.style,
                    is_space: current_space,
                });
            }
            current_space = is_space;
            current.push(ch);
        }
        if !current.is_empty() {
            tokens.push(Token {
                text: current,
                style: span.style,
                is_space: current_space,
            });
        }
    }
    tokens
}

/// Accumulates wrapped output lines.
struct LineBuilder {
    width: usize,
    lines: Vec<Vec<Span<'static>>>,
    current: Vec<Span<'static>>,
    current_width: usize,
}

impl LineBuilder {
    fn new(width: usize) -> Self {
        Self {
            width: width.max(1),
            lines: Vec::new(),
            current: Vec::new(),
            current_width: 0,
        }
    }

    fn push(&mut self, text: String, style: Style) {
        self.current_width += display_width(&text);
        self.current.push(Span::styled(text, style));
    }

    fn break_line(&mut self) {
        // Trailing spaces never need to be drawn.
        while let Some(last) = self.current.last() {
            if last.content.chars().all(|c| c == ' ') {
                self.current.pop();
            } else {
                break;
            }
        }
        self.lines.push(std::mem::take(&mut self.current));
        self.current_width = 0;
    }

    fn push_long_word(&mut self, text: &str, style: Style) {
        let mut chunk = String::new();
        for ch in text.chars() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            if self.current_width + display_width(&chunk) + w > self.width {
                self.push(std::mem::take(&mut chunk), style);
                self.break_line();
            }
            chunk.push(ch);
        }
        if !chunk.is_empty() {
            self.push(chunk, style);
        }
    }

    fn finish(mut self) -> Vec<Vec<Span<'static>>> {
        self.break_line();
        self.lines
    }
}

/// Greedy word wrap of `spans` into lines of at most `width` columns.
/// Always returns at least one (possibly empty) line.
pub fn wrap_spans(spans: &[Span<'static>], width: usize) -> Vec<Vec<Span<'static>>> {
    let mut builder = LineBuilder::new(width);
    for token in tokenize(spans) {
        let token_width = display_width(&token.text);
        if token.is_space {
            // Spaces at the start of a wrapped line are dropped; leading
            // indentation of the first line is kept.
            let at_wrapped_start = builder.current.is_empty() && !builder.lines.is_empty();
            if !at_wrapped_start && builder.current_width + token_width <= builder.width {
                builder.push(token.text, token.style);
            } else if !at_wrapped_start {
                builder.break_line();
            }
            continue;
        }
        if builder.current_width + token_width <= builder.width {
            builder.push(token.text, token.style);
        } else if token_width <= builder.width {
            builder.break_line();
            builder.push(token.text, token.style);
        } else {
            if !builder.current.is_empty() {
                builder.break_line();
            }
            builder.push_long_word(&token.text, token.style);
        }
    }
    builder.finish()
}

/// Wraps `body` after `first_prefix` / `rest_prefix` (hanging indent). Both
/// prefixes are padded to the same width.
pub fn wrap_with_prefix(
    body: &[Span<'static>],
    width: usize,
    first_prefix: Vec<Span<'static>>,
    rest_prefix: Vec<Span<'static>>,
) -> Vec<Line<'static>> {
    let first_width: usize = first_prefix.iter().map(|s| display_width(&s.content)).sum();
    let rest_width: usize = rest_prefix.iter().map(|s| display_width(&s.content)).sum();
    let prefix_width = first_width.max(rest_width);
    let inner = width.saturating_sub(prefix_width).max(1);
    wrap_spans(body, inner)
        .into_iter()
        .enumerate()
        .map(|(index, spans)| {
            let (mut prefix, used) = if index == 0 {
                (first_prefix.clone(), first_width)
            } else {
                (rest_prefix.clone(), rest_width)
            };
            if used < prefix_width {
                prefix.push(Span::raw(" ".repeat(prefix_width - used)));
            }
            prefix.extend(spans);
            Line::from(prefix)
        })
        .collect()
}

/// Wraps plain multi-line text in one style, with hanging prefixes.
pub fn wrap_text(
    text: &str,
    width: usize,
    style: Style,
    first_prefix: Vec<Span<'static>>,
    rest_prefix: Vec<Span<'static>>,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    for raw in sanitize(text).split('\n') {
        let span = Span::styled(raw.to_string(), style);
        let prefix_first = if out.is_empty() {
            first_prefix.clone()
        } else {
            rest_prefix.clone()
        };
        out.extend(wrap_with_prefix(&[span], width, prefix_first, rest_prefix.clone()));
    }
    out
}

/// Plain text content of a line.
#[cfg(test)]
pub fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Truncates `text` to `width` display columns, ending with `…` when cut.
pub fn truncate_to_width(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: &[Vec<Span<'static>>]) -> Vec<String> {
        lines
            .iter()
            .map(|spans| spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn wraps_on_word_boundaries() {
        let lines = wrap_spans(&[Span::raw("hello brave new world")], 11);
        assert_eq!(texts(&lines), vec!["hello brave", "new world"]);
    }

    #[test]
    fn splits_words_longer_than_width() {
        let lines = wrap_spans(&[Span::raw("abcdefghij")], 4);
        assert_eq!(texts(&lines), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn keeps_styles_across_a_break() {
        let bold = Style::default().add_modifier(ratatui::style::Modifier::BOLD);
        let spans = [Span::raw("one "), Span::styled("two three", bold)];
        let lines = wrap_spans(&spans, 8);
        assert_eq!(texts(&lines), vec!["one two", "three"]);
        assert_eq!(lines[1][0].style, bold);
    }

    #[test]
    fn hanging_indent_applies_to_continuation_lines() {
        let lines = wrap_with_prefix(
            &[Span::raw("aaa bbb ccc")],
            8,
            vec![Span::raw("- ")],
            vec![Span::raw("  ")],
        );
        let rendered: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(rendered, vec!["- aaa", "  bbb", "  ccc"]);
    }

    #[test]
    fn truncates_with_ellipsis() {
        assert_eq!(truncate_to_width("abcdefgh", 5), "abcd…");
        assert_eq!(truncate_to_width("abc", 5), "abc");
    }
}
