//! Codex `apply_patch` engine — Rust port of `codex-rs/apply-patch`
//! (`parser.rs` + `seek_sequence.rs` + the workspace applier).
//!
//! Supports the Lark grammar:
//!
//! ```txt
//! start: begin_patch environment_id? hunk+ end_patch
//! begin_patch: "*** Begin Patch" LF
//! environment_id: "*** Environment ID: " filename LF
//! end_patch: "*** End Patch" LF?
//! hunk: add_hunk | delete_hunk | update_hunk
//! add_hunk: "*** Add File: " filename LF add_line+
//! delete_hunk: "*** Delete File: " filename LF
//! update_hunk: "*** Update File: " filename LF change_move? change+
//! change_move: "*** Move to: " filename LF
//! change: (change_context | change_line)+ eof_line?
//! change_context: ("@@" | "@@ " /(.+)/) LF
//! change_line: ("+" | "-" | " ") /(.+)/ LF
//! eof_line: "*** End of File" LF
//! ```
//!
//! The parser is lenient like Codex's default: heredoc wrappers
//! (`<<'EOF' ... EOF`) are stripped and marker lines tolerate surrounding
//! whitespace. All target/move paths are validated through `safe_path`
//! (symlink-safe workspace containment), so a patch can never escape the
//! project folder.

use serde_json::{json, Value};
use std::fmt;
use std::path::{Path, PathBuf};

const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";
const END_PATCH_MARKER: &str = "*** End Patch";
const ADD_FILE_MARKER: &str = "*** Add File: ";
const DELETE_FILE_MARKER: &str = "*** Delete File: ";
const UPDATE_FILE_MARKER: &str = "*** Update File: ";
const MOVE_TO_MARKER: &str = "*** Move to: ";
const EOF_MARKER: &str = "*** End of File";
const ENVIRONMENT_ID_MARKER: &str = "*** Environment ID: ";

// ----------------------------------------------------------------------
// Errors
// ----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    InvalidPatch(String),
    InvalidHunk { message: String, line_number: usize },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPatch(message) => write!(f, "invalid patch: {message}"),
            Self::InvalidHunk { message, line_number } => {
                write!(f, "invalid hunk at line {line_number}, {message}")
            }
        }
    }
}

// ----------------------------------------------------------------------
// Patch model
// ----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hunk {
    AddFile {
        path: PathBuf,
        contents: String,
    },
    DeleteFile {
        path: PathBuf,
    },
    UpdateFile {
        path: PathBuf,
        move_path: Option<PathBuf>,
        chunks: Vec<UpdateFileChunk>,
    },
}

