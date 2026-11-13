//! Projects facade: registration, workspace file listing, and git operations.

use crate::api::error::{ApiError, ApiResult};
use crate::db;
use crate::util::{canonical_project_path, join_inside, now_iso};
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

/// Pre-turn git baseline: HEAD sha, optional stash of the dirty state and the
/// untracked files present before the turn ran.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GitSnapshot {
    pub id: String,
    pub run_id: String,
    pub head_sha: String,
    pub stash_sha: Option<String>,
    pub untracked: Vec<String>,
    pub created_at: String,
}

/// One entry of `git worktree list --porcelain`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct GitWorktree {
    pub path: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub is_main: bool,
}

/// How many snapshots `snapshots.json` retains.
const MAX_SNAPSHOTS: usize = 20;

/// `<project>/.locagens/snapshots.json` — the git-snapshot store.
fn snapshot_store_path(project_path: &Path) -> PathBuf {
    project_path.join(".locagens").join("snapshots.json")
}

/// Reads the snapshot list (tolerates a missing/corrupt file).
fn read_snapshots(store: &Path) -> Vec<GitSnapshot> {
    std::fs::read_to_string(store)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Persists the snapshot list, creating `.locagens/` and keeping only the
/// last `MAX_SNAPSHOTS` entries.
fn persist_snapshots(project_path: &Path, snapshots: &[GitSnapshot]) -> Result<(), String> {
    let store = snapshot_store_path(project_path);
    if let Some(parent) = store.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("Failed to create .locagens: {err}"))?;
    }
    let keep_from = snapshots.len().saturating_sub(MAX_SNAPSHOTS);
    let json = serde_json::to_string_pretty(&snapshots[keep_from..])
        .map_err(|err| format!("Failed to serialize snapshots: {err}"))?;
    std::fs::write(&store, json).map_err(|err| format!("Failed to write snapshots: {err}"))
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

    let q = query.unwrap_or_default().trim().to_string();
    if !q.is_empty() {
        // Fuzzy ranking: score every file, best match first; ties break by
        // shorter path, then lexicographically.
        let mut scored: Vec<(i64, String)> = file_list
            .iter()
            .filter_map(|file| fuzzy_file_score(file, &q).map(|score| (score, file.clone())))
            .collect();
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.len().cmp(&b.1.len()))
                .then_with(|| a.1.cmp(&b.1))
        });
        file_list = scored.into_iter().map(|(_, file)| file).collect();
    }
    file_list.truncate(150);
    Ok(file_list)
}

/// Fuzzy subsequence score of `query` against a workspace-relative `path`.
///
/// Rewards (in order of impact):
/// * basename exact match (`+100`), basename prefix (`+40`), exact substring
///   (`+50`), any match inside the basename (`+5` per char),
/// * contiguous character runs (`+15` per char after the first),
/// * word/path boundaries — `/` `-` `_` `.` or a camelCase transition
///   (`+12`),
/// * a slight penalty for longer paths so shallow files win.
///
/// Returns `None` when the query is not a (case-insensitive) subsequence of
/// the path.
pub fn fuzzy_file_score(path: &str, query: &str) -> Option<i64> {
    let query_trimmed = query.trim();
    if query_trimmed.is_empty() {
        return Some(0);
    }
    let query_lower = query_trimmed.to_lowercase();
    let path_lower = path.to_lowercase();
    let query_chars: Vec<char> = query_lower.chars().collect();
    let path_chars: Vec<char> = path_lower.chars().collect();

    // Must match as a subsequence first.
    let mut matched: Vec<usize> = Vec::with_capacity(query_chars.len());
    let mut query_index = 0usize;
    for (index, ch) in path_chars.iter().enumerate() {
        if query_index < query_chars.len() && *ch == query_chars[query_index] {
            matched.push(index);
            query_index += 1;
        }
    }
    if query_index < query_chars.len() {
        return None;
    }

    let basename_start = path_chars
        .iter()
        .rposition(|&c| c == '/')
        .map(|pos| pos + 1)
        .unwrap_or(0);
    let basename: String = path_chars[basename_start..].iter().collect();

    let mut score: i64 = 0;
    if basename == query_lower {
        score += 100;
    } else if basename.starts_with(&query_lower) {
        score += 40;
    } else if path_lower.contains(&query_lower) {
        score += 50;
    }

    let mut previous_match: Option<usize> = None;
    for (ordinal, &index) in matched.iter().enumerate() {
        if ordinal > 0 && previous_match == Some(index.wrapping_sub(1)) {
            // Contiguous run in the path.
            score += 15;
        } else if is_fuzzy_boundary(&path_chars, index) {
            score += 12;
        }
        if index >= basename_start {
            score += 5;
        }
        previous_match = Some(index);
    }

    // Shorter paths win over deep nested ones at equal quality.
    score -= path.len() as i64;
    Some(score)
}

