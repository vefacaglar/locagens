//! Terminal rendering of run events for `lcgn` (text mode).
//!
//! Assistant text streams to **stdout** so `lcgn -p "..." > out.txt` captures
//! only the answer; everything else (reasoning, tool lines, status) goes to
//! **stderr**.

use serde_json::Value;
use std::collections::HashMap;
use std::io::Write;

/// Whether ANSI colors may be used (NO_COLOR respected, both streams TTY).
pub fn colors_enabled() -> bool {
    if std::env::var_os("NO_COLOR")
        .map(|value| !value.is_empty())
        .unwrap_or(false)
    {
        return false;
    }
    use std::io::IsTerminal;
    std::io::stdout().is_terminal() && std::io::stderr().is_terminal()
}

/// Enables ANSI escape processing on Windows consoles (best effort).
pub fn enable_windows_vt() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::{
            GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
            STD_OUTPUT_HANDLE,
        };
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if !handle.is_null() {
            let mut mode = 0u32;
            if GetConsoleMode(handle, &mut mode) != 0 {
                SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }
    }
}

const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

/// Streams run events: assistant deltas to stdout, the rest to stderr.
pub struct TextRenderer<W: Write, E: Write> {
    out: W,
    err: E,
    colors: bool,
    show_thinking: bool,
    interactive: bool,
    last_content: String,
    reasoning_printed: usize,
    tool_calls: HashMap<String, (String, String)>,
}

impl<W: Write, E: Write> TextRenderer<W, E> {
    pub fn new(out: W, err: E, colors: bool, show_thinking: bool, interactive: bool) -> Self {
        Self {
            out,
            err,
            colors,
            show_thinking,
            interactive,
            last_content: String::new(),
            reasoning_printed: 0,
            tool_calls: HashMap::new(),
        }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.colors {
            format!("{code}{text}{RESET}")
        } else {
            text.to_string()
        }
    }

