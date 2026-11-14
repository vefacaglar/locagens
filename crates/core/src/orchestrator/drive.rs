//! Run lifecycle: the `Orchestrator` registry, the supervised drive task,
//! endpoint/cost resolution and sub-agent loop construction.

use super::*;
use super::prompts::{sub_agent_system_prompt, system_prompt};

impl Orchestrator {
    pub fn new() -> Self {
        Self { runs: Mutex::new(HashMap::new()), exec_sessions: Mutex::new(HashMap::new()) }
    }

    /// The live handle for a run, if the orchestrator knows about it.
    pub fn handle(&self, run_id: &str) -> Option<Arc<RunHandle>> {
        lock_ignoring_poison(&self.runs).get(run_id).cloned()
    }

    /// Gets or creates the handle for a run.
    pub fn ensure_handle(&self, run_id: &str) -> Arc<RunHandle> {
        let mut runs = lock_ignoring_poison(&self.runs);
        runs.entry(run_id.to_string()).or_insert_with(|| RunHandle::new(run_id)).clone()
    }

    /// Forgets a finished run's handle (its terminal state lives in the Db).
    pub fn drop_handle(&self, run_id: &str) {
        lock_ignoring_poison(&self.runs).remove(run_id);
    }

    /// Forgets `handle` only if it is still the run's current handle, so a
    /// finishing old drive never removes the handle of a newer drive.
    pub(super) fn drop_own_handle(&self, handle: &Arc<RunHandle>) {
        let mut runs = lock_ignoring_poison(&self.runs);
        if runs.get(&handle.run_id).is_some_and(|current| Arc::ptr_eq(current, handle)) {
            runs.remove(&handle.run_id);
        }
    }

    /// Whether a drive of this run is in flight in this process and has not
    /// been cancelled.
    pub fn is_driving(&self, run_id: &str) -> bool {
        self.handle(run_id).is_some_and(|handle| !handle.is_finished() && !handle.is_cancelled())
    }

    /// A fresh handle for the next drive of `run_id`. A previous handle that
    /// is still unwinding (cancelled) is waited for up to `grace`, then its
    /// task is aborted, so two drive loops never share a run (QA-022/023).
    /// Fails when a live, uncancelled drive owns the run.
    pub async fn prepare_drive(&self, run_id: &str, grace: Duration) -> Result<Arc<RunHandle>, String> {
        loop {
            let previous = {
                let mut runs = lock_ignoring_poison(&self.runs);
                match runs.get(run_id) {
                    Some(previous) => previous.clone(),
                    None => {
                        let fresh = RunHandle::new(run_id);
                        fresh.claimed.store(true, Ordering::SeqCst);
                        runs.insert(run_id.to_string(), fresh.clone());
                        return Ok(fresh);
                    }
                }
            };
            if !previous.is_finished() {
                if !previous.is_cancelled() {
                    return Err("Run is still generating; queue the message instead.".to_string());
                }
                self.stop_drive(&previous, grace).await;
            }
            // Swap in a fresh, claimed handle unless another caller already
            // replaced `previous` meanwhile (then re-check that one).
            let mut runs = lock_ignoring_poison(&self.runs);
            let unchanged = runs.get(run_id).is_none_or(|current| Arc::ptr_eq(current, &previous));
            if unchanged {
                let fresh = RunHandle::new(run_id);
                fresh.claimed.store(true, Ordering::SeqCst);
                runs.insert(run_id.to_string(), fresh.clone());
                return Ok(fresh);
            }
        }
    }

    /// Waits for a (cancelled) drive to finish, aborting it after `grace`.
    pub async fn stop_drive(&self, handle: &Arc<RunHandle>, grace: Duration) {
        if handle.wait_finished(grace).await {
            return;
        }
        tracing::warn!("Run {} did not unwind within {grace:?}; aborting its drive task", handle.run_id);
        handle.abort_drive();
        handle.wait_finished(Duration::from_secs(2)).await;
    }

