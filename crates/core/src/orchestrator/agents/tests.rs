//! Multi-agent tests: hub logic is exercised directly; the full spawn / wait
//! / follow-up / interrupt / cancel / permission flows run against a local
//! mock OpenAI-compatible server (non-streaming JSON responses).

use super::*;
use crate::orchestrator::llm::LlmMessage;
use crate::orchestrator::{LoopExtras, Orchestrator, TurnDiffTracker};
use crate::AppState;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ----------------------------------------------------------------------
// Pure hub tests
// ----------------------------------------------------------------------

fn new_agent(name: &str) -> (NewAgent, mpsc::UnboundedReceiver<AgentCommand>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (
        NewAgent {
            task_name: name.to_string(),
            role: "explorer".into(),
            nicknames: Vec::new(),
            commands: tx,
            interrupt: Arc::new(AtomicBool::new(false)),
        },
        rx,
    )
}

#[test]
fn task_names_are_sanitized_and_made_unique() {
    assert_eq!(sanitize_task_name("Find the Bug!!"), "find_the_bug");
    assert_eq!(sanitize_task_name("  "), "agent");
    assert_eq!(sanitize_task_name("ünï"), "n");
    assert_eq!(derive_task_name("Scan the whole repository for TODO markers please now"), "scan_the_whole_repository_for");

    let hub = AgentHub::default();
    let first = hub.register(new_agent("scout").0).unwrap();
    let second = hub.register(new_agent("scout").0).unwrap();
    assert_eq!((first.task_name.as_str(), second.task_name.as_str()), ("scout", "scout_2"));
    // A closed agent frees its name.
    hub.close(&first.id);
    assert_eq!(hub.register(new_agent("scout").0).unwrap().task_name, "scout");
}

#[test]
fn concurrency_cap_counts_completed_agents_until_closed() {
    let hub = AgentHub::default();
    let infos: Vec<AgentInfo> =
        (0..MAX_CONCURRENT_AGENTS).map(|i| hub.register(new_agent(&format!("a{i}")).0).unwrap()).collect();
    hub.finish_turn(&infos[0].id, "completed", "done".into());
    let err = hub.register(new_agent("extra").0).unwrap_err();
    assert!(err.contains("close_agent"), "{err}");
    hub.close(&infos[0].id);
    assert!(hub.register(new_agent("extra").0).is_ok());
}

#[test]
fn targets_resolve_by_id_task_name_root_path_and_nickname() {
    let hub = AgentHub::default();
    let (mut new, _rx) = new_agent("auth_review");
    new.nicknames = vec!["Ada".into()];
    let info = hub.register(new).unwrap();
    assert_eq!(info.nickname, "Ada");
    for target in [info.id.as_str(), "auth_review", "/root/auth_review", "ada"] {
        assert_eq!(hub.find(target).unwrap().id, info.id, "{target}");
    }
    assert!(hub.find("nobody").is_none());
    hub.close(&info.id);
    assert!(hub.find("auth_review").is_none());
}

#[test]
fn nicknames_are_not_reused_while_taken() {
    let hub = AgentHub::default();
    let make = |name: &str| {
        let (mut new, rx) = new_agent(name);
        new.nicknames = vec!["Ada".into(), "Linus".into()];
        (new, rx)
    };
    assert_eq!(hub.register(make("a").0).unwrap().nickname, "Ada");
    assert_eq!(hub.register(make("b").0).unwrap().nickname, "Linus");
    assert_eq!(hub.register(make("c").0).unwrap().nickname, "c", "falls back to the task name");
}

#[test]
fn list_filters_by_prefix_and_hides_closed() {
    let hub = AgentHub::default();
    let a = hub.register(new_agent("web_search").0).unwrap();
    hub.register(new_agent("web_fetch").0).unwrap();
    hub.register(new_agent("db_probe").0).unwrap();
    assert_eq!(hub.list(None).len(), 3);
    assert_eq!(hub.list(Some("web")).len(), 2);
    assert_eq!(hub.list(Some("/root/db")).len(), 1);
    hub.close(&a.id);
    assert_eq!(hub.list(Some("web")).len(), 1);
}

