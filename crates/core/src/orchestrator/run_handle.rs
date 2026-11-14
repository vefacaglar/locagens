//! `RunHandle` behaviour: cancellation, events, permission prompts and
//! pending-request snapshots of one drive.

use super::*;

impl RunHandle {
    pub(super) fn new(run_id: &str) -> Arc<Self> {
        let (cancelled_tx, _) = watch::channel(false);
        let (finished_tx, _) = watch::channel(false);
        let (events, _) = tokio::sync::broadcast::channel(512);
        Arc::new(Self {
            run_id: run_id.to_string(),
            cancel_flag: Arc::new(AtomicBool::new(false)),
            cancelled_tx,
            events,
            inbox: tokio::sync::Mutex::new(VecDeque::new()),
            inbox_closed: AtomicBool::new(false),
            permission: tokio::sync::Mutex::new(PermissionSlot::default()),
            question: tokio::sync::Mutex::new(QuestionSlot::default()),
            allow_run_tools: Mutex::new(Default::default()),
            permission_gate: tokio::sync::Mutex::new(()),
            next_prompt_id: std::sync::atomic::AtomicU64::new(1),
            agents: agents::AgentHub::default(),
            approved_hosts: Mutex::new(std::collections::HashSet::new()),
            claimed: AtomicBool::new(false),
            finished_tx,
            drive_abort: Mutex::new(None),
            turn_snapshot: Mutex::new(None),
            turn_paths: Mutex::new(Some(Default::default())),
            auto_title: AtomicBool::new(false),
        })
    }

    /// Records the post-turn tree of this turn's undo snapshot (once).
    pub(super) async fn record_turn_end(&self) {
        let Some((project, snapshot_id)) = lock_ignoring_poison(&self.turn_snapshot).take() else { return };
        if let Err(err) = crate::services::snapshots::record_turn_end_for_path(&project, &snapshot_id).await {
            tracing::warn!("Could not record the post-turn snapshot of run {}: {err}", self.run_id);
        }
        // Undo then reverts only what the agent's tools wrote (FR-008).
        if let Some(paths) = lock_ignoring_poison(&self.turn_paths).take() {
            let paths = paths.into_iter().collect();
            if let Err(err) = crate::services::snapshots::record_agent_paths(&project, &snapshot_id, paths) {
                tracing::warn!("Could not record the changed paths of run {}: {err}", self.run_id);
            }
        }
    }

    /// Records the paths a tool call may change: file tools name them,
    /// commands and MCP tools can change anything (the set is dropped).
    pub(super) fn note_tool_paths(&self, project: &Path, name: &str, args: &Value) {
        let mut guard = lock_ignoring_poison(&self.turn_paths);
        let Some(paths) = guard.as_mut() else { return };
        match tool_paths(project, name, args) {
            Some(changed) => paths.extend(changed.iter().map(|path| normalize_relative(project, path))),
            None => *guard = None,
        }
    }
}

/// Paths a tool call changes: `Some(empty)` for read-only tools, `None`
/// when it is unknowable (shell commands, MCP tools).
fn tool_paths(project: &Path, name: &str, args: &Value) -> Option<Vec<String>> {
    let arg = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_string);
    match name {
        "exec_command" | "run_command" | "write_stdin" => None,
        name if name.starts_with("mcp__") => None,
        "apply_patch" => {
            let text = arg("input").or_else(|| arg("patch")).unwrap_or_default();
            let parsed = apply_patch::parse_patch(&text).ok()?;
            Some(apply_patch::affected_paths(project, &parsed).ok()?.all())
        }
        name if tools::MUTATION_TOOLS.contains(&name) => Some(["path", "from", "to"].iter().filter_map(|key| arg(key)).collect()),
        _ => Some(Vec::new()),
    }
}

#[cfg(test)]
#[test]
fn turn_paths_follow_file_tools_and_drop_on_commands() {
    let project = std::env::temp_dir().join("locagens-turn-paths");
    let handle = RunHandle::new("run-paths");
    handle.note_tool_paths(&project, "read_file", &json!({ "path": "x" }));
    handle.note_tool_paths(&project, "write_file", &json!({ "path": "./src\\a.rs" }));
    handle.note_tool_paths(&project, "move_file", &json!({ "from": "old/../b.rs", "to": project.join("c.rs").display().to_string() }));
    let paths: Vec<String> = lock_ignoring_poison(&handle.turn_paths).clone().unwrap().into_iter().collect();
    let expected_a = if cfg!(windows) { "src/a.rs" } else { "src\\a.rs" };
    assert_eq!(paths, vec!["b.rs".to_string(), "c.rs".to_string(), expected_a.to_string()]);
    handle.note_tool_paths(&project, "exec_command", &json!({ "cmd": "make" }));
    assert!(lock_ignoring_poison(&handle.turn_paths).is_none(), "a command can change anything");
}

/// A tool path as a snapshot tree path: relative to `project`, `/`
/// separated, without `./` segments.
fn normalize_relative(project: &Path, path: &str) -> String {
    let path = Path::new(path);
    let relative = path.strip_prefix(project).unwrap_or(path);
    let mut parts: Vec<String> = Vec::new();
    for part in relative.components() {
        match part {
            std::path::Component::Normal(name) => parts.push(name.to_string_lossy().to_string()),
            std::path::Component::ParentDir => {
                parts.pop();
            }
            _ => {}
        }
    }
    parts.join("/")
}