impl Hunk {
    /// The path affected by this hunk (the move destination for renames).
    pub fn path(&self) -> &Path {
        match self {
            Hunk::AddFile { path, .. }
            | Hunk::DeleteFile { path }
            | Hunk::UpdateFile { path, .. } => path,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct UpdateFileChunk {
    /// Optional context line (usually a class/method/function definition)
    /// prepended to both sides to narrow down the chunk position.
    pub change_context: Option<String>,
    /// Contiguous block of lines to replace (starts with the change context).
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
    /// Indices into (`old_lines`, `new_lines`) parsed as shared context.
    pub context_line_indices: Vec<(usize, usize)>,
    /// When true, `old_lines` must match at the end of the file.
    pub is_end_of_file: bool,
}

impl UpdateFileChunk {
    fn push_context_line(&mut self, line: String) {
        self.context_line_indices.push((self.old_lines.len(), self.new_lines.len()));
        self.old_lines.push(line.clone());
        self.new_lines.push(line);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPatch {
    pub hunks: Vec<Hunk>,
    /// The normalized patch text (heredoc markers stripped).
    pub patch: String,
    pub environment_id: Option<String>,
}

// ----------------------------------------------------------------------
// Parsing
// ----------------------------------------------------------------------

/// Parses a patch in Codex's default lenient mode.
pub fn parse_patch(patch: &str) -> Result<ParsedPatch, ParseError> {
    let mut lines: Vec<String> = patch.trim().lines().map(str::to_string).collect();

    // Lenient heredoc handling: `<<'EOF' ... EOF` wrappers are stripped.
    if !patch_boundaries_ok(&lines) {
        let is_heredoc = matches!(lines.first().map(String::as_str), Some("<<EOF") | Some("<<'EOF'") | Some("<<\"EOF\""))
            && lines.last().map(|last| last.ends_with("EOF")).unwrap_or(false)
            && lines.len() >= 4;
        if is_heredoc {
            lines = lines[1..lines.len() - 1].to_vec();
            if !patch_boundaries_ok(&lines) {
                return Err(boundary_error(&lines));
            }
        } else {
            return Err(boundary_error(&lines));
        }
    }

    let mut hunks = Vec::new();
    let mut environment_id = None;
    let mut index = 0usize; // 0-based cursor into `lines`
    let line_number = |index: usize| index + 1;

    // `*** Begin Patch`
    if !lines[index].trim().eq_ignore_ascii_case(BEGIN_PATCH_MARKER) {
        return Err(ParseError::InvalidPatch(format!(
            "The first line of the patch must be '{BEGIN_PATCH_MARKER}'"
        )));
    }
    index += 1;
    if index < lines.len() && lines[index].trim_start().starts_with(ENVIRONMENT_ID_MARKER) {
        environment_id = Some(lines[index].trim_start()[ENVIRONMENT_ID_MARKER.len()..].trim().to_string());
        index += 1;
    }

    while index < lines.len() {
        let trimmed = lines[index].trim_start();
        if trimmed == END_PATCH_MARKER {
            index += 1;
            // Tolerate trailing blank lines after the end marker.
            if lines[index..].iter().any(|line| !line.trim().is_empty()) {
                return Err(ParseError::InvalidPatch(
                    "Unexpected content after '*** End Patch'.".to_string(),
                ));
            }
            return Ok(ParsedPatch {
                hunks,
                patch: lines.join("\n"),
                environment_id,
            });
        }

        if let Some(path) = trimmed.strip_prefix(ADD_FILE_MARKER) {
            let hunk_line = line_number(index);
            index += 1;
            let mut contents = String::new();
            let mut line_count = 0usize;
            while index < lines.len() {
                let line = lines[index].trim_start();
                if line.starts_with(ADD_FILE_MARKER)
                    || line.starts_with(DELETE_FILE_MARKER)
                    || line.starts_with(UPDATE_FILE_MARKER)
                    || line == END_PATCH_MARKER
                {
                    break;
                }
                let Some(content) = line.strip_prefix('+') else {
                    return Err(ParseError::InvalidHunk {
                        message: format!("Add File hunk expects lines beginning with '+', got '{line}'"),
                        line_number: line_number(index),
                    });
                };
                contents.push_str(content);
                contents.push('\n');
                line_count += 1;
                index += 1;
            }
            if line_count == 0 {
                return Err(ParseError::InvalidHunk {
                    message: format!("Add File hunk for path '{path}' is empty"),
                    line_number: hunk_line,
                });
            }
            hunks.push(Hunk::AddFile { path: PathBuf::from(path.trim()), contents });
            continue;
        }

        if let Some(path) = trimmed.strip_prefix(DELETE_FILE_MARKER) {
            hunks.push(Hunk::DeleteFile { path: PathBuf::from(path.trim()) });
            index += 1;
            continue;
        }

        if let Some(path) = trimmed.strip_prefix(UPDATE_FILE_MARKER) {
            let hunk_line = line_number(index);
            let path = path.trim().to_string();
            index += 1;
            let mut move_path: Option<PathBuf> = None;
            if index < lines.len() && lines[index].trim_start().starts_with(MOVE_TO_MARKER) {
                let destination = lines[index].trim_start()[MOVE_TO_MARKER.len()..].trim().to_string();
                move_path = Some(PathBuf::from(destination));
                index += 1;
            }

            let mut chunks: Vec<UpdateFileChunk> = Vec::new();
            while index < lines.len() {
                // Marker lines tolerate surrounding whitespace, but change
                // lines keep their leading space (it is the line prefix).
                let line = lines[index].trim_start();
                let raw = lines[index].as_str();
                let marker_at_line = line == END_PATCH_MARKER
                    || line.starts_with(ADD_FILE_MARKER)
                    || line.starts_with(DELETE_FILE_MARKER)
                    || line.starts_with(UPDATE_FILE_MARKER);
                if marker_at_line {
                    break;
                }

                // A chunk starts with a change context (`@@` / `@@ ctx`) or a
                // change line (` `/`-`/`+`).
                let at_bare_marker = line == "@@";
                let context = if let Some(ctx) = line.strip_prefix("@@ ") {
                    Some(ctx.to_string())
                } else if at_bare_marker {
                    None
                } else {
                    if !(raw.starts_with(' ') || raw.starts_with('-') || raw.starts_with('+')) {
                        return Err(ParseError::InvalidHunk {
                            message: format!("Unexpected line in Update File hunk: '{raw}'"),
                            line_number: line_number(index),
                        });
                    }
                    None
                };

                let mut chunk = UpdateFileChunk {
                    change_context: context.clone(),
                    ..Default::default()
                };
                if let Some(context) = context {
                    // The `@@ context` line belongs to the chunk; consume it.
                    chunk.push_context_line(context);
                    index += 1;
                } else if at_bare_marker {
                    // A bare `@@` marker is consumed without a context line.
                    index += 1;
                }
                // Otherwise the current line is the first change line and is
                // consumed by the inner loop below.

                while index < lines.len() {
                    let raw = lines[index].as_str();
                    let trimmed = raw.trim();
                    let trimmed_start = raw.trim_start();
                    if trimmed == EOF_MARKER {
                        chunk.is_end_of_file = true;
                        index += 1;
                        break;
                    }
                    if trimmed == END_PATCH_MARKER
                        || trimmed_start.starts_with("@@")
                        || trimmed_start.starts_with(ADD_FILE_MARKER)
                        || trimmed_start.starts_with(DELETE_FILE_MARKER)
                        || trimmed_start.starts_with(UPDATE_FILE_MARKER)
                        || trimmed_start.starts_with(MOVE_TO_MARKER)
                    {
                        break;
                    }
                    match raw.chars().next() {
                        Some(' ') => chunk.push_context_line(raw[1..].to_string()),
                        Some('-') => chunk.old_lines.push(raw[1..].to_string()),
                        Some('+') => chunk.new_lines.push(raw[1..].to_string()),
                        _ => {
                            return Err(ParseError::InvalidHunk {
                                message: format!("Unexpected line in Update File hunk: '{raw}'"),
                                line_number: line_number(index),
                            })
                        }
                    }
                    index += 1;
                }

                // Grammar requires each chunk to be non-empty.
                if chunk.change_context.is_none() && chunk.old_lines.is_empty() && chunk.new_lines.is_empty() {
                    return Err(ParseError::InvalidHunk {
                        message: format!("Update file hunk for path '{path}' is empty"),
                        line_number: hunk_line,
                    });
                }
                chunks.push(chunk);
            }

            if chunks.is_empty() {
                return Err(ParseError::InvalidHunk {
                    message: format!("Update file hunk for path '{path}' is empty"),
                    line_number: hunk_line,
                });
            }
            hunks.push(Hunk::UpdateFile {
                path: PathBuf::from(path),
                move_path,
                chunks,
            });
            continue;
        }

        return Err(ParseError::InvalidPatch(format!(
            "Invalid patch line: '{trimmed}'"
        )));
    }

    Err(ParseError::InvalidPatch(format!(
        "The last line of the patch must be '{END_PATCH_MARKER}'"
    )))
}

fn patch_boundaries_ok(lines: &[String]) -> bool {
    let first = lines.first().map(|line| line.trim());
    let last = lines.last().map(|line| line.trim());
    first == Some(BEGIN_PATCH_MARKER) && last == Some(END_PATCH_MARKER)
}

fn boundary_error(lines: &[String]) -> ParseError {
    let first = lines.first().map(|line| line.trim());
    if first != Some(BEGIN_PATCH_MARKER) {
        ParseError::InvalidPatch(format!(
            "The first line of the patch must be '{BEGIN_PATCH_MARKER}'"
        ))
    } else {
        ParseError::InvalidPatch(format!(
            "The last line of the patch must be '{END_PATCH_MARKER}'"
        ))
    }
}

// ----------------------------------------------------------------------
// Fuzzy sequence matching (seek_sequence)
// ----------------------------------------------------------------------

/// Attempts to find `pattern` within `lines` beginning at or after `start`.
/// Matches are attempted with decreasing strictness: exact, then ignoring
/// trailing whitespace, then both sides, then Unicode punctuation normalized.
/// When `eof` is true the search first tries to anchor at the end of the file.
pub fn seek_sequence(lines: &[String], pattern: &[String], start: usize, eof: bool) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }
    if pattern.len() > lines.len() {
        return None;
    }
    let last_start = lines.len() - pattern.len();
    // eof anchoring: try to match at the end of the file first.
    let search_start = if eof { last_start.max(start) } else { start };
    // Exact match first.
    for i in search_start..=last_start {
        if &lines[i..i + pattern.len()] == pattern {
            return Some(i);
        }
    }
    // Then rstrip match.
    for i in search_start..=last_start {
        if lines[i..i + pattern.len()]
            .iter()
            .zip(pattern)
            .all(|(line, pat)| line.trim_end() == pat.trim_end())
        {
            return Some(i);
        }
    }
    // Finally trim both sides.
    for i in search_start..=last_start {
        if lines[i..i + pattern.len()]
            .iter()
            .zip(pattern)
            .all(|(line, pat)| line.trim() == pat.trim())
        {
            return Some(i);
        }
    }
    // Most permissive pass: normalize common Unicode punctuation to ASCII.
    for i in search_start..=last_start {
        if lines[i..i + pattern.len()]
            .iter()
            .zip(pattern)
            .all(|(line, pat)| normalize_unicode(line) == normalize_unicode(pat))
        {
            return Some(i);
        }
    }
    None
}

/// Maps typographic dashes / quotes / odd spaces onto their ASCII forms so
/// diffs authored with plain ASCII still locate source lines that use them.
pub fn normalize_unicode(input: &str) -> String {
    input
        .trim()
        .chars()
        .map(|c| match c {
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
            | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
            | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

// ----------------------------------------------------------------------
// Application
// ----------------------------------------------------------------------

/// Affected paths of a patch (computed before anything touches the disk).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AffectedPaths {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
}

impl AffectedPaths {
    pub fn all(&self) -> Vec<String> {
        let mut all = self.added.clone();
        all.extend(self.modified.iter().cloned());
        all.extend(self.deleted.iter().cloned());
        all
    }
}

/// Validates every target/move path of a patch against the workspace without
/// touching the disk. Returns the affected (relative) paths grouped by kind.
pub fn affected_paths(project: &Path, patch: &ParsedPatch) -> Result<AffectedPaths, String> {
    let mut affected = AffectedPaths::default();
    for hunk in &patch.hunks {
        match hunk {
            Hunk::AddFile { path, .. } => {
                safe_target(project, path)?;
                affected.added.push(rel_display(path));
            }
            Hunk::DeleteFile { path } => {
                safe_target(project, path)?;
                affected.deleted.push(rel_display(path));
            }
            Hunk::UpdateFile { path, move_path, .. } => {
                safe_target(project, path)?;
                let mut modified = rel_display(path);
                if let Some(move_path) = move_path {
                    safe_target(project, move_path)?;
                    modified = format!("{} -> {}", rel_display(path), rel_display(move_path));
                }
                affected.modified.push(modified);
            }
        }
    }
    Ok(affected)
}

fn safe_target(project: &Path, path: &Path) -> Result<(), String> {
    let relative = path.to_string_lossy().replace('\\', "/");
    resolve_safe_path(project, &relative)
        .map(|_| ())
        .map_err(|err| format!("apply_patch: path {relative:?} rejected: {err}"))
}

fn rel_display(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Computes the new content of each updated file without mutating the disk.
/// Returns `(relative_path, new_contents, is_new_file)` tuples.
pub fn compute_resulting_contents(
    project: &Path,
    patch: &ParsedPatch,
) -> Result<Vec<(PathBuf, String, bool)>, String> {
    let mut results = Vec::new();
    for hunk in &patch.hunks {
        match hunk {
            Hunk::AddFile { path, contents } => {
                let target = resolve(project, path)?;
                if target.exists() {
                    return Err(format!(
                        "apply_patch: cannot add {}: file already exists",
                        rel_display(path)
                    ));
                }
                results.push((target, contents.clone(), true));
            }
            Hunk::DeleteFile { path } => {
                let target = resolve(project, path)?;
                if !target.exists() {
                    return Err(format!(
                        "apply_patch: cannot delete {}: file does not exist",
                        rel_display(path)
                    ));
                }
                results.push((target, String::new(), false));
            }
            Hunk::UpdateFile { path, move_path, chunks } => {
                let target = resolve(project, path)?;
                let original = std::fs::read_to_string(&target).map_err(|err| {
                    format!("apply_patch: cannot read {}: {err}", rel_display(path))
                })?;
                let updated = apply_update_chunks(&original, chunks, &rel_display(path))?;
                let destination = match move_path {
                    Some(move_path) => resolve(project, move_path)?,
                    None => target.clone(),
                };
                results.push((destination, updated, false));
            }
        }
    }
    Ok(results)
}

fn resolve(project: &Path, path: &Path) -> Result<PathBuf, String> {
    let relative = path.to_string_lossy().replace('\\', "/");
    resolve_safe_path(project, &relative)
        .map_err(|err| format!("apply_patch: path {relative:?} rejected: {err}"))
}

/// Standalone workspace path containment helper (moved from
/// `apps/backend/src/orchestrator/tools.rs::safe_path`) so this crate has
/// zero dependency on the backend.
///
/// Resolves `rel` against `project_root` while rejecting anything that
/// escapes it (traversal, absolute paths, symlinks pointing outside).
/// Non-existent paths resolve through their deepest existing ancestor, so
/// files that do not exist yet still resolve inside the workspace.
pub fn resolve_safe_path(project_root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Ok(project_root.to_path_buf());
    }
    if rel.contains('\0') {
        return Err("Path contains a null byte.".to_string());
    }
    let candidate = project_root.join(rel);

    // Walk up to the deepest existing ancestor, collecting the missing tail.
    let mut missing_tail: Vec<std::ffi::OsString> = Vec::new();
    let mut probe: PathBuf = candidate.clone();
    while !probe.exists() {
        match probe.file_name() {
            Some(file) => missing_tail.push(file.to_os_string()),
            None => return Err("Path could not be resolved.".to_string()),
        }
        if !probe.pop() {
            return Err("Path could not be resolved.".to_string());
        }
    }
    let deepest = std::fs::canonicalize(&probe)
        .map(normalize_canonical)
        .map_err(|_| "Path could not be resolved.".to_string())?;
    let project_canonical = std::fs::canonicalize(project_root)
        .map(normalize_canonical)
        .map_err(|_| "Project folder is not accessible.".to_string())?;
    if !deepest.starts_with(&project_canonical) {
        return Err("Path is outside the workspace.".to_string());
    }
    let mut resolved = deepest;
    for part in missing_tail.iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

/// Strips Windows verbatim (`\\?\`) / UNC (`\\?\UNC\`) prefixes from a
/// canonicalized path so it compares equal to the plain absolute path.
fn normalize_canonical(canonical: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = canonical.to_string_lossy();
        if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        if let Some(plain) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(plain);
        }
    }
    canonical
}

/// Applies the chunks of one `Update File` hunk to the original content.
fn apply_update_chunks(
    original: &str,
    chunks: &[UpdateFileChunk],
    display_path: &str,
) -> Result<String, String> {
    let ends_with_newline = original.ends_with('\n');
    let lines: Vec<String> = original.lines().map(str::to_string).collect();
    let mut result: Vec<String> = Vec::new();
    let mut position = 0usize;

    for chunk in chunks {
        let eof = chunk.is_end_of_file;
        let start = seek_sequence(&lines, &chunk.old_lines, position, eof).ok_or_else(|| {
            format!(
                "apply_patch: could not find the expected context in {display_path}{}",
                match chunk.change_context {
                    Some(ref context) => format!(" (context line: {context})"),
                    None => String::new(),
                }
            )
        })?;
        result.extend(lines[position..start].iter().cloned());
        result.extend(chunk.new_lines.iter().cloned());
        position = start + chunk.old_lines.len();
    }
    result.extend(lines[position..].iter().cloned());

    let mut output = result.join("\n");
    if ends_with_newline && (!output.is_empty() || !result.is_empty()) {
        output.push('\n');
    }
    Ok(output)
}

/// Applies a parsed patch inside the workspace. Returns Codex's summary
/// (`Success. Updated the following files:` …) plus the affected paths.
pub fn apply_parsed(project: &Path, patch: &ParsedPatch) -> Result<(String, String, AffectedPaths), String> {
    let affected = affected_paths(project, patch)?;
    let results = compute_resulting_contents(project, patch)?;
    let mut summary_lines: Vec<String> = Vec::new();
    let mut diff = String::new();

    for (hunk, (target, contents, _is_new)) in patch.hunks.iter().zip(results.into_iter()) {
        match hunk {
            Hunk::AddFile { path, .. } => {
                diff.push_str(&unified_diff("/dev/null", &format!("b/{}", rel_display(path)), "", &contents));
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|err| format!("apply_patch: cannot create parent directory: {err}"))?;
                }
                std::fs::write(&target, contents)
                    .map_err(|err| format!("apply_patch: cannot write {}: {err}", rel_display(path)))?;
                summary_lines.push(format!("A {}", rel_display(path)));
            }
            Hunk::DeleteFile { path, .. } => {
                let original = std::fs::read_to_string(&target)
                    .map_err(|err| format!("apply_patch: cannot read {}: {err}", rel_display(path)))?;
                diff.push_str(&unified_diff(
                    &format!("a/{}", rel_display(path)),
                    "/dev/null",
                    &original,
                    "",
                ));
                std::fs::remove_file(&target)
                    .map_err(|err| format!("apply_patch: cannot delete {}: {err}", rel_display(path)))?;
                summary_lines.push(format!("D {}", rel_display(path)));
            }
            Hunk::UpdateFile { path, move_path, .. } => {
                let original = std::fs::read_to_string(&resolve(project, path)?)
                    .map_err(|err| format!("apply_patch: cannot read {}: {err}", rel_display(path)))?;
                let destination_label = move_path
                    .as_ref()
                    .map(|move_path| rel_display(move_path))
                    .unwrap_or_else(|| rel_display(path));
                diff.push_str(&unified_diff(
                    &format!("a/{}", rel_display(path)),
                    &format!("b/{destination_label}"),
                    &original,
                    &contents,
                ));
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|err| format!("apply_patch: cannot create parent directory: {err}"))?;
                }
                std::fs::write(&target, &contents)
                    .map_err(|err| format!("apply_patch: cannot write {}: {err}", rel_display(path)))?;
                if move_path.is_some() {
                    std::fs::remove_file(&resolve(project, path)?)
                        .map_err(|err| format!("apply_patch: cannot remove moved source: {err}"))?;
                }
                let display = match move_path {
                    Some(move_path) => format!("{} -> {}", rel_display(path), rel_display(move_path)),
                    None => rel_display(path),
                };
                summary_lines.push(format!("M {display}"));
            }
        }
    }

