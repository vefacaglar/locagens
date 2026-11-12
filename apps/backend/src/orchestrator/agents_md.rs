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

/// Discover and concatenate all `AGENTS.md`-family docs between the project
/// root and the working directory, honoring the byte budget. Returns
/// `(concatenated docs, source paths)`.
pub fn load_agents_md(cwd: &Path, project_root: &Path) -> (String, Vec<PathBuf>) {
    let root = find_git_root(cwd, project_root).unwrap_or_else(|| project_root.to_path_buf());
    let mut sources = Vec::new();
    let mut parts: Vec<String> = Vec::new();
    let mut remaining = PROJECT_DOC_MAX_BYTES;

    for directory in search_dirs(cwd, &root) {
        if remaining == 0 {
            break;
        }
        for name in CANDIDATE_FILENAMES {
            let candidate = directory.join(name);
            if !candidate.is_file() {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&candidate) {
                if content.trim().is_empty() {
                    break;
                }
                let mut text = content;
                if text.len() > remaining {
                    text.truncate(remaining);
                }
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
    use std::path::PathBuf;

    fn temp_ws(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "locagens-agents-md-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn discovers_hierarchy_root_to_cwd_with_priority() {
        let ws = temp_ws("hier");
        std::fs::create_dir_all(ws.join("crates/app/src")).unwrap();
        // Git root marker.
        std::fs::write(ws.join(".git"), "").unwrap();
        std::fs::write(ws.join("AGENTS.md"), "root instructions").unwrap();
        // An override at root wins over AGENTS.md.
        std::fs::write(ws.join("AGENTS.override.md"), "root override").unwrap();
        // Empty file in the middle level is skipped entirely (level contributes nothing).
        std::fs::write(ws.join("crates/AGENTS.md"), "   \n").unwrap();
        std::fs::write(ws.join("crates/app/CLAUDE.md"), "app claude notes").unwrap();
        // .agents.md loses to CLAUDE.md at the same level.
        std::fs::write(ws.join("crates/app/.agents.md"), "hidden notes").unwrap();

        let cwd = ws.join("crates/app/src");
        let (docs, sources) = load_agents_md(&cwd, &ws);
        assert!(docs.contains("root override"));
        assert!(!docs.contains("root instructions"), "override wins per level");
        assert!(docs.contains("--- project-doc ---"));
        assert!(docs.contains("app claude notes"));
        assert!(!docs.contains("hidden notes"));
        assert_eq!(sources.len(), 2);
        assert!(sources[0].ends_with("AGENTS.override.md"));
        assert!(sources[1].ends_with("CLAUDE.md"));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn walks_up_to_git_root_but_not_beyond_project_root() {
        let ws = temp_ws("walk");
        let inner = ws.join("nested/deep");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(ws.join("AGENTS.md"), "root").unwrap();
        // No .git → project root itself is scanned, no further up.
        let (docs, sources) = load_agents_md(&inner, &ws);
        assert_eq!(docs, "root");
        assert_eq!(sources.len(), 1);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn byte_budget_is_enforced() {
        let ws = temp_ws("budget");
        std::fs::write(ws.join("AGENTS.md"), "x".repeat(PROJECT_DOC_MAX_BYTES + 500)).unwrap();
        let (docs, _) = load_agents_md(&ws, &ws);
        assert_eq!(docs.len(), PROJECT_DOC_MAX_BYTES);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn empty_docs_yield_empty_string() {
        let ws = temp_ws("empty");
        let (docs, sources) = load_agents_md(&ws, &ws);
        assert!(docs.is_empty());
        assert!(sources.is_empty());
        let _ = std::fs::remove_dir_all(&ws);
    }
}
