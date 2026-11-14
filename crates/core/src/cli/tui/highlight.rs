//! Syntax highlighting for fenced code blocks (syntect, pure-Rust regex).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use super::theme as tui_theme;

/// Code blocks longer than this are shown unhighlighted.
const MAX_HIGHLIGHT_LINES: usize = 600;
const DARK_THEME_NAME: &str = "base16-eighties.dark";
/// Readable on a light terminal background.
const LIGHT_THEME_NAME: &str = "InspiredGitHub";

fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// The syntect theme for the TUI theme: light for the light preset, dark
/// otherwise (`NO_COLOR` never gets here, see `highlight_code`).
fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        let name = if tui_theme::current().is_light() { LIGHT_THEME_NAME } else { DARK_THEME_NAME };
        let mut themes = ThemeSet::load_defaults();
        themes
            .themes
            .remove(name)
            .unwrap_or_else(|| themes.themes.into_values().next().unwrap_or_default())
    })
}

fn to_ratatui_style(style: syntect::highlighting::Style) -> Style {
    let fg = style.foreground;
    let mut out = Style::default().fg(Color::Rgb(fg.r, fg.g, fg.b));
    if style.font_style.contains(FontStyle::BOLD) {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        out = out.add_modifier(Modifier::ITALIC);
    }
    out
}

fn plain_lines(code: &str) -> Vec<Line<'static>> {
    code.lines()
        .map(|line| Line::from(Span::raw(line.to_string())))
        .collect()
}

/// One styled `Line` per source line of `code`. Unknown languages and very
/// long blocks come back unstyled.
pub fn highlight_code(code: &str, lang: &str) -> Vec<Line<'static>> {
    let lang = lang.trim();
    if tui_theme::current().plain || lang.is_empty() || code.lines().count() > MAX_HIGHLIGHT_LINES {
        return plain_lines(code);
    }
    let set = syntaxes();
    let Some(syntax) = set.find_syntax_by_token(lang) else {
        return plain_lines(code);
    };
    let mut highlighter = HighlightLines::new(syntax, theme());
    let mut out = Vec::new();
    for line in LinesWithEndings::from(code) {
        let Ok(ranges) = highlighter.highlight_line(line, set) else {
            return plain_lines(code);
        };
        let spans: Vec<Span<'static>> = ranges
            .into_iter()
            .map(|(style, text)| {
                Span::styled(
                    text.trim_end_matches(['\n', '\r']).to_string(),
                    to_ratatui_style(style),
                )
            })
            .filter(|span| !span.content.is_empty())
            .collect();
        out.push(Line::from(spans));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_color(lines: &[Line<'static>]) -> bool {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .any(|span| matches!(span.style.fg, Some(Color::Rgb(..))))
    }

    #[test]
    fn highlights_known_language() {
        let lines = highlight_code("fn main() {\n    let x = 1;\n}\n", "rust");
        assert_eq!(lines.len(), 3);
        assert!(has_color(&lines));
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "fn main() {");
    }

    #[test]
    fn unknown_language_is_plain() {
        let lines = highlight_code("some text\n", "definitely-not-a-lang");
        assert_eq!(lines.len(), 1);
        assert!(!has_color(&lines));
    }
}
