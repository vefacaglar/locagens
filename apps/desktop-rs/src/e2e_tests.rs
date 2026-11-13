//! Desktop end-to-end run test (QA-072): the real core and the desktop's own
//! code paths, without a window.
//!
//! A local mock OpenAI-compatible server streams SSE replies. The core is
//! bootstrapped like `main` does (`api::bootstrap`) inside an isolated temp
//! home (env overrides), the `UiStore` comes from the same hook the `App`
//! uses, and a headless Dioxus `VirtualDom` drives the composer's `submit`,
//! the event-stream routing (`attach_event_stream` / `process_event`) and the
//! permission card's `decide`.

use crate::components::{cards, composer};
use crate::state_ui::{self, UiStore};
use dioxus::dioxus_core::{NoOpMutations, ScopeId, VirtualDom};
use dioxus::prelude::*;
use serde_json::{json, Value};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const FINAL_TEXT: &str = "All done.";

thread_local! {
    static STORE: Cell<Option<UiStore>> = const { Cell::new(None) };
}

/// Headless root: builds the store exactly like `App` and hands it out.
fn root() -> Element {
    let store = state_ui::use_ui_store();
    use_context_provider(|| store);
    STORE.with(|slot| slot.set(Some(store)));
    rsx! {}
}

/// Every home- and app-data path the core resolves, pointed into `root`.
fn isolate_environment(root: &Path) -> PathBuf {
    let home = root.join("locagens-home");
    let app = root.join("app-support");
    for dir in [&home, &app] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let vars: [(&str, PathBuf); 13] = [
        ("LOCAGENS_HOME", home.clone()),
        ("LOCAGENS_DATA_DIR", home.clone()),
        ("CODEX_HOME", root.join("codex")),
        ("PI_CODING_AGENT_DIR", root.join("pi-agent")),
        ("APPDATA", app.clone()),
        ("XDG_CONFIG_HOME", app.clone()),
        ("XDG_DATA_HOME", root.join("xdg-data")),
        ("HOME", root.join("user-home")),
        ("LOCAGENS_SETTINGS_PATH", app.join("settings.json")),
        ("LOCAGENS_PROVIDER_CONFIG_PATH", app.join("providers.json")),
        ("LOCAGENS_SKILLS_PATH", app.join("skills")),
        ("LOCAGENS_PLUGINS_PATH", app.join("plugins")),
        ("LOCAGENS_MCP_CONFIG_PATH", app.join("mcp_servers.json")),
    ];
    for (name, value) in vars {
        std::env::set_var(name, value);
    }
    home
}

/// `data: {chunk}` lines of an OpenAI-compatible stream.
fn sse(chunks: &[Value]) -> String {
    let mut body: String = chunks.iter().map(|chunk| format!("data: {chunk}\n\n")).collect();
    body.push_str("data: [DONE]\n\n");
    body
}

/// First turn: a `run_command` tool call. After the tool result: the answer.
fn mock_reply(request: &Value) -> String {
    let has_tool_result = request["messages"]
        .as_array()
        .is_some_and(|messages| messages.iter().any(|message| message["role"] == "tool"));
    if has_tool_result {
        return sse(&[
            json!({ "choices": [{ "index": 0, "delta": { "role": "assistant", "content": "All " } }] }),
            json!({ "choices": [{ "index": 0, "delta": { "content": "done." }, "finish_reason": "stop" }] }),
        ]);
    }
    sse(&[
        json!({ "choices": [{ "index": 0, "delta": { "role": "assistant", "tool_calls": [{
            "index": 0, "id": "call-1", "type": "function",
            "function": { "name": "run_command", "arguments": "{\"command\":\"mkdir made_by_e2e\"}" }
        }] } }] }),
        json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }],
                "usage": { "prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7 } }),
    ])
}