    /// Emits a `status_changed` event on a live handle, if one exists.
    pub fn emit_status(&self, run_id: &str, status: &str) {
        if let Some(handle) = self.handle(run_id) {
            handle.emit(json!({ "type": "status_changed", "status": status }));
        }
    }

    /// Spawns the drive task for a run that already has its initial user
    /// message persisted. A supervisor awaits the task: a panic marks the
    /// run `failed` (QA-024), and every ending (normal, panic, abort) runs
    /// the same cleanup — sub-agents closed, exec sessions killed unless the
    /// turn completed, handle dropped.
    pub fn spawn(self: &Arc<Self>, state: Arc<AppState>, run_id: &str) {
        let handle = self.ensure_handle(run_id);
        let drive = drive_run(state.clone(), handle.clone());
        self.supervise(state, handle, drive);
    }

    /// Runs `drive` as the drive task of `handle` under the supervisor.
    pub(super) fn supervise(
        self: &Arc<Self>,
        state: Arc<AppState>,
        handle: Arc<RunHandle>,
        drive: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        handle.claimed.store(true, Ordering::SeqCst);
        let drive = tokio::spawn(drive);
        *lock_ignoring_poison(&handle.drive_abort) = Some(drive.abort_handle());
        let orchestrator = self.clone();
        tokio::spawn(async move {
            if let Err(err) = drive.await {
                if err.is_panic() {
                    let reason = panic_message(err.into_panic());
                    tracing::error!("Run {} drive task panicked: {reason}", handle.run_id);
                    record_drive_panic(&state, &handle, &reason);
                }
            }
            orchestrator.finish_drive(&state, &handle).await;
        });
    }

    /// Kills every exec session of every run (process exit): sessions kept
    /// after a `done` turn would otherwise outlive the app (FR-022).
    pub async fn kill_all_exec_sessions(&self, processes: &locagens_execpolicy::processes::ProcessManager) {
        let run_ids: Vec<String> = lock_ignoring_poison(&self.exec_sessions).keys().cloned().collect();
        for run_id in run_ids {
            self.kill_exec_sessions(processes, &run_id).await;
        }
    }

    pub(super) async fn finish_drive(&self, state: &AppState, handle: &Arc<RunHandle>) {
        handle.agents.shutdown();
        let completed = state.db.get_run_full(&handle.run_id).is_some_and(|run| run.status == "done");
        if !completed {
            self.kill_exec_sessions(&state.processes, &handle.run_id).await;
        }
        // Normally recorded by the drive; this covers panics and aborts.
        handle.record_turn_end().await;
        self.drop_own_handle(handle);
        handle.mark_finished();
    }
}

/// Hands messages that were queued but never folded into the turn (it was
/// cancelled, failed, or they arrived as it ended) back to the frontends as
/// `queued_messages_undelivered { messages }`, so they can be resent instead
/// of being lost (FR-010).
pub(super) async fn emit_undelivered_messages(handle: &RunHandle) {
    let messages: Vec<String> = {
        let mut inbox = handle.inbox.lock().await;
        // Under the inbox lock: `queue_message` refuses from here on.
        handle.inbox_closed.store(true, Ordering::SeqCst);
        inbox.drain(..).collect()
    };
    if !messages.is_empty() {
        handle.emit(json!({ "type": "queued_messages_undelivered", "runId": handle.run_id, "messages": messages }));
    }
}

