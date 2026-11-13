//! Live `apply_patch` progress while the model is still streaming the call.
//!
//! Tool-call arguments arrive as partial JSON (`{"input":"*** Begin Pat…`).
//! [`PatchArgsStream`] decodes the patch string out of that JSON
//! incrementally and feeds it to the line-level
//! [`StreamingPatchParser`], so the run can emit `patch_progress` events (the
//! file list, `+`/`-` counts) before the call completes. Purely advisory: the
//! final apply path still parses the complete arguments.

use locagens_apply_patch::streaming::{FileProgress, StreamingPatchParser};
use serde_json::{json, Value};

/// Top-level argument keys that carry the patch text (`patch` is the Codex
/// compatibility alias of `input`).
const PATCH_KEYS: [&str; 2] = ["input", "patch"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scan {
    /// Between tokens.
    Structure,
    /// Inside a top-level key string.
    Key,
    /// Inside a string value we do not care about.
    SkippedString,
    /// Inside the patch string value.
    PatchString,
}

/// Incremental decoder + parser for one streaming `apply_patch` call.
pub struct PatchArgsStream {
    parser: StreamingPatchParser,
    scan: Scan,
    depth: u32,
    expect_key: bool,
    key: String,
    last_key: String,
    /// Characters of the current string after a backslash (`\u` needs 4 hex).
    escape: Option<String>,
    high_surrogate: Option<u16>,
    /// Number of argument bytes already consumed.
    consumed: usize,
}

impl Default for PatchArgsStream {
    fn default() -> Self {
        Self {
            parser: StreamingPatchParser::new(),
            scan: Scan::Structure,
            depth: 0,
            expect_key: false,
            key: String::new(),
            last_key: String::new(),
            escape: None,
            high_surrogate: None,
            consumed: 0,
        }
    }
}

impl PatchArgsStream {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn files(&self) -> &[FileProgress] {
        self.parser.files()
    }

    pub fn ended(&self) -> bool {
        self.parser.ended()
    }

    /// Feeds the full arguments received so far (the accumulator hands out
    /// snapshots, not deltas). Returns true when the progress changed.
    pub fn update(&mut self, arguments_so_far: &str) -> bool {
        // Snapshots only grow; a shorter one means a restarted call.
        if arguments_so_far.len() < self.consumed || !arguments_so_far.is_char_boundary(self.consumed) {
            *self = Self::default();
        }
        let fresh = &arguments_so_far[self.consumed..];
        self.consumed = arguments_so_far.len();
        let mut decoded = String::new();
        for ch in fresh.chars() {
            self.step(ch, &mut decoded);
        }
        !decoded.is_empty() && self.parser.push(&decoded)
    }

    fn step(&mut self, ch: char, patch_out: &mut String) {
        match self.scan {
            Scan::Structure => self.step_structure(ch),
            Scan::Key => self.step_key(ch),
            Scan::SkippedString => self.step_skipped(ch),
            Scan::PatchString => self.step_patch(ch, patch_out),
        }
    }

    fn step_structure(&mut self, ch: char) {
        match ch {
            '{' => {
                self.depth += 1;
                self.expect_key = self.depth == 1;
            }
            '}' => self.depth = self.depth.saturating_sub(1),
            ',' => self.expect_key = self.depth == 1,
            ':' => self.expect_key = false,
            '"' => {
                self.escape = None;
                if self.depth == 1 && self.expect_key {
                    self.key.clear();
                    self.scan = Scan::Key;
                } else if self.depth == 1 && PATCH_KEYS.contains(&self.last_key.as_str()) {
                    self.scan = Scan::PatchString;
                } else {
                    self.scan = Scan::SkippedString;
                }
            }
            _ => {}
        }
    }

    fn step_key(&mut self, ch: char) {
        if self.escape.take().is_some() {
            self.key.push(ch);
        } else if ch == '\\' {
            self.escape = Some(String::new());
        } else if ch == '"' {
            self.last_key = std::mem::take(&mut self.key);
            self.expect_key = false;
            self.scan = Scan::Structure;
        } else {
            self.key.push(ch);
        }
    }

    fn step_skipped(&mut self, ch: char) {
        if self.escape.take().is_some() {
            return;
        }
        match ch {
            '\\' => self.escape = Some(String::new()),
            '"' => self.scan = Scan::Structure,
            _ => {}
        }
    }

    fn step_patch(&mut self, ch: char, out: &mut String) {
        if let Some(pending) = self.escape.as_mut() {
            if pending.is_empty() && ch != 'u' {
                self.escape = None;
                self.flush_lone_surrogate(out);
                out.push(match ch {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    other => other, // `"`, `\`, `/`
                });
                return;
            }
            pending.push(ch);
            // "u" + 4 hex digits
            if pending.len() == 5 {
                let code = u16::from_str_radix(&pending[1..], 16).unwrap_or(0xFFFD);
                self.escape = None;
                self.push_utf16(code, out);
            }
            return;
        }
        match ch {
            '\\' => self.escape = Some(String::new()),
            '"' => {
                self.flush_lone_surrogate(out);
                self.scan = Scan::Structure;
            }
            other => {
                self.flush_lone_surrogate(out);
                out.push(other);
            }
        }
    }

    fn push_utf16(&mut self, code: u16, out: &mut String) {
        if let Some(high) = self.high_surrogate.take() {
            if (0xDC00..0xE000).contains(&code) {
                let scalar = 0x10000 + (((high as u32) - 0xD800) << 10) + ((code as u32) - 0xDC00);
                out.push(char::from_u32(scalar).unwrap_or('\u{FFFD}'));
                return;
            }
            out.push('\u{FFFD}');
        }
        if (0xD800..0xDC00).contains(&code) {
            self.high_surrogate = Some(code);
        } else {
            out.push(char::from_u32(code as u32).unwrap_or('\u{FFFD}'));
        }
    }

    fn flush_lone_surrogate(&mut self, out: &mut String) {
        if self.high_surrogate.take().is_some() {
            out.push('\u{FFFD}');
        }
    }

    /// The `patch_progress` event payload for this call.
    pub fn event(&self, run_id: &str, message_id: &str, call_id: &str, agent_name: Option<&str>) -> Value {
        json!({
            "type": "patch_progress",
            "runId": run_id,
            "messageId": message_id,
            "callId": call_id,
            "agentName": agent_name,
            "complete": self.ended(),
            "files": self.files(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use locagens_apply_patch::streaming::FileOp;

    fn args_for(patch: &str) -> String {
        json!({ "input": patch }).to_string()
    }

    fn run_in_chunks(args: &str, size: usize) -> PatchArgsStream {
        let mut stream = PatchArgsStream::new();
        let chars: Vec<char> = args.chars().collect();
        let mut so_far = String::new();
        for chunk in chars.chunks(size) {
            so_far.extend(chunk);
            stream.update(&so_far);
        }
        stream
    }

    #[test]
    fn decodes_escaped_json_incrementally() {
        let patch = "*** Begin Patch\n*** Add File: a \"q\".txt\n+tab\there\n*** Update File: b.rs\n-x\n+y\n*** End Patch";
        for size in [1, 2, 3, 5, 7, 64] {
            let stream = run_in_chunks(&args_for(patch), size);
            let files = stream.files();
            assert_eq!(files.len(), 2, "size {size}");
            assert_eq!(files[0].path, "a \"q\".txt");
            assert_eq!((files[0].op, files[0].added), (FileOp::Add, 1));
            assert_eq!((files[1].removed, files[1].added), (1, 1));
            assert!(stream.ended(), "size {size}");
        }
    }

    #[test]
    fn unicode_escapes_and_surrogate_pairs_decode() {
        // serde_json emits raw UTF-8, so hand-write \u escapes like some providers do.
        let args = r#"{"input":"*** Begin Patch\n*** Add File: özet-🎉.md\n+x\n*** End Patch"}"#;
        for size in [1, 4, 100] {
            let stream = run_in_chunks(args, size);
            assert_eq!(stream.files()[0].path, "özet-🎉.md", "size {size}");
        }
    }

    #[test]
    fn raw_utf8_split_across_updates_is_safe() {
        let args = args_for("*** Begin Patch\n*** Add File: 日本語/ä.txt\n+ü\n*** End Patch");
        let stream = run_in_chunks(&args, 1);
        assert_eq!(stream.files()[0].path, "日本語/ä.txt");
    }

    #[test]
    fn patch_alias_key_and_other_keys_are_handled() {
        let args = r#"{"note":"*** Add File: not-this","patch":"*** Begin Patch\n*** Delete File: x\n*** End Patch"}"#;
        let stream = run_in_chunks(args, 3);
        assert_eq!(stream.files().len(), 1);
        assert_eq!(stream.files()[0].path, "x");
    }

    #[test]
    fn nested_object_values_are_not_mistaken_for_the_patch() {
        let args = r#"{"meta":{"input":"*** Begin Patch\n*** Add File: nope\n"},"input":"*** Begin Patch\n*** Add File: yes\n"}"#;
        let stream = run_in_chunks(args, 2);
        assert_eq!(stream.files().len(), 1);
        assert_eq!(stream.files()[0].path, "yes");
    }

    #[test]
    fn update_reports_change_only_when_progress_moves() {
        let mut stream = PatchArgsStream::new();
        assert!(!stream.update(r#"{"inp"#));
        assert!(!stream.update(r#"{"input":"*** Begin Patch\n"#));
        assert!(stream.update(r#"{"input":"*** Begin Patch\n*** Add File: f\n"#));
        let event = stream.event("run-1", "msg-1", "call-1", Some("worker"));
        assert_eq!(event["type"], "patch_progress");
        assert_eq!(event["files"][0]["path"], "f");
        assert_eq!(event["files"][0]["op"], "add");
        assert_eq!(event["complete"], false);
    }

    #[test]
    fn restarted_arguments_reset_the_parser() {
        let mut stream = PatchArgsStream::new();
        stream.update(&args_for("*** Begin Patch\n*** Add File: first\n"));
        stream.update(r#"{"input":"*** Begin Patch\n*** Add File: second\n"#);
        assert_eq!(stream.files().len(), 1);
        assert_eq!(stream.files()[0].path, "second");
    }
}
