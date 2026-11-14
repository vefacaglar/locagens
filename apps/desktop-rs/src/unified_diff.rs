//! Unified-diff model for the Changes review: splits a multi-file `git diff`
//! into files, parses hunks with old/new line numbers, counts `+`/`-` lines
//! and computes the unmodified gaps between hunks. Pure functions only; the
//! view lives in `components::changes_review`.

use std::collections::HashSet;

/// How a file changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
}

/// Kind of one hunk line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// One body line of a hunk with its line numbers on each side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

/// One `@@ -a,b +c,d @@ section` hunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    /// Text after the closing `@@` (the function or block context).
    pub section: String,
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    /// First old line the hunk covers (a zero-length range sits after
    /// `old_start`).
    fn old_begin(&self) -> u32 {
        if self.old_count == 0 { self.old_start + 1 } else { self.old_start }
    }

    fn new_begin(&self) -> u32 {
        if self.new_count == 0 { self.new_start + 1 } else { self.new_start }
    }

    /// One past the last old line the hunk covers.
    fn old_end(&self) -> u32 {
        self.old_begin() + self.old_count
    }

    fn new_end(&self) -> u32 {
        self.new_begin() + self.new_count
    }
}

/// One file of a diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    /// Path before the change; `None` for an added file.
    pub old_path: Option<String>,
    /// Path after the change; `None` for a deleted file.
    pub new_path: Option<String>,
    pub status: FileStatus,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
    pub added: usize,
    pub removed: usize,
}

impl FileDiff {
    fn empty() -> Self {
        FileDiff { old_path: None, new_path: None, status: FileStatus::Modified, binary: false, hunks: Vec::new(), added: 0, removed: 0 }
    }

    /// The path the file has now (the old one for a deleted file).
    pub fn path(&self) -> &str {
        self.new_path.as_deref().or(self.old_path.as_deref()).unwrap_or_default()
    }

    /// Changed lines (`+` and `-`).
    pub fn changed_lines(&self) -> usize {
        self.added + self.removed
    }

    /// A diff of a file that is all new (`removed: false`) or all gone.
    pub fn whole_file(path: &str, text: &str, removed: bool) -> Self {
        let mut file = FileDiff::empty();
        let lines: Vec<&str> = text.lines().collect();
        let count = lines.len() as u32;
        let body = lines.iter().enumerate().map(|(index, line)| {
            let number = Some(index as u32 + 1);
            DiffLine {
                kind: if removed { LineKind::Removed } else { LineKind::Added },
                old: if removed { number } else { None },
                new: if removed { None } else { number },
                text: (*line).to_string(),
            }
        });
        if removed {
            file.old_path = Some(path.to_string());
            file.status = FileStatus::Deleted;
            file.removed = lines.len();
        } else {
            file.new_path = Some(path.to_string());
            file.status = FileStatus::Added;
            file.added = lines.len();
        }
        if count > 0 {
            file.hunks.push(Hunk {
                old_start: if removed { 1 } else { 0 },
                old_count: if removed { count } else { 0 },
                new_start: if removed { 0 } else { 1 },
                new_count: if removed { 0 } else { count },
                section: String::new(),
                lines: body.collect(),
            });
        }
        file
    }
}

/// `a/x` / `b/x` / `"a/x y"` → `x`; `/dev/null` → `None`. A trailing
/// tab-separated timestamp (non-git diffs) is dropped.
fn diff_path(raw: &str) -> Option<String> {
    let raw = raw.split('\t').next().unwrap_or_default().trim_end();
    let raw = unquote(raw);
    if raw == "/dev/null" {
        return None;
    }
    let stripped = raw.strip_prefix("a/").or_else(|| raw.strip_prefix("b/")).unwrap_or(&raw);
    Some(stripped.to_string())
}

