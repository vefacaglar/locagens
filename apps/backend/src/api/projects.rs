//! Projects facade: registration, workspace file listing, and git operations.

use crate::api::error::{ApiError, ApiResult};
use crate::db;
use crate::util::{canonical_project_path, join_inside};
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Input for `POST /api/projects` (create a registered project).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectInput {
    pub path: Option<String>,
    pub name: Option<String>,
}

/// Input for `POST /api/projects/git/commit`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitInput {
    pub path: Option<String>,
    pub message: Option<String>,
    pub action: Option<String>,
}

/// Response of `GET /api/projects/git/status` (`{isGit:false}` or
/// `{isGit:true,branch,hasChanges}`).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    pub is_git: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_changes: Option<bool>,
}

/// One changed file as reported by `GET /api/projects/git/diff-details`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct GitFileDiff {
    pub path: String,
    pub kind: String,
    pub old_text: String,
    pub new_text: String,
}

/// Canonicalizes `raw` and verifies the project is registered (legacy stored
/// spellings match too). Errors are BadRequest with the route messages;
/// mirrors the checks `GET /api/projects/*` performs.
pub fn require_registered_project(state: &AppState, raw: &str) -> ApiResult<PathBuf> {
    let canonical = canonical_project_path(raw).map_err(ApiError::bad_request)?;
    let canonical_str = canonical.to_string_lossy().to_string();
    if state.db.get_project(&canonical_str).is_some() {
        return Ok(canonical);
    }
    // Compatibility for pre-canonicalization stored spellings.
    let legacy_match = state
        .db
        .list_projects()
        .iter()
        .any(|project| canonical_project_path(&project.path).ok().as_ref() == Some(&canonical));
    if !legacy_match {
        return Err(ApiError::bad_request("Project is not registered in Locagens."));
    }
    Ok(canonical)
}

/// Lists every registered project. Mirrors `GET /api/projects`.
pub async fn list_projects(state: &AppState) -> ApiResult<Vec<db::Project>> {
    Ok(state.db.list_projects())
}

/// Registers a project folder (canonical path + derived or given name).
/// Mirrors `POST /api/projects`.
pub async fn create_project(state: &AppState, input: CreateProjectInput) -> ApiResult<db::Project> {
    let Some(raw_path) = input.path.as_deref().map(str::trim).filter(|p| !p.is_empty()) else {
        return Err(ApiError::bad_request("Missing required field: path"));
    };

    let resolved_path = canonical_project_path(raw_path).map_err(ApiError::bad_request)?;
    let resolved_path_str = resolved_path.to_string_lossy().to_string();
    let resolved_name = input
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            resolved_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Workspace".to_string())
        });

    Ok(state.db.create_project(&resolved_path_str, &resolved_name))
}

/// Removes a registered project by path (trimmed). Mirrors `DELETE /api/projects`.
pub async fn delete_project(state: &AppState, path: &str) -> ApiResult<()> {
    state.db.delete_project(path.trim());
    Ok(())
}

