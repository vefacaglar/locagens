//! Markdown → HTML rendering and unified-diff parsing used by the chat
//! thread, plan panel and permission cards.

use pulldown_cmark::{html, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::sync::OnceLock;
use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Code blocks above this size are shown as plain text (highlighting cost).
const MAX_HIGHLIGHT_BYTES: usize = 64 * 1024;

fn syntax_set() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Escapes text for HTML element content and double-quoted attributes.
pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Highlights `code` as `lang` into classed `<span>`s (`s-keyword`, `s-string`
/// ...; colours come from the theme stylesheet). `None` for unknown languages
/// or oversized input, so callers fall back to escaped plain text.
fn highlight_code(code: &str, lang: &str) -> Option<String> {
    if lang.is_empty() || code.len() > MAX_HIGHLIGHT_BYTES {
        return None;
    }
    let syntaxes = syntax_set();
    let syntax = syntaxes.find_syntax_by_token(lang)?;
    let mut generator =
        ClassedHTMLGenerator::new_with_class_style(syntax, syntaxes, ClassStyle::SpacedPrefixed { prefix: "s-" });
    for line in LinesWithEndings::from(code) {
        generator.parse_html_for_line_which_includes_newline(line).ok()?;
    }
    Some(generator.finalize())
}

/// HTML of one fenced / indented code block: a card whose copy button lives
/// on the non-scrolling wrapper and carries the raw code (escaped) for the
/// clipboard handler.
fn code_block_html(code: &str, lang: &str) -> String {
    let body = highlight_code(code, lang).unwrap_or_else(|| escape_html(code));
    let lang_class = if lang.is_empty() {
        String::new()
    } else {
        format!(" class=\"language-{}\"", escape_html(lang))
    };
    format!(
        "<div class=\"code-block\"><button class=\"code-copy\" type=\"button\" title=\"Copy\" data-code=\"{raw}\" onclick=\"locagensCopy(this)\"><span class=\"ico-copy\">{copy}</span><span class=\"ico-check\">{check}</span></button><pre><code{lang_class}>{body}</code></pre></div>",
        raw = escape_html(code),
        copy = crate::components::icons::COPY_SVG,
        check = crate::components::icons::CHECK_SVG,
    )
}

/// JavaScript installed once at startup: copies a code block's raw text and
/// flips its button to a check mark for 1.5 seconds.
pub const COPY_SCRIPT: &str = r#"
window.locagensCopy = function (button) {
  var done = function () {
    button.classList.add('copied');
    setTimeout(function () { button.classList.remove('copied'); }, 1500);
  };
  navigator.clipboard.writeText(button.dataset.code || '').then(done, function () {});
};
"#;

/// Renders Markdown (tables, strikethrough, task lists enabled) to HTML.
///
/// Raw HTML in the source is treated as text (model output is untrusted),
/// tables get a scroll wrapper and code blocks become highlighted cards with
/// a copy button.
pub fn render_markdown(md: &str) -> String {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut events: Vec<Event> = Vec::new();
    let mut code: Option<(String, String)> = None;
    for event in Parser::new_ext(md, options) {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((lang, text)) = code.take() {
                    events.push(Event::Html(code_block_html(&text, &lang).into()));
                }
            }
            Event::Text(text) if code.is_some() => {
                if let Some((_, buffer)) = code.as_mut() {
                    buffer.push_str(&text);
                }
            }
            Event::Start(Tag::Table(alignments)) => {
                events.push(Event::Html("<div class=\"table-wrap\">".into()));
                events.push(Event::Start(Tag::Table(alignments)));
            }
            Event::End(TagEnd::Table) => {
                events.push(Event::End(TagEnd::Table));
                events.push(Event::Html("</div>".into()));
            }
            // Never pass model-provided HTML through.
            Event::Html(raw) | Event::InlineHtml(raw) => events.push(Event::Text(raw)),
            other => events.push(other),
        }
    }
    let mut html_out = String::with_capacity(md.len() + 32);
    html::push_html(&mut html_out, events.into_iter());
    html_out
}

