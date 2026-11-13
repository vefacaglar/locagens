//! Incremental `*** Begin Patch` reader (port of the idea behind Codex's
//! `streaming_parser.rs`).
//!
//! The model streams the patch a few characters at a time. This parser
//! consumes those fragments and reports *progress* — which files the patch
//! touches and how many lines it adds/removes so far — so a UI can render
//! the file list before the tool call completes.
//!
//! It is deliberately forgiving: it never fails mid-stream (a half-written
//! patch is normal) and its output is advisory only. The authoritative
//! parse/apply path stays `parse_patch` + `apply_parsed`.

use serde::Serialize;

const BEGIN_PATCH: &str = "*** Begin Patch";
const END_PATCH: &str = "*** End Patch";
const ADD_FILE: &str = "*** Add File: ";
const DELETE_FILE: &str = "*** Delete File: ";
const UPDATE_FILE: &str = "*** Update File: ";
const MOVE_TO: &str = "*** Move to: ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOp {
    Add,
    Update,
    Delete,
}

/// Progress of one file hunk seen so far.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileProgress {
    pub path: String,
    pub op: FileOp,
    /// Rename target of an `*** Move to:` line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub move_to: Option<String>,
    pub added: usize,
    pub removed: usize,
}

/// Feed text fragments with [`push`](Self::push); read [`files`](Self::files).
#[derive(Debug, Default, Clone)]
pub struct StreamingPatchParser {
    /// Incomplete trailing line (no `\n` seen yet).
    line_buffer: String,
    started: bool,
    ended: bool,
    files: Vec<FileProgress>,
}

