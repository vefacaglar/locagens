//! Skill discovery, parsing and installation — port of `SkillRegistry.ts` +
//! `parseSkillMd.ts`.
//!
//! Skills are `SKILL.md` instruction packs with YAML frontmatter (`name`,
//! `description`). They are discovered from the Locagens user skills root,
//! the `pi` agent skills directory, and the project's `.locagens/skills`
//! folder (project skills override user skills with the same name).

use crate::paths;
use crate::util::canonical_project_path;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const SKILL_FILE: &str = "SKILL.md";
const MAX_BODY_CHARS: usize = 100_000;
const MAX_SKILL_FILE_CHARS: usize = 120_000;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub source: String, // "user" | "project"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SkillsListResponse {
    pub skills: Vec<SkillSummary>,
    pub roots: SkillRoots,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SkillRoots {
    pub user: String,
    pub project: Option<String>,
}

pub struct ParsedSkillMd {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// Parses a SKILL.md file. Returns None when required fields are missing/invalid.
pub fn parse_skill_md(raw: &str) -> Option<ParsedSkillMd> {
    if raw.trim().is_empty() {
        return None;
    }
    let normalized = raw.strip_prefix('\u{FEFF}').unwrap_or(raw);
    if !normalized.trim_start().starts_with("---") {
        return None;
    }
    // Frontmatter may be preceded only by whitespace; locate the opening.
    let trimmed_start = normalized.trim_start();
    if !trimmed_start.starts_with("---") {
        return None;
    }
    let offset = normalized.len() - trimmed_start.len();
    let body_str = &normalized[offset + 3..];
    let end_rel = body_str.find("\n---")?;
    let frontmatter = body_str[..end_rel].trim_start_matches(['\r', '\n']);
    let rest = &body_str[end_rel + 4..];
    let mut body = rest.trim_start_matches(['\r', '\n']).to_string();
    if body.chars().count() > MAX_BODY_CHARS {
        body = body.chars().take(MAX_BODY_CHARS).collect();
        body.push_str("\n\n[skill body truncated]");
    }

    let fields = parse_frontmatter_fields(frontmatter);
    let name = normalize_name(fields.get("name")?)?;
    let description = normalize_description(fields.get("description")?)?;
    Some(ParsedSkillMd {
        name,
        description,
        body,
    })
}

fn frontmatter_line_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^([A-Za-z][A-Za-z0-9_-]*)\s*:\s*(.*)$").unwrap())
}

