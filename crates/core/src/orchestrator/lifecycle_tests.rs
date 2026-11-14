//! Run-lifecycle regression tests (QA-022 … QA-033): one drive per run,
//! cancellation that is awaited and final, panic supervision, exec-session
//! cleanup, the shared permission prompt, and the `spawn_agents` batch on the
//! agent hub. They reuse the mock-LLM harness of `agents/tests.rs`.

use super::agents::tests::{echo_handler, harness, non_system_messages, parse, spawn, text_reply, MockReply};
use super::network_access::NETWORK_ACCESS_TOOL;
use super::*;
use crate::api::runs::{self as runs_api, ContinueRunPayload, CreateRunPayload};
use crate::api::ApiErrorKind;
use std::sync::atomic::AtomicUsize;

fn create_payload(project: &Path, task: &str) -> CreateRunPayload {
    CreateRunPayload {
        task: task.into(),
        project_path: Some(project.display().to_string()),
        provider_id: "mockprov".into(),
        model: "mock-1".into(),
        mode: Some("chat".into()),
        ..Default::default()
    }
}

fn continue_payload(task: &str) -> ContinueRunPayload {
    ContinueRunPayload { task: task.into(), mode: Some("chat".into()), ..Default::default() }
}

async fn wait_for_status(state: &AppState, run_id: &str, status: &str) {
    for _ in 0..200 {
        if state.db.get_run_full(run_id).is_some_and(|run| run.status == status) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let actual = state.db.get_run_full(run_id).map(|run| run.status);
    panic!("run {run_id} never reached {status}; is {actual:?}");
}

/// A db run the handle-level tests can patch statuses on.
fn seed_run(state: &AppState, run_id: &str, status: &str) {
    state.db.create_run(crate::db::Run {
        id: run_id.into(),
        status: status.into(),
        created_at: crate::util::now_iso(),
        updated_at: crate::util::now_iso(),
        ..Default::default()
    });
}

fn prompt(id: &str, name: &str) -> PermissionPrompt {
    PermissionPrompt {
        tool_call: json!({ "id": id, "function": { "name": name, "arguments": "{\"cmd\":\"cargo build\"}" } }),
        preview: None,
        agent_role: None,
        agent_name: None,
        armed: None,
    }
}

async fn pending_tool_name(handle: &RunHandle) -> Option<String> {
    handle
        .pending_permission()
        .await
        .and_then(|pending| pending.pointer("/toolCall/function/name").and_then(Value::as_str).map(str::to_string))
}

async fn wait_for_pending(handle: &RunHandle, name: &str) {
    for _ in 0..100 {
        if pending_tool_name(handle).await.as_deref() == Some(name) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("no pending {name} prompt; pending is {:?}", pending_tool_name(handle).await);
}

/// QA-022: a run that is generating cannot get a second drive loop.
#[tokio::test]
async fn continue_is_rejected_while_the_run_is_generating() {
    let h = harness(|_| MockReply { message: json!({ "role": "assistant", "content": "slow" }), delay_ms: 1_000 }).await;
    let started = runs_api::create_run(&h.state, &create_payload(&h.project, "first")).await.unwrap();
    let run_id = started.run.id.clone();

    let err = runs_api::continue_run(&h.state, &run_id, &continue_payload("second")).await.err().expect("must conflict");
    assert_eq!(err.kind, ApiErrorKind::Conflict);
    assert!(err.message.contains("still generating"), "{}", err.message);

    wait_for_status(&h.state, &run_id, "done").await;
    // Once the turn is over the run continues normally on a fresh handle.
    let continued = runs_api::continue_run(&h.state, &run_id, &continue_payload("second")).await.unwrap();
    assert!(!Arc::ptr_eq(&continued.handle, &started.handle));
    wait_for_status(&h.state, &run_id, "done").await;
    assert_eq!(h.mock.request_count(), 2);
}

/// QA-023: cancel interrupts a stalled provider call, waits for the drive,
/// and the next turn never revives the cancelled loop.
#[tokio::test]
async fn cancel_is_awaited_and_final_for_the_cancelled_drive() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let h = harness(move |_| {
        let first = counter.fetch_add(1, Ordering::SeqCst) == 0;
        MockReply {
            message: json!({ "role": "assistant", "content": "answer" }),
            delay_ms: if first { 20_000 } else { 0 },
        }
    })
    .await;
    let started = runs_api::create_run(&h.state, &create_payload(&h.project, "first")).await.unwrap();
    let run_id = started.run.id.clone();
    while h.mock.request_count() == 0 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let begun = std::time::Instant::now();
    runs_api::cancel_run(&h.state, &run_id).await.unwrap();
    assert!(begun.elapsed() < Duration::from_secs(4), "cancel waited for the stalled stream: {:?}", begun.elapsed());
    assert!(started.handle.is_finished(), "cancel_run returns once the drive has unwound");
    assert!(h.state.orchestrator.handle(&run_id).is_none());
    assert_eq!(h.state.db.get_run_full(&run_id).unwrap().status, "cancelled");

    let continued = runs_api::continue_run(&h.state, &run_id, &continue_payload("second")).await.unwrap();
    wait_for_status(&h.state, &run_id, "done").await;
    assert!(started.handle.is_cancelled(), "the old handle stays cancelled");
    assert!(!continued.handle.is_cancelled());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(h.mock.request_count(), 2, "the cancelled loop made no further calls");
}

/// FR-010: a message queued while the final answer streams starts another
/// round instead of being lost.
#[tokio::test]
async fn a_message_queued_during_the_final_answer_is_answered() {
    let h = harness(|body| {
        let last = non_system_messages(body).last().and_then(|m| m["content"].as_str()).unwrap_or_default().to_string();
        MockReply { message: json!({ "role": "assistant", "content": format!("re: {last}") }), delay_ms: 400 }
    })
    .await;
    let started = runs_api::create_run(&h.state, &create_payload(&h.project, "first")).await.unwrap();
    let run_id = started.run.id.clone();
    while h.mock.request_count() == 0 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    runs_api::queue_message(&h.state, &run_id, "queued later").await.unwrap();
    wait_for_status(&h.state, &run_id, "done").await;
    assert_eq!(h.mock.request_count(), 2, "the queued message got its own round");
    let messages = h.state.db.list_messages(&run_id);
    let texts: Vec<(&str, &str)> = messages.iter().map(|m| (m.role.as_str(), m.content.as_str())).collect();
    assert_eq!(texts, [("user", "first"), ("assistant", "re: first"), ("user", "queued later"), ("assistant", "re: queued later")]);
}

/// QA-024: a panicking drive task ends `failed` and releases its handle.
#[tokio::test]
async fn a_panicking_drive_is_marked_failed_and_its_handle_dropped() {
    let h = harness(echo_handler).await;
    seed_run(&h.state, "run-panic", "generating");
    let handle = h.state.orchestrator.ensure_handle("run-panic");
    let mut events = handle.events.subscribe();
    h.state.orchestrator.supervise(h.state.clone(), handle.clone(), async { panic!("index out of bounds") });

    assert!(handle.wait_finished(Duration::from_secs(5)).await);
    let run = h.state.db.get_run_full("run-panic").unwrap();
    assert_eq!(run.status, "failed");
    assert!(run.error_message.unwrap_or_default().contains("index out of bounds"));
    assert!(h.state.orchestrator.handle("run-panic").is_none());
    let mut saw_failed = false;
    while let Ok(event) = events.try_recv() {
        saw_failed |= event["type"] == "run_failed";
    }
    assert!(saw_failed, "subscribers get run_failed");
}

/// QA-027: exec sessions die with a cancelled turn; a `done` turn keeps them
/// for `write_stdin` next turn, a failed one does not.
#[tokio::test]
async fn exec_sessions_are_killed_when_a_turn_is_cancelled_or_fails() {
    let h = harness(echo_handler).await;
    let command = if cfg!(windows) { "%SystemRoot%\\System32\\ping.exe -n 30 127.0.0.1 > nul" } else { "sleep 30" };
    let info = h.state.processes.spawn(command, h.project.clone(), None);
    h.state.orchestrator.track_exec_session("run-exec", &info.id, command);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(h.state.processes.is_running(&info.id), Some(true), "{:?}", h.state.processes.get_logs(&info.id));

    runs_api::cancel_run(&h.state, "run-exec").await.unwrap();
    assert!(h.state.processes.get(&info.id).is_none(), "the session is killed and dropped");
    assert!(h.state.orchestrator.exec_sessions("run-exec").is_empty());

    for (status, kept) in [("done", true), ("failed", false)] {
        let run_id = format!("run-exec-{status}");
        seed_run(&h.state, &run_id, status);
        h.state.orchestrator.track_exec_session(&run_id, "proc-fake", "");
        let handle = h.state.orchestrator.ensure_handle(&run_id);
        h.state.orchestrator.finish_drive(&h.state, &handle).await;
        assert_eq!(!h.state.orchestrator.exec_sessions(&run_id).is_empty(), kept, "{status}");
    }
}

/// FR-022: shutdown kills the sessions a `done` turn kept.
#[tokio::test]
async fn shutdown_kills_exec_sessions_kept_after_done_turns() {
    let h = harness(echo_handler).await;
    let command = if cfg!(windows) { "%SystemRoot%\\System32\\ping.exe -n 30 127.0.0.1 > nul" } else { "sleep 30" };
    let info = h.state.processes.spawn(command, h.project.clone(), None);
    seed_run(&h.state, "run-kept", "done");
    h.state.orchestrator.track_exec_session("run-kept", &info.id, command);
    runs_api::shutdown(&h.state).await;
    assert!(h.state.processes.get(&info.id).is_none(), "the session is killed and dropped");
    assert!(h.state.orchestrator.exec_sessions("run-kept").is_empty());
}

/// QA-028 / QA-031 / QA-032: network approvals queue behind tool prompts on
/// the one permission slot, each prompt gets its own answer, grants are
/// persisted once (by the asking side), and "Allow for Run" hosts live and
/// die with the drive's handle.
#[tokio::test]
async fn network_and_tool_prompts_share_one_queue() {
    let h = harness(echo_handler).await;
    seed_run(&h.state, "run-test", "generating");
    let handle = h.handle.clone();

    let tool_prompt = {
        let (handle, db) = (handle.clone(), h.state.db.clone());
        tokio::spawn(async move { handle.ask_permission(&db, prompt("call-1", "exec_command")).await })
    };
    wait_for_pending(&handle, "exec_command").await;
    assert_eq!(h.state.db.get_run_full("run-test").unwrap().status, "awaiting_permission");

    let (policy, window) = network_access::command_network_policy(&h.state, &handle, &h.project, &json!({}));
    let decider = policy.decider.clone().unwrap();
    let host = "qa-032.invalid".to_string();
    let network_prompt = tokio::spawn(decider(locagens_execpolicy::network_proxy::NetworkRequest { host: host.clone(), port: 443 }));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(pending_tool_name(&handle).await.as_deref(), Some("exec_command"), "the network prompt waits its turn");
    assert!(!network_prompt.is_finished(), "it is queued, not silently denied");

    // The facade only delivers the answer: no grant rows from it (QA-031).
    assert!(runs_api::apply_permission_decision(&h.state, &handle, "allow_always").await);
    assert_eq!(tool_prompt.await.unwrap().as_deref(), Some("allow_always"));
    assert!(h.state.db.list_permissions().is_empty(), "tool grants are persisted by gated_execute only");
    assert!(!handle.tool_allowed_for_run("exec_command") && !handle.tool_allowed_for_run(NETWORK_ACCESS_TOOL));

    wait_for_pending(&handle, NETWORK_ACCESS_TOOL).await;
    assert!(runs_api::apply_permission_decision(&h.state, &handle, "allow_run").await);
    assert!(network_prompt.await.unwrap());
    assert!(handle.pending_permission().await.is_none());
    assert_eq!(h.state.db.get_run_full("run-test").unwrap().status, "generating");
    // "Allow for Run" approves the host only, never every later tool call.
    assert!(!handle.tool_allowed_for_run("exec_command") && !handle.tool_allowed_for_run(NETWORK_ACCESS_TOOL));
    assert!(network_access::command_allowlist(&h.state, &handle, &h.project, &json!({})).allows(&host));
    let next_drive = RunHandle::new("run-test");
    assert!(!network_access::command_allowlist(&h.state, &next_drive, &h.project, &json!({})).allows(&host));

    // allow_project persists exactly one network_access grant.
    let second = tokio::spawn(decider(locagens_execpolicy::network_proxy::NetworkRequest { host: "qa-031.invalid".into(), port: 443 }));
    wait_for_pending(&handle, NETWORK_ACCESS_TOOL).await;
    runs_api::apply_permission_decision(&h.state, &handle, "allow_project").await;
    assert!(second.await.unwrap());
    let rules = h.state.db.list_permissions();
    assert_eq!(rules.len(), 1, "{rules:?}");
    assert_eq!((rules[0].tool.as_str(), rules[0].network_domains.clone()), (NETWORK_ACCESS_TOOL, vec!["qa-031.invalid".to_string()]));

    // Once the command's window closes, a queued network prompt is skipped.
    drop(window);
    assert!(!decider(locagens_execpolicy::network_proxy::NetworkRequest { host: "late.invalid".into(), port: 80 }).await);
}

/// QA-029: closing a sub-agent that waits on a permission prompt clears the
/// slot and the status, and later prompts still get through.
#[tokio::test]
async fn closing_an_agent_mid_prompt_clears_the_permission_slot() {
    let h = harness(|body| {
        if non_system_messages(body).iter().any(|m| m["role"] == "tool") {
            return text_reply("written");
        }
        MockReply {
            message: json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call-1", "type": "function",
                    "function": { "name": "write_file", "arguments": "{\"path\":\"out.txt\",\"content\":\"hi\"}" }
                }]
            }),
            delay_ms: 0,
        }
    })
    .await;
    seed_run(&h.state, "run-test", "generating");
    let parent = h.parent("ask_permissions");
    spawn(&parent, "builder", "write out.txt", "worker").await;
    wait_for_pending(&h.handle, "write_file").await;
    assert_eq!(h.state.db.get_run_full("run-test").unwrap().status, "awaiting_permission");

    parse(parent.dispatch_agent_tool("close_agent", &json!({ "target": "builder" })).await);
    for _ in 0..50 {
        if h.handle.pending_permission().await.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(h.handle.pending_permission().await.is_none(), "no stale card");
    assert_eq!(h.state.db.get_run_full("run-test").unwrap().status, "generating");
    assert!(!h.project.join("out.txt").exists());

    // The prompt gate was released: the next prompt shows up at once.
    let next = {
        let (handle, db) = (h.handle.clone(), h.state.db.clone());
        tokio::spawn(async move { handle.ask_permission(&db, prompt("call-2", "run_command")).await })
    };
    wait_for_pending(&h.handle, "run_command").await;
    runs_api::apply_permission_decision(&h.state, &h.handle, "deny").await;
    assert_eq!(next.await.unwrap().as_deref(), Some("deny"));
}