impl RunHandle {

    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }

    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
        self.cancelled_tx.send_replace(true);
    }

    /// Whether this handle's drive has finished (or was never claimed by a
    /// drive: `prepare_drive` / `spawn` claim it).
    pub fn is_finished(&self) -> bool {
        !self.claimed.load(Ordering::SeqCst) || *self.finished_tx.borrow()
    }

    pub(super) fn mark_finished(&self) {
        self.finished_tx.send_replace(true);
    }

    /// Waits up to `timeout` for the drive to finish; `true` when it did.
    pub async fn wait_finished(&self, timeout: Duration) -> bool {
        if self.is_finished() {
            return true;
        }
        let mut rx = self.finished_tx.subscribe();
        tokio::time::timeout(timeout, async {
            while !*rx.borrow_and_update() {
                if rx.changed().await.is_err() {
                    return;
                }
            }
        })
        .await
        .is_ok()
    }

    /// Aborts the drive task; its supervisor still runs the cleanup.
    pub(super) fn abort_drive(&self) {
        if let Some(abort) = lock_ignoring_poison(&self.drive_abort).as_ref() {
            abort.abort();
        }
    }

    /// Shows one permission prompt and waits for the decision. Every prompt
    /// (tool calls of the main agent and sub-agents, network approvals) goes
    /// through here: prompts queue on `permission_gate`, own the slot by id,
    /// and a drop guard clears the slot and restores the status even when
    /// the waiting future is aborted. `None` means cancelled / skipped.
    pub(crate) async fn ask_permission(
        self: &Arc<Self>,
        db: &Arc<crate::db::Db>,
        prompt: PermissionPrompt,
    ) -> Option<String> {
        let gate = tokio::select! {
            gate = self.permission_gate.lock() => gate,
            _ = self.cancelled() => return None,
        };
        if prompt.armed.as_ref().is_some_and(|armed| !armed.load(Ordering::SeqCst)) || self.is_cancelled() {
            return None;
        }
        let prompt_id = self.next_prompt_id.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = oneshot::channel::<String>();
        {
            let mut slot = self.permission.lock().await;
            slot.pending = Some(PendingPermission { tool_call: prompt.tool_call.clone(), preview: prompt.preview.clone() });
            slot.sender = Some(sender);
            slot.prompt_id = prompt_id;
            slot.agent_role = prompt.agent_role.clone();
            slot.agent_name = prompt.agent_name.clone();
        }
        let cleanup = PromptCleanup { handle: self.clone(), db: db.clone(), prompt_id };
        db.patch_run(&self.run_id, |run| run.status = "awaiting_permission".into());
        self.emit(json!({ "type": "status_changed", "status": "awaiting_permission" }));
        self.emit(json!({
            "type": "permission_requested",
            "runId": self.run_id,
            "promptId": prompt_id,
            "toolCall": prompt.tool_call,
            "preview": prompt.preview,
            "agentRole": prompt.agent_role,
            "agentName": prompt.agent_name,
        }));
        let decision = tokio::select! {
            decision = receiver => decision.ok(),
            _ = self.cancelled() => None,
        };
        drop(cleanup);
        drop(gate);
        decision
    }

    /// Completes when cancellation fires (for `tokio::select!`).
    pub async fn cancelled(&self) {
        let mut rx = self.cancelled_tx.subscribe();
        loop {
            if *rx.borrow() {
                return;
            }
            if rx.changed().await.is_err() {
                return;
            }
        }
    }

    pub fn emit(&self, event: Value) {
        // Dropping when no one listens (or the buffer lags) is fine.
        let _ = self.events.send(event);
    }

    /// Records "Allow for Run" for one tool.
    pub fn allow_tool_for_run(&self, tool: &str) {
        self.allow_run_tools.lock().expect("allow_run lock poisoned").insert(tool.to_string());
    }

    /// Whether `tool` was approved with "Allow for Run" earlier in this run.
    pub fn tool_allowed_for_run(&self, tool: &str) -> bool {
        self.allow_run_tools.lock().expect("allow_run lock poisoned").contains(tool)
    }

    /// `{ runId, promptId, toolCall, preview, agentRole, agentName }` of the
    /// pending permission, if any (the same shape as `permission_requested`).
    pub async fn pending_permission(&self) -> Option<Value> {
        let slot = self.permission.lock().await;
        slot.pending.as_ref().map(|pending| {
            json!({
                "runId": self.run_id,
                "promptId": slot.prompt_id,
                "toolCall": pending.tool_call,
                "preview": pending.preview,
                "agentRole": slot.agent_role,
                "agentName": slot.agent_name,
            })
        })
    }

    /// Delivers `decision` to prompt `prompt_id` only while that prompt still
    /// owns the slot, so a late or repeated answer never lands on the next
    /// queued prompt. Returns `false` when the prompt is gone.
    pub async fn answer_permission(&self, prompt_id: u64, decision: &str) -> bool {
        let mut slot = self.permission.lock().await;
        if slot.prompt_id != prompt_id || slot.pending.is_none() {
            return false;
        }
        slot.pending = None;
        let sender = slot.sender.take();
        drop(slot);
        sender.is_some_and(|sender| sender.send(decision.to_string()).is_ok())
    }

    /// `{ runId, questions }` of the pending question, if any.
    pub async fn pending_question(&self) -> Option<Value> {
        let slot = self.question.lock().await;
        slot.pending.as_ref().map(|questions| {
            json!({
                "runId": self.run_id,
                "questions": questions,
            })
        })
    }
}