fn parse_frontmatter_fields(frontmatter: &str) -> BTreeMap<String, String> {
    let re = frontmatter_line_regex();
    let mut fields = BTreeMap::new();
    for line in frontmatter.split(['\r', '\n']) {
        if let Some(caps) = re.captures(line) {
            let key = caps.get(1).map(|m| m.as_str().to_string()).unwrap_or_default();
            let value = caps.get(2).map(|m| m.as_str().to_string()).unwrap_or_default();
            let value = unquote(&value);
            fields.insert(key, value);
        }
    }
    fields
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

/// Skill names are lowercase slugs: letters, digits, hyphen, underscore, slash for nested.
fn normalize_name(value: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^[a-z0-9][a-z0-9/_-]*$").unwrap());
    let name = value.trim().to_lowercase();
    if name.is_empty() || name.chars().count() > 64 {
        return None;
    }
    if !re.is_match(&name) || name.contains("..") || name.starts_with('/') || name.ends_with('/') {
        return None;
    }
    Some(name)
}

fn normalize_description(value: &str) -> Option<String> {
    let description = whitespace_collapse(value.trim());
    if description.is_empty() || description.chars().count() > 1024 {
        return None;
    }
    Some(description)
}

fn whitespace_collapse(text: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\s+").unwrap());
    re.replace_all(text, " ").to_string()
}

/// The primary (Locagens) user skills root.
pub fn user_root() -> PathBuf {
    paths::skills_root()
}

/// Ensure + realpath the user skills directory.
pub fn ensure_user_root() -> PathBuf {
    let root = user_root();
    let _ = std::fs::create_dir_all(&root);
    crate::util::normalize_canonical(std::fs::canonicalize(&root).unwrap_or(root))
}

/// The user root is primary; `~/.pi/agent/skills` is a secondary user source.
pub fn pi_root() -> PathBuf {
    paths::pi_skills_root()
}

/// Project skill root (`.locagens/skills`), or None when unset/inaccessible.
pub fn project_root(project_path: Option<&str>) -> Option<PathBuf> {
    let trimmed = project_path?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let base = std::fs::canonicalize(trimmed).ok()?;
    Some(base.join(".locagens").join("skills"))
}

pub fn ensure_project_root(project_path: &str) -> Option<PathBuf> {
    let root = project_root(Some(project_path))?;
    let _ = std::fs::create_dir_all(&root);
    std::fs::canonicalize(&root).ok().map(crate::util::normalize_canonical)
}

fn scan_root(root: &Path, source: &str, by_name: &mut BTreeMap<String, SkillSummary>) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let dir_name = entry.file_name().to_string_lossy().to_string();
        if dir_name.starts_with('.') {
            continue;
        }
        if !entry.path().is_dir() {
            continue;
        }
        let skill_file = entry.path().join(SKILL_FILE);
        let raw = match std::fs::read_to_string(&skill_file) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        let parsed = match parse_skill_md(&raw) {
            Some(parsed) => parsed,
            None => continue,
        };
        by_name.insert(
            parsed.name.clone(),
            SkillSummary {
                name: parsed.name,
                description: parsed.description,
                source: source.to_string(),
                body: Some(parsed.body),
            },
        );
    }
}

/// Discovers skills for a run. User skills load first; project skills override
/// the same name. The Locagens root wins over the pi agent root for a name.
pub fn list(project_path: Option<&str>) -> Vec<SkillSummary> {
    let mut by_name = BTreeMap::new();
    scan_root(&user_root(), "user", &mut by_name);
    let pi_root = pi_root();
    if pi_root != user_root() {
        scan_root(&pi_root, "user", &mut by_name);
    }
    if let Some(project_root) = project_root(project_path) {
        scan_root(&project_root, "project", &mut by_name);
    }
    by_name.into_values().collect()
}

/// Installs a SKILL.md into the user or project skills root, under <name>/SKILL.md.
pub fn install(
    target: &str,
    content: &str,
    project_path: Option<&str>,
) -> Result<SkillSummary, String> {
    let raw = content;
    if raw.trim().is_empty() {
        return Err("SKILL.md content is empty.".to_string());
    }
    if raw.len() > MAX_SKILL_FILE_CHARS {
        return Err("SKILL.md is too large (max ~120KB).".to_string());
    }
    let parsed = parse_skill_md(raw)
        .ok_or_else(|| "Invalid SKILL.md. Need YAML frontmatter with name and description, e.g.\n---\nname: my-skill\ndescription: \"...\"\n---\n".to_string())?;

    let root = match target {
        "user" => ensure_user_root(),
        "project" => {
            let raw_project = project_path.unwrap_or("").trim().to_string();
            if raw_project.is_empty() {
                return Err("projectPath is required for project skills.".to_string());
            }
            ensure_project_root(&raw_project)
                .ok_or_else(|| "Could not create project skills folder.".to_string())?
        }
        other => return Err(format!("target must be \"user\" or \"project\" (got {other:?})")),
    };

    // Folder name follows the skill name (slug); reject path segments.
    let folder_name = parsed.name.rsplit('/').next().unwrap_or("").to_string();
    if folder_name.is_empty() || folder_name.contains("..") {
        return Err("Invalid skill name.".to_string());
    }

    let resolved_dir = root.join(&folder_name);
    let canonical_dir = std::fs::canonicalize(&resolved_dir).ok();
    let _ = std::fs::create_dir_all(&resolved_dir);
    let real_dir = std::fs::canonicalize(&resolved_dir).map(crate::util::normalize_canonical)
        .map_err(|_| "Skill directory resolves outside the skills root.".to_string())?;
    if let Some(prev) = canonical_dir {
        if prev != real_dir {
            return Err("Skill directory resolves outside the skills root.".to_string());
        }
    }
    if !real_dir.starts_with(&root) {
        return Err("Skill directory resolves outside the skills root.".to_string());
    }

    let skill_file = real_dir.join(SKILL_FILE);
    std::fs::write(&skill_file, raw.strip_prefix('\u{FEFF}').unwrap_or(raw))
        .map_err(|err| format!("Failed to write SKILL.md: {err}"))?;

    Ok(SkillSummary {
        name: parsed.name,
        description: parsed.description,
        source: target.to_string(),
        body: Some(parsed.body),
    })
}