/// One line of a unified diff (`+` added, `-` removed, `@` hunk header,
/// ` ` context, everything else = plain text).
#[derive(Clone, Debug, PartialEq)]
pub struct DiffLine {
    pub kind: char,
    pub text: String,
}

/// Parses a unified diff into display lines.
pub fn parse_diff(diff: &str) -> Vec<DiffLine> {
    diff.lines()
        .map(|line| {
            let kind = match line.chars().next() {
                Some('+') => '+',
                Some('-') => '-',
                Some('@') => '@',
                Some(' ') => ' ',
                _ => 'x',
            };
            DiffLine { kind, text: line.to_string() }
        })
        .collect()
}

/// Whether a text blob looks like a unified diff (used to auto-render
/// `DiffView` inside tool outputs and previews).
pub fn looks_like_diff(text: &str) -> bool {
    let mut hunk = false;
    let mut plus = false;
    let mut minus = false;
    for line in text.lines() {
        if line.starts_with("@@") {
            hunk = true;
        } else if line.starts_with("+++") || line.starts_with("---") {
            // file headers — count as markers too
            hunk = true;
        } else if line.starts_with('+') {
            plus = true;
        } else if line.starts_with('-') {
            minus = true;
        }
    }
    hunk && (plus || minus)
}

/// Builds a small pseudo-diff string from before/after content so the
/// `DiffView` renderer can display it (used for permission previews, which
/// carry `oldContent`/`newContent` rather than a unified diff).
pub fn pseudo_diff(old: Option<&str>, new: Option<&str>) -> String {
    let mut out = String::new();
    if let Some(old) = old {
        for line in old.lines() {
            out.push_str("- ");
            out.push_str(line);
            out.push('\n');
        }
    }
    match new {
        Some(new) => {
            for line in new.lines() {
                out.push_str("+ ");
                out.push_str(line);
                out.push('\n');
            }
        }
        None => out.push_str("+ (file deleted)\n"),
    }
    out
}