/// True when `index` sits at a word/path boundary: start of string, right
/// after `/`, `-`, `_`, `.`, or a lower→upper camelCase transition.
fn is_fuzzy_boundary(path_chars: &[char], index: usize) -> bool {
    if index == 0 {
        return true;
    }
    let current = path_chars[index];
    let previous = path_chars[index - 1];
    matches!(previous, '/' | '-' | '_' | '.')
        || (previous.is_lowercase() && current.is_uppercase())
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
/// Stages files (`git add -- <files>`); the whole worktree when `files` is
/// empty. Mirrors what the desktop Git tab needs on top of the legacy HTTP
/// commit action (which stages everything implicitly).
pub async fn git_stage(state: &AppState, path: &str, files: &[String]) -> ApiResult<()> {
    let canonical_path = require_registered_project(state, path)?;
    let mut args: Vec<&str> = Vec::new();
    if files.is_empty() {
        args.extend(["add", "--all", "--", "."]);
    } else {
        args.push("add");
        args.push("--");
    }
    if let Err(err) = git(&canonical_path, &args).await {
        return Err(ApiError::internal(err));
    }
    for file in files {
        if let Err(err) = git(&canonical_path, &["add", "--", file]).await {
            return Err(ApiError::internal(err));
        }
    }
    Ok(())
}

/// Unstages files, keeping their worktree content
/// (`git reset -q HEAD -- <files>`); resets the whole index when `files` is
/// empty.
pub async fn git_unstage(state: &AppState, path: &str, files: &[String]) -> ApiResult<()> {
    let canonical_path = require_registered_project(state, path)?;
    if files.is_empty() {
        if let Err(err) = git(&canonical_path, &["reset", "-q", "HEAD"]).await {
            return Err(ApiError::internal(err));
        }
        return Ok(());
    }
    for file in files {
        if let Err(err) = git(&canonical_path, &["reset", "-q", "HEAD", "--", file]).await {
            return Err(ApiError::internal(err));
        }
    }
    Ok(())
}

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

// ----------------------------------------------------------------------
// Git baseline snapshots & turn undo
// ----------------------------------------------------------------------

/// Captures a pre-turn git baseline for `project_path` (best-effort; a
/// non-git repo or a repo without commits yields `Ok(None)`).
///
/// The snapshot records HEAD, the currently untracked files, and — when the
/// working tree is dirty — a stash commit of the pre-turn changes so
/// `undo_last_turn_for_path` can restore exactly that state.
pub async fn create_git_snapshot_for_path(
    project_path: &Path,
    run_id: &str,
) -> Result<Option<GitSnapshot>, String> {
    if !project_path.join(".git").exists()
        || git(project_path, &["rev-parse", "--is-inside-work-tree"])
            .await
            .is_err()
    {
        return Ok(None);
    }
    // Repos without any commit have no HEAD to restore to.
    let head_sha = match git(project_path, &["rev-parse", "HEAD"]).await {
        Ok(sha) => sha.trim().to_string(),
        Err(_) => return Ok(None),
    };
    if head_sha.is_empty() {
        return Ok(None);
    }

    let untracked: Vec<String> = git(project_path, &["ls-files", "--others", "--exclude-standard"])
        .await
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !is_locagens_path(line))
        .map(str::to_string)
        .collect();

    // Stash the dirty pre-turn state. `git stash create` does NOT include
    // untracked files (it silently treats `-u/--include-untracked` as the
    // stash message), so push a real stash entry, read its sha, restore the
    // worktree and drop the entry again: the (now dangling) stash commit
    // keeps the full pre-turn state — tracked changes, index and untracked
    // files — for `git stash apply <sha>` during undo.
    let stash_before = git(project_path, &["rev-parse", "-q", "--verify", "stash@{0}"])
        .await
        .ok()
        .map(|out| out.trim().to_string());
    let stash_sha = match git(
        project_path,
        &["stash", "push", "-u", "-m", "locagens pre-turn snapshot"],
    )
    .await
    {
        Ok(output) if !output.contains("No local changes") => {
            let stash_top = git(project_path, &["rev-parse", "-q", "--verify", "stash@{0}"])
                .await
                .ok()
                .map(|out| out.trim().to_string())
                .filter(|sha| !sha.is_empty());
            match stash_top {
                Some(sha) if Some(&sha) != stash_before.as_ref() => {
                    // Put the worktree back to the pre-turn state, then drop
                    // the list entry (the sha stays valid for the undo).
                    let applied = git(project_path, &["stash", "apply", "--index", "stash@{0}"])
                        .await
                        .is_ok()
                        || git(project_path, &["stash", "apply", "stash@{0}"])
                            .await
                            .is_ok();
                    if applied {
                        let _ = git(project_path, &["stash", "drop", "stash@{0}"]).await;
                    }
                    Some(sha)
                }
                _ => None,
            }
        }
        _ => None,
    };

    let snapshot = GitSnapshot {
        id: format!("snap-{}", uuid::Uuid::new_v4()),
        run_id: run_id.to_string(),
        head_sha: head_sha.clone(),
        stash_sha,
        untracked,
        created_at: now_iso(),
    };

    let mut snapshots = read_snapshots(&snapshot_store_path(project_path));
    snapshots.push(snapshot.clone());
    persist_snapshots(project_path, &snapshots)?;
    Ok(Some(snapshot))
}

