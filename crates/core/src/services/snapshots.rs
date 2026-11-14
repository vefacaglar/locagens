//! Git baseline snapshots and turn undo: the `snapshots.json` store under
//! `~/.locagens/projects/<project>/snapshots/` and the undo engine.

use crate::db;
use crate::services::git::{git, git_bytes};
use crate::util::{join_inside, now_iso};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

/// Serialises read-modify-write cycles of `snapshots.json` within the
/// process, so two runs in one project cannot lose each other's entries.
static STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_store() -> std::sync::MutexGuard<'static, ()> {
    STORE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// How many snapshots `snapshots.json` retains.
const MAX_SNAPSHOTS: usize = 20;

/// Where runtime snapshot state lives: the Locagens home (`~/.locagens`;
/// a temp directory in unit tests, see `crate::paths`).
fn snapshot_home() -> PathBuf {
    crate::paths::locagens_home()
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
    locagens_config::private_file::write_private_atomic(&store, json.as_bytes())
        .map_err(|err| format!("Failed to write snapshots: {err}"))
}

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

    let _guard = lock_store();
    let mut snapshots = load_snapshots(project_path);
    snapshots.push(snapshot.clone());
    persist_snapshots(project_path, &snapshots)?;
    Ok(Some(snapshot))
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
        // Regular files only: a symlink would be stored (and undone) as a
        // regular file, so symlinks are left out of snapshots and undo.
        .filter(|path| is_regular_file(&project_path.join(path)))
        .map(str::to_string)
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

/// Hashes `files` exactly as they are on disk; `write` stores the blobs.
/// Writing skips files whose size and mtime match the previous snapshot
/// (`HashCache`), so only changed files are read again (FR-007).
async fn hash_files(project_path: &Path, files: &[String], write: bool) -> Result<Vec<String>, String> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let mut cache = if write { HashCache::load(project_path) } else { HashCache::default() };
    let stats: Vec<Option<FileStat>> = files.iter().map(|file| FileStat::of(&project_path.join(file))).collect();
    let mut shas: Vec<Option<String>> = files
        .iter()
        .zip(&stats)
        .map(|(file, stat)| cache.lookup(file, stat.as_ref()))
        .collect();
    let missing: Vec<usize> = (0..files.len()).filter(|index| shas[*index].is_none()).collect();
    if !missing.is_empty() {
        let mut args = vec!["hash-object", "--no-filters", "--stdin-paths"];
        if write {
            args.insert(1, "-w");
        }
        let input = missing.iter().map(|index| files[*index].as_str()).collect::<Vec<_>>().join("\n").into_bytes();
        let output = git_bytes(project_path, &args, &[], Some(input)).await?;
        let hashed: Vec<String> = String::from_utf8_lossy(&output).lines().map(str::to_string).collect();
        if hashed.len() != missing.len() {
            return Err("git hash-object returned an unexpected number of hashes".to_string());
        }
        for (index, sha) in missing.into_iter().zip(hashed) {
            if let (true, Some(stat)) = (write, stats[index].as_ref()) {
                cache.insert(&files[index], stat, &sha);
            }
            shas[index] = Some(sha);
        }
    }
    if write {
        cache.save(project_path);
    }
    Ok(shas.into_iter().flatten().collect())
}

/// Size and modification time of a file.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct FileStat {
    size: u64,
    mtime_ns: u128,
}

impl FileStat {
    fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::symlink_metadata(path).ok()?;
        let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
        Some(Self { size: meta.len(), mtime_ns: mtime.as_nanos() })
    }

    /// A file modified within the last two seconds may still change without
    /// moving its mtime (coarse timestamps), so it is always rehashed.
    fn is_settled(&self) -> bool {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        now.saturating_sub(self.mtime_ns) > 2_000_000_000
    }
}

#[derive(Serialize, Deserialize)]
struct HashCacheEntry {
    stat: FileStat,
    sha: String,
    /// Unix seconds when the blob was last written; stale entries are
    /// rehashed so a blob git has since pruned is written again.
    written_at: u64,
}

/// `path -> (stat, blob sha)` of blobs already written to the object store,
/// kept next to `snapshots.json`.
#[derive(Default, Serialize, Deserialize)]
struct HashCache {
    entries: std::collections::HashMap<String, HashCacheEntry>,
}

/// Cache entries older than this are not trusted (git prunes unreachable
/// objects after two weeks).
const HASH_CACHE_MAX_AGE_SECS: u64 = 7 * 24 * 3600;

fn unix_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl HashCache {
    fn path(project_path: &Path) -> PathBuf {
        snapshot_dir(project_path).join("hashcache.json")
    }

    fn load(project_path: &Path) -> Self {
        std::fs::read_to_string(Self::path(project_path))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn lookup(&self, file: &str, stat: Option<&FileStat>) -> Option<String> {
        let (entry, stat) = (self.entries.get(file)?, stat?);
        let fresh = unix_secs().saturating_sub(entry.written_at) < HASH_CACHE_MAX_AGE_SECS;
        (fresh && entry.stat == *stat && stat.is_settled()).then(|| entry.sha.clone())
    }

    fn insert(&mut self, file: &str, stat: &FileStat, sha: &str) {
        self.entries.insert(
            file.to_string(),
            HashCacheEntry { stat: *stat, sha: sha.to_string(), written_at: unix_secs() },
        );
    }

    /// Best effort; drops stale entries.
    fn save(mut self, project_path: &Path) {
        let now = unix_secs();
        self.entries.retain(|_, entry| now.saturating_sub(entry.written_at) < HASH_CACHE_MAX_AGE_SECS);
        let Ok(json) = serde_json::to_vec(&self) else { return };
        let path = Self::path(project_path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, json);
    }
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
    let _guard = lock_store();
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

/// True for a regular file (symlinks do not count).
fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_file())
}

/// True for paths inside the managed `.locagens` folder (never touched by
/// snapshot/undo logic).
fn is_locagens_path(relative: &str) -> bool {
    relative == ".locagens"
        || relative.starts_with(".locagens/")
        || relative.starts_with(".locagens\\")
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
    let existing: Vec<String> = touched.iter().filter(|path| is_regular_file(&project_path.join(path))).cloned().collect();
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
    let _guard = lock_store();
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

#[cfg(test)]
mod tests {
    use super::*;

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