    /// Renders one run event. Only `message_*`, `round_started`,
    /// `run_title_changed` and `plan_updated` produce output.
    pub fn on_event(&mut self, event: &Value) {
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "message_created" | "message_updated" => {
                if let Some(message) = event.get("message") {
                    self.on_message(message);
                }
            }
            "round_started" => {
                if self.interactive {
                    let round = event.get("round").and_then(Value::as_i64).unwrap_or(0);
                    let _ = writeln!(
                        self.err,
                        "{}",
                        self.paint(DIM, &format!("· round {round}"))
                    );
                    let _ = self.err.flush();
                }
            }
            "run_title_changed" => {
                if self.interactive {
                    let title = event.get("title").and_then(Value::as_str).unwrap_or("");
                    let _ = writeln!(
                        self.err,
                        "{}",
                        self.paint(DIM, &format!("· title: {title}"))
                    );
                    let _ = self.err.flush();
                }
            }
            "plan_updated" => {
                let title = event
                    .pointer("/plan/title")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let _ = writeln!(
                    self.err,
                    "{}",
                    self.paint(DIM, &format!("· plan updated: {title}"))
                );
                let _ = self.err.flush();
            }
            _ => {}
        }
    }

    fn on_message(&mut self, message: &Value) {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        match role {
            "assistant" => {
                if let Some(calls) = message
                    .get("rawResponse")
                    .and_then(Value::as_str)
                    .and_then(|raw| serde_json::from_str::<Vec<Value>>(raw).ok())
                {
                    for call in calls {
                        let id = call.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                        let name = call
                            .pointer("/function/name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_string();
                        let args = call
                            .pointer("/function/arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("{}")
                            .to_string();
                        self.tool_calls.insert(id, (name, summarize_args(&args)));
                    }
                }
                let content = message.get("content").and_then(Value::as_str).unwrap_or("");
                self.stream_content(content);
                if self.show_thinking {
                    if let Some(reasoning) = message
                        .get("reasoningContent")
                        .and_then(Value::as_str)
                        .filter(|r| !r.is_empty())
                    {
                        self.stream_reasoning(reasoning);
                    }
                }
            }
            "tool" => self.on_tool_result(message),
            _ => {}
        }
    }

    fn stream_content(&mut self, content: &str) {
        if content.is_empty() || content == self.last_content {
            return;
        }
        let delta = if content.starts_with(self.last_content.as_str()) {
            content[self.last_content.len()..].to_string()
        } else {
            // Replaced (not extended): print everything after the common prefix.
            let common = content
                .chars()
                .zip(self.last_content.chars())
                .take_while(|(a, b)| a == b)
                .count();
            let byte = content
                .char_indices()
                .nth(common)
                .map(|(i, _)| i)
                .unwrap_or(content.len());
            content[byte..].to_string()
        };
        self.last_content = content.to_string();
        if !delta.is_empty() {
            let _ = self.out.write_all(delta.as_bytes());
            let _ = self.out.flush();
        }
    }

    fn stream_reasoning(&mut self, reasoning: &str) {
        let total = reasoning.chars().count();
        if total <= self.reasoning_printed {
            return;
        }
        let delta: String = reasoning.chars().skip(self.reasoning_printed).collect();
        self.reasoning_printed = total;
        let _ = writeln!(self.err, "{}", self.paint(DIM, &delta));
        let _ = self.err.flush();
    }

    fn on_tool_result(&mut self, message: &Value) {
        let call_id = message.get("rawResponse").and_then(Value::as_str).unwrap_or("");
        let (name, args) = self
            .tool_calls
            .remove(call_id)
            .unwrap_or_else(|| ("tool".to_string(), String::new()));
        let content = message.get("content").and_then(Value::as_str).unwrap_or("");
        let failed = content.starts_with("Tool error")
            || content.starts_with("The user denied permission");
        let mark = if failed {
            self.paint(RED, "✖")
        } else {
            self.paint(GREEN, "✔")
        };
        let mut line = format!("{mark} {name}{args}");
        let summary = first_line(content);
        if !summary.is_empty() {
            line.push_str(&format!(" {}", self.paint(DIM, &format!("· {summary}"))));
        }
        let _ = writeln!(self.err, "{line}");
        let _ = self.err.flush();
    }

    /// Prints any part of the final answer that was not streamed yet.
    pub fn finalize(&mut self, final_output: &str) {
        self.stream_content(final_output);
        let needs_newline = !self.last_content.is_empty()
            && !self.last_content.ends_with('\n');
        if needs_newline {
            let _ = writeln!(self.out);
            let _ = self.out.flush();
            self.last_content.push('\n');
        }
        self.reasoning_printed = 0;
        self.tool_calls.clear();
    }
}

fn first_line(content: &str) -> String {
    let line = content.lines().next().unwrap_or("").trim();
    if line.chars().count() > 80 {
        let mut cut: String = line.chars().take(80).collect();
        cut.push('…');
        cut
    } else {
        line.to_string()
    }
}

fn summarize_args(raw: &str) -> String {
    let args: Value = match serde_json::from_str(raw) {
        Ok(args) => args,
        Err(_) => return String::new(),
    };
    if args.as_object().map(|o| o.is_empty()).unwrap_or(false) {
        return String::new();
    }
    if let Some(from) = args.get("from").and_then(Value::as_str).filter(|v| !v.is_empty()) {
        let to = args.get("to").and_then(Value::as_str).unwrap_or("");
        return format!(" {from} -> {to}");
    }
    for key in ["path", "command", "url", "query", "name", "title"] {
        if let Some(value) = args.get(key).and_then(Value::as_str).filter(|v| !v.is_empty()) {
            return format!(" {value}");
        }
    }
    format!(" {}", first_line(raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn renderer() -> TextRenderer<Vec<u8>, Vec<u8>> {
        TextRenderer::new(Vec::new(), Vec::new(), false, false, false)
    }

    fn message(content: &str, reasoning: Option<&str>) -> Value {
        let mut message = serde_json::json!({
            "type": "message_updated",
            "message": { "role": "assistant", "content": content },
        });
        if let Some(reasoning) = reasoning {
            message["message"]["reasoningContent"] = serde_json::json!(reasoning);
        }
        message
    }

    #[test]
    fn streams_content_deltas() {
        let mut r = renderer();
        r.on_event(&message("Hello", None));
        assert_eq!(String::from_utf8(r.out.clone()).unwrap(), "Hello");
        r.on_event(&message("Hello, world", None));
        assert_eq!(String::from_utf8(r.out.clone()).unwrap(), "Hello, world");
        r.on_event(&message("Hello, world", None));
        assert_eq!(String::from_utf8(r.out.clone()).unwrap(), "Hello, world");
    }

    #[test]
    fn handles_replaced_content() {
        let mut r = renderer();
        r.on_event(&message("abc", None));
        r.on_event(&message("abx", None));
        assert_eq!(String::from_utf8(r.out.clone()).unwrap(), "abcx");
    }

    #[test]
    fn thinking_off_means_no_reasoning_output() {
        let mut r = renderer();
        r.on_event(&message("", Some("secret thoughts")));
        assert!(r.out.is_empty());
        // Reasoning goes to stderr only when enabled.
        let mut r = TextRenderer::new(Vec::new(), Vec::new(), false, true, false);
        r.on_event(&message("", Some("secret thoughts")));
        assert_eq!(String::from_utf8(r.err.clone()).unwrap(), "secret thoughts\n");
        assert!(r.out.is_empty());
    }

    #[test]
    fn tool_lines_render_with_summary() {
        let mut r = renderer();
        let call = serde_json::json!({
            "id": "c1",
            "function": { "name": "read_file", "arguments": "{\"path\":\"src/main.rs\"}" }
        });
        r.on_event(&serde_json::json!({
            "type": "message_updated",
            "message": {
                "role": "assistant",
                "content": "",
                "rawResponse": serde_json::to_string(&[call]).unwrap()
            }
        }));
        r.on_event(&serde_json::json!({
            "type": "message_created",
            "message": { "role": "tool", "content": "fn main() {}\n…", "rawResponse": "c1" }
        }));
        let err = String::from_utf8(r.err.clone()).unwrap();
        assert!(err.contains("✔ read_file src/main.rs"), "got: {err}");
        assert!(err.contains("· fn main() {}"));
    }

    #[test]
    fn finalize_prints_missing_tail_and_newline() {
        let mut r = renderer();
        r.on_event(&message("partial", None));
        r.finalize("partial answer\n");
        let out = String::from_utf8(r.out.clone()).unwrap();
        assert_eq!(out, "partial answer\n");
        // No double newline when the answer already ends with one.
        let mut r = renderer();
        r.finalize("done\n");
        assert_eq!(String::from_utf8(r.out.clone()).unwrap(), "done\n");
    }

    #[test]
    fn summarize_args_picks_informative_keys() {
        assert_eq!(summarize_args("{}"), "");
        assert_eq!(summarize_args("{\"path\":\"a.rs\"}"), " a.rs");
        assert_eq!(
            summarize_args("{\"from\":\"a.rs\",\"to\":\"b.rs\"}"),
            " a.rs -> b.rs"
        );
    }
}