impl StreamingPatchParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Files seen so far, in patch order.
    pub fn files(&self) -> &[FileProgress] {
        &self.files
    }

    /// True once `*** Begin Patch` has been read.
    pub fn started(&self) -> bool {
        self.started
    }

    /// True once the `*** End Patch` line has been read.
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// Consumes a fragment. Returns true when [`files`](Self::files) or the
    /// ended flag changed (a new file header appeared or a line count moved).
    pub fn push(&mut self, fragment: &str) -> bool {
        let before = self.snapshot_key();
        self.line_buffer.push_str(fragment);
        while let Some(newline) = self.line_buffer.find('\n') {
            let rest = self.line_buffer.split_off(newline + 1);
            let mut line = std::mem::replace(&mut self.line_buffer, rest);
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
            self.process_line(&line);
        }
        // A trailing marker line without its newline is still informative
        // (`*** End Patch` is often the last bytes of the stream).
        if self.started && !self.ended && self.line_buffer.trim() == END_PATCH {
            self.ended = true;
        }
        before != self.snapshot_key()
    }

    fn snapshot_key(&self) -> (usize, usize, usize, bool) {
        let (added, removed) = self
            .files
            .iter()
            .fold((0, 0), |(a, r), file| (a + file.added, r + file.removed));
        (self.files.len(), added, removed, self.ended)
    }

    fn process_line(&mut self, line: &str) {
        let trimmed = line.trim();
        if !self.started {
            if trimmed == BEGIN_PATCH {
                self.started = true;
            }
            return;
        }
        if self.ended {
            return;
        }
        if trimmed == END_PATCH {
            self.ended = true;
        } else if let Some(path) = trimmed.strip_prefix(ADD_FILE) {
            self.open_file(path, FileOp::Add);
        } else if let Some(path) = trimmed.strip_prefix(DELETE_FILE) {
            self.open_file(path, FileOp::Delete);
        } else if let Some(path) = trimmed.strip_prefix(UPDATE_FILE) {
            self.open_file(path, FileOp::Update);
        } else if let Some(target) = trimmed.strip_prefix(MOVE_TO) {
            if let Some(file) = self.files.last_mut() {
                file.move_to = Some(target.to_string());
            }
        } else {
            self.count_body_line(line);
        }
    }

    fn open_file(&mut self, path: &str, op: FileOp) {
        self.files.push(FileProgress {
            path: path.to_string(),
            op,
            move_to: None,
            added: 0,
            removed: 0,
        });
    }

    fn count_body_line(&mut self, line: &str) {
        let Some(file) = self.files.last_mut() else {
            return;
        };
        if line.starts_with('+') {
            file.added += 1;
        } else if line.starts_with('-') && file.op == FileOp::Update {
            file.removed += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "*** Begin Patch\n*** Add File: a.txt\n+one\n+two\n*** Update File: src/b.rs\n*** Move to: src/c.rs\n@@ fn x\n ctx\n-old\n+new\n+more\n*** Delete File: gone.txt\n*** End Patch\n";

    fn expected() -> Vec<FileProgress> {
        vec![
            FileProgress { path: "a.txt".into(), op: FileOp::Add, move_to: None, added: 2, removed: 0 },
            FileProgress { path: "src/b.rs".into(), op: FileOp::Update, move_to: Some("src/c.rs".into()), added: 2, removed: 1 },
            FileProgress { path: "gone.txt".into(), op: FileOp::Delete, move_to: None, added: 0, removed: 0 },
        ]
    }

    #[test]
    fn whole_patch_in_one_fragment() {
        let mut parser = StreamingPatchParser::new();
        assert!(parser.push(PATCH));
        assert_eq!(parser.files(), expected().as_slice());
        assert!(parser.started() && parser.ended());
    }

    #[test]
    fn every_chunk_size_yields_the_same_result() {
        for size in 1..=9 {
            let mut parser = StreamingPatchParser::new();
            let chars: Vec<char> = PATCH.chars().collect();
            for chunk in chars.chunks(size) {
                parser.push(&chunk.iter().collect::<String>());
            }
            assert_eq!(parser.files(), expected().as_slice(), "chunk size {size}");
            assert!(parser.ended(), "chunk size {size}");
        }
    }

    #[test]
    fn partial_header_is_not_reported_until_the_line_completes() {
        let mut parser = StreamingPatchParser::new();
        parser.push("*** Begin Patch\n*** Add Fi");
        assert!(parser.files().is_empty());
        parser.push("le: src/ne");
        assert!(parser.files().is_empty());
        assert!(parser.push("w.rs\n+x"));
        assert_eq!(parser.files()[0].path, "src/new.rs");
        // The `+x` line is still incomplete.
        assert_eq!(parser.files()[0].added, 0);
        parser.push("\n");
        assert_eq!(parser.files()[0].added, 1);
    }

    #[test]
    fn unicode_paths_and_content_survive_fragmenting() {
        let mut parser = StreamingPatchParser::new();
        let text = "*** Begin Patch\n*** Add File: docs/özet-日本語.md\n+héllo wörld 🎉\n*** End Patch";
        for ch in text.chars() {
            parser.push(&ch.to_string());
        }
        assert_eq!(parser.files()[0].path, "docs/özet-日本語.md");
        assert_eq!(parser.files()[0].added, 1);
        assert!(parser.ended(), "End Patch without trailing newline");
    }

    #[test]
    fn crlf_lines_and_garbage_before_begin_are_tolerated() {
        let mut parser = StreamingPatchParser::new();
        parser.push("<<'EOF'\r\n*** Begin Patch\r\n*** Delete File: x\r\n*** End Patch\r\n");
        assert_eq!(parser.files().len(), 1);
        assert_eq!(parser.files()[0].path, "x");
        assert!(parser.ended());
    }

    #[test]
    fn push_reports_whether_anything_changed() {
        let mut parser = StreamingPatchParser::new();
        assert!(!parser.push("*** Begin "));
        assert!(!parser.push("Patch\n"));
        assert!(parser.push("*** Add File: f\n"));
        assert!(!parser.push(""));
        assert!(parser.push("+a\n"));
    }

    #[test]
    fn removed_lines_only_count_in_updates() {
        let mut parser = StreamingPatchParser::new();
        // A `-`-prefixed line inside an Add hunk is not valid; do not count it.
        parser.push("*** Begin Patch\n*** Add File: f\n-nope\n+yes\n");
        assert_eq!((parser.files()[0].added, parser.files()[0].removed), (1, 0));
    }
}