/// The text of a task panic payload.
pub(super) fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| text.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Lands `failed` for a run whose drive task panicked (unless it already
/// reached a terminal state).
pub(super) fn record_drive_panic(state: &AppState, handle: &RunHandle, reason: &str) {
    let terminal = state
        .db
        .get_run_full(&handle.run_id)
        .is_some_and(|run| TERMINAL_STATUSES.contains(&run.status.as_str()));
    if terminal {
        return;
    }
    let error = format!("Internal error: the run's agent loop crashed ({reason}).");
    state.db.patch_run(&handle.run_id, |run| {
        run.status = "failed".into();
        run.error_message = Some(error.clone());
    });
    handle.emit(json!({ "type": "status_changed", "status": "failed" }));
    handle.emit(json!({ "type": "run_failed", "errorMessage": error }));
}

// ----------------------------------------------------------------------
// Endpoint & cost resolution
// ----------------------------------------------------------------------

pub(super) fn provider_display_name(providers: &[crate::pi_providers::ProviderMetadata], provider_id: &str) -> String {
    providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .map(|provider| provider.display_name.clone())
        .unwrap_or_else(|| provider_id.to_string())
}

/// Whether the model accepts an OpenAI `reasoning_effort` parameter.
pub(super) fn reasoning_supported(model_id: &str, info: Option<&crate::pi_providers::ModelInfo>) -> bool {
    if let Some(true) = info.and_then(|info| info.reasoning) {
        return true;
    }
    // The model name without a router prefix (`openai/o3-mini` -> `o3-mini`).
    let lower = model_id.to_lowercase();
    let id = lower.rsplit('/').next().unwrap_or_default();
    // `o1` / `o3` / `o4` only as the whole name or its first `-` segment, so
    // ids that merely contain them (`gpt-4o3x`, `yolo1`) do not match (FR-076).
    let o_series = ["o1", "o3", "o4"]
        .iter()
        .any(|marker| id.split('-').next() == Some(*marker));
    o_series
        || ["gpt-5", "glm-5", "deepseek-r", "reasoner", "qwq", "qwen3", "thinking", "gemini-2.5", "gemini-3"]
            .iter()
            .any(|marker| id.contains(marker))
}

/// Whether a base URL points at this machine (`localhost`, `*.localhost`
/// or a loopback address).
pub(super) fn is_local_base_url(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url) else { return false };
    match url.host() {
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost") || name.to_ascii_lowercase().ends_with(".localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Builds a resolved streaming endpoint from the `pi` provider engine.
pub(super) async fn resolve_endpoint(
    pm: &PiProviderManager,
    provider_id: &str,
    model: &str,
    reasoning_effort: Option<&str>,
    session_id: &str,
) -> Result<llm::Endpoint, String> {
    let (mut base_url, mut api) = pm.resolve_endpoint(provider_id);
    if base_url.is_empty() {
        return Err(format!("No baseUrl configured for provider \"{provider_id}\"."));
    }
    let api_key = pm.resolve_api_key(provider_id, None).await.unwrap_or_default();
    // Local servers (Ollama, LM Studio, llama.cpp, vLLM) need no key (FR-041).
    if api_key.is_empty() && provider_id != "ollama" && !is_local_base_url(&base_url) {
        return Err(format!(
            "No API key available for provider \"{provider_id}\". Add one in Settings → Providers."
        ));
    }
    // Per-model overrides (models.json / models-store.json) win over the
    // provider-level defaults — e.g. opencode-go routes each model family to
    // its own inference endpoint.
    let info = pm.model_info(provider_id, model);
    if let Some(model_base) = info.as_ref().and_then(|info| info.base_url.clone()).filter(|url| !url.is_empty()) {
        base_url = model_base;
    }
    if let Some(model_api) = info.as_ref().and_then(|info| info.api.clone()).filter(|api| !api.is_empty()) {
        api = model_api;
    }
    let quirk = llm::ProviderQuirk::detect(provider_id, &base_url, &api_key);
    if quirk == llm::ProviderQuirk::GithubCopilot {
        // The chat endpoint depends on the account: it is part of the token.
        base_url = pm.copilot_base_url(&api_key);
    }
    Ok(llm::Endpoint {
        base_url,
        api,
        api_key,
        session_id: session_id.to_string(),
        model: model.to_string(),
        reasoning_effort: reasoning_effort.filter(|e| !e.is_empty()).map(str::to_string),
        max_tokens: info.as_ref().and_then(|info| info.max_tokens),
        reasoning_supported: reasoning_supported(model, info.as_ref()),
        quirk,
    })
}

/// Computes the cost of one call: the provider's own reported cost wins,
/// then `pi`'s per-million model pricing.
pub(super) fn cost_of(usage: &llm::Usage, info: Option<&crate::pi_providers::ModelInfo>) -> f64 {
    if let Some(provider_cost) = usage.provider_cost.filter(|cost| *cost > 0.0) {
        return provider_cost;
    }
    let Some(cost) = info.and_then(|info| info.cost.as_ref()) else {
        return 0.0;
    };
    let per_million = |keys: &[&str]| -> f64 {
        keys.iter()
            .find_map(|key| cost.get(*key).and_then(Value::as_f64))
            .unwrap_or(0.0)
    };
    let input = per_million(&["inputPerMillion", "input", "promptPerMillion"]);
    let output = per_million(&["outputPerMillion", "output", "completionPerMillion"]);
    let cache_read = per_million(&["cacheReadPerMillion", "cacheRead"]);
    let cache_write = per_million(&["cacheWritePerMillion", "cacheWrite"]);
    // `input_tokens` includes the cached tokens (all protocols): bill only
    // the uncached part at the input rate (FR-014).
    let uncached = (usage.input_tokens - usage.cache_read_tokens - usage.cache_write_tokens).max(0);
    (input * uncached as f64
        + output * usage.output_tokens as f64
        + cache_read * usage.cache_read_tokens as f64
        + cache_write * usage.cache_write_tokens as f64)
        / 1_000_000.0
}

/// Shared HTTP client for streaming calls (see [`llm::shared_http_client`]:
/// connection reuse, connect timeout, per-read idle timeout).
pub(super) fn endpoint_http(_state: &AppState) -> reqwest::Client {
    llm::shared_http_client()
}

/// Builds the loop of one sub-agent (history seeded with its task). The
/// persistent agents in `agents.rs` keep it alive across turns.
pub(super) async fn build_sub_loop(
    spec: SubAgentSpec,
    agent_inbox: Option<mpsc::UnboundedReceiver<agents::AgentCommand>>,
    turn_diff: Arc<TurnDiffTracker>,
    interrupt: Option<Arc<AtomicBool>>,
) -> AgentLoop {
    let providers = spec.state.pi.list_providers().await;
    let project_display = spec.project_path.display().to_string();
    let title = spec.title.clone();
    let task = spec.task.clone();
    let mut system = sub_agent_system_prompt(&spec.role, &title, &task, &project_display, spec.readonly);
    if let Some(instructions) = spec.instructions.as_deref() {
        system.push_str("\nRole instructions:\n");
        system.push_str(instructions);
        system.push('\n');
    }
    AgentLoop {
        state: spec.state,
        handle: spec.handle,
        run_id: spec.run_id,
        project_path: spec.project_path,
        mode: spec.mode,
        provider_display_name: provider_display_name(&providers, &spec.provider_id),
        provider_id: spec.provider_id,
        model: spec.model,
        reasoning_effort: spec.reasoning_effort,
        agent_role: Some(spec.role),
        agent_name: Some(spec.title.clone()),
        history: vec![llm::LlmMessage {
            role: "user".into(),
            content: spec.task.clone(),
            ..Default::default()
        }],
        tools: if spec.readonly { tools::readonly_tool_defs(false) } else { worker_tool_defs() },
        system,
        depth_left: spec.depth_left,
        turn_diff,
        agent_inbox,
        extras: LoopExtras { interrupt, readonly: spec.readonly, ..Default::default() },
    }
}

// ----------------------------------------------------------------------
// Drive task
// ----------------------------------------------------------------------

/// The spawned task that owns one run: seeds history, drives the loop and
/// lands the terminal status + events.
/// Handle cleanup (sub-agents, exec sessions, handle removal) is done by the
/// supervisor in [`Orchestrator::spawn`], so it also runs after a panic.
pub(super) async fn drive_run(state: Arc<AppState>, handle: Arc<RunHandle>) {
    let run_id = handle.run_id.clone();
    if handle.is_cancelled() {
        return;
    }

    let Some(run) = state.db.get_run_full(&run_id) else {
        return;
    };

    state.db.patch_run(&run_id, |run| {
        run.status = "generating".into();
        run.error_message = None;
    });
    handle.emit(json!({ "type": "run_started", "runId": run_id }));
    handle.emit(json!({ "type": "status_changed", "status": "generating" }));

    let mut history = seed_history(&state.db.list_messages(&run_id));
    let providers = state.pi.list_providers().await;
    let mut session_start_context: Option<String> = None;

    let run_full = state.db.get_run_full(&run_id).unwrap_or_else(|| run.clone());
    let tool_names = tool_names_for_mode(&run_full.mode);

    // Live MCP tools (qualified `mcp__<server>__<tool>` defs) are exposed in
    // every non-readonly mode.
    let mut tools = tools::filter_tool_defs(&tool_names);
    if run_full.mode != "chat" && run_full.mode != "plan" && !run_full.project_path.is_empty() {
        tools.extend(crate::services::mcp::discover_tool_defs(Some(&run_full.project_path)).await);
    }
    // Large MCP tool sets are loaded on demand through `tool_search`.
    let (mut tools, deferred_tools) = tool_search::split_deferred(tools);
    agents::apply_project_roles(&mut tools, Path::new(&run_full.project_path));

    // SessionStart lifecycle hook: additional context is appended to the
    // system prompt after the agent is constructed.
    if !run_full.project_path.is_empty() {
        let session_start = crate::services::hooks::run_hook_event(
            "SessionStart",
            Path::new(&run_full.project_path),
            &json!({
                "session_id": run_id,
                "cwd": run_full.project_path,
                "hook_event_name": "SessionStart",
                "source": if history.len() > 1 { "resume" } else { "startup" },
                "model": run_full.model,
                "permission_mode": run_full.mode,
            }),
        )
        .await;
        if let Some(context) = session_start.context_joined() {
            session_start_context = Some(context);
        }
    }

    // Codex-style <environment_context> block as the leading user message.
    if !run_full.project_path.is_empty() && !history.is_empty() {
        let context = env_context::environment_context_for_run(&run_full.mode, &run_full.project_path);
        history.insert(
            0,
            llm::LlmMessage {
                role: "user".into(),
                content: context,
                ..Default::default()
            },
        );
    }

    let mut agent = AgentLoop {
        handle: handle.clone(),
        run_id: run_id.clone(),
        project_path: PathBuf::from(&run_full.project_path),
        mode: run_full.mode.clone(),
        provider_display_name: provider_display_name(&providers, &run_full.provider_id),
        provider_id: run_full.provider_id.clone(),
        model: run_full.model.clone(),
        reasoning_effort: run_full.reasoning_effort.clone(),
        agent_role: None,
        agent_name: None,
        history,
        tools,
        system: system_prompt(&state, &run_full),
        depth_left: 1,
        state: state.clone(),
        turn_diff: Arc::new(TurnDiffTracker::new()),
        agent_inbox: None,
        extras: LoopExtras { deferred_tools, interrupt: None, readonly: false },
    };
    if let Some(context) = session_start_context {
        agent.system.push_str("\n\n");
        agent.system.push_str(&context);
    }

    // `$skill` mentions from the latest user prompt inject the full skill
    // bodies as context (budgeted so a pile of mentions can't blow context).
    let latest_user_prompt = state
        .db
        .list_messages(&run_id)
        .into_iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.content)
        .unwrap_or_else(|| run_full.task.clone());
    let prompt_block = agent.user_prompt_hook(&latest_user_prompt).await;
    let mentioned =
        crate::services::skills::resolve_mentioned_skills(&run_full.project_path, &latest_user_prompt);
    let mut skill_budget: usize = 16 * 1024;
    let mut skill_context = String::new();
    for skill in mentioned {
        let Some(body) = skill.body.as_deref().filter(|body| !body.trim().is_empty()) else {
            continue;
        };
        let take = locagens_protocol::text::floor_char_boundary(body, skill_budget);
        if take == 0 {
            break;
        }
        skill_context.push_str(&format!(
            "\n<skill_context name=\"{}\">\n",
            skill.name
        ));
        skill_context.push_str(&body[..take]);
        if take < body.len() {
            skill_context.push_str("\n[skill body truncated]");
        }
        skill_context.push_str("\n</skill_context>");
        skill_budget -= take;
        if skill_budget == 0 {
            break;
        }
    }
    if !skill_context.is_empty() {
        agent.system.push_str("\n\n## Mentioned skills\n");
        agent.system.push_str(&skill_context);
    }

    // Pre-turn git baseline (best-effort): lets `/undo` restore the exact
    // pre-turn state. Never fails a non-git run.
    if run_full.mode != "plan" && run_full.mode != "chat" && !run_full.project_path.is_empty() {
        let project = PathBuf::from(&run_full.project_path);
        if let Ok(Some(snapshot)) = crate::services::snapshots::create_git_snapshot_for_path(&project, &run_id).await {
            *lock_ignoring_poison(&handle.turn_snapshot) = Some((project, snapshot.id));
        }
    }

    // First turn: name the chat in parallel with the turn.
    let title_task = auto_title::start(&state, &handle, &run_full);
    let result = match prompt_block {
        Some(reason) => Err(AgentError::Failed(format!("Blocked by UserPromptSubmit hook: {reason}"))),
        None => agent.run(MAX_ROUNDS).await,
    };
    if let Some(task) = title_task {
        if result.is_ok() {
            // Usually long done; never hold the turn's end for long.
            let abort = task.abort_handle();
            if tokio::time::timeout(Duration::from_secs(10), task).await.is_err() {
                abort.abort();
            }
        } else {
            task.abort();
        }
    }
    handle.agents.shutdown();
    emit_undelivered_messages(&handle).await;
    // Post-turn tree before the terminal status, so an immediate `/undo`
    // already knows which files this turn changed.
    handle.record_turn_end().await;
    hook_calls::session_end_hook(&run_full.project_path, &run_id, &result).await;

    // Land the terminal state unless `cancel_run` already recorded it.
    // Drives of one run never overlap (`Orchestrator::prepare_drive`), so
    // the run's status here belongs to this drive.
    let current = state.db.get_run_full(&run_id);
    if current.map(|run| TERMINAL_STATUSES.contains(&run.status.as_str())).unwrap_or(false) {
        return;
    }
    match result {
        Ok(final_output) => {
            let final_output = review::finalize_review_run(&state.db, &run_full, &handle, final_output);
            state.db.patch_run(&run_id, |run| {
                run.status = "done".into();
                run.error_message = None;
            });
            handle.emit(json!({ "type": "status_changed", "status": "done" }));
            handle.emit(json!({ "type": "run_completed", "finalOutput": final_output }));
        }
        Err(AgentError::Cancelled) => {
            state.db.patch_run(&run_id, |run| run.status = "cancelled".into());
            handle.emit(json!({ "type": "status_changed", "status": "cancelled" }));
        }
        Err(AgentError::Failed(err)) => {
            state.db.patch_run(&run_id, |run| {
                run.status = "failed".into();
                run.error_message = Some(err.clone());
            });
            handle.emit(json!({ "type": "status_changed", "status": "failed" }));
            handle.emit(json!({ "type": "run_failed", "errorMessage": err }));
        }
    }
}
