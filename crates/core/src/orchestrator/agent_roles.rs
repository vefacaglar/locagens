//! Named agent roles for `spawn_agent` (Codex `agent-roles/`).
//!
//! Codex roles are TOML files (`name`, `description`, `developer_instructions`,
//! `nickname_candidates`, plus config overrides) merged over two built-ins,
//! `explorer` and `worker`. Locagens loads the same shape from
//!
//! 1. `<project>/.locagens/agents/*.toml` (highest precedence; only for a
//!    trusted project folder, see `services::project_trust`),
//! 2. `<agent dir>/agents/*.toml` for every dir in the agent-dir chain
//!    (`~/.locagens`, `~/.codex`, `~/.pi/agent`, in that order),
//! 3. the built-ins.
//!
//! Supported keys: `name` (defaults to the file stem), `description`
//! (required), `developer_instructions` (appended to the sub-agent system
//! prompt), `nickname_candidates`, and `sandbox_mode = "read-only"` to make
//! the role read-only. Other Codex keys are ignored.

use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRole {
    pub name: String,
    pub description: String,
    /// Extra system-prompt text for agents of this role.
    pub instructions: Option<String>,
    /// Read-only roles get the explorer toolset.
    pub readonly: bool,
    pub nicknames: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RoleFile {
    name: Option<String>,
    description: Option<String>,
    developer_instructions: Option<String>,
    nickname_candidates: Option<Vec<String>>,
    sandbox_mode: Option<String>,
}

/// The two Codex built-ins.
pub fn builtin_roles() -> Vec<AgentRole> {
    vec![
        AgentRole {
            name: "explorer".into(),
            description: "Read-only research: answers specific codebase questions. Spawn several in parallel for independent questions and trust their results.".into(),
            instructions: None,
            readonly: true,
            nicknames: Vec::new(),
        },
        AgentRole {
            name: "worker".into(),
            description: "Full workspace tools: carries out a well-scoped implementation task. Give each worker a disjoint set of files.".into(),
            instructions: None,
            readonly: false,
            nicknames: Vec::new(),
        },
    ]
}

/// Parses one role file. `name_hint` (the file stem) is used when the file
/// has no `name`.
pub fn parse_role(contents: &str, name_hint: &str) -> Result<AgentRole, String> {
    let file: RoleFile = toml::from_str(contents).map_err(|err| format!("invalid role file: {err}"))?;
    let name = file
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(name_hint)
        .to_string();
    if !is_valid_role_name(&name) {
        return Err(format!("invalid role name \"{name}\" (use letters, digits, '-' and '_')."));
    }
    let description = file
        .description
        .as_deref()
        .map(str::trim)
        .filter(|description| !description.is_empty())
        .ok_or_else(|| format!("role \"{name}\" must define a non-empty `description`."))?
        .to_string();
    let nicknames = file
        .nickname_candidates
        .unwrap_or_default()
        .into_iter()
        .map(|nickname| nickname.trim().to_string())
        .filter(|nickname| !nickname.is_empty())
        .collect();
    Ok(AgentRole {
        name,
        description,
        instructions: file.developer_instructions.map(|text| text.trim().to_string()).filter(|text| !text.is_empty()),
        readonly: matches!(file.sandbox_mode.as_deref(), Some("read-only" | "read_only")),
        nicknames,
    })
}

fn is_valid_role_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

/// Directories scanned for role files, lowest precedence first.
fn role_dirs(project: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = crate::paths::agent_dir_chain()
        .into_iter()
        .rev()
        .map(|dir| dir.join("agents"))
        .collect();
    // Project roles can redefine `explorer` as writable: trusted folders only.
    if crate::services::project_trust::is_trusted(project) {
        dirs.push(project.join(".locagens").join("agents"));
    }
    dirs
}

/// Built-ins overlaid by user and project role files. Bad files are skipped.
pub fn load_roles(project: &Path) -> Vec<AgentRole> {
    load_roles_from(&role_dirs(project))
}

pub fn load_roles_from(dirs: &[PathBuf]) -> Vec<AgentRole> {
    let mut roles = builtin_roles();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
            .collect();
        files.sort();
        for path in files {
            let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
            let Ok(contents) = std::fs::read_to_string(&path) else { continue };
            if let Ok(role) = parse_role(&contents, stem) {
                roles.retain(|existing| existing.name != role.name);
                roles.push(role);
            }
        }
    }
    roles
}

