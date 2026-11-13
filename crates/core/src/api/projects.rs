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

/// Pre-turn git baseline: HEAD sha, a git tree holding the exact bytes of
/// every non-ignored worktree file, a copy of the index, and the untracked
/// files present before the turn ran.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GitSnapshot {
    pub id: String,
    pub run_id: String,
    pub head_sha: String,
    /// Tree of the pre-turn worktree (blobs hashed with `--no-filters`).
    /// `None` for snapshots written by older versions (not undoable).
    #[serde(default)]
    pub tree_sha: Option<String>,
    /// Legacy stash-based snapshots only; no longer written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stash_sha: Option<String>,
    #[serde(default)]
    pub untracked: Vec<String>,
    pub created_at: String,
    /// Worktree tree right after the turn ended (same hashing as
    /// `tree_sha`). Undo restores only the paths whose bytes differ between
    /// the two trees, and skips those changed again since (QA-030).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_tree_sha: Option<String>,
    /// HEAD right after the turn ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_head_sha: Option<String>,
    /// Set on the snapshot returned by an undo: the paths written back or
    /// deleted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restored: Vec<String>,
    /// Set on the snapshot returned by an undo: paths the turn changed that
    /// were left alone because they changed again after the turn (user edits
    /// or a later turn).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
}

/// `run_id` suffix of the backup snapshot an undo takes of the state it
/// replaces; undoing that snapshot redoes the undone turn.
pub const UNDO_BACKUP_SUFFIX: &str = "#undo";

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

/// Where runtime snapshot state lives: the Locagens home (`~/.locagens`).
/// Unit tests use a temp directory so they never touch the real home.
fn snapshot_home() -> PathBuf {
    if cfg!(test) {
        std::env::temp_dir().join("locagens-test-home")
    } else {
        crate::paths::locagens_home()
    }
}

/// `~/.locagens/projects/<sanitized-project-path>/snapshots/` — the
/// git-snapshot store and the index copies. Runtime state never lives in the
/// user's project (QA-041).
fn snapshot_dir(project_path: &Path) -> PathBuf {
    snapshot_home()
        .join("projects")
        .join(db::sanitize_project_path(&project_path.to_string_lossy()))
        .join("snapshots")
}

/// `<snapshot_dir>/snapshots.json` — the git-snapshot store.
fn snapshot_store_path(project_path: &Path) -> PathBuf {
    snapshot_dir(project_path).join("snapshots.json")
}

/// Moves the legacy in-project store (`<project>/.locagens/snapshots.json`
/// plus `snapshots/*.index`) under the Locagens home, once. The old files are
/// removed; the rest of `.locagens/` (rules) is left alone.
fn migrate_legacy_snapshots(project_path: &Path) {
    let old_dir = project_path.join(".locagens");
    let old_store = old_dir.join("snapshots.json");
    if !old_store.is_file() {
        return;
    }
    let new_store = snapshot_store_path(project_path);
    if !new_store.exists() {
        let copied = (|| -> std::io::Result<()> {
            let new_dir = snapshot_dir(project_path);
            std::fs::create_dir_all(&new_dir)?;
            if let Ok(entries) = std::fs::read_dir(old_dir.join("snapshots")) {
                for entry in entries.flatten() {
                    let _ = std::fs::copy(entry.path(), new_dir.join(entry.file_name()));
                }
            }
            std::fs::copy(&old_store, &new_store).map(|_| ())
        })();
        if let Err(err) = copied {
            tracing::warn!("Could not migrate {}: {err}", old_store.display());
            return;
        }
    }
    let _ = std::fs::remove_file(&old_store);
    let _ = std::fs::remove_dir_all(old_dir.join("snapshots"));
    let _ = std::fs::remove_dir(&old_dir); // only succeeds when empty
}

/// The snapshot list of a project (migrating the legacy store first).
fn load_snapshots(project_path: &Path) -> Vec<GitSnapshot> {
    migrate_legacy_snapshots(project_path);
    read_snapshots(&snapshot_store_path(project_path))
}

