//! Hierarchical `AGENTS.md` discovery — Rust port of `codex-rs/core/src/agents_md.rs`.
//!
//! Discovery walks upward from the working directory to the git root
//! (the directory containing `.git`), stopping at the project root or the
//! filesystem root. From the root back down to the working directory, each
//! directory contributes at most one doc file, checked in this order:
//! `AGENTS.override.md`, `AGENTS.md`, `CLAUDE.md`, `.agents.md`.
//!
//! Non-empty files are concatenated, separated by
//! `"\n\n--- project-doc ---\n\n"`, within a 32 KiB byte budget
//! (`project_doc_max_bytes`).

use std::path::{Path, PathBuf};

/// Budget for the concatenated project docs (Codex's `project_doc_max_bytes`).
pub const PROJECT_DOC_MAX_BYTES: usize = 32 * 1024;
const AGENTS_MD_SEPARATOR: &str = "\n\n--- project-doc ---\n\n";

/// Candidate filenames per directory, in priority order (Codex's default
/// fallback chain plus Claude's `CLAUDE.md`).
pub const CANDIDATE_FILENAMES: [&str; 4] = [
    "AGENTS.override.md",
    "AGENTS.md",
    "CLAUDE.md",
    ".agents.md",
];

/// Finds the git root for `start`: the nearest ancestor containing `.git`.
/// Returns `None` when no ancestor (up to and including `stop_at`) has one.
pub fn find_git_root(start: &Path, stop_at: &Path) -> Option<PathBuf> {
    let stop = std::fs::canonicalize(stop_at).ok()?;
    let mut cursor = std::fs::canonicalize(start).ok()?;
    loop {
        if cursor.join(".git").exists() {
            return Some(cursor);
        }
        if cursor == stop {
            return None;
        }
        if !cursor.pop() {
            return None;
        }
    }
}

/// Directories to scan, ordered from project root (or git root) down to `cwd`.
/// At most one doc file is picked per directory level.
pub fn search_dirs(cwd: &Path, project_root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut cursor = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let root = std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    loop {
        dirs.push(cursor.clone());
        if cursor == root {
            break;
        }
        match cursor.parent() {
            Some(parent) if parent != cursor => cursor = parent.to_path_buf(),
            _ => break,
        }
    }
    dirs.reverse();
    dirs
}

/// Discovery knobs (`project_doc_max_bytes` / `project_doc_fallback_filenames`).
#[derive(Debug, Clone)]
pub struct DocOptions {
    pub max_bytes: usize,
    /// Extra candidate names tried after the built-in ones.
    pub fallback_filenames: Vec<String>,
}

impl Default for DocOptions {
    fn default() -> Self {
        Self { max_bytes: PROJECT_DOC_MAX_BYTES, fallback_filenames: Vec::new() }
    }
}

/// Discover and concatenate all `AGENTS.md`-family docs between the project
/// root and the working directory, honoring the byte budget. Returns
/// `(concatenated docs, source paths)`.
pub fn load_agents_md(cwd: &Path, project_root: &Path) -> (String, Vec<PathBuf>) {
    load_agents_md_with(cwd, project_root, &DocOptions::default())
}