/// Undoes git's C-style path quoting (`"a/sp\"ace"`); other text unchanged.
fn unquote(raw: &str) -> String {
    let Some(inner) = raw.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) else {
        return raw.to_string();
    };
    let mut bytes = Vec::new();
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next() {
            Some('n') => bytes.push(b'\n'),
            Some('t') => bytes.push(b'\t'),
            Some(digit @ '0'..='7') => {
                let mut value = digit.to_digit(8).unwrap_or(0);
                for _ in 0..2 {
                    match chars.peek().and_then(|c| c.to_digit(8)) {
                        Some(next) => {
                            value = value * 8 + next;
                            chars.next();
                        }
                        None => break,
                    }
                }
                bytes.push(value as u8);
            }
            Some(other) => {
                let mut buf = [0u8; 4];
                bytes.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
            None => {}
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The two paths of a `diff --git a/x b/y` header.
fn git_header_paths(rest: &str) -> (Option<String>, Option<String>) {
    if let Some(quoted) = rest.strip_prefix('"') {
        // `"a/x" "b/y"` or `"a/x" b/y`
        if let Some(end) = quoted.find("\" ").map(|index| index + 2) {
            return (diff_path(&rest[..end]), diff_path(rest[end..].trim_start()));
        }
    }
    // Same path on both sides: `a/P b/P` splits in the middle.
    if !rest.len().is_multiple_of(2) {
        let mid = rest.len() / 2;
        if rest.is_char_boundary(mid) && rest.as_bytes()[mid] == b' ' && rest[..mid].get(2..) == rest[mid + 1..].get(2..) {
            return (diff_path(&rest[..mid]), diff_path(&rest[mid + 1..]));
        }
    }
    match rest.rfind(" b/") {
        Some(split) => (diff_path(&rest[..split]), diff_path(&rest[split + 1..])),
        None => (None, None),
    }
}

/// `@@ -a,b +c,d @@ section` → the hunk without lines.
fn parse_hunk_header(line: &str) -> Option<Hunk> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, section) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |text: &str| -> Option<(u32, u32)> {
        match text.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((text.parse().ok()?, 1)),
        }
    };
    let (old_start, old_count) = range(old)?;
    let (new_start, new_count) = range(new)?;
    Some(Hunk { old_start, old_count, new_start, new_count, section: section.trim().to_string(), lines: Vec::new() })
}

/// Splits `text` (a `git diff` or plain `---`/`+++` unified diff, one or
/// more files) into files with parsed hunks.
pub fn parse_unified_diff(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    // Lines still expected in the current hunk on each side.
    let (mut old_left, mut new_left) = (0u32, 0u32);
    let (mut old_no, mut new_no) = (0u32, 0u32);
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if old_left > 0 || new_left > 0 {
            let (kind, body) = match line.as_bytes().first() {
                Some(b'+') => (LineKind::Added, &line[1..]),
                Some(b'-') => (LineKind::Removed, &line[1..]),
                Some(b' ') => (LineKind::Context, &line[1..]),
                Some(b'\\') => continue,
                // Some tools strip the space of an empty context line.
                None => (LineKind::Context, ""),
                _ => {
                    // Malformed: the hunk ended early; parse as a header.
                    old_left = 0;
                    new_left = 0;
                    files_header(&mut files, line, &mut lines, &mut old_left, &mut new_left, &mut old_no, &mut new_no);
                    continue;
                }
            };
            let Some(file) = files.last_mut() else { break };
            let Some(hunk) = file.hunks.last_mut() else { break };
            let (old, new) = match kind {
                LineKind::Added => {
                    new_left = new_left.saturating_sub(1);
                    file.added += 1;
                    new_no += 1;
                    (None, Some(new_no - 1))
                }
                LineKind::Removed => {
                    old_left = old_left.saturating_sub(1);
                    file.removed += 1;
                    old_no += 1;
                    (Some(old_no - 1), None)
                }
                LineKind::Context => {
                    old_left = old_left.saturating_sub(1);
                    new_left = new_left.saturating_sub(1);
                    old_no += 1;
                    new_no += 1;
                    (Some(old_no - 1), Some(new_no - 1))
                }
            };
            hunk.lines.push(DiffLine { kind, old, new, text: body.to_string() });
            continue;
        }
        files_header(&mut files, line, &mut lines, &mut old_left, &mut new_left, &mut old_no, &mut new_no);
    }
    files
}