/// Reads the snapshot list (tolerates a missing/corrupt file).
fn read_snapshots(store: &Path) -> Vec<GitSnapshot> {
    std::fs::read_to_string(store)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Persists the snapshot list, creating the store directory and keeping only
/// the last `MAX_SNAPSHOTS` entries.
fn persist_snapshots(project_path: &Path, snapshots: &[GitSnapshot]) -> Result<(), String> {
    let store = snapshot_store_path(project_path);
    if let Some(parent) = store.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("Failed to create the snapshot store: {err}"))?;
    }
    let keep_from = snapshots.len().saturating_sub(MAX_SNAPSHOTS);
    prune_index_copies(project_path, &snapshots[keep_from..]);
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

// ----------------------------------------------------------------------
// Project trust (see `services::project_trust`)
// ----------------------------------------------------------------------

/// Trust state of a project folder.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTrust {
    pub trusted: bool,
    /// Project-controlled executable config (`.locagens/hooks.json`,
    /// `mcp.json`, plugins, rules, agent roles) ignored because the folder
    /// is not trusted. Empty when trusted or when there is none.
    pub ignored_files: Vec<String>,
}

/// Whether the folder's project config (hooks, MCP servers, plugins,
/// rules, agent roles) is loaded. Unknown / inaccessible folders are not.
pub fn is_project_trusted(path: &str) -> bool {
    crate::services::project_trust::is_trusted_str(Some(path))
}

/// The folder's trust state plus what it would load once trusted.
pub async fn project_trust(path: &str) -> ApiResult<ProjectTrust> {
    let canonical = canonical_project_path(path).map_err(ApiError::bad_request)?;
    let ignored = crate::services::project_trust::ignored_config_files(&canonical);
    Ok(ProjectTrust {
        trusted: crate::services::project_trust::is_trusted(&canonical),
        ignored_files: ignored
            .iter()
            .map(|file| file.strip_prefix(&canonical).unwrap_or(file).display().to_string())
            .collect(),
    })
}

/// Trusts (or explicitly distrusts) a project folder; persisted as
/// `[projects."<path>"] trust_level` in `~/.locagens/config.toml`. The
/// filesystem root and the home directory cannot be trusted as a whole.
pub async fn trust_project(path: &str, trusted: bool) -> ApiResult<ProjectTrust> {
    let canonical = canonical_project_path(path).map_err(ApiError::bad_request)?;
    crate::services::project_trust::set_trusted(&canonical, trusted).map_err(ApiError::internal)?;
    project_trust(&canonical.to_string_lossy()).await
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
/// Nothing in the worktree or the user's index is touched: every non-ignored
/// file is hashed into the object store as-is (`--no-filters`, so autocrlf /
/// `.gitattributes` never rewrite line endings), a tree is written from a
/// throwaway index, and the real index is copied next to `snapshots.json`
/// (both under `~/.locagens/projects/<project>/snapshots/`).
/// `undo_last_turn_for_path` restores exactly those bytes.
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

    let id = format!("snap-{}", uuid::Uuid::new_v4());
    let tree_sha = write_worktree_tree(project_path, &id).await?;
    save_index_copy(project_path, &id).await;

    let snapshot = GitSnapshot {
        id,
        run_id: run_id.to_string(),
        head_sha,
        tree_sha: Some(tree_sha),
        stash_sha: None,
        untracked,
        created_at: now_iso(),
        after_tree_sha: None,
        after_head_sha: None,
        restored: Vec::new(),
        skipped: Vec::new(),
    };

    let mut snapshots = load_snapshots(project_path);
    snapshots.push(snapshot.clone());
    persist_snapshots(project_path, &snapshots)?;
    Ok(Some(snapshot))
}

