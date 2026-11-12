//! Markdown → HTML rendering and unified-diff parsing used by the chat
//! thread, plan panel and permission cards.

use pulldown_cmark::{html, Options, Parser};

/// Renders Markdown (tables, strikethrough, task lists enabled) to HTML.
pub fn render_markdown(md: &str) -> String {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let parser = Parser::new_ext(md, options);
    let mut html_out = String::with_capacity(md.len() + 32);
    html::push_html(&mut html_out, parser);
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
