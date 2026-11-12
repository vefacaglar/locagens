//! Projects, workspace file listing, native folder picker, and git routes.

use crate::util::{canonical_project_path, json_error, join_inside};
use crate::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Deserialize, Default)]
pub struct ProjectPathQuery {
    pub path: Option<String>,
    pub query: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectInput {
    pub path: Option<String>,
    pub name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitInput {
    pub path: Option<String>,
    pub message: Option<String>,
    pub action: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateMessageInput {
    pub run_id: Option<String>,
}

/// Canonicalize + verify the project is registered (legacy spellings match too).
fn require_registered_project(state: &AppState, raw: &str) -> Result<PathBuf, String> {
    let canonical = canonical_project_path(raw)?;
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
        return Err("Project is not registered in Locagens.".to_string());
    }
    Ok(canonical)
}

/// Public wrapper other route modules use for registered-project validation.
pub fn require_registered_project_pub(state: &AppState, raw: &str) -> Result<PathBuf, String> {
    require_registered_project(state, raw)
}

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

pub async fn list_projects(State(state): State<Arc<AppState>>) -> Response {
    Json(state.db.list_projects()).into_response()
}

pub async fn create_project(
    State(state): State<Arc<AppState>>,
    body: Option<Json<CreateProjectInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    let Some(raw_path) = input.path.as_deref().map(str::trim).filter(|p| !p.is_empty()) else {
        return json_error(StatusCode::BAD_REQUEST, "Missing required field: path");
    };

    let resolved_path = match canonical_project_path(raw_path) {
        Ok(path) => path,
        Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
    };
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

    let project = state.db.create_project(&resolved_path_str, &resolved_name);
    Json(project).into_response()
}

pub async fn delete_project(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProjectPathQuery>,
) -> Response {
    let Some(raw_path) = query.path.filter(|p| !p.is_empty()) else {
        return json_error(StatusCode::BAD_REQUEST, "Missing required query parameter: path");
    };
    state.db.delete_project(raw_path.trim());
    Json(json!({ "success": true })).into_response()
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

pub async fn list_files(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProjectPathQuery>,
) -> Response {
    let Some(raw_path) = query.path.filter(|p| !p.is_empty()) else {
        return json_error(StatusCode::BAD_REQUEST, "Missing required query parameter: path");
    };

    let canonical_path = match require_registered_project(&state, &raw_path) {
        Ok(path) => path,
        Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
    };

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

    let q = query.query.unwrap_or_default().trim().to_lowercase();
    if !q.is_empty() {
        file_list.retain(|f| f.to_lowercase().contains(&q));
    }
    file_list.truncate(150);
    Json(json!({ "files": file_list })).into_response()
}

pub async fn select_dir() -> Response {
    #[cfg(target_os = "macos")]
    {
        let output = tokio::process::Command::new("osascript")
            .args(["-e", "POSIX path of (choose folder with prompt \"Select a project folder:\")"])
            .output()
            .await;
        match output {
            Ok(out) if out.status.success() => {
                let selected = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if selected.is_empty() {
                    return json_error(StatusCode::BAD_REQUEST, "No folder selected");
                }
                match canonical_project_path(&selected) {
                    Ok(path) => {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "Workspace".to_string());
                        Json(json!({ "path": path.to_string_lossy(), "name": name })).into_response()
                    }
                    Err(err) => json_error(StatusCode::BAD_REQUEST, &err),
                }
            }
            Ok(_) => json_error(StatusCode::BAD_REQUEST, "Selection cancelled by user."),
            Err(err) => json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to open folder picker: {err}"),
            ),
        }
    }
    #[cfg(windows)]
    {
        let script = "Add-Type -AssemblyName System.Windows.Forms; \
                      $f = New-Object System.Windows.Forms.FolderBrowserDialog; \
                      $f.Description = 'Select a project folder:'; \
                      $r = $f.ShowDialog((New-Object System.Windows.Forms.Form -Property @{TopMost = $true})); \
                      if ($r -eq [System.Windows.Forms.DialogResult]::OK) { $f.SelectedPath }";
        let output = tokio::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .output()
            .await;
        match output {
            Ok(out) if out.status.success() => {
                let selected = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if selected.is_empty() {
                    return json_error(StatusCode::BAD_REQUEST, "No folder selected");
                }
                match canonical_project_path(&selected) {
                    Ok(path) => {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "Workspace".to_string());
                        Json(json!({ "path": path.to_string_lossy(), "name": name })).into_response()
                    }
                    Err(err) => json_error(StatusCode::BAD_REQUEST, &err),
                }
            }
            Ok(_) => json_error(StatusCode::BAD_REQUEST, "Selection cancelled by user."),
            Err(err) => json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to open folder picker: {err}"),
            ),
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        json_error(
            StatusCode::BAD_REQUEST,
            "Automatic folder picking is only supported on macOS and Windows. Please input the path manually.",
        )
    }
}

