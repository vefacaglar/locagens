//! Run lifecycle: create, patch, fork, delete, owner claims and merging.

use super::*;

impl Db {
    // ------------------------------------------------------------------
    // Runs
    // ------------------------------------------------------------------

    /// Starts the run's rollout with a `session_meta` line (or appends a
    /// `run_update` when the run id is already known). An active run is
    /// claimed by this process (see [`Db::run_startup_tasks`]).
    pub fn create_run(&self, run: Run) -> Run {
        let mut guard = self.write();
        let state = &mut *guard;
        state.deleted_runs.remove(&run.id);
        let kind = if state.runs.contains_key(&run.id) {
            "run_update"
        } else {
            "session_meta"
        };
        state.runs.insert(run.id.clone(), run.clone());
        append_event(&self.root, state, &run.id, &run.created_at, kind, run_payload(&run));
        append_session_index(&self.root, &run);
        sync_run_claim(&self.root, state, &run);
        run
    }

    pub fn update_run_status(&self, id: &str, status: &str, error_message: Option<&str>) {
        let mut guard = self.write();
        let state = &mut *guard;
        let snapshot = state.runs.get_mut(id).map(|run| {
            run.status = status.to_string();
            run.error_message = error_message.map(str::to_string);
            run.updated_at = now_iso();
            run.last_active_at = Some(now_iso());
            run.clone()
        });
        if let Some(run) = snapshot {
            append_event(&self.root, state, id, &run.created_at, "run_update", run_payload(&run));
            append_session_index(&self.root, &run);
            sync_run_claim(&self.root, state, &run);
        }
    }

    /// Applies an arbitrary metadata patch to a run (title, model fields,
    /// status, …) and appends the new snapshot as a `run_update` line.
    pub fn patch_run(&self, id: &str, patch: impl FnOnce(&mut Run)) -> Option<Run> {
        let mut guard = self.write();
        let state = &mut *guard;
        let updated = state.runs.get_mut(id).map(|run| {
            let old_title = run.title.clone();
            patch(run);
            run.updated_at = now_iso();
            run.last_active_at = Some(now_iso());
            (run.clone(), old_title != run.title)
        });
        let (run, title_changed) = updated?;
        append_event(&self.root, state, id, &run.created_at, "run_update", run_payload(&run));
        if title_changed {
            append_session_index(&self.root, &run);
        }
        sync_run_claim(&self.root, state, &run);
        Some(run)
    }

    pub fn touch_run(&self, id: &str) {
        let mut guard = self.write();
        let state = &mut *guard;
        let snapshot = state.runs.get_mut(id).map(|run| {
            run.updated_at = now_iso();
            run.last_active_at = Some(now_iso());
            run.clone()
        });
        if let Some(run) = snapshot {
            append_event(&self.root, state, id, &run.created_at, "run_update", run_payload(&run));
        }
    }

    pub fn list_runs(&self) -> Vec<Run> {
        self.read().runs.values().cloned().collect()
    }

    pub fn get_run_full(&self, id: &str) -> Option<Run> {
        self.read().runs.get(id).cloned()
    }

    /// Projected view kept for Phase 1 route compatibility.
    pub fn get_run(&self, id: &str) -> Option<RunRow> {
        self.get_run_full(id).map(|run| RunRow {
            id: run.id,
            title: run.title,
            project_path: Some(run.project_path).filter(|p| !p.is_empty()),
            project_name: run.project_name,
            provider_id: run.provider_id,
            provider_display_name: run.provider_display_name,
            model: run.model,
        })
    }

    /// Forks a run: writes a new rollout holding the source's metadata
    /// (`session_meta` with `forked_from`), messages (up to and including
    /// `up_to_message_id`; all of them when `None`) and plan under fresh ids,
    /// titled `<title> (fork)`. Returns `None` when the source run or the
    /// fork-point message does not exist.
    pub fn fork_run(&self, source_run_id: &str, up_to_message_id: Option<&str>) -> Option<Run> {
        let source = self.get_run_full(source_run_id)?;
        let messages = self.list_messages(source_run_id);
        let mut copied: Vec<RunMessage> = Vec::new();
        for message in &messages {
            copied.push(message.clone());
            if let Some(mid) = up_to_message_id {
                if message.id == mid {
                    break;
                }
            }
        }
        if let Some(mid) = up_to_message_id {
            if !copied.iter().any(|message| message.id == mid) {
                return None;
            }
        }

        let new_id = format!("run-{}", uuid::Uuid::new_v4());
        let now = now_iso();
        let mut fork = source.clone();
        fork.id = new_id.clone();
        fork.title = if source.title.trim().is_empty() {
            "Fork".to_string()
        } else {
            format!("{} (fork)", source.title)
        };
        fork.status = "done".to_string();
        fork.error_message = None;
        fork.ephemeral = false;
        fork.created_at = now.clone();
        fork.updated_at = now.clone();
        fork.last_active_at = Some(now.clone());

        let mut meta = run_payload(&fork);
        meta["forked_from"] = json!(source_run_id);
        if let Some(mid) = up_to_message_id {
            meta["forked_up_to_message_id"] = json!(mid);
        }
        let mut buffer = rollout_line(&now, "session_meta", &meta);
        for message in copied.iter_mut() {
            message.id = format!("msg-{}", uuid::Uuid::new_v4());
            message.run_id = new_id.clone();
            buffer.push_str(&rollout_line(&now, "message", &message_payload(message)));
        }
        let plan = self.get_plan(source_run_id).map(|mut plan| {
            plan.run_id = new_id.clone();
            plan
        });
        if let Some(plan) = plan.as_ref() {
            buffer.push_str(&rollout_line(&now, "plan", &plan_payload(plan)));
        }

        let path = new_rollout_path(&self.root, &new_id, &fork.created_at);
        append_text(&path, &buffer);
        let mut guard = self.write();
        let state = &mut *guard;
        state.rollouts.insert(new_id.clone(), path);
        state.runs.insert(new_id.clone(), fork.clone());
        state.messages.insert(new_id.clone(), copied);
        if let Some(plan) = plan {
            state.plans.insert(new_id, plan);
        }
        append_session_index(&self.root, &fork);
        Some(fork)
    }