/// Runs `git` with optional extra env and stdin, returning raw stdout bytes.
async fn git_bytes(
    project_path: &Path,
    args: &[&str],
    env: &[(&str, &Path)],
    stdin: Option<Vec<u8>>,
) -> Result<Vec<u8>, String> {
    use tokio::io::AsyncWriteExt;
    let mut command = tokio::process::Command::new("git");
    command
        .args(args)
        .current_dir(project_path)
        .stdin(if stdin.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command.spawn().map_err(|err| format!("Failed to run git: {err}"))?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(&input).await.map_err(|err| format!("git stdin: {err}"))?;
        drop(pipe);
    }
    let output = child.wait_with_output().await.map_err(|err| format!("git: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("git {}: {stderr}", args.first().unwrap_or(&"")));
    }
    Ok(output.stdout)
}

/// Non-ignored worktree files (tracked + untracked), excluding `.locagens/`.
/// Paths with a newline cannot go through `--stdin-paths` and are skipped.
async fn worktree_files(project_path: &Path) -> Result<Vec<String>, String> {
    let raw = git_bytes(project_path, &["ls-files", "-z", "-c", "-o", "--exclude-standard"], &[], None).await?;
    let mut files: Vec<String> = raw
        .split(|byte| *byte == 0)
        .filter_map(|path| std::str::from_utf8(path).ok())
        .filter(|path| !path.is_empty() && !path.contains('\n') && !is_locagens_path(path))
        // Deleted-but-tracked files are listed by `-c`; only existing files count.
        .filter(|path| project_path.join(path).is_file())
        .map(str::to_string)
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

/// Hashes `files` exactly as they are on disk; `write` stores the blobs.
async fn hash_files(project_path: &Path, files: &[String], write: bool) -> Result<Vec<String>, String> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec!["hash-object", "--no-filters", "--stdin-paths"];
    if write {
        args.insert(1, "-w");
    }
    let input = files.join("\n").into_bytes();
    let output = git_bytes(project_path, &args, &[], Some(input)).await?;
    let shas: Vec<String> = String::from_utf8_lossy(&output).lines().map(str::to_string).collect();
    if shas.len() != files.len() {
        return Err("git hash-object returned an unexpected number of hashes".to_string());
    }
    Ok(shas)
}

/// Writes a tree of the current worktree bytes using a throwaway index.
async fn write_worktree_tree(project_path: &Path, id: &str) -> Result<String, String> {
    let files = worktree_files(project_path).await?;
    let shas = hash_files(project_path, &files, true).await?;
    let modes = executable_files(project_path).await;
    let mut index_info = String::new();
    for (file, sha) in files.iter().zip(&shas) {
        let mode = if modes.contains(file) { "100755" } else { "100644" };
        index_info.push_str(&format!("{mode} {sha}\t{file}\n"));
    }
    let temp_index = std::env::temp_dir().join(format!("locagens-{id}.index"));
    let env = [("GIT_INDEX_FILE", temp_index.as_path())];
    let result = async {
        git_bytes(project_path, &["update-index", "--add", "--index-info"], &env, Some(index_info.into_bytes())).await?;
        let tree = git_bytes(project_path, &["write-tree"], &env, None).await?;
        Ok::<String, String>(String::from_utf8_lossy(&tree).trim().to_string())
    }
    .await;
    let _ = std::fs::remove_file(&temp_index);
    result
}

/// Tracked files with the executable bit (kept in the snapshot tree).
async fn executable_files(project_path: &Path) -> std::collections::HashSet<String> {
    let raw = git_bytes(project_path, &["ls-files", "-s", "-z"], &[], None).await.unwrap_or_default();
    raw.split(|byte| *byte == 0)
        .filter_map(|entry| std::str::from_utf8(entry).ok())
        .filter_map(|entry| {
            let (meta, path) = entry.split_once('\t')?;
            meta.starts_with("100755").then(|| path.to_string())
        })
        .collect()
}

/// Index-copy id suffix of the post-turn index (`<id>.after.index`).
const AFTER_INDEX_SUFFIX: &str = ".after";

/// Records the post-turn state for snapshot `snapshot_id`: the worktree tree,
/// HEAD and a copy of the index. Idempotent; a missing snapshot is ignored.
pub async fn record_turn_end_for_path(project_path: &Path, snapshot_id: &str) -> Result<(), String> {
    let recorded = load_snapshots(project_path)
        .iter()
        .find(|snapshot| snapshot.id == snapshot_id)
        .map(|snapshot| snapshot.after_tree_sha.is_some());
    if recorded != Some(false) {
        return Ok(());
    }
    let tree = write_worktree_tree(project_path, &format!("{snapshot_id}-after")).await?;
    let head = git(project_path, &["rev-parse", "HEAD"]).await.unwrap_or_default().trim().to_string();
    save_index_copy(project_path, &format!("{snapshot_id}{AFTER_INDEX_SUFFIX}")).await;
    let mut snapshots = load_snapshots(project_path);
    if let Some(snapshot) = snapshots.iter_mut().find(|snapshot| snapshot.id == snapshot_id) {
        snapshot.after_tree_sha = Some(tree);
        snapshot.after_head_sha = Some(head).filter(|head| !head.is_empty());
    }
    persist_snapshots(project_path, &snapshots)
}

/// `mode sha path` lines of an index (the real one, or a copy), ignoring
/// the stat data git refreshes on every `git status`.
async fn index_entries(project_path: &Path, index: Option<&Path>) -> Option<Vec<u8>> {
    let env: Vec<(&str, &Path)> = index.map(|index| vec![("GIT_INDEX_FILE", index)]).unwrap_or_default();
    git_bytes(project_path, &["ls-files", "-s", "-z"], &env, None).await.ok()
}

/// `<snapshot_dir>/<id>.index` — copy of the pre-turn index.
fn index_copy_path(project_path: &Path, id: &str) -> PathBuf {
    snapshot_dir(project_path).join(format!("{id}.index"))
}

/// Absolute path of the repository's index (handles linked worktrees).
async fn git_index_path(project_path: &Path) -> Option<PathBuf> {
    let raw = git(project_path, &["rev-parse", "--git-path", "index"]).await.ok()?;
    let path = PathBuf::from(raw.trim());
    Some(if path.is_absolute() { path } else { project_path.join(path) })
}

/// Copies the real index next to the snapshot store (best effort).
async fn save_index_copy(project_path: &Path, id: &str) {
    let Some(index) = git_index_path(project_path).await else { return };
    let target = index_copy_path(project_path, id);
    if let Some(parent) = target.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::copy(index, target);
}

/// Deletes index copies of snapshots no longer in the store.
fn prune_index_copies(project_path: &Path, kept: &[GitSnapshot]) {
    let dir = snapshot_dir(project_path);
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix(".index") else { continue };
        let id = id.strip_suffix(AFTER_INDEX_SUFFIX).unwrap_or(id);
        if !kept.iter().any(|snapshot| snapshot.id == id) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
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

/// Undoes a turn (the run's latest, or the newest of any run):
///
/// - Only paths the turn changed are touched: those whose bytes differ
///   between the pre-turn tree and the post-turn tree. A touched path whose
///   current bytes differ from the post-turn bytes (edited by the user or a
///   later turn since) is left alone and reported in `skipped`. Snapshots
///   from older versions without a post-turn tree fall back to restoring
///   every file that differs from the pre-turn tree.
/// - HEAD is reset (soft) and the index restored only if they still are
///   what the turn left behind.
/// - The replaced state is saved first as a `<run_id>#undo` snapshot, so the
///   undo itself can be undone.
///
/// The used snapshot is consumed (removed from the store).
pub async fn undo_last_turn_for_path(
    project_path: &Path,
    run_id: Option<&str>,
) -> Result<GitSnapshot, String> {
    let snapshots = load_snapshots(project_path);
    let index = match run_id {
        Some(run_id) => snapshots.iter().rposition(|snap| snap.run_id == run_id),
        None => snapshots.len().checked_sub(1),
    }
    .ok_or_else(|| "No git snapshot available to undo".to_string())?;
    let mut snapshot = snapshots[index].clone();
    let tree_sha = snapshot
        .tree_sha
        .clone()
        .ok_or_else(|| "This snapshot was made by an older Locagens version and cannot be undone".to_string())?;

    let before = snapshot_tree_entries(project_path, &tree_sha).await?;
    let after = match snapshot.after_tree_sha.as_deref() {
        Some(after_tree) => Some(snapshot_tree_entries(project_path, after_tree).await?),
        None => None,
    };
    // Paths the turn changed, and the bytes it left them with.
    let touched: Vec<String> = match &after {
        Some(after) => {
            let mut paths: Vec<String> = before
                .keys()
                .chain(after.keys())
                .filter(|path| before.get(*path) != after.get(*path))
                .cloned()
                .collect();
            paths.sort();
            paths.dedup();
            paths
        }
        None => {
            let mut paths: Vec<String> = worktree_files(project_path).await?;
            paths.extend(before.keys().cloned());
            paths.sort();
            paths.dedup();
            paths
        }
    };
    let existing: Vec<String> = touched.iter().filter(|path| project_path.join(path).is_file()).cloned().collect();
    let existing_shas = hash_files(project_path, &existing, false).await?;
    let current: std::collections::HashMap<&str, &str> =
        existing.iter().map(String::as_str).zip(existing_shas.iter().map(String::as_str)).collect();

    // Back up what is about to be replaced (undo of the undo).
    let backup = create_git_snapshot_for_path(project_path, &format!("{}{UNDO_BACKUP_SUFFIX}", snapshot.run_id)).await?;

    for path in &touched {
        let now = current.get(path.as_str()).copied();
        let wanted = before.get(path).map(String::as_str);
        if now == wanted {
            continue;
        }
        if let Some(after) = &after {
            if now != after.get(path).map(String::as_str) {
                snapshot.skipped.push(path.clone());
                continue;
            }
        }
        match wanted {
            None => remove_inside(project_path, path),
            Some(sha) => {
                let Some(full_path) = crate::util::writable_inside(project_path, path) else { continue };
                let bytes = git_bytes(project_path, &["cat-file", "blob", sha], &[], None).await?;
                if let Some(parent) = full_path.parent() {
                    std::fs::create_dir_all(parent).map_err(|err| format!("Failed to restore {path}: {err}"))?;
                }
                std::fs::write(&full_path, bytes).map_err(|err| format!("Failed to restore {path}: {err}"))?;
            }
        }
        snapshot.restored.push(path.clone());
    }

    // HEAD (if the turn committed) and the index, unless they moved since.
    let head_now = git(project_path, &["rev-parse", "HEAD"]).await.unwrap_or_default().trim().to_string();
    let head_is_turns = snapshot.after_head_sha.as_deref().is_none_or(|after| after == head_now);
    if head_now != snapshot.head_sha && head_is_turns {
        git(project_path, &["reset", "-q", "--soft", &snapshot.head_sha])
            .await
            .map_err(|err| format!("git reset failed: {err}"))?;
    }
    let copy = index_copy_path(project_path, &snapshot.id);
    let after_copy = index_copy_path(project_path, &format!("{}{AFTER_INDEX_SUFFIX}", snapshot.id));
    let index_is_turns = !after_copy.is_file()
        || index_entries(project_path, None).await == index_entries(project_path, Some(&after_copy)).await;
    if let (true, true, Some(index)) = (copy.is_file(), index_is_turns && head_is_turns, git_index_path(project_path).await) {
        std::fs::copy(&copy, index).map_err(|err| format!("Failed to restore the git index: {err}"))?;
    }

    if let Some(backup) = backup {
        record_turn_end_for_path(project_path, &backup.id).await?;
    }
    let mut snapshots = load_snapshots(project_path);
    snapshots.retain(|kept| kept.id != snapshot.id);
    persist_snapshots(project_path, &snapshots)?;
    let _ = std::fs::remove_file(copy);
    let _ = std::fs::remove_file(after_copy);
    Ok(snapshot)
}

/// `path -> blob sha` for every file in a snapshot tree.
async fn snapshot_tree_entries(
    project_path: &Path,
    tree_sha: &str,
) -> Result<std::collections::BTreeMap<String, String>, String> {
    let raw = git_bytes(project_path, &["ls-tree", "-r", "-z", tree_sha], &[], None).await?;
    Ok(raw
        .split(|byte| *byte == 0)
        .filter_map(|entry| std::str::from_utf8(entry).ok())
        .filter_map(|entry| {
            let (meta, path) = entry.split_once('\t')?;
            let sha = meta.split_whitespace().nth(2)?;
            Some((path.to_string(), sha.to_string()))
        })
        .collect())
}

/// Deletes a file inside the project (canonical containment check) and then
/// any parent directories it left empty.
fn remove_inside(project_path: &Path, relative: &str) {
    let Some(full_path) = join_inside(project_path, relative) else { return };
    let canonical_project = std::fs::canonicalize(project_path)
        .map(crate::util::normalize_canonical)
        .unwrap_or_else(|_| project_path.to_path_buf());
    let canonical_full = std::fs::canonicalize(&full_path)
        .map(crate::util::normalize_canonical)
        .unwrap_or(full_path);
    if !canonical_full.starts_with(&canonical_project) || std::fs::remove_file(&canonical_full).is_err() {
        return;
    }
    let mut parent = canonical_full.parent().map(Path::to_path_buf);
    while let Some(dir) = parent {
        if dir == canonical_project
            || !dir.starts_with(&canonical_project)
            || dir.ends_with(".git")
            || dir.ends_with(".locagens")
            || std::fs::remove_dir(&dir).is_err()
        {
            break;
        }
        parent = dir.parent().map(Path::to_path_buf);
    }
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

    #[tokio::test]
    async fn trust_facade_reports_ignored_config_and_persists_trust() {
        let project = std::env::temp_dir().join(format!("locagens-trust-api-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(project.join(".locagens")).unwrap();
        std::fs::write(project.join(".locagens").join("mcp.json"), "{}").unwrap();
        let path = project.display().to_string();
        assert!(!is_project_trusted(&path));
        let before = project_trust(&path).await.unwrap();
        assert!(!before.trusted);
        assert_eq!(before.ignored_files.len(), 1);
        assert!(before.ignored_files[0].ends_with("mcp.json"), "{:?}", before.ignored_files);

        let after = trust_project(&path, true).await.unwrap();
        assert_eq!(after, ProjectTrust { trusted: true, ignored_files: Vec::new() });
        assert!(is_project_trusted(&path));
        let revoked = trust_project(&path, false).await.unwrap();
        assert!(!revoked.trusted);
        // The home directory can never be trusted as a whole.
        assert!(trust_project(&crate::util::home_dir().display().to_string(), true).await.is_err());
        let _ = std::fs::remove_dir_all(&project);
    }

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
        assert!(snapshot.tree_sha.is_some(), "snapshot must record a worktree tree");
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
        // …and the snapshot store lives under the Locagens home, never in
        // the project.
        assert!(!root.join(".locagens").exists());
        assert!(snapshot_dir(&root).starts_with(snapshot_home()));

        // Undo without a matching snapshot fails cleanly.
        assert!(undo_last_turn_for_path(&root, Some("missing")).await.is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn git_snapshot_is_byte_exact_and_leaves_the_worktree_alone() {
        let root = std::env::temp_dir().join(format!("locagens-snap-eol-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("lf.txt"), "one\ntwo\n").unwrap();
        std::fs::write(root.join("crlf.txt"), "one\r\ntwo\r\n").unwrap();
        run_git(&root, &["init", "-q"]);
        // Windows' common default: convert on checkout/commit.
        run_git(&root, &["config", "core.autocrlf", "true"]);
        run_git(&root, &["add", "."]);
        run_git(&root, &["commit", "-q", "-m", "init"]);

        // Pre-turn: mixed endings, a staged change, an untracked file.
        std::fs::write(root.join("lf.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(root.join("crlf.txt"), "one\r\nmixed\n").unwrap();
        std::fs::write(root.join("src/staged.rs"), "fn a() {}\n").unwrap();
        run_git(&root, &["add", "src/staged.rs"]);
        std::fs::write(root.join("notes.txt"), "keep\r\n").unwrap();
        let before = |name: &str| std::fs::read(root.join(name)).unwrap();
        let (lf, crlf, staged, notes) = (before("lf.txt"), before("crlf.txt"), before("src/staged.rs"), before("notes.txt"));

        create_git_snapshot_for_path(&root, "run-eol").await.unwrap().expect("snapshot");
        // Taking the snapshot must not rewrite anything.
        assert_eq!(before("lf.txt"), lf);
        assert_eq!(before("crlf.txt"), crlf);

        // The turn edits, deletes, creates (incl. a new directory).
        std::fs::write(root.join("lf.txt"), "changed\r\n").unwrap();
        std::fs::remove_file(root.join("crlf.txt")).unwrap();
        std::fs::write(root.join("src/staged.rs"), "fn b() {}\n").unwrap();
        std::fs::create_dir_all(root.join("gen/deep")).unwrap();
        std::fs::write(root.join("gen/deep/out.txt"), "x").unwrap();

        undo_last_turn_for_path(&root, Some("run-eol")).await.unwrap();
        assert_eq!(before("lf.txt"), lf);
        assert_eq!(before("crlf.txt"), crlf);
        assert_eq!(before("src/staged.rs"), staged);
        assert_eq!(before("notes.txt"), notes);
        assert!(!root.join("gen").exists(), "turn-created dirs are removed");
        // The staged change is staged again.
        let staged_names = std::process::Command::new("git")
            .args(["diff", "--cached", "--name-only"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&staged_names.stdout).contains("src/staged.rs"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// QA-030: undo only reverts what the turn changed; later user edits and
    /// files created after the turn survive, and the undo itself is undoable.
    #[tokio::test]
    async fn undo_leaves_changes_made_after_the_turn_alone_and_can_be_redone() {
        let root = std::env::temp_dir().join(format!("locagens-snap-narrow-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        for name in ["main.rs", "lib.rs", "keep.rs"] {
            std::fs::write(root.join(name), format!("// {name} base\n")).unwrap();
        }
        run_git(&root, &["init", "-q"]);
        run_git(&root, &["add", "."]);
        run_git(&root, &["commit", "-q", "-m", "init"]);

        let snapshot = create_git_snapshot_for_path(&root, "run-n").await.unwrap().expect("snapshot");
        // The turn edits main.rs and lib.rs and creates gen.txt.
        std::fs::write(root.join("main.rs"), "// main.rs by the turn\n").unwrap();
        std::fs::write(root.join("lib.rs"), "// lib.rs by the turn\n").unwrap();
        std::fs::write(root.join("gen.txt"), "turn artifact\n").unwrap();
        record_turn_end_for_path(&root, &snapshot.id).await.unwrap();

        // Afterwards the user edits main.rs again and creates notes.txt.
        std::fs::write(root.join("main.rs"), "// main.rs edited by the user\n").unwrap();
        std::fs::write(root.join("notes.txt"), "user notes\n").unwrap();

        let undone = undo_last_turn_for_path(&root, Some("run-n")).await.unwrap();
        let read = |name: &str| std::fs::read_to_string(root.join(name)).unwrap();
        assert_eq!(read("lib.rs"), "// lib.rs base\n", "the turn's own change is reverted");
        assert!(!root.join("gen.txt").exists(), "the turn's new file is removed");
        assert_eq!(read("main.rs"), "// main.rs edited by the user\n", "user edit survives");
        assert_eq!(read("notes.txt"), "user notes\n", "file created after the turn survives");
        assert_eq!(read("keep.rs"), "// keep.rs base\n");
        assert_eq!(undone.skipped, vec!["main.rs".to_string()]);
        let mut restored = undone.restored.clone();
        restored.sort();
        assert_eq!(restored, vec!["gen.txt".to_string(), "lib.rs".to_string()]);

        // The undo left a backup: undoing it redoes the turn's changes.
        let backup_run = format!("run-n{UNDO_BACKUP_SUFFIX}");
        undo_last_turn_for_path(&root, Some(&backup_run)).await.unwrap();
        assert_eq!(read("lib.rs"), "// lib.rs by the turn\n");
        assert_eq!(read("gen.txt"), "turn artifact\n");
        assert_eq!(read("main.rs"), "// main.rs edited by the user\n");
        assert_eq!(read("notes.txt"), "user notes\n");
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

    #[test]
    fn legacy_in_project_snapshot_store_moves_under_the_locagens_home() {
        let root = std::env::temp_dir().join(format!("locagens-snap-migrate-{}", uuid::Uuid::new_v4()));
        let legacy = root.join(".locagens");
        std::fs::create_dir_all(legacy.join("snapshots")).unwrap();
        std::fs::create_dir_all(legacy.join("rules")).unwrap();
        std::fs::write(legacy.join("rules").join("default.rules"), "# rules").unwrap();
        std::fs::write(legacy.join("snapshots").join("snap-old.index"), "idx").unwrap();
        let entry = GitSnapshot {
            id: "snap-old".into(),
            run_id: "run-old".into(),
            head_sha: "abc".into(),
            tree_sha: Some("def".into()),
            stash_sha: None,
            untracked: vec![],
            created_at: now_iso(),
            after_tree_sha: None,
            after_head_sha: None,
            restored: vec![],
            skipped: vec![],
        };
        std::fs::write(
            legacy.join("snapshots.json"),
            serde_json::to_string(&vec![entry]).unwrap(),
        )
        .unwrap();

        let loaded = load_snapshots(&root);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "snap-old");
        // The runtime state moved out of the project…
        assert!(!legacy.join("snapshots.json").exists());
        assert!(!legacy.join("snapshots").exists());
        assert!(snapshot_store_path(&root).is_file());
        assert!(index_copy_path(&root, "snap-old").is_file());
        // …but user-visible rules stay where they were.
        assert!(legacy.join("rules").join("default.rules").is_file());
        // Loading again is stable.
        assert_eq!(load_snapshots(&root).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(snapshot_dir(&root).parent().unwrap());
    }
}