/// QA-033: `spawn_agents` runs on the agent hub: it respects the shared
/// concurrency cap, returns every result, and frees its slots afterwards.
#[tokio::test]
async fn spawn_agents_batch_runs_on_the_agent_hub() {
    let h = harness(echo_handler).await;
    let mut parent = h.parent("accept_edits");
    for i in 0..3 {
        spawn(&parent, &format!("busy{i}"), "t", "explorer").await;
    }
    let batch = json!({ "agents": [
        { "role": "explorer", "task": "task one" },
        { "role": "worker", "task": "task two" }
    ] });
    let over = parent.dispatch_tool("c1", "spawn_agents", &batch).await;
    assert!(matches!(over, Err(AgentError::Failed(ref e)) if e.contains("slots")), "{over:?}");

    for i in 0..3 {
        parse(parent.dispatch_agent_tool("close_agent", &json!({ "target": format!("busy{i}") })).await);
    }
    let out = parent.dispatch_tool("c2", "spawn_agents", &batch).await.unwrap();
    assert!(out.contains("## explorer: task one\nn=1 last=task one"), "{out}");
    assert!(out.contains("## worker: task two\nn=1 last=task two"), "{out}");
    assert_eq!(h.handle.agents.free_slots(), agents::MAX_CONCURRENT_AGENTS, "batch agents are closed");
    let tagged = h.state.db.list_messages("run-test");
    assert!(tagged.iter().any(|m| m.agent_role.as_deref() == Some("worker")));
}