    /// Deletes the run's rollout file (usage lines live in it and go too).
    /// The id is tombstoned, so a drive task that is still unwinding cannot
    /// write a headless rollout back. Callers that may be deleting an active
    /// run should go through `api::runs::delete_run`, which cancels first.
    pub fn delete_run(&self, id: &str) -> bool {
        let mut guard = self.write();
        let state = &mut *guard;
        remove_run(&self.root, state, id)
    }
}

// ----------------------------------------------------------------------
// Run ownership, deletion and merging
// ----------------------------------------------------------------------

/// Claims (active run) or releases (terminal run) this process's owner lock
/// for `run`, so other processes know whether it is still being driven.
pub(super) fn sync_run_claim(root: &Path, state: &mut StorageState, run: &Run) {
    if ACTIVE_RUN_STATUSES.contains(&run.status.as_str()) {
        if !state.run_claims.contains_key(&run.id) {
            if let Some(file) = acquire_run_lock(root, &run.id) {
                state.run_claims.insert(run.id.clone(), file);
            }
        }
    } else {
        release_run_claim(root, state, &run.id);
    }
}

pub(super) fn release_run_claim(root: &Path, state: &mut StorageState, run_id: &str) {
    if let Some(file) = state.run_claims.remove(run_id) {
        let _ = file.unlock();
        drop(file);
        let _ = std::fs::remove_file(run_lock_path(root, run_id));
    }
}

/// Drops every in-memory trace of a run (no file is touched).
pub(super) fn forget_run(state: &mut StorageState, id: &str) {
    state.runs.remove(id);
    state.messages.remove(id);
    state.plans.remove(id);
    state.usage.retain(|entry| entry.run_id != id);
    state.rollouts.remove(id);
}

/// Deletes a run: tombstones the id, releases its owner lock, drops its
/// memory and removes its rollout file. Returns whether the run existed.
pub(super) fn remove_run(root: &Path, state: &mut StorageState, id: &str) -> bool {
    state.deleted_runs.insert(id.to_string());
    release_run_claim(root, state, id);
    state.stream_checkpoints.retain(|message_id, _| {
        !state.messages.get(id).is_some_and(|messages| messages.iter().any(|m| &m.id == message_id))
    });
    let existed = state.runs.contains_key(id);
    let path = state.rollouts.get(id).cloned();
    forget_run(state, id);
    if let Some(path) = path {
        state.rollout_sigs.remove(&path);
        let _ = std::fs::remove_file(&path);
        prune_empty_dirs(&path, &root.join("sessions"));
    }
    existed
}

/// Folds freshly replayed runs into `state`, except runs this process owns
/// or has deleted.
pub(super) fn merge_replayed_runs(state: &mut StorageState, mut fresh: StorageState) {
    let ids: Vec<String> = fresh.runs.keys().cloned().collect();
    for id in ids {
        if state.run_claims.contains_key(&id) || state.deleted_runs.contains(&id) {
            continue;
        }
        if let Some(run) = fresh.runs.remove(&id) {
            state.runs.insert(id.clone(), run);
        }
        state.messages.insert(id.clone(), fresh.messages.remove(&id).unwrap_or_default());
        match fresh.plans.remove(&id) {
            Some(plan) => {
                state.plans.insert(id.clone(), plan);
            }
            None => {
                state.plans.remove(&id);
            }
        }
        state.usage.retain(|entry| entry.run_id != id);
        state.usage.extend(fresh.usage.iter().filter(|entry| entry.run_id == id).cloned());
        if let Some(path) = fresh.rollouts.remove(&id) {
            state.rollouts.insert(id, path);
        }
    }
}