    let summary = if summary_lines.is_empty() {
        "Success. No files were changed by this patch.".to_string()
    } else {
        format!(
            "Success. Updated the following files:
{}",
            summary_lines.join("
")
        )
    };
    Ok((summary, diff, affected))
}

/// Parses and applies a raw patch string inside the workspace.
pub fn apply_patch_in_workspace(
    project: &Path,
    patch_text: &str,
) -> Result<(String, String, AffectedPaths), String> {
    let parsed = parse_patch(patch_text).map_err(|err| err.to_string())?;
    apply_parsed(project, &parsed)
}

// ----------------------------------------------------------------------
// Unified diff generation (shared with preview & TurnDiffTracker)
// ----------------------------------------------------------------------

/// Context lines shown around each diff hunk.
pub const DIFF_CONTEXT: usize = 3;
/// Above this line count a plain LCS diff becomes quadratic; fall back to a
/// coarse one-hunk summary instead.
const MAX_DIFF_LINES: usize = 4000;

/// Unified diff between two texts with `a_label` / `b_label` headers.
pub fn unified_diff(a_label: &str, b_label: &str, old: &str, new: &str) -> String {
    let old_lines: Vec<&str> = if old.is_empty() { Vec::new() } else { old.lines().collect() };
    let new_lines: Vec<&str> = if new.is_empty() { Vec::new() } else { new.lines().collect() };

    if old == new {
        return String::new();
    }

    let mut output = format!("--- {a_label}
+++ {b_label}
");
    if old_lines.len() > MAX_DIFF_LINES || new_lines.len() > MAX_DIFF_LINES {
        output.push_str("@@ files differ (content too large to diff) @@
");
        return output;
    }

    let ops = lcs_ops(&old_lines, &new_lines);

    // Group changed ops (plus DIFF_CONTEXT surrounding context lines) into
    // hunks, merging runs that are closer than 2 * DIFF_CONTEXT apart.
    let mut groups: Vec<(usize, usize)> = Vec::new(); // (first op idx, last op idx)
    for (index, (_, _, op)) in ops.iter().enumerate() {
        if *op != Op::Equal {
            let start = index.saturating_sub(DIFF_CONTEXT);
            let end = (index + DIFF_CONTEXT).min(ops.len() - 1);
            let merged = groups
                .last()
                .map(|(_, prev_end)| start <= prev_end + 1)
                .unwrap_or(false);
            if merged {
                groups.last_mut().unwrap().1 = end;
            } else {
                groups.push((start, end));
            }
        }
    }

    for (group_start, group_end) in groups {
        let window = &ops[group_start..=group_end];
        let old_count = window.iter().filter(|(_, _, op)| *op != Op::Insert).count();
        let new_count = window.iter().filter(|(_, _, op)| *op != Op::Delete).count();
        let old_start = window.iter().find(|(_, _, op)| *op != Op::Insert).map(|(i, _, _)| i + 1).unwrap_or(0);
        let new_start = window.iter().find(|(_, _, op)| *op != Op::Delete).map(|(_, j, _)| j + 1).unwrap_or(0);

        output.push_str(&format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@
"));
        for (old_index, new_index, op) in window {
            match op {
                Op::Equal => output.push_str(&format!(" {}
", old_lines[*old_index])),
                Op::Delete => output.push_str(&format!("-{}
", old_lines[*old_index])),
                Op::Insert => output.push_str(&format!("+{}
", new_lines[*new_index])),
            }
        }
    }

    output
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Equal,
    Delete,
    Insert,
}

fn lcs_ops(old_lines: &[&str], new_lines: &[&str]) -> Vec<(usize, usize, Op)> {
    let n = old_lines.len();
    let m = new_lines.len();
    // DP table of LCS lengths.
    let mut table = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if old_lines[i] == new_lines[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let mut ops = Vec::with_capacity(n + m);
    let mut i = 0usize;
    let mut j = 0usize;
    while i < n && j < m {
        if old_lines[i] == new_lines[j] {
            ops.push((i, j, Op::Equal));
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            ops.push((i, j, Op::Delete));
            i += 1;
        } else {
            ops.push((i, j, Op::Insert));
            j += 1;
        }
    }
    while i < n {
        ops.push((i, j, Op::Delete));
        i += 1;
    }
    while j < m {
        ops.push((i, j, Op::Insert));
        j += 1;
    }
    ops
}

/// Builds the combined unified-diff preview for a patch without mutating the
/// disk. Returns `(diff, AffectedPaths)`.
pub fn preview_patch(project: &Path, patch_text: &str) -> Result<(String, AffectedPaths), String> {
    let parsed = parse_patch(patch_text).map_err(|err| err.to_string())?;
    let affected = affected_paths(project, &parsed)?;
    let mut diff = String::new();

    for hunk in &parsed.hunks {
        match hunk {
            Hunk::AddFile { path, contents } => {
                let label = format!("a/{}", rel_display(path));
                let _ = label;
                diff.push_str(&unified_diff(
                    &format!("/dev/null"),
                    &format!("b/{}", rel_display(path)),
                    "",
                    contents,
                ));
            }
            Hunk::DeleteFile { path } => {
                let target = resolve(project, path)?;
                let original = std::fs::read_to_string(&target)
                    .map_err(|err| format!("apply_patch: cannot read {}: {err}", rel_display(path)))?;
                diff.push_str(&unified_diff(
                    &format!("a/{}", rel_display(path)),
                    "/dev/null",
                    &original,
                    "",
                ));
            }
            Hunk::UpdateFile { path, move_path, chunks } => {
                let target = resolve(project, path)?;
                let original = std::fs::read_to_string(&target)
                    .map_err(|err| format!("apply_patch: cannot read {}: {err}", rel_display(path)))?;
                let updated = apply_update_chunks(&original, chunks, &rel_display(path))?;
                let destination = move_path.as_ref().map(|m| rel_display(m)).unwrap_or_else(|| rel_display(path));
                diff.push_str(&unified_diff(
                    &format!("a/{}", rel_display(path)),
                    &format!("b/{destination}"),
                    &original,
                    &updated,
                ));
            }
        }
    }

    Ok((diff, affected))
}

/// JSON payload used by both the tool response and the UI diff card.
pub fn affected_paths_json(affected: &AffectedPaths, diff: &str) -> Value {
    json!({
        "added": affected.added,
        "modified": affected.modified,
        "deleted": affected.deleted,
        "diff": diff,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_ws(label: &str) -> PathBuf {
        // tempfile-based unique scratch workspace (deleted on guard drop, but
        // these tests clean up explicitly and only need the path).
        let dir = tempfile::Builder::new()
            .prefix(&format!("locagens-apply-patch-{label}-"))
            .tempdir()
            .unwrap();
        // Leak the guard so the directory outlives this function; the tests
        // remove it explicitly at the end.
        dir.keep()
    }

    const ADD_PATCH: &str = "*** Begin Patch\n*** Add File: src/new.rs\n+fn main() {}\n*** End Patch\n";
    const DELETE_PATCH: &str = "*** Begin Patch\n*** Delete File: src/old.rs\n*** End Patch\n";

    #[test]
    fn parse_add_delete_update_move() {
        let patch = parse_patch(concat!(
            "*** Begin Patch\n",
            "*** Add File: path/add.py\n",
            "+abc\n",
            "+def\n",
            "*** Delete File: path/delete.py\n",
            "*** Update File: path/update.py\n",
            "*** Move to: path/update2.py\n",
            "@@ def f():\n",
            " pass\n",
            "-print(1)\n",
            "+print(2)\n",
            "*** End of File\n",
            "*** End Patch\n"
        ))
        .unwrap();
        assert_eq!(patch.hunks.len(), 3);
        assert_eq!(
            patch.hunks[0],
            Hunk::AddFile { path: PathBuf::from("path/add.py"), contents: "abc\ndef\n".into() }
        );
        assert_eq!(
            patch.hunks[1],
            Hunk::DeleteFile { path: PathBuf::from("path/delete.py") }
        );
        match &patch.hunks[2] {
            Hunk::UpdateFile { path, move_path, chunks } => {
                assert_eq!(path, &PathBuf::from("path/update.py"));
                assert_eq!(move_path.as_deref(), Some(Path::new("path/update2.py")));
                assert_eq!(chunks.len(), 1);
                assert_eq!(chunks[0].old_lines, vec!["def f():", "pass", "print(1)"]);
                assert_eq!(chunks[0].new_lines, vec!["def f():", "pass", "print(2)"]);
                assert!(chunks[0].is_end_of_file);
                assert_eq!(chunks[0].context_line_indices, vec![(0, 0), (1, 1)]);
            }
            other => panic!("unexpected hunk {other:?}"),
        }
    }

    #[test]
    fn parse_lenient_heredoc_and_whitespace() {
        let patch = parse_patch("<<'EOF'\n*** Begin Patch\n*** Add File: foo\n+hi\n*** End Patch\nEOF\n").unwrap();
        assert_eq!(
            patch.hunks,
            vec![Hunk::AddFile { path: PathBuf::from("foo"), contents: "hi\n".into() }]
        );
        // Whitespace-tolerant markers.
        let patch = parse_patch("  *** Begin Patch  \n*** Add File: x\n+1\n*** End Patch").unwrap();
        assert_eq!(patch.hunks.len(), 1);
    }

    #[test]
    fn parse_rejects_bad_boundaries_and_empty_hunks() {
        assert!(parse_patch("bad").is_err());
        assert!(parse_patch("*** Begin Patch\nbad").is_err());
        assert!(parse_patch(
            "*** Begin Patch\n*** Update File: test.py\n*** End Patch"
        )
        .is_err());
        assert!(parse_patch("*** Begin Patch\n*** End Patch").unwrap().hunks.is_empty());
    }

    #[test]
    fn seek_sequence_strictness_ladder() {
        let lines: Vec<String> = vec!["foo   ".into(), "bar\t\t".into(), "  baz".into()];
        let pattern: Vec<String> = vec!["foo".into(), "bar".into()];
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
        let pattern: Vec<String> = vec!["bar".into(), "baz".into()];
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(1));
        // Unicode punctuation normalization.
        let lines: Vec<String> = vec!["a \u{2013} b".into(), "c\u{2019}d".into()];
        let pattern: Vec<String> = vec!["a - b".into(), "c'd".into()];
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
        // eof anchoring.
        let lines: Vec<String> = vec!["one".into(), "two".into(), "three".into()];
        assert_eq!(seek_sequence(&lines, &["three".to_string()], 0, true), Some(2));
        // Pattern longer than input never panics.
        assert_eq!(seek_sequence(&["x".to_string()], &["a".into(), "b".into()], 0, false), None);
    }

    #[test]
    fn apply_add_update_delete_move_end_to_end() {
        let ws = temp_ws("e2e");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/old.rs"), "fn a() {}\nfn b() {}\n").unwrap();

        // Add + Update in one patch.
        let (summary, diff, affected) = apply_patch_in_workspace(
            &ws,
            concat!(
                "*** Begin Patch\n",
                "*** Add File: src/new.rs\n",
                "+fn c() {}\n",
                "*** Update File: src/old.rs\n",
                "@@ fn a() {}\n",
                "-fn b() {}\n",
                "+fn d() {}\n",
                "*** End Patch\n"
            ),
        )
        .unwrap();
        assert!(diff.contains("+++ b/src/new.rs"));
        assert!(diff.contains("-fn b() {}"));
        assert!(diff.contains("+fn d() {}"));
        assert!(summary.starts_with("Success. Updated the following files:"));
        assert!(summary.contains("A src/new.rs"));
        assert!(summary.contains("M src/old.rs"));
        assert_eq!(std::fs::read_to_string(ws.join("src/new.rs")).unwrap(), "fn c() {}\n");
        assert_eq!(
            std::fs::read_to_string(ws.join("src/old.rs")).unwrap(),
            "fn a() {}\nfn d() {}\n"
        );
        assert_eq!(affected.added, vec!["src/new.rs"]);
        assert_eq!(affected.modified, vec!["src/old.rs"]);

        // Move + multi-chunk update.
        let (summary, _, _) = apply_patch_in_workspace(
            &ws,
            concat!(
                "*** Begin Patch\n",
                "*** Update File: src/old.rs\n",
                "*** Move to: src/renamed.rs\n",
                "@@ fn d() {}\n",
                "+fn e() {}\n",
                "*** End Patch\n"
            ),
        )
        .unwrap();
        assert!(summary.contains("M src/old.rs -> src/renamed.rs"));
        assert!(!ws.join("src/old.rs").exists());
        assert_eq!(
            std::fs::read_to_string(ws.join("src/renamed.rs")).unwrap(),
            "fn a() {}\nfn d() {}\nfn e() {}\n"
        );

        // Delete.
        let delete_renamed = "*** Begin Patch\n*** Delete File: src/renamed.rs\n*** End Patch\n";
        let (summary, _, affected) = apply_patch_in_workspace(&ws, delete_renamed).unwrap();
        assert_eq!(summary, "Success. Updated the following files:\nD src/renamed.rs");
        assert_eq!(affected.deleted, vec!["src/renamed.rs"]);
        assert!(!ws.join("src/renamed.rs").exists());

        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn apply_delete_missing_or_add_existing_fails() {
        let ws = temp_ws("fails");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        assert!(apply_patch_in_workspace(&ws, DELETE_PATCH).is_err());
        std::fs::write(ws.join("src/old.rs"), "x").unwrap();
        // Adding over an existing file fails...
        assert!(apply_patch_in_workspace(&ws, ADD_PATCH.replace("src/new.rs", "src/old.rs").as_str()).is_err());
        // ...and deleting an existing file succeeds.
        assert!(apply_patch_in_workspace(&ws, DELETE_PATCH).is_ok());
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn fuzzy_whitespace_and_unicode_matching_applies() {
        let ws = temp_ws("fuzzy");
        std::fs::write(ws.join("doc.md"), "A \u{2013} B:\nsmart \u{201C}quote\u{201D}\n").unwrap();
        let patch = concat!(
            "*** Begin Patch\n",
            "*** Update File: doc.md\n",
            "@@ A - B:\n",
            " smart \"quote\"\n",
            "+matched\n",
            "*** End Patch\n"
        );
        let result = apply_patch_in_workspace(&ws, patch);
        assert!(result.is_ok(), "fuzzy match should succeed: {result:?}");
        let content = std::fs::read_to_string(ws.join("doc.md")).unwrap();
        assert!(content.contains("matched"));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn path_traversal_is_rejected() {
        let ws = temp_ws("traversal");
        let mut patches = vec![
            "*** Begin Patch\n*** Add File: ../escape.txt\n+x\n*** End Patch\n",
            "*** Begin Patch\n*** Update File: ../../etc/passwd\n@@\n+x\n*** End Patch\n",
            // Windows drive-letter targets only read as traversal on Windows;
            // elsewhere `C:/...` is a plain relative folder name.
        ];
        #[cfg(windows)]
        patches.push("*** Begin Patch\n*** Delete File: C:/Windows/win.ini\n*** End Patch\n");
        patches.push("*** Begin Patch\n*** Update File: a.txt\n*** Move to: ../sneak.txt\n@@\n+x\n*** End Patch\n");
        for patch in patches {
            let error = apply_patch_in_workspace(&ws, patch).unwrap_err();
            assert!(error.contains("rejected"), "expected rejection for {patch}: {error}");
        }
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn preview_generates_diff_without_mutating_disk() {
        let ws = temp_ws("preview");
        std::fs::write(ws.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let (diff, affected) = preview_patch(
            &ws,
            concat!(
                "*** Begin Patch\n",
                "*** Update File: a.txt\n",
                "@@ one\n",
                "-two\n",
                "+TWO\n",
                "*** Add File: b.txt\n",
                "+hi\n",
                "*** End Patch\n"
            ),
        )
        .unwrap();
        assert!(diff.contains("--- a/a.txt"));
        assert!(diff.contains("+++ b/a.txt"));
        assert!(diff.contains("@@ -1,3 +1,3 @@"));
        assert!(diff.contains("-two"));
        assert!(diff.contains("+TWO"));
        assert!(diff.contains("+++ b/b.txt"));
        assert_eq!(affected.modified, vec!["a.txt"]);
        assert_eq!(affected.added, vec!["b.txt"]);
        // Preview must not have mutated anything.
        assert_eq!(std::fs::read_to_string(ws.join("a.txt")).unwrap(), "one\ntwo\nthree\n");
        assert!(!ws.join("b.txt").exists());
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn unified_diff_headers_and_counts() {
        let diff = unified_diff("a/x", "b/x", "a\nb\nc\n", "a\nB\nc\nd\n");
        assert!(diff.starts_with("--- a/x\n+++ b/x\n"));
        assert!(diff.contains("@@ -1,3 +1,4 @@"));
        assert!(diff.contains("-b"));
        assert!(diff.contains("+B"));
        assert!(diff.contains("+d"));
        assert!(unified_diff("a/x", "b/x", "same\n", "same\n").is_empty());
    }
}