/// Handles one line outside a hunk body: file headers, extended headers and
/// hunk headers.
fn files_header<'a>(
    files: &mut Vec<FileDiff>,
    line: &str,
    lines: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
    old_left: &mut u32,
    new_left: &mut u32,
    old_no: &mut u32,
    new_no: &mut u32,
) {
    if let Some(rest) = line.strip_prefix("diff --git ") {
        let (old_path, new_path) = git_header_paths(rest);
        files.push(FileDiff { old_path, new_path, ..FileDiff::empty() });
        return;
    }
    if line.starts_with("--- ") && lines.peek().is_some_and(|next| next.starts_with("+++ ")) {
        let old_path = diff_path(&line[4..]);
        let new_path = lines.next().and_then(|next| diff_path(&next[4..]));
        // A plain diff (no `diff --git`) starts a file here; in a git diff
        // the file exists already and has no hunks yet.
        let starts_file = files.last().is_none_or(|file| !file.hunks.is_empty() || file.binary);
        if starts_file {
            files.push(FileDiff::empty());
        }
        let Some(file) = files.last_mut() else { return };
        match (&old_path, &new_path) {
            (None, Some(_)) => file.status = FileStatus::Added,
            (Some(_), None) => file.status = FileStatus::Deleted,
            _ => {}
        }
        if file.status != FileStatus::Renamed {
            file.old_path = old_path;
            file.new_path = new_path;
        }
        return;
    }
    let Some(file) = files.last_mut() else { return };
    if let Some(hunk) = parse_hunk_header(line) {
        *old_left = hunk.old_count;
        *new_left = hunk.new_count;
        *old_no = hunk.old_start.max(1);
        *new_no = hunk.new_start.max(1);
        file.hunks.push(hunk);
    } else if line.starts_with("new file mode") {
        file.status = FileStatus::Added;
        file.old_path = None;
    } else if line.starts_with("deleted file mode") {
        file.status = FileStatus::Deleted;
        file.new_path = None;
    } else if let Some(path) = line.strip_prefix("rename from ").or_else(|| line.strip_prefix("copy from ")) {
        file.status = FileStatus::Renamed;
        file.old_path = Some(unquote(path));
    } else if let Some(path) = line.strip_prefix("rename to ").or_else(|| line.strip_prefix("copy to ")) {
        file.status = FileStatus::Renamed;
        file.new_path = Some(unquote(path));
    } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
        file.binary = true;
    }
}

/// Unmodified lines between hunks (or before the first / after the last).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gap {
    /// Index of the hunk that follows; `hunks.len()` for the trailing gap.
    pub before_hunk: usize,
    pub old_start: u32,
    pub new_start: u32,
    pub count: u32,
}

/// The gaps of `file` from its hunk headers. The trailing gap needs the new
/// file's line count (`new_total`); without it only gaps before hunks count.
pub fn gaps(file: &FileDiff, new_total: Option<usize>) -> Vec<Gap> {
    let mut out = Vec::new();
    let (mut old_next, mut new_next) = (1u32, 1u32);
    for (index, hunk) in file.hunks.iter().enumerate() {
        let count = hunk.new_begin().saturating_sub(new_next);
        if count > 0 {
            out.push(Gap { before_hunk: index, old_start: old_next, new_start: new_next, count });
        }
        old_next = hunk.old_end();
        new_next = hunk.new_end();
    }
    if let (Some(total), false) = (new_total, file.hunks.is_empty()) {
        let count = (total as u32 + 1).saturating_sub(new_next);
        if count > 0 {
            out.push(Gap { before_hunk: file.hunks.len(), old_start: old_next, new_start: new_next, count });
        }
    }
    out
}

/// One rendered row of the review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// A collapsed gap: its index in [`gaps`], size, the following hunk's
    /// section text and whether its lines are available to expand.
    Gap { index: usize, count: u32, section: String, expandable: bool },
    Line(DiffLine),
}

/// Rows of `file`: gaps and hunk lines in order. Gaps listed in `expanded`
/// are replaced by their context lines taken from `content` (the new file).
/// At most `limit` line rows are returned; the second value counts the rest.
pub fn review_rows(file: &FileDiff, content: Option<&str>, expanded: &HashSet<usize>, limit: usize) -> (Vec<Row>, usize) {
    let total = content.map(|text| text.lines().count());
    let file_gaps = gaps(file, total);
    let readable = |gap: &Gap| total.is_some_and(|total| (gap.new_start + gap.count - 1) as usize <= total);
    let mut sink = RowSink { rows: Vec::new(), shown: 0, hidden: 0, limit };
    for hunk_index in 0..=file.hunks.len() {
        for (gap_index, gap) in file_gaps.iter().enumerate().filter(|(_, gap)| gap.before_hunk == hunk_index) {
            let expandable = readable(gap);
            if expandable && expanded.contains(&gap_index) {
                let text = content.unwrap_or_default();
                for (offset, line) in text.lines().skip(gap.new_start as usize - 1).take(gap.count as usize).enumerate() {
                    let offset = offset as u32;
                    sink.line(DiffLine { kind: LineKind::Context, old: Some(gap.old_start + offset), new: Some(gap.new_start + offset), text: line.to_string() });
                }
            } else if sink.shown < limit {
                let section = file.hunks.get(hunk_index).map(|hunk| hunk.section.clone()).unwrap_or_default();
                sink.rows.push(Row::Gap { index: gap_index, count: gap.count, section, expandable });
            }
        }
        if let Some(hunk) = file.hunks.get(hunk_index) {
            for line in &hunk.lines {
                sink.line(line.clone());
            }
        }
    }
    (sink.rows, sink.hidden)
}