pub async fn git_status(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProjectPathQuery>,
) -> Response {
    let Some(raw_path) = query.path.filter(|p| !p.is_empty()) else {
        return json_error(StatusCode::BAD_REQUEST, "Missing required query parameter: path");
    };
    let canonical_path = match require_registered_project(&state, &raw_path) {
        Ok(path) => path,
        Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
    };

    if (git(&canonical_path, &["rev-parse", "--is-inside-work-tree"]).await).is_err() {
        return Json(json!({ "isGit": false })).into_response();
    }
    let branch = git(&canonical_path, &["branch", "--show-current"])
        .await
        .unwrap_or_default();
    let status = git(&canonical_path, &["status", "--porcelain"])
        .await
        .unwrap_or_default();

    Json(json!({
        "isGit": true,
        "branch": branch.trim(),
        "hasChanges": !status.trim().is_empty()
    }))
    .into_response()
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

pub async fn git_diff_details(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProjectPathQuery>,
) -> Response {
    let Some(raw_path) = query.path.filter(|p| !p.is_empty()) else {
        return json_error(StatusCode::BAD_REQUEST, "Missing required query parameter: path");
    };
    let canonical_path = match require_registered_project(&state, &raw_path) {
        Ok(path) => path,
        Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
    };

    let status_output = match git(
        &canonical_path,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )
    .await
    {
        Ok(out) => out,
        Err(err) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &err),
    };
    let entries = parse_porcelain_z(&status_output);

    let mut results: Vec<Value> = Vec::new();
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

        results.push(json!({
            "path": entry.path,
            "kind": kind,
            "oldText": old_text,
            "newText": new_text
        }));
    }

    Json(json!({ "files": results })).into_response()
}

pub async fn git_generate_message(
    State(state): State<Arc<AppState>>,
    body: Option<Json<GenerateMessageInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    let Some(run_id) = input.run_id.filter(|id| !id.is_empty()) else {
        return json_error(StatusCode::BAD_REQUEST, "Missing required body parameter: runId");
    };
    let Some(run) = state.db.get_run(&run_id) else {
        return json_error(StatusCode::NOT_FOUND, "Run not found");
    };

    let raw_project = run
        .project_path
        .clone()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| state.default_project_path.to_string_lossy().to_string());
    let project_path = match require_registered_project(&state, &raw_project) {
        Ok(path) => path,
        Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
    };

    let diff = git(&project_path, &["diff", "HEAD"]).await.unwrap_or_default();
    if diff.trim().is_empty() {
        let status = git(&project_path, &["status", "--porcelain"])
            .await
            .unwrap_or_default();
        if status.trim().is_empty() {
            return Json(json!({ "message": "chore: update workspace" })).into_response();
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

    Json(json!({ "message": message })).into_response()
}

pub async fn git_commit(
    State(state): State<Arc<AppState>>,
    body: Option<Json<CommitInput>>,
) -> Response {
    let Some(Json(input)) = body else {
        return json_error(StatusCode::BAD_REQUEST, "Invalid JSON body.");
    };
    let Some(raw_path) = input.path.as_deref().map(str::trim).filter(|p| !p.is_empty()) else {
        return json_error(StatusCode::BAD_REQUEST, "Missing required body parameter: path");
    };
    let action = input.action.as_deref().unwrap_or_default();
    if !["commit", "commit-push", "push"].contains(&action) {
        return json_error(
            StatusCode::BAD_REQUEST,
            "Invalid action. Must be 'commit', 'commit-push', or 'push'",
        );
    }
    let message = input.message.clone().unwrap_or_default();
    if (action == "commit" || action == "commit-push") && message.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "Missing required parameter: message");
    }

    let canonical_path = match require_registered_project(&state, raw_path) {
        Ok(path) => path,
        Err(err) => return json_error(StatusCode::BAD_REQUEST, &err),
    };

    if action == "commit" || action == "commit-push" {
        if let Err(err) = git(&canonical_path, &["add", "--all", "--", "."]).await {
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, &err);
        }
        // Do not execute repository-controlled hooks from the backend API.
        if let Err(err) = git(
            &canonical_path,
            &["-c", "core.hooksPath=/dev/null", "commit", "-m", message.trim()],
        )
        .await
        {
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, &err);
        }
    }

    if action == "commit-push" || action == "push" {
        if let Err(err) = git(&canonical_path, &["push"]).await {
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, &err);
        }
    }

    Json(json!({ "success": true })).into_response()
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