/// True for paths inside the managed `.locagens` folder (never touched by
/// snapshot/undo logic).
fn is_locagens_path(relative: &str) -> bool {
    relative == ".locagens"
        || relative.starts_with(".locagens/")
        || relative.starts_with(".locagens\\")
}

/// `create_git_snapshot_for_path` for a registered project.
pub async fn create_git_snapshot(
    state: &AppState,
    path: &str,
    run_id: &str,
) -> ApiResult<Option<GitSnapshot>> {
    let canonical = require_registered_project(state, path)?;
    create_git_snapshot_for_path(&canonical, run_id)
        .await
        .map_err(ApiError::internal)
}

/// Undoes the last turn: restores tracked files to the snapshot's HEAD,
/// removes files created during the turn, and re-applies the stashed
/// pre-turn changes. The used snapshot is consumed (removed from the store).
pub async fn undo_last_turn_for_path(
    project_path: &Path,
    run_id: Option<&str>,
) -> Result<GitSnapshot, String> {
    let mut snapshots = read_snapshots(&snapshot_store_path(project_path));
    let index = match run_id {
        Some(run_id) => snapshots.iter().rposition(|snap| snap.run_id == run_id),
        None => snapshots.len().checked_sub(1),
    }
    .ok_or_else(|| "No git snapshot available to undo".to_string())?;
    let snapshot = snapshots.remove(index);
    persist_snapshots(project_path, &snapshots)?;

    // 1. Tracked files back to the pre-turn HEAD.
    git(project_path, &["reset", "--hard", &snapshot.head_sha])
        .await
        .map_err(|err| format!("git reset failed: {err}"))?;

    // 2. Delete files the turn created (untracked now, unknown before it).
    //    With a stash present, the pre-existing untracked files are removed
    //    too: `git stash apply` restores them from the stash's untracked
    //    tree, and keeping them on disk would make the apply conflict.
    if let Ok(output) = git(project_path, &["ls-files", "--others", "--exclude-standard"]).await {
        let known: std::collections::HashSet<&str> = snapshot
            .untracked
            .iter()
            .map(String::as_str)
            .collect();
        for line in output.lines() {
            let relative = line.trim();
            if relative.is_empty() || is_locagens_path(relative) {
                continue;
            }
            let known_file = known.contains(relative);
            let delete = if snapshot.stash_sha.is_some() {
                // Everything untracked will be re-created by the stash apply.
                true
            } else {
                // Without a stash only turn-created files are removed.
                !known_file
            };
            if !delete {
                continue;
            }
            if let Some(full_path) = join_inside(project_path, relative) {
                // Canonical containment check before deleting.
                let canonical_project = std::fs::canonicalize(project_path)
                    .map(crate::util::normalize_canonical)
                    .unwrap_or_else(|_| project_path.to_path_buf());
                let canonical_full = std::fs::canonicalize(&full_path)
                    .map(crate::util::normalize_canonical)
                    .unwrap_or(full_path);
                if canonical_full.starts_with(&canonical_project) {
                    if canonical_full.is_dir() {
                        let _ = std::fs::remove_dir_all(&canonical_full);
                    } else {
                        let _ = std::fs::remove_file(&canonical_full);
                        // Drop now-empty parent dirs (best effort).
                        let mut parent = canonical_full.parent().map(Path::to_path_buf);
                        while let Some(dir) = parent {
                            if dir == canonical_project
                                || !dir.starts_with(&canonical_project)
                                || dir.ends_with(".git")
                                || dir.ends_with(".locagens")
                            {
                                break;
                            }
                            if std::fs::remove_dir(&dir).is_ok() {
                                parent = dir.parent().map(Path::to_path_buf);
                            } else {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    // 3. Re-apply the pre-turn dirty state.
    if let Some(sha) = snapshot.stash_sha.as_deref().filter(|sha| !sha.is_empty()) {
        if git(project_path, &["stash", "apply", "--index", sha])
            .await
            .is_err()
        {
            let _ = git(project_path, &["stash", "apply", sha]).await;
        }
    }

    Ok(snapshot)
}

/// `undo_last_turn_for_path` for a registered project.
pub async fn undo_last_turn(
    state: &AppState,
    path: &str,
    run_id: Option<&str>,
) -> ApiResult<GitSnapshot> {
    let canonical = require_registered_project(state, path)?;
    undo_last_turn_for_path(&canonical, run_id)
        .await
        .map_err(ApiError::internal)
}

// ----------------------------------------------------------------------
// Git worktrees
// ----------------------------------------------------------------------

/// Parses `git worktree list --porcelain` output; the first entry (or the
/// entry matching the project path) is the main worktree.
fn parse_worktrees(output: &str, project_path: &Path) -> Vec<GitWorktree> {
    let mut worktrees: Vec<GitWorktree> = Vec::new();
    let mut current: Option<GitWorktree> = None;
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(done) = current.take() {
                worktrees.push(done);
            }
            current = Some(GitWorktree {
                path: rest.trim().to_string(),
                ..Default::default()
            });
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            if let Some(worktree) = current.as_mut() {
                worktree.head = Some(rest.trim().to_string());
            }
        } else if let Some(rest) = line.strip_prefix("branch ") {
            if let Some(worktree) = current.as_mut() {
                worktree.branch = Some(
                    rest.trim()
                        .trim_start_matches("refs/heads/")
                        .to_string(),
                );
            }
        }
    }
    if let Some(done) = current.take() {
        worktrees.push(done);
    }
    let project_str = project_path.to_string_lossy().to_string();
    for (index, worktree) in worktrees.iter_mut().enumerate() {
        worktree.is_main = index == 0 || worktree.path == project_str;
    }
    worktrees
}

/// Lists the repository's worktrees (`git worktree list --porcelain`).
pub async fn list_worktrees(state: &AppState, path: &str) -> ApiResult<Vec<GitWorktree>> {
    let canonical = require_registered_project(state, path)?;
    let output = git(&canonical, &["worktree", "list", "--porcelain"])
        .await
        .map_err(ApiError::internal)?;
    Ok(parse_worktrees(&output, &canonical))
}

/// Creates (or checks out) `branch` in a dedicated worktree directory next to
/// the project: `<parent>/.locagens-worktrees/<project>/<branch-slug>`.
/// With `register`, the worktree is also registered as a selectable project.
pub async fn create_worktree(
    state: &AppState,
    path: &str,
    branch: &str,
    register: bool,
) -> ApiResult<GitWorktree> {
    let canonical = require_registered_project(state, path)?;
    let branch = branch.trim();
    let branch_valid = !branch.is_empty()
        && !branch.starts_with('-')
        && !branch.contains("..")
        && branch
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.'));
    if !branch_valid {
        return Err(ApiError::bad_request(
            "Branch name must only contain letters, digits, '-', '_', '/' or '.'.",
        ));
    }
    let project_name = canonical
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".to_string());
    let slug: String = branch
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let base = canonical
        .parent()
        .map(|parent| parent.join(".locagens-worktrees").join(&project_name))
        .unwrap_or_else(|| canonical.join(".locagens").join("worktrees"));
    std::fs::create_dir_all(&base).map_err(|err| ApiError::internal(format!("Failed to create worktree base: {err}")))?;
    let worktree_dir = base.join(&slug);
    if worktree_dir.exists() {
        return Err(ApiError::bad_request(format!(
            "Worktree path already exists: {}",
            worktree_dir.display()
        )));
    }
    let worktree_str = worktree_dir.to_string_lossy().to_string();

    if git(
        &canonical,
        &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
    )
    .await
    .is_ok()
    {
        git(&canonical, &["worktree", "add", &worktree_str, branch])
            .await
            .map_err(ApiError::internal)?;
    } else {
        git(&canonical, &["worktree", "add", "-b", branch, &worktree_str])
            .await
            .map_err(ApiError::internal)?;
    }

    if register {
        let name = format!("{project_name} ({branch})");
        // Registration is best-effort; a conflict with an existing project
        // entry must not fail the worktree creation.
        let _ = create_project(
            state,
            CreateProjectInput {
                path: Some(worktree_str.clone()),
                name: Some(name),
            },
        )
        .await;
    }

    let head = git(Path::new(&worktree_str), &["rev-parse", "HEAD"])
        .await
        .ok()
        .map(|sha| sha.trim().to_string());
    Ok(GitWorktree {
        path: worktree_str,
        head,
        branch: Some(branch.to_string()),
        is_main: false,
    })
}

/// Removes a worktree (`git worktree remove [--force]`); refuses to remove
/// the main project folder. Unregisters the worktree's project registration
/// when one exists.
pub async fn remove_worktree(
    state: &AppState,
    path: &str,
    worktree_path: &str,
    force: bool,
) -> ApiResult<()> {
    let canonical = require_registered_project(state, path)?;
    let target = worktree_path.trim();
    if target.is_empty() {
        return Err(ApiError::bad_request("Worktree path is required."));
    }
    let same_path = Path::new(target) == canonical
        || std::fs::canonicalize(target)
            .map(|resolved| resolved == canonical)
            .unwrap_or(false);
    if same_path {
        return Err(ApiError::bad_request(
            "Refusing to remove the main project worktree.",
        ));
    }
    let mut args: Vec<&str> = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(target);
    git(&canonical, &args).await.map_err(ApiError::internal)?;

    // Drop a matching project registration (best effort).
    if let Ok(resolved) = canonical_project_path(target) {
        state.db.delete_project(&resolved.to_string_lossy());
    }
    Ok(())
}

// ----------------------------------------------------------------------
// Working-tree diff
// ----------------------------------------------------------------------

/// The project's `git diff` (`--cached` when `staged`); empty string when the
/// tree is clean.
pub async fn git_diff(state: &AppState, path: &str, staged: bool) -> ApiResult<String> {
    let canonical = require_registered_project(state, path)?;
    let args: &[&str] = if staged { &["diff", "--cached"] } else { &["diff"] };
    git(&canonical, args).await.map_err(ApiError::internal)
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

    #[test]
    fn fuzzy_file_search_ranks_exact_and_boundary_matches_first() {
        assert_eq!(fuzzy_file_score("anything", ""), Some(0));
        assert_eq!(fuzzy_file_score("src/app.ts", "zzz"), None);

        // Basename exact beats a mid-word subsequence match, even though the
        // mid-word path also contains the query as a substring.
        let exact = fuzzy_file_score("src/main.rs", "main").unwrap();
        let scattered = fuzzy_file_score("src/domain/manager.ts", "main").unwrap();
        assert!(exact > scattered, "{exact} must beat {scattered}");

        // Path boundaries (`/`, camelCase) beat mid-word matches.
        let boundary = fuzzy_file_score("src/FileView.tsx", "fv").unwrap();
        let midword = fuzzy_file_score("src/overflowview.tsx", "fv").unwrap();
        assert!(boundary > midword, "{boundary} must beat {midword}");

        // Contiguous runs beat scattered matches.
        let contiguous = fuzzy_file_score("src/fu.rs", "fu").unwrap();
        let scattered_pair = fuzzy_file_score("src/f_u.rs", "fu").unwrap();
        assert!(contiguous > scattered_pair);

        // `list_files` ordering follows the score (exact basename first).
        let files = vec![
            "docs/domain/manual.md".to_string(),
            "src/main.rs".to_string(),
            "src/domain/manager.ts".to_string(),
        ];
        let mut scored: Vec<(i64, String)> = files
            .iter()
            .filter_map(|f| fuzzy_file_score(f, "main").map(|s| (s, f.clone())))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        assert_eq!(scored[0].1, "src/main.rs");
    }

    /// Runs git in `dir`, asserting success (test-local, synchronous).
    fn run_git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .expect("git must be available");
        assert!(status.success(), "git {args:?} failed");
    }

    #[tokio::test]
    async fn git_snapshot_and_undo_restores_modified_and_untracked_files() {
        let root = std::env::temp_dir().join(format!(
            "locagens-snap-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();

        // Initial commit with one tracked file.
        std::fs::write(root.join("tracked.txt"), "base\n").unwrap();
        run_git(&root, &["init", "-q"]);
        run_git(&root, &["add", "."]);
        run_git(&root, &["commit", "-q", "-m", "init"]);

        // Pre-turn dirty state: modify the tracked file + one untracked file.
        std::fs::write(root.join("tracked.txt"), "pre-turn\n").unwrap();
        std::fs::write(root.join("notes.txt"), "keep me\n").unwrap();
        let snapshot = create_git_snapshot_for_path(&root, "run-1")
            .await
            .unwrap()
            .expect("snapshot for a committed git repo");
        assert!(!snapshot.head_sha.is_empty());
        assert!(snapshot.stash_sha.is_some(), "dirty tree must be stashed");
        assert!(snapshot.untracked.iter().any(|f| f == "notes.txt"));

        // The turn modifies the tracked file again and creates a new file.
        std::fs::write(root.join("tracked.txt"), "post-turn\n").unwrap();
        std::fs::write(root.join("generated.txt"), "turn artifact\n").unwrap();

        let restored = undo_last_turn_for_path(&root, Some("run-1"))
            .await
            .unwrap();
        assert_eq!(restored.id, snapshot.id);

        // Pre-turn tracked content is back…
        assert_eq!(
            std::fs::read_to_string(root.join("tracked.txt")).unwrap(),
            "pre-turn\n"
        );
        // …the pre-turn untracked file survives…
        assert_eq!(
            std::fs::read_to_string(root.join("notes.txt")).unwrap(),
            "keep me\n"
        );
        // …the turn artifact is gone…
        assert!(!root.join("generated.txt").exists());
        // …and `.locagens/` is never touched.
        assert!(root.join(".locagens").exists());

        // Undo without a matching snapshot fails cleanly.
        assert!(undo_last_turn_for_path(&root, Some("missing")).await.is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn parses_worktree_porcelain() {
        let output = "worktree /repo/main\nHEAD abc\nbranch refs/heads/main\n\nworktree /repo/wt\nHEAD def\nbranch refs/heads/feature\n";
        let worktrees = parse_worktrees(output, Path::new("/repo/main"));
        assert_eq!(worktrees.len(), 2);
        assert!(worktrees[0].is_main);
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
        assert!(!worktrees[1].is_main);
        assert_eq!(worktrees[1].branch.as_deref(), Some("feature"));
    }
}

