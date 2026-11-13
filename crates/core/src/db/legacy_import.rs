//! One-time read-only import of the legacy `<app_support>/data` layout.

use super::*;

// ----------------------------------------------------------------------
// Legacy import
// ----------------------------------------------------------------------

/// The legacy data dir to import from, only when `root` is the default
/// Locagens home (so custom roots and tests never read the real app data).
pub(super) fn default_legacy_dir_for(root: &Path) -> Option<PathBuf> {
    (root == crate::paths::locagens_home()).then(|| crate::paths::app_support_dir().join("data"))
}

pub(super) fn load_optional_plan(path: &Path) -> Option<Plan> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// One-time import of the legacy layout (`projects.json`, `permissions.json`,
/// `memory.json`, `usage_logs.jsonl`, `sessions/<id>/{run.json,messages.jsonl,
/// plan.json}`) into rollouts, Markdown memories and the index files. The
/// legacy directory is only read. `<root>/.migrated-from-data-v1` is written
/// last, so an interrupted import is retried (existing files are not
/// overwritten). Usage lines of runs that no longer exist are dropped.
pub(super) fn migrate_legacy_data(legacy: &Path, root: &Path) -> anyhow::Result<()> {
    let marker = root.join(MIGRATION_MARKER);
    if marker.exists() || !legacy.is_dir() {
        return Ok(());
    }
    let _lock = lock_root(root);
    if marker.exists() {
        return Ok(());
    }

    for name in ["projects.json", "permissions.json"] {
        let (from, to) = (legacy.join(name), root.join(name));
        if from.is_file() && !to.exists() {
            std::fs::copy(&from, &to)?;
        }
    }

    let mut usage_by_run: HashMap<String, Vec<UsageLogEntry>> = HashMap::new();
    let mut max_usage_id = 0;
    for entry in read_jsonl_or::<UsageLogEntry>(&legacy.join("usage_logs.jsonl")) {
        max_usage_id = max_usage_id.max(entry.id);
        usage_by_run.entry(entry.run_id.clone()).or_default().push(entry);
    }

    let mut history: Vec<(u64, String, String)> = Vec::new();
    let mut index_lines = String::new();
    if let Ok(entries) = std::fs::read_dir(legacy.join("sessions")) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let dir = entry.path();
            let dir_id = entry.file_name().to_string_lossy().to_string();
            let mut run: Run = read_json_or(&dir.join("run.json"));
            let messages = read_jsonl_or::<RunMessage>(&dir.join("messages.jsonl"));
            let plan = load_optional_plan(&dir.join("plan.json"));
            let has_run = !run.id.is_empty();
            if !has_run && messages.is_empty() && plan.is_none() {
                continue;
            }
            if !has_run {
                run.id = dir_id;
            }
            let created = if !run.created_at.is_empty() {
                run.created_at.clone()
            } else {
                messages.first().map(|m| m.created_at.clone()).unwrap_or_else(now_iso)
            };
            let path = new_rollout_path(root, &run.id, &created);
            if path.exists() {
                continue;
            }

            let mut buffer = String::new();
            if has_run {
                buffer.push_str(&rollout_line(&created, "session_meta", &run_payload(&run)));
            }
            for message in &messages {
                buffer.push_str(&rollout_line(&message.created_at, "message", &message_payload(message)));
                if message.role == "user" && message.agent_role.is_none() {
                    history.push((unix_secs(&message.created_at), run.id.clone(), message.content.clone()));
                }
            }
            if let Some(plan) = plan.as_ref() {
                buffer.push_str(&rollout_line(&plan.updated_at, "plan", &plan_payload(plan)));
            }
            for usage in usage_by_run.remove(&run.id).unwrap_or_default() {
                let payload = serde_json::to_value(&usage).unwrap_or(Value::Null);
                buffer.push_str(&rollout_line(&usage.created_at, "usage", &payload));
            }
            write_text_atomic(&path, &buffer)?;
            if has_run {
                index_lines.push_str(
                    &json!({ "id": run.id, "title": run.title, "updated_at": run.updated_at }).to_string(),
                );
                index_lines.push('\n');
            }
        }
    }
    if !index_lines.is_empty() {
        append_text(&root.join(SESSION_INDEX_FILE), &index_lines);
    }
    if !history.is_empty() && !root.join(HISTORY_FILE).exists() {
        history.sort_by_key(|(ts, _, _)| *ts);
        let mut buffer = String::new();
        for (ts, session_id, text) in history {
            buffer.push_str(&json!({ "session_id": session_id, "ts": ts, "text": text }).to_string());
            buffer.push('\n');
        }
        append_text(&root.join(HISTORY_FILE), &buffer);
    }

    let mut taken: HashSet<i64> = HashSet::new();
    for memory in read_json_or::<Vec<Memory>>(&legacy.join("memory.json")) {
        if !taken.insert(memory.id) {
            continue;
        }
        let path = memory_file_path(root, &memory);
        if !path.exists() {
            write_text_atomic(&path, &render_memory(&memory))?;
        }
    }

    if max_usage_id > 0 {
        std::fs::write(root.join(USAGE_SEQ_FILE), max_usage_id.to_string())?;
    }
    std::fs::write(&marker, format!("{}\nimported from {}\n", now_iso(), legacy.display()))?;
    Ok(())
}