#[tokio::test]
async fn wait_returns_pending_updates_immediately_and_consumes_them() {
    let hub = AgentHub::default();
    let info = hub.register(new_agent("scout").0).unwrap();
    hub.finish_turn(&info.id, "completed", "found it".into());
    assert!(hub.has_unread(&info.id));
    let outcome = hub.wait_for_update(Duration::from_secs(5), std::future::pending()).await;
    match outcome {
        WaitOutcome::Updates(updates) => {
            assert_eq!(updates.len(), 1);
            assert_eq!(updates[0].output, "found it");
            assert_eq!(updates[0].task_name, "scout");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(!hub.has_unread(&info.id));
    // Nothing running + nothing pending: do not hang.
    let again = hub.wait_for_update(Duration::from_secs(5), std::future::pending()).await;
    assert_eq!(again, WaitOutcome::NothingToWaitFor);
}

#[tokio::test]
async fn wait_times_out_while_agents_are_still_running() {
    let hub = AgentHub::default();
    hub.register(new_agent("busy").0).unwrap();
    let outcome = hub.wait_for_update(Duration::from_millis(60), std::future::pending()).await;
    assert_eq!(outcome, WaitOutcome::TimedOut);
}

#[tokio::test]
async fn wait_wakes_on_a_later_update_and_on_cancellation() {
    let hub = Arc::new(AgentHub::default());
    let info = hub.register(new_agent("busy").0).unwrap();
    let finisher = hub.clone();
    let id = info.id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        finisher.finish_turn(&id, "failed", "boom".into());
    });
    match hub.wait_for_update(Duration::from_secs(5), std::future::pending()).await {
        WaitOutcome::Updates(updates) => assert_eq!((updates[0].status.as_str(), updates[0].output.as_str()), ("failed", "boom")),
        other => panic!("unexpected {other:?}"),
    }

    hub.set_status(&info.id, "running");
    let outcome = hub.wait_for_update(Duration::from_secs(5), tokio::time::sleep(Duration::from_millis(20))).await;
    assert_eq!(outcome, WaitOutcome::Cancelled);
}

#[test]
fn close_drops_unread_updates_and_blocks_sends() {
    let hub = AgentHub::default();
    let info = hub.register(new_agent("x").0).unwrap();
    hub.finish_turn(&info.id, "completed", "out".into());
    assert_eq!(hub.close(&info.id).as_deref(), Some("completed"));
    assert!(!hub.has_unread(&info.id));
    assert!(hub.send(&info.id, AgentCommand::Task("t".into())).is_err());
    // Late results from a closed agent are ignored.
    hub.finish_turn(&info.id, "completed", "late".into());
    assert!(!hub.has_unread(&info.id));
}

#[test]
fn interrupt_flag_only_raised_for_running_agents() {
    let hub = AgentHub::default();
    let (new, _rx) = new_agent("x");
    let flag = new.interrupt.clone();
    let info = hub.register(new).unwrap();
    hub.finish_turn(&info.id, "completed", String::new());
    assert_eq!(hub.interrupt(&info.id).as_deref(), Some("completed"));
    assert!(!flag.load(Ordering::SeqCst));
    hub.set_status(&info.id, "running");
    assert_eq!(hub.interrupt(&info.id).as_deref(), Some("running"));
    assert!(flag.load(Ordering::SeqCst));
}

#[test]
fn wait_timeout_is_clamped_to_codex_bounds() {
    assert_eq!(clamp_wait_ms(None), Ok(WAIT_DEFAULT_MS));
    assert_eq!(clamp_wait_ms(Some(1)), Ok(WAIT_MIN_MS));
    assert_eq!(clamp_wait_ms(Some(45_000)), Ok(45_000));
    assert!(clamp_wait_ms(Some(WAIT_MAX_MS as i64 + 1)).is_err());
}