/// Collects rows up to a cap of line rows, counting the ones left out.
struct RowSink {
    rows: Vec<Row>,
    shown: usize,
    hidden: usize,
    limit: usize,
}

impl RowSink {
    fn line(&mut self, line: DiffLine) {
        if self.shown < self.limit {
            self.rows.push(Row::Line(line));
            self.shown += 1;
        } else {
            self.hidden += 1;
        }
    }
}

/// Lockfiles and generated bundles start collapsed in the review.
pub fn is_generated(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    const NAMES: &[&str] = &["package-lock.json", "npm-shrinkwrap.json", "pnpm-lock.yaml", "go.sum", "bun.lockb"];
    NAMES.contains(&name.as_str()) || name.ends_with(".lock") || name.ends_with(".min.js") || name.ends_with(".min.css") || name.ends_with(".map")
}

/// `1170` → `1,170` (the separator is the caller's locale).
pub fn group_digits(value: u32, separator: char) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(separator);
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_FILES: &str = "diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 use std::fmt;
+use std::io;

 fn a() {}
@@ -20,4 +21,3 @@ fn main() {
     let x = 1;
-    let y = 2;
-    let z = 3;
+    let y = 4;
 }
diff --git a/readme.md b/readme.md
--- a/readme.md
+++ b/readme.md
@@ -5 +5 @@
-old
+new
";

    #[test]
    fn splits_files_and_counts_lines() {
        let files = parse_unified_diff(TWO_FILES);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path(), "src/lib.rs");
        assert_eq!((files[0].added, files[0].removed), (2, 2));
        assert_eq!(files[0].hunks.len(), 2);
        assert_eq!(files[1].path(), "readme.md");
        assert_eq!((files[1].added, files[1].removed), (1, 1));
        assert_eq!(files[1].status, FileStatus::Modified);
    }

    #[test]
    fn hunk_lines_carry_old_and_new_numbers() {
        let files = parse_unified_diff(TWO_FILES);
        let hunk = &files[0].hunks[1];
        assert_eq!((hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count), (20, 4, 21, 3));
        assert_eq!(hunk.section, "fn main() {");
        let numbers: Vec<(LineKind, Option<u32>, Option<u32>)> = hunk.lines.iter().map(|line| (line.kind, line.old, line.new)).collect();
        assert_eq!(
            numbers,
            vec![
                (LineKind::Context, Some(20), Some(21)),
                (LineKind::Removed, Some(21), None),
                (LineKind::Removed, Some(22), None),
                (LineKind::Added, None, Some(22)),
                (LineKind::Context, Some(23), Some(23)),
            ]
        );
        assert_eq!(hunk.lines[3].text, "    let y = 4;");
        // An empty context line keeps its place.
        assert_eq!(files[0].hunks[0].lines[2], DiffLine { kind: LineKind::Context, old: Some(2), new: Some(3), text: String::new() });
        // `@@ -5 +5 @@` means one line on each side.
        assert_eq!((files[1].hunks[0].old_count, files[1].hunks[0].new_count), (1, 1));
    }

    #[test]
    fn body_lines_that_look_like_headers_stay_in_the_hunk() {
        let diff = "diff --git a/x.txt b/x.txt\n--- a/x.txt\n+++ b/x.txt\n@@ -1,2 +1,2 @@\n--- not a header\n+++ also not\n ctx\n";
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!((files[0].added, files[0].removed), (1, 1));
        assert_eq!(files[0].hunks[0].lines[0].text, "-- not a header");
    }

    #[test]
    fn gaps_come_from_hunk_headers() {
        let files = parse_unified_diff(TWO_FILES);
        // Hunk 1 covers new 1..=4, hunk 2 starts at new 21: 16 lines between.
        assert_eq!(gaps(&files[0], None), vec![Gap { before_hunk: 1, old_start: 4, new_start: 5, count: 16 }]);
        // With the file's length the trailing gap shows up too.
        assert_eq!(gaps(&files[0], Some(30))[1], Gap { before_hunk: 2, old_start: 24, new_start: 24, count: 7 });
        // A first hunk that starts late leaves a leading gap.
        assert_eq!(gaps(&files[1], Some(5)), vec![Gap { before_hunk: 0, old_start: 1, new_start: 1, count: 4 }]);
        // Pure insertion after line 4 (`-4,0 +5,2`) starts at old line 5.
        let insert = parse_unified_diff("--- a/y\n+++ b/y\n@@ -4,0 +5,2 @@\n+a\n+b\n");
        assert_eq!(gaps(&insert[0], Some(8)), vec![
            Gap { before_hunk: 0, old_start: 1, new_start: 1, count: 4 },
            Gap { before_hunk: 1, old_start: 5, new_start: 7, count: 2 },
        ]);
    }

    #[test]
    fn rows_collapse_and_expand_gaps() {
        let files = parse_unified_diff(TWO_FILES);
        let content: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        let (rows, hidden) = review_rows(&files[0], Some(&content), &HashSet::new(), 1000);
        assert_eq!(hidden, 0);
        assert!(matches!(&rows[4], Row::Gap { index: 0, count: 16, section, expandable: true } if section == "fn main() {"));
        assert!(matches!(rows.last(), Some(Row::Gap { index: 1, count: 7, .. })));

        let expanded: HashSet<usize> = [0].into_iter().collect();
        let (rows, _) = review_rows(&files[0], Some(&content), &expanded, 1000);
        assert_eq!(rows[4], Row::Line(DiffLine { kind: LineKind::Context, old: Some(4), new: Some(5), text: "line 5".into() }));
        assert_eq!(rows.iter().filter(|row| matches!(row, Row::Line(_))).count(), 4 + 16 + 5);

        // Without content the gap is shown but cannot expand.
        let (rows, _) = review_rows(&files[0], None, &expanded, 1000);
        assert!(matches!(&rows[4], Row::Gap { expandable: false, .. }));

        // The cap counts line rows only.
        let (rows, hidden) = review_rows(&files[0], None, &HashSet::new(), 3);
        assert_eq!(rows.iter().filter(|row| matches!(row, Row::Line(_))).count(), 3);
        assert_eq!(hidden, 6);
    }

    #[test]
    fn new_deleted_renamed_and_binary_files() {
        let diff = "diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..1111111
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+one
+two
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
--- a/gone.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/old name.txt b/new name.txt
similarity index 100%
rename from old name.txt
rename to new name.txt
diff --git a/img.png b/img.png
index 1111111..2222222 100644
Binary files a/img.png and b/img.png differ
diff --git \"a/caf\\303\\251.txt\" \"b/caf\\303\\251.txt\"
--- \"a/caf\\303\\251.txt\"
+++ \"b/caf\\303\\251.txt\"
@@ -1 +1 @@
-a
+b
";
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 5);
        assert_eq!((files[0].status, files[0].old_path.as_deref(), files[0].path()), (FileStatus::Added, None, "new.txt"));
        assert_eq!(files[0].hunks[0].lines[1].new, Some(2));
        assert_eq!((files[1].status, files[1].path(), files[1].removed), (FileStatus::Deleted, "gone.txt", 1));
        assert_eq!(files[1].hunks[0].lines[0].old, Some(1));
        assert_eq!(files[2].status, FileStatus::Renamed);
        assert_eq!((files[2].old_path.as_deref(), files[2].path()), (Some("old name.txt"), "new name.txt"));
        assert!(files[2].hunks.is_empty());
        assert!(files[3].binary);
        assert_eq!(files[3].path(), "img.png");
        assert_eq!(files[4].path(), "caf\u{e9}.txt");
    }

    #[test]
    fn plain_diffs_without_git_headers_split_on_file_headers() {
        let diff = "--- a/one\t2026-01-01\n+++ b/one\t2026-01-02\n@@ -1 +1 @@\n-a\n+b\n--- /dev/null\n+++ b/two\n@@ -0,0 +1 @@\n+c\n";
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path(), "one");
        assert_eq!((files[1].path(), files[1].status), ("two", FileStatus::Added));
    }

    #[test]
    fn whole_file_diffs_and_helpers() {
        let added = FileDiff::whole_file("n.txt", "a\r\nb\r\n", false);
        assert_eq!((added.status, added.added, added.hunks[0].lines[1].new), (FileStatus::Added, 2, Some(2)));
        assert_eq!(added.hunks[0].lines[1].text, "b");
        let gone = FileDiff::whole_file("g.txt", "x", true);
        assert_eq!((gone.status, gone.removed, gone.path()), (FileStatus::Deleted, 1, "g.txt"));
        assert!(gaps(&added, Some(2)).is_empty());
        assert!(FileDiff::whole_file("e.txt", "", false).hunks.is_empty());
        assert!(is_generated("Cargo.lock") && is_generated("web/package-lock.json") && !is_generated("src/lock.rs"));
        assert_eq!((group_digits(1170, ','), group_digits(999, ','), group_digits(1234567, '.')), ("1,170".into(), "999".into(), "1.234.567".into()));
    }
}