/// Deletes a skill folder under the user or project skills root.
pub fn delete(target: &str, name: &str, project_path: Option<&str>) -> Result<bool, String> {
    let raw_name = name.trim().to_lowercase();
    if raw_name.is_empty() {
        return Err("Skill name is required.".to_string());
    }
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^[a-z0-9][a-z0-9/_-]*$").unwrap());
    if !re.is_match(&raw_name)
        || raw_name.contains("..")
        || raw_name.starts_with('/')
        || raw_name.ends_with('/')
    {
        return Err("Invalid skill name.".to_string());
    }
    let folder_name = raw_name.rsplit('/').next().unwrap_or("").to_string();
    if folder_name.is_empty() {
        return Err("Invalid skill name.".to_string());
    }

    let root = match target {
        "user" => user_root(),
        "project" => {
            let raw_project = project_path.unwrap_or("").trim().to_string();
            if raw_project.is_empty() {
                return Err("projectPath is required for project skills.".to_string());
            }
            project_root(Some(&raw_project))
                .ok_or_else(|| "Invalid project skills path.".to_string())?
        }
        other => return Err(format!("target must be \"user\" or \"project\" (got {other:?})")),
    };

    if !root.exists() {
        return Ok(false);
    }
    let real_root = crate::util::normalize_canonical(std::fs::canonicalize(&root).unwrap_or(root.clone()));
    let resolved_dir = real_root.join(&folder_name);
    if !resolved_dir.exists() {
        return Ok(false);
    }
    let real_dir = std::fs::canonicalize(&resolved_dir).map(crate::util::normalize_canonical)
        .map_err(|_| "Skill path escapes the skills directory.".to_string())?;
    if !real_dir.starts_with(&real_root) {
        return Err("Skill path escapes the skills directory.".to_string());
    }
    std::fs::remove_dir_all(&real_dir)
        .map_err(|err| format!("Failed to delete skill: {err}"))?;
    Ok(true)
}

/// Validate a project path for skills callers (registered or not, we only need
/// an existing directory).
pub fn validate_project_path(raw: &str) -> Option<String> {
    canonical_project_path(raw).ok().map(|p| p.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_skill_frontmatter() {
        let raw = "---\nname: my-skill\ndescription: \"Does useful things\"\n---\n\n# Body\ncontent here";
        let parsed = parse_skill_md(raw).expect("should parse");
        assert_eq!(parsed.name, "my-skill");
        assert_eq!(parsed.description, "Does useful things");
        assert!(parsed.body.contains("# Body"));
    }

    #[test]
    fn rejects_missing_description() {
        let raw = "---\nname: my-skill\n---\nbody";
        assert!(parse_skill_md(raw).is_none());
    }

    #[test]
    fn rejects_bad_names() {
        let raw = "---\nname: ../evil\ndescription: x\n---\nbody";
        assert!(parse_skill_md(raw).is_none());
    }
}
