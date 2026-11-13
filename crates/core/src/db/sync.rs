//! Loading the store and re-reading what other processes changed.

use super::*;

impl Db {
    /// Re-reads the rollouts that changed on disk so runs created or changed
    /// by another process (the desktop app or a second `lcgn`) become
    /// visible. Incremental: only rollouts whose mtime/size changed are
    /// replayed, and runs this process is driving (they hold their owner
    /// lock, and may have in-memory streaming state) are never overwritten.
    pub fn refresh_runs(&self) {
        let mut paths = Vec::new();
        collect_rollouts(&self.root.join("sessions"), &mut paths);
        paths.sort();
        let mut guard = self.write();
        let state = &mut *guard;
        let seen: HashSet<PathBuf> = paths.iter().cloned().collect();

        for path in paths {
            let sig = file_sig(&path);
            if state.rollout_sigs.get(&path) == Some(&sig) {
                continue;
            }
            state.rollout_sigs.insert(path.clone(), sig);
            let mut fresh = StorageState::default();
            replay_rollout(&path, &mut fresh);
            merge_replayed_runs(state, fresh);
        }

        // Rollouts deleted by another process take their runs with them.
        let gone: Vec<PathBuf> = state
            .rollout_sigs
            .keys()
            .filter(|path| !seen.contains(*path))
            .cloned()
            .collect();
        for path in gone {
            state.rollout_sigs.remove(&path);
            let orphans: Vec<String> = state
                .rollouts
                .iter()
                .filter(|(id, owned)| **owned == path && !state.run_claims.contains_key(*id))
                .map(|(id, _)| id.clone())
                .collect();
            for id in orphans {
                forget_run(state, &id);
            }
        }
        state.usage.sort_by_key(|entry| entry.id);
        state.next_usage_id = next_usage_id_from(&self.root, &state.usage);
    }

    /// `refresh_runs` at most every [`REFRESH_INTERVAL`]; cheap enough to call
    /// from every run listing.
    pub fn refresh_runs_throttled(&self) {
        {
            let mut last = self.last_refresh.lock().unwrap_or_else(PoisonError::into_inner);
            if last.is_some_and(|at| at.elapsed() < REFRESH_INTERVAL) {
                return;
            }
            *last = Some(Instant::now());
        }
        self.refresh_runs();
    }

    /// Reloads `permissions.json` / `projects.json` / the memory files when
    /// another process changed them, so grants revoked elsewhere take effect
    /// here (QA-039).
    pub(super) fn sync_shared_files(&self, projects: bool, permissions: bool, memories: bool) {
        let (projects_sig, permissions_sig) = {
            let state = self.read();
            (state.projects_sig, state.permissions_sig)
        };
        let stale = (projects && file_sig(&self.root.join("projects.json")) != projects_sig)
            || (permissions && file_sig(&self.root.join("permissions.json")) != permissions_sig)
            || (memories && memories_signature(&self.root) != self.read().memories_sig);
        if stale {
            reload_shared_files(&self.root, &mut self.write());
        }
    }
}

// ----------------------------------------------------------------------
// JSON primitives
// ----------------------------------------------------------------------

/// Atomic text write: write a `.tmp` sibling, then rename over the target.
pub(super) fn write_text_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Atomic JSON write so readers never observe a half-written file.
pub(super) fn write_json_atomic(path: &Path, value: &impl Serialize) {
    if let Err(err) = (|| -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
        write_text_atomic(path, &json)
    })() {
        tracing::error!("[Database] Failed to persist {}: {err}", path.display());
    }
}

// ----------------------------------------------------------------------
// Startup loading
// ----------------------------------------------------------------------

pub(super) fn read_json_or<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub(super) fn read_jsonl_or<T: for<'de> Deserialize<'de>>(path: &Path) -> Vec<T> {
    std::fs::read(path)
        .map(|bytes| {
            String::from_utf8_lossy(&bytes)
                .lines()
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn load_state(root: &Path) -> anyhow::Result<StorageState> {
    let mut state = StorageState::default();
    reload_shared_files(root, &mut state);
    load_rollouts(root, &mut state);
    state.next_usage_id = next_usage_id_from(root, &state.usage);
    Ok(state)
}

/// Re-reads the cross-process-shared files (`projects.json`,
/// `permissions.json`, memory Markdown) into `state`. Used at startup and by
/// `with_shared_mutation` under the advisory lock, so a mutation always
/// applies on top of the latest on-disk state written by another process.
pub(super) fn reload_shared_files(root: &Path, state: &mut StorageState) {
    // Signatures first: a write racing with the read below is then picked up
    // by the next sync instead of being lost.
    state.projects_sig = file_sig(&root.join("projects.json"));
    state.permissions_sig = file_sig(&root.join("permissions.json"));
    state.memories_sig = memories_signature(root);
    state.projects = read_json_or::<Vec<Project>>(&root.join("projects.json"));
    state.permissions = read_json_or::<Vec<PermissionRule>>(&root.join("permissions.json"));
    let memories = load_memories(root);
    state.memory_files = memories
        .iter()
        .map(|(memory, path)| (memory.id, path.clone()))
        .collect();
    state.memories = memories.into_iter().map(|(memory, _)| memory).collect();
    state.next_permission_id = state
        .permissions
        .iter()
        .map(|rule| rule.id)
        .max()
        .unwrap_or(0)
        + 1;
    state.next_memory_id = state.memories.iter().map(|m| m.id).max().unwrap_or(0) + 1;
}