/// Runs `git` in `project_path` and returns stdout; stderr (or the spawn
/// failure) becomes the error message.
async fn git(project_path: &Path, args: &[&str]) -> Result<String, String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(project_path)
        .output()
        .await
        .map_err(|err| format!("Failed to run git: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("git exited with status {}", output.status)
        } else {
            stderr
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Fallback file walk used when git is unavailable.
fn walk_files(dir: &Path, base: &str, out: &mut Vec<String>, depth: usize) {
    if depth > 5 || out.len() > 500 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "node_modules" || name == "dist" || name == "build" {
            continue;
        }
        let rel = if base.is_empty() { name.clone() } else { format!("{base}/{name}") };
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_dir() {
            walk_files(&entry.path(), &rel, out, depth + 1);
        } else if file_type.is_file() {
            out.push(rel);
        }
    }
}

/// Lists workspace files (git ls-files with a directory-walk fallback),
/// filtered by `query`, truncated to 150. Mirrors `GET /api/projects/files`.
pub async fn list_files(state: &AppState, path: &str, query: Option<&str>) -> ApiResult<Vec<String>> {
    let canonical_path = require_registered_project(state, path)?;

    let mut file_list: Vec<String> = match git(&canonical_path, &["ls-files", "-co", "--exclude-standard"]).await {
        Ok(out) => out
            .split('\n')
            .map(|f| f.trim().to_string())
            .filter(|f| !f.is_empty())
            .collect(),
        Err(_) => {
            let mut walked = Vec::new();
            walk_files(&canonical_path, "", &mut walked, 0);
            walked
        }
    };

    let q = query.unwrap_or_default().trim().to_lowercase();
    if !q.is_empty() {
        file_list.retain(|f| f.to_lowercase().contains(&q));
    }
    file_list.truncate(150);
    Ok(file_list)
}

/// Reports whether the project is a git work tree, its branch, and whether it
/// has changes. Mirrors `GET /api/projects/git/status`.
pub async fn git_status(state: &AppState, path: &str) -> ApiResult<GitStatus> {
    let canonical_path = require_registered_project(state, path)?;

    if (git(&canonical_path, &["rev-parse", "--is-inside-work-tree"]).await).is_err() {
        return Ok(GitStatus {
            is_git: false,
            ..Default::default()
        });
    }
    let branch = git(&canonical_path, &["branch", "--show-current"])
        .await
        .unwrap_or_default();
    let status = git(&canonical_path, &["status", "--porcelain"])
        .await
        .unwrap_or_default();

    Ok(GitStatus {
        is_git: true,
        branch: Some(branch.trim().to_string()),
        has_changes: Some(!status.trim().is_empty()),
    })
}

#[derive(Clone)]
struct GitStatusEntry {
    xy: String,
    path: String,
    old_path: Option<String>,
}

/// Parses `git status --porcelain=v1 -z` output (NUL-separated records).
fn parse_porcelain_z(output: &str) -> Vec<GitStatusEntry> {
    let records: Vec<&str> = output.split('\0').collect();
    let mut entries = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        index += 1;
        if record.len() < 4 {
            continue;
        }
        let xy = record[0..2].to_string();
        let file_path = record[3..].to_string();
        if xy.contains('R') || xy.contains('C') {
            let old_path = records.get(index).map(|s| s.to_string()).unwrap_or_default();
            index += 1;
            entries.push(GitStatusEntry {
                xy,
                path: file_path,
                old_path: Some(old_path),
            });
        } else {
            entries.push(GitStatusEntry {
                xy,
                path: file_path,
                old_path: None,
            });
        }
    }
    entries
}

/// Per-file old/new text for every changed file in the work tree.
/// Mirrors `GET /api/projects/git/diff-details`.
pub async fn git_diff_details(state: &AppState, path: &str) -> ApiResult<Vec<GitFileDiff>> {
    let canonical_path = require_registered_project(state, path)?;

    let status_output = git(
        &canonical_path,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )
    .await
    .map_err(ApiError::internal)?;
    let entries = parse_porcelain_z(&status_output);

    let mut results: Vec<GitFileDiff> = Vec::new();
    for entry in entries {
        let mut kind = "edited";
        if entry.xy.starts_with('R') {
            kind = "moved";
        } else if entry.xy.contains('A') || entry.xy.contains('?') {
            kind = "created";
        } else if entry.xy.contains('D') {
            kind = "deleted";
        }

        let old_path = entry.old_path.clone().unwrap_or_else(|| entry.path.clone());
        let new_path = entry.path.clone();

        let mut old_text = String::new();
        if kind != "created" {
            // HEAD might not exist yet (e.g. initial commit).
            old_text = git(&canonical_path, &["show", &format!("HEAD:{old_path}")])
                .await
                .unwrap_or_default();
        }

        let mut new_text = String::new();
        if kind != "deleted" {
            if let Some(full_path) = join_inside(&canonical_path, &new_path) {
                new_text = std::fs::read_to_string(&full_path).unwrap_or_default();
            }
        }

        results.push(GitFileDiff {
            path: entry.path,
            kind: kind.to_string(),
            old_text,
            new_text,
        });
    }

    Ok(results)
}