#[test]
fn dangling_tool_calls_get_synthetic_results() {
    let call = |id: &str| json!({ "id": id, "function": { "name": "read_file", "arguments": "{}" } });
    let mut history = vec![
        LlmMessage { role: "user".into(), content: "go".into(), ..Default::default() },
        LlmMessage {
            role: "assistant".into(),
            content: String::new(),
            tool_calls: Some(vec![call("c1"), call("c2")]),
            ..Default::default()
        },
        LlmMessage { role: "tool".into(), content: "ok".into(), tool_call_id: Some("c1".into()), ..Default::default() },
    ];
    repair_dangling_tool_calls(&mut history);
    assert_eq!(history.len(), 4);
    assert_eq!(history[3].tool_call_id.as_deref(), Some("c2"));
    // Idempotent.
    repair_dangling_tool_calls(&mut history);
    assert_eq!(history.len(), 4);
}

#[test]
fn tool_specs_cover_the_v2_surface() {
    let defs = tool_defs(&agent_roles::builtin_roles());
    let names: Vec<&str> = defs.iter().filter_map(|d| d.pointer("/function/name").and_then(Value::as_str)).collect();
    assert_eq!(names, AGENT_TOOL_NAMES);
    let spawn = defs[0].to_string();
    assert!(spawn.contains("explorer (read-only)") && spawn.contains("worker (full tools)"));
    assert_eq!(defs[0]["function"]["parameters"]["required"], json!(["task_name", "message"]));
}

#[test]
fn project_roles_reach_the_spawn_spec() {
    let project = std::env::temp_dir().join(format!("locagens-agents-proj-{}", uuid::Uuid::new_v4().simple()));
    let dir = project.join(".locagens").join("agents");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("reviewer.toml"), "description = \"Reviews diffs for bugs\"\nsandbox_mode = \"read-only\"").unwrap();
    // Untrusted folder: its role files are ignored.
    let mut tools = tool_defs(&agent_roles::builtin_roles());
    apply_project_roles(&mut tools, &project);
    assert!(!tools[0].to_string().contains("reviewer"));
    crate::services::project_trust::trust_for_session(&project);
    let mut tools = tool_defs(&agent_roles::builtin_roles());
    apply_project_roles(&mut tools, &project);
    assert!(tools[0].to_string().contains("- reviewer (read-only): Reviews diffs for bugs"));
    let _ = std::fs::remove_dir_all(project);
}

// ----------------------------------------------------------------------
// Mock-LLM flows
// ----------------------------------------------------------------------

struct MockReply {
    message: Value,
    delay_ms: u64,
}

fn text_reply(text: &str) -> MockReply {
    MockReply { message: json!({ "role": "assistant", "content": text }), delay_ms: 0 }
}

