//! Usage log entries (`usage` lines of the run rollout).

use super::*;

impl Db {
    // ------------------------------------------------------------------
    // Usage logs (`usage` lines of the run's rollout)
    // ------------------------------------------------------------------

    /// Appends a usage line to the run's rollout. The cross-process lock is
    /// held only while the id is allocated from `.usage_seq`; the shared
    /// files are not re-read (QA-036).
    pub fn append_usage(&self, mut entry: UsageLogEntry) -> UsageLogEntry {
        let mut guard = self.write();
        let state = &mut *guard;
        if state.deleted_runs.contains(&entry.run_id) {
            return entry;
        }
        if entry.id == 0 {
            let _lock = self.cross_process_lock();
            entry.id = allocate_usage_id(&self.root, state);
        }
        state.usage.push(entry.clone());
        let payload = serde_json::to_value(&entry).unwrap_or(Value::Null);
        append_event(&self.root, state, &entry.run_id, &entry.created_at, "usage", payload);
        entry
    }


    pub fn list_usage(&self, run_id: Option<&str>) -> Vec<UsageLogEntry> {
        let state = self.read();
        state
            .usage
            .iter()
            .filter(|entry| run_id.map(|id| entry.run_id == id).unwrap_or(true))
            .cloned()
            .collect()
    }
}

/// Next usage id: above everything in memory and in the cross-process
/// counter file `.usage_seq` (usage lines live in separate rollouts, so the
/// counter is what keeps ids unique between processes).
pub(super) fn allocate_usage_id(root: &Path, state: &mut StorageState) -> i64 {
    let on_disk = std::fs::read_to_string(root.join(USAGE_SEQ_FILE))
        .ok()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .unwrap_or(0);
    let id = on_disk.max(state.next_usage_id - 1).max(0) + 1;
    if let Err(err) = std::fs::write(root.join(USAGE_SEQ_FILE), id.to_string()) {
        tracing::error!("[Database] Failed to write {USAGE_SEQ_FILE}: {err}");
    }
    state.next_usage_id = id + 1;
    id
}

pub(super) fn next_usage_id_from(root: &Path, usage: &[UsageLogEntry]) -> i64 {
    let on_disk = std::fs::read_to_string(root.join(USAGE_SEQ_FILE))
        .ok()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .unwrap_or(0);
    usage.iter().map(|entry| entry.id).max().unwrap_or(0).max(on_disk) + 1
}