/// [`load_agents_md`] with an explicit byte budget and fallback filenames.
/// A zero budget disables project docs entirely.
pub fn load_agents_md_with(
    cwd: &Path,
    project_root: &Path,
    options: &DocOptions,
) -> (String, Vec<PathBuf>) {
    let root = find_git_root(cwd, project_root).unwrap_or_else(|| project_root.to_path_buf());
    let mut sources = Vec::new();
    let mut parts: Vec<String> = Vec::new();
    let mut remaining = options.max_bytes;
    let names: Vec<&str> = CANDIDATE_FILENAMES
        .iter()
        .copied()
        .chain(options.fallback_filenames.iter().map(String::as_str))
        .filter(|name| !name.trim().is_empty())
        .collect();

    for directory in search_dirs(cwd, &root) {
        if remaining == 0 {
            break;
        }
        for name in &names {
            let candidate = directory.join(name);
            if !candidate.is_file() {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&candidate) {
                // An empty doc counts as missing: try the next name, so an
                // empty AGENTS.md does not hide CLAUDE.md.
                if content.trim().is_empty() {
                    continue;
                }
                let mut text = content;
                locagens_protocol::text::truncate_at_char_boundary(&mut text, remaining);
                remaining = remaining.saturating_sub(text.len());
                parts.push(text);
                sources.push(candidate);
            }
            break; // at most one file per directory level
        }
    }

    (parts.join(AGENTS_MD_SEPARATOR), sources)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_ws(label: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("locagens-agents-md-{label}-"))
            .tempdir()
            .unwrap()
    }

    #[test]
    fn discovers_hierarchy_root_to_cwd_with_priority() {
        let ws = temp_ws("hier");
        std::fs::create_dir_all(ws.path().join("crates/app/src")).unwrap();
        // Git root marker.
        std::fs::write(ws.path().join(".git"), "").unwrap();
        std::fs::write(ws.path().join("AGENTS.md"), "root instructions").unwrap();
        // An override at root wins over AGENTS.md.
        std::fs::write(ws.path().join("AGENTS.override.md"), "root override").unwrap();
        // Empty file in the middle level is skipped entirely (level contributes nothing).
        std::fs::write(ws.path().join("crates/AGENTS.md"), "   \n").unwrap();
        std::fs::write(ws.path().join("crates/app/CLAUDE.md"), "app claude notes").unwrap();
        // .agents.md loses to CLAUDE.md at the same level.
        std::fs::write(ws.path().join("crates/app/.agents.md"), "hidden notes").unwrap();

        let cwd = ws.path().join("crates/app/src");
        let (docs, sources) = load_agents_md(&cwd, ws.path());
        assert!(docs.contains("root override"));
        assert!(!docs.contains("root instructions"), "override wins per level");
        assert!(docs.contains("--- project-doc ---"));
        assert!(docs.contains("app claude notes"));
        assert!(!docs.contains("hidden notes"));
        assert_eq!(sources.len(), 2);
        assert!(sources[0].ends_with("AGENTS.override.md"));
        assert!(sources[1].ends_with("CLAUDE.md"));
    }

    #[test]
    fn empty_agents_md_falls_back_to_claude_md() {
        let ws = temp_ws("fallback");
        std::fs::write(ws.path().join("AGENTS.md"), "  
").unwrap();
        std::fs::write(ws.path().join("CLAUDE.md"), "claude rules").unwrap();
        let (docs, sources) = load_agents_md(ws.path(), ws.path());
        assert_eq!(docs, "claude rules");
        assert!(sources[0].ends_with("CLAUDE.md"));
    }

    #[test]
    fn walks_up_to_git_root_but_not_beyond_project_root() {
        let ws = temp_ws("walk");
        let inner = ws.path().join("nested/deep");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(ws.path().join("AGENTS.md"), "root").unwrap();
        // No .git → project root itself is scanned, no further up.
        let (docs, sources) = load_agents_md(&inner, ws.path());
        assert_eq!(docs, "root");
        assert_eq!(sources.len(), 1);
    }

    #[test]
    fn byte_budget_is_enforced() {
        let ws = temp_ws("budget");
        std::fs::write(ws.path().join("AGENTS.md"), "x".repeat(PROJECT_DOC_MAX_BYTES + 500)).unwrap();
        let (docs, _) = load_agents_md(ws.path(), ws.path());
        assert_eq!(docs.len(), PROJECT_DOC_MAX_BYTES);
    }

    #[test]
    fn empty_docs_yield_empty_string() {
        let ws = temp_ws("empty");
        let (docs, sources) = load_agents_md(ws.path(), ws.path());
        assert!(docs.is_empty());
        assert!(sources.is_empty());
    }

    #[test]
    fn options_set_budget_and_fallback_filenames() {
        let ws = temp_ws("opts");
        std::fs::write(ws.path().join("TEAM.md"), "team notes here").unwrap();
        let (docs, _) = load_agents_md(ws.path(), ws.path());
        assert!(docs.is_empty(), "TEAM.md is not a default candidate");

        let options = DocOptions { max_bytes: 4, fallback_filenames: vec!["TEAM.md".into()] };
        let (docs, sources) = load_agents_md_with(ws.path(), ws.path(), &options);
        assert_eq!(docs, "team");
        assert_eq!(sources.len(), 1);

        let disabled = DocOptions { max_bytes: 0, ..options };
        assert!(load_agents_md_with(ws.path(), ws.path(), &disabled).0.is_empty());
    }
}
