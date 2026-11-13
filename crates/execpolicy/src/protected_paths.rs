//! Workspace paths that hold code Locagens or git runs later, so nothing the
//! agent does may write them without the user seeing it.
//!
//! One definition shared by the command sandboxes (Seatbelt / Bubblewrap /
//! Windows deny-write ACEs) and the in-process file tools (`write_file`,
//! `apply_patch`, …), so both enforce the same boundary.

use std::path::{Component, Path};

/// How a protected entry is matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectedKind {
    /// The directory and everything below it.
    Subtree,
    /// Exactly this file.
    File,
}

/// A workspace-relative protected path (`/`-separated).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtectedPath {
    pub relative: &'static str,
    pub kind: ProtectedKind,
    /// Created before a sandboxed command starts, so the command cannot
    /// create it first and own it.
    pub pre_create: bool,
}

impl ProtectedPath {
    /// The absolute path below `root`, joined component by component.
    pub fn under(&self, root: &Path) -> std::path::PathBuf {
        self.relative.split('/').fold(root.to_path_buf(), |path, part| path.join(part))
    }
}

/// Paths sandboxed commands may never write in `workspace-write`:
/// `.locagens` (hooks.json, mcp.json, rules, agent roles, plugins) and git
/// hooks and config (run by the user's next git command).
pub const SANDBOX_PROTECTED_PATHS: [ProtectedPath; 3] = [
    ProtectedPath { relative: ".locagens", kind: ProtectedKind::Subtree, pre_create: true },
    ProtectedPath { relative: ".git/hooks", kind: ProtectedKind::Subtree, pre_create: false },
    ProtectedPath { relative: ".git/config", kind: ProtectedKind::File, pre_create: false },
];

/// Extra top-level entries the file tools protect. Commands must keep
/// writing `.git/objects` & co. (git itself), but a file tool never needs to
/// touch git metadata or another agent's config directory.
const FILE_TOOL_PROTECTED_ROOTS: [&str; 4] = [".git", ".locagens", ".codex", ".agents"];

/// Normalizes one path component the way Windows resolves it: case folded,
/// alternate-data-stream suffix (`name:stream`) dropped and trailing dots /
/// spaces trimmed (`.git.` and `.git ` open `.git`).
fn normalized_component(component: &str) -> String {
    let name = component.split(':').next().unwrap_or(component);
    name.trim_end_matches(['.', ' ']).to_lowercase()
}

fn normalized_components(relative: &Path) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(normalized_component(&part.to_string_lossy())),
            Component::CurDir => {}
            // Parent / root / prefix components mean the path was not a
            // plain workspace-relative path: treat it as protected.
            _ => return None,
        }
    }
    Some(parts)
}

fn starts_with_entry(parts: &[String], relative: &str) -> bool {
    let entry: Vec<&str> = relative.split('/').collect();
    parts.len() >= entry.len() && parts.iter().zip(&entry).all(|(part, expected)| part == expected)
}

/// Whether a workspace-relative path is protected for the in-process file
/// tools: anything under `.git`, `.locagens`, `.codex` or `.agents` (which
/// covers every sandbox-protected path). Unnormalizable paths count as
/// protected.
pub fn is_protected_for_file_tools(relative: &Path) -> bool {
    let Some(parts) = normalized_components(relative) else { return true };
    let sandbox = SANDBOX_PROTECTED_PATHS.iter().any(|entry| starts_with_entry(&parts, entry.relative));
    sandbox || parts.first().is_some_and(|first| FILE_TOOL_PROTECTED_ROOTS.contains(&first.as_str()))
}

/// [`is_protected_for_file_tools`] for an absolute path inside `root`
/// (both already canonical). Paths outside `root` count as protected.
pub fn is_protected_in_workspace(root: &Path, absolute: &Path) -> bool {
    match absolute.strip_prefix(root) {
        Ok(relative) => is_protected_for_file_tools(relative),
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_tools_protect_git_and_agent_config_dirs() {
        for path in [
            ".git/hooks/pre-commit",
            ".git/config",
            ".git/modules/sub/hooks/post-checkout",
            ".git",
            ".locagens/hooks.json",
            ".locagens/rules/default.rules",
            ".locagens/agents/explorer.toml",
            ".codex/config.toml",
            ".agents/skills/x/SKILL.md",
            ".GIT/HOOKS/pre-commit",
            ".Locagens/mcp.json",
            ".locagens./hooks.json",
            ".locagens /hooks.json",
            ".locagens/hooks.json::$DATA",
            "./.locagens/hooks.json",
            "../outside",
        ] {
            assert!(is_protected_for_file_tools(Path::new(path)), "{path} must be protected");
        }
        for path in ["src/main.rs", ".github/workflows/ci.yml", "docs/.locagens.md", "a/.git/x", ".gitignore", "README.md"] {
            assert!(!is_protected_for_file_tools(Path::new(path)), "{path} stays writable");
        }
    }

    #[test]
    fn workspace_absolute_paths_are_resolved_against_the_root() {
        let root = Path::new("/work/app");
        assert!(is_protected_in_workspace(root, Path::new("/work/app/.git/hooks/x")));
        assert!(!is_protected_in_workspace(root, Path::new("/work/app/src/x.rs")));
        assert!(is_protected_in_workspace(root, Path::new("/elsewhere/x")));
    }

    #[test]
    fn sandbox_paths_are_covered_by_the_file_tool_check() {
        for entry in SANDBOX_PROTECTED_PATHS {
            assert!(is_protected_for_file_tools(Path::new(entry.relative)));
        }
    }
}
