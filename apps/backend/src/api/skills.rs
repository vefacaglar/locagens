//! Skills facade: list, read, install, skills-folder and delete logic for
//! the Settings → Skills feature.

use crate::api::error::{ApiError, ApiResult};
use crate::services::skills as skill_service;
use crate::AppState;
use serde::{Deserialize, Serialize};

/// Input for `POST /api/skills` (install) and `DELETE /api/skills/:name`
/// (target/projectPath may arrive via body or query).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct InstallSkillInput {
    pub target: Option<String>,
    pub content: Option<String>,
    pub project_path: Option<String>,
}

/// Resolved skills folder for `POST /api/skills/folder` (`{path,target}`).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillsFolder {
    pub path: String,
    pub target: String,
}

/// Resolves `raw` to a registered project path, falling back to any existing
/// directory (`skill_service::validate_project_path`); `None` when absent or
/// unresolvable.
fn resolve_project(state: &AppState, raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim().to_string();
    if raw.is_empty() {
        return None;
    }
    // Try a registered-project check, fall back to any existing directory.
    match crate::api::projects::require_registered_project(state, &raw) {
        Ok(path) => Some(path.to_string_lossy().to_string()),
        Err(_) => skill_service::validate_project_path(&raw),
    }
}

/// Lists installed skills plus the user/project skill roots.
/// Mirrors `GET /api/skills`.
pub async fn list_skills(state: &AppState, project_path: Option<&str>) -> ApiResult<skill_service::SkillsListResponse> {
    let project = resolve_project(state, project_path);
    Ok(skill_service::SkillsListResponse {
        skills: skill_service::list(project.as_deref()),
        roots: skill_service::SkillRoots {
            user: skill_service::user_root().to_string_lossy().to_string(),
            project: skill_service::project_root(project.as_deref())
                .map(|p| p.to_string_lossy().to_string()),
        },
    })
}

/// Finds one skill by name (trimmed; empty name is a BadRequest).
/// Mirrors `GET /api/skills/:name`.
pub async fn get_skill(state: &AppState, name: &str, project_path: Option<&str>) -> ApiResult<skill_service::SkillSummary> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::bad_request("Skill name is required"));
    }
    let project = resolve_project(state, project_path);
    let found = skill_service::list(project.as_deref())
        .into_iter()
        .find(|skill| skill.name == name);
    match found {
        Some(skill) => Ok(skill),
        None => Err(ApiError::not_found(format!("Skill \"{name}\" not found"))),
    }
}

/// Installs a SKILL.md into the user or project skills root.
/// Mirrors `POST /api/skills`.
pub async fn install_skill(state: &AppState, input: InstallSkillInput) -> ApiResult<skill_service::SkillSummary> {
    let target = match input.target.as_deref() {
        Some("project") => "project",
        Some("user") => "user",
        _ => {
            return Err(ApiError::bad_request(
                "target must be \"user\" or \"project\"",
            ))
        }
    };
    let content = input.content.clone().unwrap_or_default();
    if content.trim().is_empty() {
        return Err(ApiError::bad_request(
            "content is required (full SKILL.md text)",
        ));
    }

    let project = if target == "project" {
        let raw = input.project_path.clone().unwrap_or_default();
        if raw.trim().is_empty() {
            return Err(ApiError::bad_request(
                "projectPath is required for project skills",
            ));
        }
        Some(
            crate::api::projects::require_registered_project(state, raw.trim())?
                .to_string_lossy()
                .to_string(),
        )
    } else {
        None
    };

    skill_service::install(target, &content, project.as_deref()).map_err(ApiError::bad_request)
}

/// Ensures the skills folder for `target` exists and returns its path; the
/// caller opens it. Mirrors `POST /api/skills/folder`.
pub async fn skills_folder(state: &AppState, input: InstallSkillInput) -> ApiResult<SkillsFolder> {
    let target = match input.target.as_deref() {
        Some("project") => "project",
        Some("user") => "user",
        _ => {
            return Err(ApiError::bad_request(
                "target must be \"user\" or \"project\"",
            ))
        }
    };

    if target == "user" {
        let dir = skill_service::ensure_user_root();
        return Ok(SkillsFolder {
            path: dir.to_string_lossy().to_string(),
            target: "user".to_string(),
        });
    }

    let raw = input.project_path.clone().unwrap_or_default();
    if raw.trim().is_empty() {
        return Err(ApiError::bad_request(
            "projectPath is required for project skills folder",
        ));
    }
    let project = crate::api::projects::require_registered_project(state, raw.trim())?;

    let dir = match skill_service::ensure_project_root(&project.to_string_lossy()) {
        Some(dir) => dir,
        None => {
            return Err(ApiError::bad_request(
                "Could not create project skills folder",
            ))
        }
    };
    // The skills folder must stay inside the project.
    if !dir.starts_with(&project) {
        return Err(ApiError::bad_request(
            "Skills folder resolved outside the project",
        ));
    }
    Ok(SkillsFolder {
        path: dir.to_string_lossy().to_string(),
        target: "project".to_string(),
    })
}

/// Deletes a skill by name from the given target; returns whether anything was
/// deleted. Mirrors `DELETE /api/skills/:name`.
pub async fn delete_skill(
    state: &AppState,
    name: &str,
    target: Option<&str>,
    project_path: Option<&str>,
) -> ApiResult<bool> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::bad_request("Skill name is required"));
    }
    let target = match target {
        Some("project") => "project",
        Some("user") => "user",
        _ => {
            return Err(ApiError::bad_request(
                "target must be \"user\" or \"project\"",
            ))
        }
    };

    let project = if target == "project" {
        let raw = project_path.unwrap_or_default();
        if raw.trim().is_empty() {
            return Err(ApiError::bad_request(
                "projectPath is required for project skills",
            ));
        }
        Some(
            crate::api::projects::require_registered_project(state, raw.trim())?
                .to_string_lossy()
                .to_string(),
        )
    } else {
        None
    };

    skill_service::delete(target, &name, project.as_deref()).map_err(ApiError::bad_request)
}
