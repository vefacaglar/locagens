//! Copying text out of the TUI (`/copy`, the login code): an OSC 52 escape
//! sequence (works over SSH and in most modern terminals) plus the
//! platform's clipboard command as a fallback.

use base64::Engine as _;
use std::io::Write;
use std::process::{Command, Stdio};

/// Terminals cap OSC 52 payloads; beyond this only the native command runs.
const OSC52_MAX_BYTES: usize = 100_000;

/// The OSC 52 "set clipboard" sequence for `text`.
pub fn osc52_sequence(text: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    format!("\u{1b}]52;c;{encoded}\u{7}")
}

/// The last fenced code block of Markdown `text` (without the fences).
pub fn last_fenced_block(text: &str) -> Option<String> {
    let mut last: Option<String> = None;
    let mut current: Option<Vec<&str>> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            match current.take() {
                Some(block) => last = Some(block.join("\n")),
                None => current = Some(Vec::new()),
            }
        } else if let Some(block) = current.as_mut() {
            block.push(line);
        }
    }
    last
}

/// `1 line` / `42 lines` for the confirmation message.
pub fn line_count_label(text: &str) -> String {
    let count = text.lines().count().max(1);
    if count == 1 {
        "1 line".to_string()
    } else {
        format!("{count} lines")
    }
}

fn native_command() -> Vec<(&'static str, &'static [&'static str])> {
    if cfg!(windows) {
        vec![("clip", &[])]
    } else if cfg!(target_os = "macos") {
        vec![("pbcopy", &[])]
    } else {
        vec![
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    }
}

/// Pipes `text` to a clipboard command; `clip.exe` wants UTF-16 with a BOM.
fn pipe_to_native(text: &str) -> bool {
    for (program, args) in native_command() {
        let Ok(mut child) = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        let payload: Vec<u8> = if cfg!(windows) {
            let mut bytes = vec![0xFF, 0xFE];
            bytes.extend(text.encode_utf16().flat_map(|unit| unit.to_le_bytes()));
            bytes
        } else {
            text.as_bytes().to_vec()
        };
        if let Some(mut stdin) = child.stdin.take() {
            if stdin.write_all(&payload).is_err() {
                let _ = child.kill();
                continue;
            }
        }
        if child.wait().is_ok_and(|status| status.success()) {
            return true;
        }
    }
    false
}

/// Copies `text`; returns `false` when no route is known to have worked
/// (the OSC 52 write itself cannot be confirmed).
pub fn copy_text(text: &str) -> bool {
    let mut sent = false;
    if text.len() <= OSC52_MAX_BYTES {
        let mut out = std::io::stdout();
        sent = out.write_all(osc52_sequence(text).as_bytes()).and_then(|()| out.flush()).is_ok();
    }
    pipe_to_native(text) || sent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_wraps_base64() {
        assert_eq!(osc52_sequence("hi"), "\u{1b}]52;c;aGk=\u{7}");
    }

    #[test]
    fn finds_the_last_fenced_block() {
        let text = "intro\n```rust\nfn a() {}\n```\nmid\n```\nlet b = 1;\nlet c = 2;\n```\nend";
        assert_eq!(last_fenced_block(text).as_deref(), Some("let b = 1;\nlet c = 2;"));
        assert_eq!(last_fenced_block("no code here"), None);
        assert_eq!(last_fenced_block("```\nunterminated"), None);
    }

    #[test]
    fn labels_line_counts() {
        assert_eq!(line_count_label("one"), "1 line");
        assert_eq!(line_count_label("a\nb\nc"), "3 lines");
    }
}