fn non_system_messages(body: &Value) -> Vec<Value> {
    body["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|message| message["role"] != "system")
        .collect()
}

struct Mock {
    base_url: String,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl Mock {
    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

async fn start_mock(handler: impl Fn(&Value) -> MockReply + Send + Sync + 'static) -> Mock {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://127.0.0.1:{}/v1", listener.local_addr().unwrap().port());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(handler);
    let seen = requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { return };
            let handler = handler.clone();
            let seen = seen.clone();
            tokio::spawn(async move {
                let Some(body) = read_request_body(&mut stream).await else { return };
                seen.lock().unwrap().push(body.clone());
                let reply = handler(&body);
                if reply.delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(reply.delay_ms)).await;
                }
                let payload = json!({
                    "id": "chatcmpl-mock",
                    "object": "chat.completion",
                    "choices": [{ "index": 0, "finish_reason": "stop", "message": reply.message }],
                    "usage": { "prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7 }
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    Mock { base_url, requests }
}

async fn read_request_body(stream: &mut tokio::net::TcpStream) -> Option<Value> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]).to_lowercase();
    let length: usize = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:").and_then(|value| value.trim().parse().ok()))
        .unwrap_or(0);
    while buffer.len() < header_end + length {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    serde_json::from_slice(&buffer[header_end..]).ok()
}

struct Harness {
    state: Arc<AppState>,
    handle: Arc<RunHandle>,
    project: PathBuf,
    mock: Mock,
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("locagens-agents-{label}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn harness(handler: impl Fn(&Value) -> MockReply + Send + Sync + 'static) -> Harness {
    let mock = start_mock(handler).await;
    let agent_dir = temp_dir("agentdir");
    std::fs::write(
        agent_dir.join("auth.json"),
        json!({ "mockprov": { "type": "api_key", "key": "sk-mock-test-key" } }).to_string(),
    )
    .unwrap();
    std::fs::write(
        agent_dir.join("models.json"),
        json!({ "providers": { "mockprov": {
            "baseUrl": mock.base_url, "api": "openai-compatible", "models": [ { "id": "mock-1" } ]
        } } })
        .to_string(),
    )
    .unwrap();
    let data = temp_dir("data");
    let state = Arc::new(AppState {
        db: Arc::new(crate::db::Db::open(&data).unwrap()),
        pi: Arc::new(crate::pi_providers::PiProviderManager::with_dirs(agent_dir, data.clone())),
        orchestrator: Arc::new(Orchestrator::new()),
        settings_path: data.join("settings.json"),
        default_project_path: data.clone(),
        processes: Arc::new(crate::services::processes::ProcessManager::new()),
    });
    Harness { state, handle: RunHandle::new("run-test"), project: temp_dir("project"), mock }
}

impl Harness {
    fn parent(&self, mode: &str) -> AgentLoop {
        AgentLoop {
            state: self.state.clone(),
            handle: self.handle.clone(),
            run_id: "run-test".into(),
            project_path: self.project.clone(),
            mode: mode.into(),
            provider_id: "mockprov".into(),
            provider_display_name: "Mock".into(),
            model: "mock-1".into(),
            reasoning_effort: None,
            agent_role: None,
            agent_name: None,
            history: Vec::new(),
            // The main agent's real toolset for the mode: dispatch refuses
            // tools that were never offered.
            tools: crate::orchestrator::tools::filter_tool_defs(&crate::orchestrator::tool_names_for_mode(mode)),
            system: String::new(),
            depth_left: 1,
            turn_diff: Arc::new(TurnDiffTracker::new()),
            agent_inbox: None,
            extras: LoopExtras::default(),
        }
    }
}

fn parse(result: Result<String, AgentError>) -> Value {
    match result {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|_| json!({ "text": text })),
        Err(AgentError::Failed(error)) => panic!("tool failed: {error}"),
        Err(AgentError::Cancelled) => panic!("tool cancelled"),
    }
}

async fn wait_one(parent: &AgentLoop) -> Value {
    parse(parent.wait_agent_tool(Duration::from_secs(10)).await)
}

async fn spawn(parent: &AgentLoop, name: &str, message: &str, role: &str) -> Value {
    parse(parent.dispatch_agent_tool("spawn_agent", &json!({ "task_name": name, "message": message, "agent_type": role })).await)
}

fn echo_handler(body: &Value) -> MockReply {
    let messages = non_system_messages(body);
    let last = messages.last().and_then(|m| m["content"].as_str()).unwrap_or_default().to_string();
    text_reply(&format!("n={} last={last}", messages.len()))
}

#[tokio::test]
async fn spawn_wait_returns_output_and_tags_messages() {
    let harness = harness(echo_handler).await;
    let parent = harness.parent("accept_edits");
    let spawned = spawn(&parent, "Scout One", "map the repo", "explorer").await;
    assert_eq!(spawned["task_name"], "scout_one");
    assert_eq!(spawned["role"], "explorer");

    let waited = wait_one(&parent).await;
    assert_eq!(waited["timed_out"], false);
    let update = &waited["updates"][0];
    assert_eq!(update["status"], "completed");
    assert_eq!(update["task_name"], "scout_one");
    assert_eq!(update["output"], "n=1 last=map the repo");

    // The child's assistant message is persisted with the agentRole/agentName contract.
    let messages = harness.state.db.list_messages("run-test");
    let tagged: Vec<_> = messages.iter().filter(|m| m.role == "assistant").collect();
    assert_eq!(tagged.len(), 1);
    assert_eq!(tagged[0].agent_role.as_deref(), Some("explorer"));
    assert_eq!(tagged[0].agent_name.as_deref(), Some("scout_one"));
}

#[tokio::test]
async fn followup_keeps_conversation_while_send_message_starts_no_turn() {
    let harness = harness(echo_handler).await;
    let parent = harness.parent("accept_edits");
    spawn(&parent, "scout", "first", "explorer").await;
    wait_one(&parent).await;
    assert_eq!(harness.mock.request_count(), 1);

    // send_message: queued, no new turn (no new LLM request, status stays idle).
    let sent = parse(parent.dispatch_agent_tool("send_message", &json!({ "target": "scout", "message": "fyi" })).await);
    assert!(sent["note"].as_str().unwrap().contains("no new turn"));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(harness.mock.request_count(), 1);
    assert_eq!(parse(parent.dispatch_agent_tool("list_agents", &json!({})).await)["agents"][0]["status"], "completed");

    // followup_task: starts a turn and sees the whole history: user, assistant, fyi, task.
    parse(parent.dispatch_agent_tool("followup_task", &json!({ "target": "scout", "message": "second" })).await);
    let waited = wait_one(&parent).await;
    assert_eq!(waited["updates"][0]["output"], "n=4 last=second");
    assert_eq!(harness.mock.request_count(), 2);
}

#[tokio::test]
async fn concurrent_agents_run_in_parallel_and_report_independently() {
    let harness = harness(|body| {
        let task = non_system_messages(body)[0]["content"].as_str().unwrap_or_default().to_string();
        MockReply { message: json!({ "role": "assistant", "content": format!("done {task}") }), delay_ms: 300 }
    })
    .await;
    let parent = harness.parent("accept_edits");
    let started = std::time::Instant::now();
    for name in ["a", "b", "c"] {
        spawn(&parent, name, &format!("task-{name}"), "explorer").await;
    }
    let mut outputs = Vec::new();
    while outputs.len() < 3 {
        let waited = wait_one(&parent).await;
        for update in waited["updates"].as_array().unwrap() {
            outputs.push(update["output"].as_str().unwrap().to_string());
        }
    }
    outputs.sort();
    assert_eq!(outputs, vec!["done task-a", "done task-b", "done task-c"]);
    assert!(started.elapsed() < Duration::from_millis(850), "agents ran serially: {:?}", started.elapsed());
}

#[tokio::test]
async fn spawn_validates_role_cap_and_depth() {
    let harness = harness(echo_handler).await;
    let parent = harness.parent("accept_edits");

    let bad_role = parent.dispatch_agent_tool("spawn_agent", &json!({ "task_name": "x", "message": "m", "agent_type": "wizard" })).await;
    assert!(matches!(bad_role, Err(AgentError::Failed(ref e)) if e.contains("Available roles")));
    let empty = parent.dispatch_agent_tool("spawn_agent", &json!({ "task_name": "x", "message": "  " })).await;
    assert!(matches!(empty, Err(AgentError::Failed(_))));

    for i in 0..MAX_CONCURRENT_AGENTS {
        spawn(&parent, &format!("a{i}"), "t", "explorer").await;
    }
    let over = parent.dispatch_agent_tool("spawn_agent", &json!({ "task_name": "x", "message": "m" })).await;
    assert!(matches!(over, Err(AgentError::Failed(ref e)) if e.contains("Agent limit")));

    parse(parent.dispatch_agent_tool("close_agent", &json!({ "target": "a0" })).await);
    spawn(&parent, "again", "t", "explorer").await;

    // Children (depth 0) have no agent tools at all.
    let mut child = harness.parent("accept_edits");
    child.depth_left = 0;
    let err = child.dispatch_tool("c1", "spawn_agent", &json!({ "task_name": "z", "message": "m" })).await;
    assert!(matches!(err, Err(AgentError::Failed(ref e)) if e.contains("Unknown tool")));
    harness.handle.agents.shutdown();
}

#[tokio::test]
async fn plan_mode_forces_workers_read_only() {
    let harness = harness(echo_handler).await;
    let parent = harness.parent("plan");
    let roles = agent_roles::load_roles(&harness.project);
    assert!(agent_roles::resolve_role(&roles, Some("worker"), "plan").unwrap().readonly);
    // The spawn itself succeeds; the read-only toolset is what the loop gets.
    let spawned = spawn(&parent, "w", "t", "worker").await;
    assert_eq!(spawned["role"], "worker");
    wait_one(&parent).await;
}

#[tokio::test]
async fn interrupt_ends_the_turn_but_keeps_the_agent_usable() {
    let slow_first = Arc::new(AtomicBool::new(true));
    let flag = slow_first.clone();
    let harness = harness(move |_body| {
        let delay = if flag.swap(false, Ordering::SeqCst) { 5_000 } else { 0 };
        MockReply { message: json!({ "role": "assistant", "content": "ok" }), delay_ms: delay }
    })
    .await;
    let parent = harness.parent("accept_edits");
    spawn(&parent, "slowpoke", "think hard", "explorer").await;
    tokio::time::sleep(Duration::from_millis(150)).await;

    let started = std::time::Instant::now();
    let interrupted = parse(parent.dispatch_agent_tool("interrupt_agent", &json!({ "target": "slowpoke" })).await);
    assert_eq!(interrupted["previous_status"], "running");
    let waited = wait_one(&parent).await;
    assert_eq!(waited["updates"][0]["status"], "interrupted");
    assert!(started.elapsed() < Duration::from_secs(3), "interrupt must not wait for the slow reply");

    // Still alive: a follow-up runs a fresh turn.
    parse(parent.dispatch_agent_tool("followup_task", &json!({ "target": "slowpoke", "message": "retry" })).await);
    let waited = wait_one(&parent).await;
    assert_eq!(waited["updates"][0]["status"], "completed");
    assert_eq!(waited["updates"][0]["output"], "ok");
}

#[tokio::test]
async fn close_agent_frees_the_slot_and_discards_its_results() {
    let harness = harness(echo_handler).await;
    let parent = harness.parent("accept_edits");
    spawn(&parent, "temp", "t", "explorer").await;
    let closed = parse(parent.dispatch_agent_tool("close_agent", &json!({ "target": "temp" })).await);
    assert!(["running", "completed"].contains(&closed["previous_status"].as_str().unwrap()));
    let listed = parse(parent.dispatch_agent_tool("list_agents", &json!({})).await);
    assert_eq!(listed["agents"].as_array().unwrap().len(), 0);
    let err = parent.dispatch_agent_tool("followup_task", &json!({ "target": "temp", "message": "x" })).await;
    assert!(matches!(err, Err(AgentError::Failed(ref e)) if e.contains("no live agent")));
    let waited = wait_one(&parent).await;
    assert!(waited["updates"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn parent_cancellation_cancels_running_and_idle_children() {
    let harness = harness(|_| MockReply { message: json!({ "role": "assistant", "content": "late" }), delay_ms: 5_000 }).await;
    let parent = harness.parent("accept_edits");
    spawn(&parent, "slow", "t", "explorer").await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // A parent blocked in wait_agent is released by cancellation too.
    let handle = harness.handle.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        handle.cancel();
    });
    let started = std::time::Instant::now();
    let waited = parent.wait_agent_tool(Duration::from_secs(30)).await;
    assert!(matches!(waited, Err(AgentError::Cancelled)));
    assert!(started.elapsed() < Duration::from_secs(2));

    // The in-flight child turn is dropped without waiting for the 5s reply.
    for _ in 0..40 {
        if harness.handle.agents.list(None)[0].status == "cancelled" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("child never observed the parent's cancellation");
}

#[tokio::test]
async fn child_permission_requests_surface_through_the_parent_flow() {
    let harness = harness(|body| {
        let has_tool_result = non_system_messages(body).iter().any(|m| m["role"] == "tool");
        if has_tool_result {
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
    let parent = harness.parent("ask_permissions");
    let mut events = harness.handle.events.subscribe();
    spawn(&parent, "builder", "write out.txt", "worker").await;

    // The child's request is announced on the parent run's event stream, tagged with the agent.
    let event = loop {
        let event = tokio::time::timeout(Duration::from_secs(10), events.recv()).await.unwrap().unwrap();
        if event["type"] == "permission_requested" {
            break event;
        }
    };
    assert_eq!(event["agentName"], "builder");
    assert_eq!(event["agentRole"], "worker");
    assert_eq!(event["toolCall"]["function"]["name"], "write_file");
    assert!(harness.handle.pending_permission().await.is_some());

    // Answering through the run's normal permission slot unblocks the child.
    let sender = harness.handle.permission.lock().await.sender.take().unwrap();
    sender.send("allow_once".to_string()).unwrap();
    let waited = wait_one(&parent).await;
    assert_eq!(waited["updates"][0]["output"], "written");
    assert_eq!(std::fs::read_to_string(harness.project.join("out.txt")).unwrap(), "hi");
}

// ----------------------------------------------------------------------
// Other new tools through the real dispatcher
// ----------------------------------------------------------------------

#[tokio::test]
async fn tool_search_dispatch_adds_specs_to_the_next_request_tools() {
    let harness = harness(echo_handler).await;
    let mut parent = harness.parent("accept_edits");
    let def = |name: &str, description: &str| {
        json!({ "type": "function", "function": { "name": name, "description": description, "parameters": { "type": "object", "properties": {} } } })
    };
    parent.extras.deferred_tools = vec![
        def("mcp__github__create_issue", "Create an issue"),
        def("mcp__slack__post_message", "Post a Slack message"),
    ];
    parent.tools.clear();
    let out = parent.dispatch_tool("c1", "tool_search", &json!({ "query": "slack message" })).await.unwrap();
    assert!(out.contains("mcp__slack__post_message"));
    let names: Vec<&str> = parent.tools.iter().filter_map(|t| t.pointer("/function/name").and_then(Value::as_str)).collect();
    assert_eq!(names, vec!["mcp__slack__post_message"]);
}

#[tokio::test]
async fn dispatch_refuses_tools_that_were_not_offered() {
    let harness = harness(echo_handler).await;
    // A read-only explorer offered only read tools cannot write in Build mode.
    let mut explorer = harness.parent("accept_edits");
    explorer.tools = crate::orchestrator::tools::readonly_tool_defs(false);
    explorer.extras.readonly = true;
    let err = explorer.dispatch_tool("c1", "write_file", &json!({ "path": "x.txt", "content": "pwned" })).await;
    assert!(matches!(err, Err(AgentError::Failed(ref e)) if e.contains("not available")), "{err:?}");
    assert!(!harness.project.join("x.txt").exists());
    // Even if a mutation tool were offered, a read-only agent may not run it.
    explorer.tools = crate::orchestrator::tools::filter_tool_defs(&["read_file", "write_file"]);
    let refused = explorer.dispatch_tool("c2", "write_file", &json!({ "path": "x.txt", "content": "pwned" })).await.unwrap();
    assert!(refused.contains("read-only agent"), "{refused}");
    assert!(!harness.project.join("x.txt").exists());

    // Chat mode does not offer update_plan / fetch_url, so they never dispatch.
    let mut chat = harness.parent("chat");
    for tool in ["update_plan", "fetch_url", "write_file", "exec_command"] {
        let err = chat.dispatch_tool("c3", tool, &json!({})).await;
        assert!(matches!(err, Err(AgentError::Failed(ref e)) if e.contains("not available")), "{tool}: {err:?}");
    }
}

#[tokio::test]
async fn write_stdin_only_reaches_sessions_of_the_same_run() {
    let harness = harness(echo_handler).await;
    let mut parent = harness.parent("auto");
    let err = parent.dispatch_tool("c1", "write_stdin", &json!({ "session_id": "proc-1-1", "chars": "x\n" })).await;
    assert!(matches!(err, Err(AgentError::Failed(ref e)) if e.contains("unknown session")), "{err:?}");
}

#[tokio::test]
async fn protected_paths_always_prompt_in_build_mode_and_grants_do_not_cover_them() {
    let harness = harness(echo_handler).await;
    let parent = harness.parent("accept_edits");
    // A standing grant and "Allow for Run" for write_file exist…
    harness.state.db.upsert_permission("global", "", "write_file", "", Vec::new(), "allow_always");
    harness.handle.allow_tool_for_run("write_file");
    // …ordinary files are written without a prompt…
    parent.gated_execute("c0", "write_file", &json!({ "path": "ok.txt", "content": "fine" })).await.unwrap();
    assert!(harness.project.join("ok.txt").exists());

    // …but `.locagens/hooks.json` pauses for an explicit decision.
    let mut events = harness.handle.events.subscribe();
    let handle = harness.handle.clone();
    let answer = tokio::spawn(async move {
        loop {
            let event = events.recv().await.unwrap();
            if event["type"] == "permission_requested" {
                let sender = handle.permission.lock().await.sender.take().unwrap();
                sender.send("Deny".to_string()).unwrap();
                return event;
            }
        }
    });
    let hooks = json!({ "path": ".locagens/hooks.json", "content": "{\"SessionStart\":\"evil\"}" });
    let result = parent.gated_execute("c1", "write_file", &hooks).await.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(10), answer).await.unwrap().unwrap();
    assert_eq!(event["preview"]["protectedPath"], ".locagens/hooks.json");
    assert!(result.contains("denied"), "an unknown answer (\"Deny\") denies: {result}");
    assert!(!harness.project.join(".locagens").join("hooks.json").exists());

    // apply_patch into .git/hooks is gated the same way.
    let patch = json!({ "input": "*** Begin Patch\n*** Add File: .git/hooks/pre-commit\n+evil\n*** End Patch" });
    assert_eq!(
        crate::orchestrator::tools::protected_mutation_target(&harness.project, "apply_patch", &patch).as_deref(),
        Some(".git/hooks/pre-commit")
    );
    // Full Access does not prompt (commands are unsandboxed there anyway).
    let auto = harness.parent("auto");
    auto.gated_execute("c2", "write_file", &hooks).await.unwrap();
    assert!(harness.project.join(".locagens").join("hooks.json").exists());
}

#[tokio::test]
async fn dangerous_commands_prompt_even_with_allow_run() {
    let harness = harness(echo_handler).await;
    let parent = harness.parent("accept_edits");
    harness.handle.allow_tool_for_run("run_command");
    let mut events = harness.handle.events.subscribe();
    let handle = harness.handle.clone();
    let answer = tokio::spawn(async move {
        loop {
            let event = events.recv().await.unwrap();
            if event["type"] == "permission_requested" {
                handle.permission.lock().await.sender.take().unwrap().send("deny".to_string()).unwrap();
                return event;
            }
        }
    });
    let result = parent.gated_execute("c1", "run_command", &json!({ "command": "git push --force origin main" })).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), answer).await.unwrap().unwrap();
    assert!(result.contains("denied"), "{result}");
}

#[tokio::test]
async fn context_remaining_uses_history_and_the_catalog_window() {
    let harness = harness(echo_handler).await;
    let mut parent = harness.parent("accept_edits");
    parent.history.push(LlmMessage { role: "user".into(), content: "x".repeat(8000), ..Default::default() });
    let out: Value = serde_json::from_str(&parent.dispatch_tool("c1", "get_context_remaining", &json!({})).await.unwrap()).unwrap();
    assert_eq!(out["estimated_tokens_used"], 2000);
    // The mock model declares no contextWindow: falls back to the compaction threshold.
    assert!(out["context_window"].is_null());
    assert_eq!(out["tokens_left"], 88_000);
}

#[tokio::test]
async fn host_executable_dispatch_reports_missing_binaries_and_bad_names() {
    let harness = harness(echo_handler).await;
    let mut parent = harness.parent("chat");
    let out = parent.dispatch_tool("c1", "host_executable", &json!({ "name": "definitely-not-installed-xyz" })).await.unwrap();
    let parsed: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(parsed["found"], false);
    let err = parent.dispatch_tool("c2", "host_executable", &json!({ "name": "" })).await;
    assert!(matches!(err, Err(AgentError::Failed(_))));
}
