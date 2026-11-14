//! Rollout file format: paths, append, replay, index and history files.

use super::*;

// ----------------------------------------------------------------------
// Rollout files
// ----------------------------------------------------------------------

pub(super) fn run_payload(run: &Run) -> Value {
    serde_json::to_value(run).unwrap_or(Value::Null)
}

/// Large `Run` fields a `run_update` line repeats only when they changed;
/// replay keeps the previous value when a key is absent (FR-068).
const HEAVY_RUN_KEYS: [&str; 3] = ["task", "systemPrompt", "appendSystemPrompt"];

/// `run_update` payload: the run without the heavy fields `previous`
/// already recorded (a field cleared to `None` is written as `null`).
pub(super) fn run_update_payload(previous: Option<&Run>, run: &Run) -> Value {
    let mut payload = run_payload(run);
    let (Some(previous), Some(object)) = (previous.map(run_payload), payload.as_object_mut()) else {
        return payload;
    };
    for key in HEAVY_RUN_KEYS {
        let before = previous.get(key).cloned().unwrap_or(Value::Null);
        let now = object.get(key).cloned().unwrap_or(Value::Null);
        if before == now {
            object.remove(key);
        } else {
            object.insert(key.to_string(), now);
        }
    }
    payload
}

/// Replays a `run_update` line onto the previously replayed run: keys the
/// line leaves out keep their earlier value.
fn merged_run_update(previous: Option<&Run>, payload: Value) -> Option<Run> {
    let merged = match (previous.map(run_payload), payload) {
        (Some(Value::Object(mut base)), Value::Object(update)) => {
            base.extend(update);
            Value::Object(base)
        }
        (_, payload) => payload,
    };
    serde_json::from_value::<Run>(merged).ok()
}

pub(super) fn message_payload(message: &RunMessage) -> Value {
    serde_json::to_value(message).unwrap_or(Value::Null)
}

pub(super) fn plan_payload(plan: &Plan) -> Value {
    serde_json::to_value(plan).unwrap_or(Value::Null)
}

/// One serialized rollout line, newline-terminated.
pub(super) fn rollout_line(timestamp: &str, kind: &str, payload: &Value) -> String {
    let mut line = json!({ "timestamp": timestamp, "type": kind, "payload": payload }).to_string();
    line.push('\n');
    line
}

pub(super) fn safe_file_part(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

pub(super) fn parse_time(iso: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(iso)
        .map(|time| time.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

pub(super) fn unix_secs(iso: &str) -> u64 {
    parse_time(iso).timestamp().max(0) as u64
}

/// `sessions/YYYY/MM/DD/rollout-YYYY-MM-DDThh-mm-ss-<run_id>.jsonl`.
pub(super) fn new_rollout_path(root: &Path, run_id: &str, created_at: &str) -> PathBuf {
    let time = parse_time(created_at);
    root.join("sessions")
        .join(time.format("%Y").to_string())
        .join(time.format("%m").to_string())
        .join(time.format("%d").to_string())
        .join(format!(
            "rollout-{}-{}.jsonl",
            time.format("%Y-%m-%dT%H-%M-%S"),
            safe_file_part(run_id)
        ))
}

/// Appends one event to the run's rollout, creating the file (date-sharded
/// by `created_hint`) on first write.
pub(super) fn append_event(
    root: &Path,
    state: &mut StorageState,
    run_id: &str,
    created_hint: &str,
    kind: &str,
    payload: Value,
) {
    if state.deleted_runs.contains(run_id) {
        return;
    }
    let path = state
        .rollouts
        .entry(run_id.to_string())
        .or_insert_with(|| new_rollout_path(root, run_id, created_hint))
        .clone();
    append_text(&path, &rollout_line(&now_iso(), kind, &payload));
}

/// Appends text to a file (creating it and its directories) and flushes. If
/// the file ends in a truncated line, a newline is written first so the new
/// line is not glued onto the corrupt one.
pub(super) fn append_text(path: &Path, text: &str) {
    if let Err(err) = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(path)?;
        let mut buffer = String::new();
        if file.metadata()?.len() > 0 {
            file.seek(SeekFrom::End(-1))?;
            let mut last = [0u8; 1];
            file.read_exact(&mut last)?;
            if last[0] != b'\n' {
                buffer.push('\n');
            }
        }
        buffer.push_str(text);
        file.write_all(buffer.as_bytes())?;
        file.flush()?;
        Ok(())
    })() {
        tracing::error!("[Database] Failed to append {}: {err}", path.display());
    }
}

/// Removes now-empty date directories above a deleted rollout, stopping at
/// (and keeping) `stop`.
pub(super) fn prune_empty_dirs(file: &Path, stop: &Path) {
    let mut dir = file.parent();
    while let Some(current) = dir {
        if current == stop || !current.starts_with(stop) || std::fs::remove_dir(current).is_err() {
            break;
        }
        dir = current.parent();
    }
}

pub(super) fn collect_rollouts(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            collect_rollouts(&path, out);
        } else {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("rollout-") && name.ends_with(".jsonl") {
                out.push(path);
            }
        }
    }
}