/// Picks the role for a spawn request. `None`/blank means `explorer` (the
/// conservative default). Read-only run modes (`plan`, `chat`) force the role
/// read-only whatever was asked for.
pub fn resolve_role(roles: &[AgentRole], requested: Option<&str>, mode: &str) -> Result<AgentRole, String> {
    let wanted = requested.map(str::trim).filter(|name| !name.is_empty()).unwrap_or("explorer");
    let mut role = roles
        .iter()
        .find(|role| role.name.eq_ignore_ascii_case(wanted))
        .cloned()
        .ok_or_else(|| {
            let names: Vec<&str> = roles.iter().map(|role| role.name.as_str()).collect();
            format!("Unknown agent_type \"{wanted}\". Available roles: {}.", names.join(", "))
        })?;
    if mode == "plan" || mode == "chat" {
        role.readonly = true;
    }
    Ok(role)
}

/// Role listing for the `spawn_agent` tool description.
pub fn describe_roles(roles: &[AgentRole]) -> String {
    roles
        .iter()
        .map(|role| {
            let access = if role.readonly { "read-only" } else { "full tools" };
            format!("- {} ({access}): {}", role.name, role.description)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locagens-roles-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn builtins_are_explorer_readonly_and_worker_full() {
        let roles = builtin_roles();
        assert!(roles.iter().find(|r| r.name == "explorer").unwrap().readonly);
        assert!(!roles.iter().find(|r| r.name == "worker").unwrap().readonly);
    }

    #[test]
    fn parses_codex_style_role_file() {
        let role = parse_role(
            "description = \"Reviews diffs\"\ndeveloper_instructions = \"Be terse.\"\nnickname_candidates = [\"Ada\", \" \", \"Linus\"]\nsandbox_mode = \"read-only\"\nmodel = \"ignored\"\n",
            "reviewer",
        )
        .unwrap();
        assert_eq!(role.name, "reviewer");
        assert_eq!(role.instructions.as_deref(), Some("Be terse."));
        assert_eq!(role.nicknames, vec!["Ada", "Linus"]);
        assert!(role.readonly);
    }

    #[test]
    fn rejects_missing_description_and_bad_names() {
        assert!(parse_role("name = \"x\"", "x").unwrap_err().contains("description"));
        assert!(parse_role("description = \"d\"\nname = \"bad name!\"", "x").unwrap_err().contains("invalid role name"));
        assert!(parse_role("not toml [", "x").is_err());
    }

    #[test]
    fn later_dirs_override_earlier_ones_and_builtins() {
        let low = temp_dir();
        let high = temp_dir();
        std::fs::write(low.join("worker.toml"), "description = \"low worker\"\ndeveloper_instructions = \"low\"").unwrap();
        std::fs::write(low.join("docs.toml"), "description = \"docs writer\"").unwrap();
        std::fs::write(high.join("worker.toml"), "description = \"high worker\"").unwrap();
        std::fs::write(high.join("broken.toml"), "description = ").unwrap();
        let roles = load_roles_from(&[low.clone(), high.clone()]);
        let worker = roles.iter().find(|r| r.name == "worker").unwrap();
        assert_eq!(worker.description, "high worker");
        assert!(worker.instructions.is_none());
        assert!(roles.iter().any(|r| r.name == "docs"));
        assert!(!roles.iter().any(|r| r.name == "broken"));
        assert_eq!(roles.iter().filter(|r| r.name == "worker").count(), 1);
        let _ = std::fs::remove_dir_all(low);
        let _ = std::fs::remove_dir_all(high);
    }

    #[test]
    fn resolve_defaults_to_explorer_and_forces_readonly_in_plan_mode() {
        let roles = builtin_roles();
        assert_eq!(resolve_role(&roles, None, "auto").unwrap().name, "explorer");
        assert_eq!(resolve_role(&roles, Some("  "), "auto").unwrap().name, "explorer");
        let worker = resolve_role(&roles, Some("Worker"), "accept_edits").unwrap();
        assert_eq!(worker.name, "worker");
        assert!(!worker.readonly);
        assert!(resolve_role(&roles, Some("worker"), "plan").unwrap().readonly);
        assert!(resolve_role(&roles, Some("worker"), "chat").unwrap().readonly);
        let err = resolve_role(&roles, Some("wizard"), "auto").unwrap_err();
        assert!(err.contains("explorer, worker"), "{err}");
    }

    #[test]
    fn describe_lists_access_levels() {
        let text = describe_roles(&builtin_roles());
        assert!(text.contains("- explorer (read-only)"));
        assert!(text.contains("- worker (full tools)"));
    }
}