/// Starts the mock server; returns its `/v1` base URL and the request log.
async fn start_mock_server() -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://127.0.0.1:{}/v1", listener.local_addr().unwrap().port());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { return };
            let log = log.clone();
            tokio::spawn(async move {
                let Some(body) = read_request_body(&mut stream).await else { return };
                log.lock().unwrap().push(body.clone());
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n{}",
                    mock_reply(&body)
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (base_url, requests)
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

/// Runs the VirtualDom's tasks (event streams, spawned api calls) until
/// `done` holds; fails after a generous deadline.
async fn pump_until(dom: &mut VirtualDom, what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !dom.in_runtime(&done) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        let _ = tokio::time::timeout(Duration::from_millis(20), dom.wait_for_work()).await;
        dom.render_immediate(&mut NoOpMutations);
    }
}

fn run_status(store: UiStore) -> Option<String> {
    store.active_run.peek().as_ref().map(|run| run.status.clone())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn composer_run_streams_prompts_for_permission_and_finishes() {
    let root_dir = std::env::temp_dir().join(format!(
        "locagens-desktop-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let home = isolate_environment(&root_dir);
    let project = root_dir.join("project");
    std::fs::create_dir_all(&project).unwrap();

    // Provider config pointing at the mock, as Settings would write it.
    let (base_url, requests) = start_mock_server().await;
    std::fs::write(
        home.join("auth.json"),
        json!({ "mockprov": { "type": "api_key", "key": "sk-mock-e2e" } }).to_string(),
    )
    .unwrap();
    std::fs::write(
        home.join("models.json"),
        json!({ "providers": { "mockprov": {
            "baseUrl": base_url, "api": "openai-compatible", "models": [ { "id": "mock-1" } ]
        } } })
        .to_string(),
    )
    .unwrap();

    // Same bootstrap as `main`.
    let state = Arc::new(locagens_core::api::bootstrap().expect("bootstrap"));
    state_ui::set_core(state.clone());

    let mut dom = VirtualDom::new(root);
    dom.rebuild_in_place();
    let store = STORE.with(Cell::get).expect("root rendered the store");

    // The composer's send path, as if the user typed and pressed Enter.
    dom.in_scope(ScopeId::APP, || {
        let mut store = store;
        store.selected_project.set(Some(project.display().to_string()));
        store.selected_provider.set("mockprov".into());
        store.selected_model.set("mock-1".into());
        store.selected_mode.set("accept_edits".into());
        store.prompt.set("make a folder".into());
        composer::submit(&store, Signal::new(false));
    });

    // The streamed tool call surfaces as a permission card in Build mode.
    pump_until(&mut dom, "the permission prompt", || store.pending_permission.peek().is_some()).await;
    let run_id = store.active_run_id.peek().clone().expect("the run is open");
    let request = store.pending_permission.peek().clone().unwrap();
    assert_eq!(request["toolCall"]["function"]["name"], "run_command", "{request}");
    assert_eq!(run_status(store).as_deref(), Some("awaiting_permission"));
    assert!(store.status_toast.peek().is_none(), "no error toast: {:?}", store.status_toast.peek());

    // Answer it through the permission card's handler (deny: nothing runs).
    let decided = dom.in_scope(ScopeId::APP, || {
        let busy = Signal::new(false);
        spawn(cards::decide(store, run_id.clone(), "deny", busy));
        busy
    });
    pump_until(&mut dom, "the run to finish", || {
        run_status(store).as_deref() == Some("done")
            && store.messages.peek().iter().any(|m| m.role == "assistant" && m.content == FINAL_TEXT)
    })
    .await;
    assert!(store.pending_permission.peek().is_none(), "the card is gone");
    assert!(!*decided.peek(), "the card handler finished");

    // The core agrees, and the model saw the denial as the tool result.
    let run = locagens_core::api::runs::get_run(&state, &run_id).await.unwrap();
    assert_eq!(run.status, "done");
    let requests = requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2, "one request per model turn");
    let tool_result = requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool")
        .cloned()
        .unwrap();
    assert_eq!(tool_result["tool_call_id"], "call-1");
    assert!(tool_result["content"].as_str().unwrap().contains("denied"), "{tool_result}");
    assert!(!project.join("made_by_e2e").exists(), "a denied command never runs");

    drop(dom);
    let _ = std::fs::remove_dir_all(&root_dir);
}