/// Summarizes a tool-call argument JSON object into a short one-line hint
/// (`command …`, `path …`, `url …`).
pub fn summarize_args(arguments: &serde_json::Value) -> String {
    let string_arg = |keys: &[&str]| -> Option<String> {
        keys.iter().find_map(|key| {
            arguments
                .get(*key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
    };
    let fallback = |value: Option<String>| -> String {
        match value {
            Some(value) => {
                let mut text: String = value.chars().take(120).collect();
                if value.chars().count() > 120 {
                    text.push('…');
                }
                text
            }
            None => String::new(),
        }
    };
    fallback(string_arg(&["command"])
        .or_else(|| string_arg(&["path", "from"]))
        .or_else(|| string_arg(&["url"]))
        .or_else(|| string_arg(&["query"]))
        .or_else(|| {
            arguments
                .get("old_text")
                .and_then(serde_json::Value::as_str)
                .map(|old| format!("edit {}…", old.chars().take(40).collect::<String>()))
        })
        .or_else(|| arguments.get("task").and_then(serde_json::Value::as_str).map(str::to_string))
        .or_else(|| serde_json::to_string(arguments).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn renders_tables_and_tasklists() {
        let table = render_markdown("| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(table.contains("<table>"), "table tag missing: {table}");
        assert!(table.contains("<td>1</td>"), "cell missing: {table}");

        let tasks = render_markdown("- [x] done\n- [ ] todo\n");
        assert!(
            tasks.contains("checkbox"),
            "task list checkbox missing: {tasks}"
        );
    }

    #[test]
    fn enables_strikethrough() {
        let html = render_markdown("~~gone~~");
        assert!(html.contains("<del>"), "strikethrough missing: {html}");
    }

    #[test]
    fn parses_diff_line_kinds() {
        let lines = parse_diff("@@ -1,2 +1,3 @@\n context\n-removed\n+added\nplain\n");
        let kinds: Vec<char> = lines.iter().map(|line| line.kind).collect();
        assert_eq!(kinds, vec!['@', ' ', '-', '+', 'x']);
        assert_eq!(lines[2].text, "-removed");
    }

    #[test]
    fn empty_diff_parses_to_nothing() {
        assert!(parse_diff("").is_empty());
    }

    #[test]
    fn detects_diff_like_text() {
        assert!(looks_like_diff("--- a.txt\n+++ b.txt\n@@ -1 +1 @@\n-old\n+new"));
        assert!(looks_like_diff("@@ -1 +1 @@\n+only added"));
        assert!(!looks_like_diff("just some text\n+with a plus"));
        assert!(!looks_like_diff("@@ hunk marker only"));
        assert!(!looks_like_diff(""));
    }

    #[test]
    fn builds_pseudo_diff_from_contents() {
        let diff = pseudo_diff(Some("old line"), Some("new line"));
        assert!(diff.contains("- old line"));
        assert!(diff.contains("+ new line"));
        assert!(pseudo_diff(Some("old"), None).contains("+ (file deleted)"));
    }

    #[test]
    fn summarizes_command_args() {
        let args: Value = serde_json::json!({ "command": "cargo test --workspace" });
        assert_eq!(summarize_args(&args), "cargo test --workspace");
    }

    #[test]
    fn summarizes_path_args() {
        let args: Value = serde_json::json!({ "path": "src/main.rs" });
        assert_eq!(summarize_args(&args), "src/main.rs");

        let url_args: Value = serde_json::json!({ "url": "https://example.com" });
        assert_eq!(summarize_args(&url_args), "https://example.com");
    }

    #[test]
    fn summarizes_edit_args() {
        let args: Value = serde_json::json!({ "old_text": "let x = 1;" });
        assert_eq!(summarize_args(&args), "edit let x = 1;…");
    }

    #[test]
    fn truncates_long_args() {
        let long = "x".repeat(200);
        let args: Value = serde_json::json!({ "command": long });
        let summary = summarize_args(&args);
        assert_eq!(summary.chars().count(), 121); // 120 chars + ellipsis
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn falls_back_to_json_serialization() {
        let args: Value = serde_json::json!({ "custom_key": 42 });
        assert_eq!(summarize_args(&args), r#"{"custom_key":42}"#);
    }

    #[test]
    fn table_has_header_body_and_scroll_wrapper() {
        let html = render_markdown("| A | B |\n|---|---|\n| 1 | 2 |\n");
        assert!(html.starts_with("<div class=\"table-wrap\"><table>"), "{html}");
        assert!(html.contains("<thead>") && html.contains("<th>A</th>"));
        assert!(html.contains("<tbody>") && html.contains("<td>1</td>"));
        assert!(html.trim_end().ends_with("</table>\n</div>") || html.trim_end().ends_with("</div>"));
    }

    #[test]
    fn fenced_code_gets_wrapper_copy_button_and_highlighting() {
        for (lang, code) in [("rust", "fn main() { let x = 1; }"), ("python", "def f():\n    return 1")] {
            let html = render_markdown(&format!("```{lang}\n{code}\n```\n"));
            assert!(html.contains("<div class=\"code-block\">"), "{html}");
            assert!(html.contains("class=\"code-copy\""), "{html}");
            assert!(html.contains("<span class=\"s-"), "no highlight spans for {lang}: {html}");
            assert!(html.contains(&format!("language-{lang}")));
        }
    }

    #[test]
    fn copy_button_carries_the_raw_code_not_the_html() {
        let html = render_markdown("```rust\nlet a = \"<b>\";\n```\n");
        assert!(html.contains("data-code=\"let a = &quot;&lt;b&gt;&quot;;\n\""), "{html}");
    }

    #[test]
    fn unknown_language_falls_back_to_escaped_plain_text() {
        let html = render_markdown("```nosuchlang\n<script>alert(1)</script>\n```\n");
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"), "{html}");
        assert!(!html.contains("<script>"));
        assert!(!html.contains("<span class=\"s-"));
    }

    #[test]
    fn raw_html_from_the_model_is_escaped() {
        let html = render_markdown("hello <img src=x onerror=alert(1)> <b>bold</b>\n\n<div>block</div>\n");
        assert!(!html.contains("<img") && !html.contains("<b>") && !html.contains("<div>block"));
        assert!(html.contains("&lt;img"));
    }

    #[test]
    fn inline_code_stays_a_code_element() {
        assert!(render_markdown("use `x < y`").contains("<code>x &lt; y</code>"));
    }
}