/// Generates a conventional-commit style message from the run's diff.
/// Mirrors `POST /api/projects/git/generate-message` (the "Missing required
/// body parameter: runId" check stays in the HTTP adapter).
pub async fn git_generate_message(state: &AppState, run_id: &str) -> ApiResult<String> {
    let Some(run) = state.db.get_run(run_id) else {
        return Err(ApiError::not_found("Run not found"));
    };

    let raw_project = run
        .project_path
        .clone()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| state.default_project_path.to_string_lossy().to_string());
    let project_path = require_registered_project(state, &raw_project)?;

    let diff = git(&project_path, &["diff", "HEAD"]).await.unwrap_or_default();
    if diff.trim().is_empty() {
        let status = git(&project_path, &["status", "--porcelain"])
            .await
            .unwrap_or_default();
        if status.trim().is_empty() {
            return Ok("chore: update workspace".to_string());
        }
    }

    // Truncate very large diffs to avoid blowing the model context.
    const MAX_DIFF_LEN: usize = 12_000;
    let diff_truncated = if diff.chars().count() > MAX_DIFF_LEN {
        let head: String = diff.chars().take(MAX_DIFF_LEN).collect();
        format!("{head}\n... [diff truncated] ...")
    } else {
        diff
    };

    // Phase 1 has no provider integration yet, so produce a deterministic
    // conventional-commit style message from the diff shape. The LLM-backed
    // generator hooks in once provider support lands (apps/api parity).
    let changed_files = diff_truncated
        .lines()
        .filter(|line| line.starts_with("diff --git "))
        .count();
    let changed_files = if changed_files == 0 { 1 } else { changed_files };
    let message = format!(
        "chore: update workspace ({} file{} changed)",
        changed_files,
        if changed_files == 1 { "" } else { "s" }
    );

    Ok(message)
}

/// Stages everything, commits (hooks disabled), and optionally pushes.
/// Mirrors `POST /api/projects/git/commit`.
pub async fn git_commit(state: &AppState, input: GitCommitInput) -> ApiResult<()> {
    let Some(raw_path) = input.path.as_deref().map(str::trim).filter(|p| !p.is_empty()) else {
        return Err(ApiError::bad_request("Missing required body parameter: path"));
    };
    let action = input.action.as_deref().unwrap_or_default();
    if !["commit", "commit-push", "push"].contains(&action) {
        return Err(ApiError::bad_request(
            "Invalid action. Must be 'commit', 'commit-push', or 'push'",
        ));
    }
    let message = input.message.clone().unwrap_or_default();
    if (action == "commit" || action == "commit-push") && message.trim().is_empty() {
        return Err(ApiError::bad_request("Missing required parameter: message"));
    }

    let canonical_path = require_registered_project(state, raw_path)?;

    if action == "commit" || action == "commit-push" {
        if let Err(err) = git(&canonical_path, &["add", "--all", "--", "."]).await {
            return Err(ApiError::internal(err));
        }
        // Do not execute repository-controlled hooks from the backend API.
        if let Err(err) = git(
            &canonical_path,
            &["-c", "core.hooksPath=/dev/null", "commit", "-m", message.trim()],
        )
        .await
        {
            return Err(ApiError::internal(err));
        }
    }

    if action == "commit-push" || action == "push" {
        if let Err(err) = git(&canonical_path, &["push"]).await {
            return Err(ApiError::internal(err));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_z_with_renames() {
        let output = "R  a.txt\0b.txt\0M  c.txt\0?? d.txt\0";
        let entries = parse_porcelain_z(output);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].xy, "R ");
        assert_eq!(entries[0].path, "a.txt");
        assert_eq!(entries[0].old_path.as_deref(), Some("b.txt"));
        assert_eq!(entries[1].xy, "M ");
        assert_eq!(entries[1].path, "c.txt");
        assert_eq!(entries[2].xy, "??");
        assert_eq!(entries[2].path, "d.txt");
    }
}