/// Replays one rollout file into `state` (last write wins). Unparseable
/// lines, including a truncated last line, are skipped.
pub(super) fn replay_rollout(path: &Path, state: &mut StorageState) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let raw = String::from_utf8_lossy(&bytes);
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(RolloutLine { kind, payload }) = serde_json::from_str::<RolloutLine>(line) else {
            continue;
        };
        match kind.as_str() {
            "session_meta" | "run_update" => {
                let id = payload.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                if let Some(run) = merged_run_update(state.runs.get(&id), payload) {
                    if !run.id.is_empty() {
                        state.rollouts.insert(run.id.clone(), path.to_path_buf());
                        state.runs.insert(run.id.clone(), run);
                    }
                }
            }
            "message" | "message_update" => {
                if let Ok(message) = serde_json::from_value::<RunMessage>(payload) {
                    state
                        .rollouts
                        .entry(message.run_id.clone())
                        .or_insert_with(|| path.to_path_buf());
                    upsert_message(state.messages.entry(message.run_id.clone()).or_default(), message);
                }
            }
            "plan" => {
                if let Ok(plan) = serde_json::from_value::<Plan>(payload) {
                    state
                        .rollouts
                        .entry(plan.run_id.clone())
                        .or_insert_with(|| path.to_path_buf());
                    state.plans.insert(plan.run_id.clone(), plan);
                }
            }
            "usage" => {
                if let Ok(entry) = serde_json::from_value::<UsageLogEntry>(payload) {
                    state
                        .rollouts
                        .entry(entry.run_id.clone())
                        .or_insert_with(|| path.to_path_buf());
                    state.usage.push(entry);
                }
            }
            _ => {}
        }
    }
}

/// Scans `sessions/` and replays every rollout into `state`.
pub(super) fn load_rollouts(root: &Path, state: &mut StorageState) {
    let mut paths = Vec::new();
    collect_rollouts(&root.join("sessions"), &mut paths);
    paths.sort();
    for path in paths {
        state.rollout_sigs.insert(path.clone(), file_sig(&path));
        replay_rollout(&path, state);
    }
    state.usage.sort_by_key(|entry| entry.id);
}

// ----------------------------------------------------------------------
// Index files
// ----------------------------------------------------------------------

/// `session_index.jsonl`: append-only `{id, title, updated_at}`; the last
/// line per id wins.
pub(super) fn append_session_index(root: &Path, run: &Run) {
    if run.ephemeral {
        return;
    }
    let mut line = json!({
        "id": run.id,
        "title": run.title,
        "updated_at": run.updated_at,
    })
    .to_string();
    line.push('\n');
    append_text(&root.join(SESSION_INDEX_FILE), &line);
}

/// `history.jsonl`: append-only `{session_id, ts, text}`, one per user prompt.
pub(super) fn append_history(root: &Path, session_id: &str, text: &str, ts: u64) {
    // `history.persistence = "none"` in config.toml turns the file off.
    if !crate::config_runtime::current().history_persistence_enabled() {
        return;
    }
    let mut line = json!({ "session_id": session_id, "ts": ts, "text": text }).to_string();
    line.push('\n');
    append_text(&root.join(HISTORY_FILE), &line);
}
