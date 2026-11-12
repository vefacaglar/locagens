//! End-to-end `lcgn -p` integration test against a local mock
//! OpenAI-compatible HTTP server (same idea as the `pi_providers` tests, but
//! driving the whole CLI: provider resolution → orchestrator → tools →
//! rendering → exit codes).

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct MockServer {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl MockServer {
    /// Serves queued JSON responses (one per HTTP request, FIFO). When the
    /// queue is empty it falls back to `fallback`.
    fn start(responses: Vec<Value>, fallback: Value) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let queue = Arc::new(Mutex::new(std::collections::VecDeque::from(responses)));
        let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let queue_clone = queue.clone();
        let requests_clone = requests.clone();
        let stop_clone = stop.clone();
        std::thread::spawn(move || {
            listener.set_nonblocking(false).ok();
            for stream in listener.incoming() {
                if stop_clone.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                // Read headers + body (honouring Content-Length).
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 8192];
                let header_end = loop {
                    let read = stream.read(&mut chunk).unwrap_or(0);
                    if read == 0 {
                        break buffer.len();
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                    if let Some(position) =
                        buffer.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        break position + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&buffer[..header_end.min(buffer.len())])
                    .to_lowercase();
                let content_length: usize = headers
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse().ok())
                    })
                    .unwrap_or(0);
                while buffer.len() < header_end + content_length {
                    let read = stream.read(&mut chunk).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                }
                requests_clone
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buffer).to_string());

                let body = {
                    let mut queue = queue_clone.lock().unwrap();
                    queue
                        .pop_front()
                        .unwrap_or_else(|| fallback.clone())
                        .to_string()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });

        Self {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            requests,
        }
    }
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "lcgn-it-{label}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Env {
    data: PathBuf,
    agent: PathBuf,
    project: PathBuf,
}

fn setup_env(label: &str, base_url: &str) -> Env {
    let data = temp_dir(&format!("{label}-data"));
    let agent = temp_dir(&format!("{label}-agent"));
    let project = temp_dir(&format!("{label}-proj"));
    std::fs::write(
        agent.join("auth.json"),
        json!({ "mockprov": { "type": "api_key", "key": "sk-mock-test-key" } }).to_string(),
    )
    .unwrap();
    std::fs::write(
        agent.join("models.json"),
        json!({
            "providers": {
                "mockprov": {
                    "baseUrl": base_url,
                    "api": "openai-compatible",
                    "models": [ { "id": "mock-1" } ]
                }
            }
        })
        .to_string(),
    )
    .unwrap();
    Env { data, agent, project }
}

fn run_lcgn(env: &Env, args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_lcgn"))
        .args(args)
        .current_dir(&env.project)
        .env("PI_CODING_AGENT_DIR", &env.agent)
        .env("LOCAGENS_DATA_DIR", &env.data)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

fn completion(content: &str) -> Value {
    json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "finish_reason": "stop",
            "message": { "role": "assistant", "content": content }
        }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 4, "total_tokens": 14 }
    })
}

#[test]
fn lcgn_print_mode_end_to_end() {
    let server = MockServer::start(Vec::new(), completion("OK"));
    let env = setup_env("print", &server.base_url);

    let (code, stdout, stderr) = run_lcgn(
        &env,
        &[
            "-p",
            "say OK",
            "--model",
            "mockprov/mock-1",
            "--no-session",
        ],
    );
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout.trim(), "OK", "stdout: {stdout} · stderr: {stderr}");
    // Usage/cost summary lands on stderr, not stdout.
    assert!(stderr.contains("tokens:"), "stderr: {stderr}");
    assert!(stdout.lines().all(|line| line.trim() == "OK"));

    // The provider request hit the mock with the task in the messages.
    let requests = server.requests.lock().unwrap().clone();
    assert!(!requests.is_empty());
    assert!(requests[0].contains("say OK"));
    assert!(requests[0].contains("/chat/completions"));
    assert!(requests[0].to_lowercase().contains("authorization: bearer sk-mock-test-key"));

    // Ephemeral run: no session folders persisted.
    let sessions = env.data.join("sessions");
    assert!(
        !sessions.exists() || std::fs::read_dir(&sessions).unwrap().next().is_none(),
        "no session should persist with --no-session"
    );
}

#[test]
fn lcgn_tool_round_trip_persists_session() {
    let tool_call = json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "finish_reason": "tool_calls",
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call-1",
                    "type": "function",
                    "function": { "name": "list_directory", "arguments": "{}" }
                }]
            }
        }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12 }
    });
    let server = MockServer::start(vec![tool_call], completion("listed the files"));
    let env = setup_env("tools", &server.base_url);

    // A file the model should see when it lists the directory.
    std::fs::write(env.project.join("hello.txt"), "hi").unwrap();

    let (code, stdout, stderr) = run_lcgn(
        &env,
        &["-p", "list the files here", "--model", "mockprov/mock-1"],
    );
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("listed the files"), "stdout: {stdout}");

    let requests = server.requests.lock().unwrap().clone();
    assert!(
        requests.len() >= 2,
        "expected two rounds (tool call + final), got {}",
        requests.len()
    );
    // The tool result was fed back into the second request.
    assert!(requests[1].contains("hello.txt") || requests[1].contains("tool"));

    // The session (without --no-session) is persisted as a normal Locagens run.
    let sessions = env.data.join("sessions");
    let count = std::fs::read_dir(&sessions)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert!(count >= 1, "expected a persisted session");
}

#[test]
fn lcgn_unknown_model_exits_2() {
    let server = MockServer::start(Vec::new(), completion("OK"));
    let env = setup_env("unknown", &server.base_url);
    let (code, _stdout, stderr) = run_lcgn(
        &env,
        &["-p", "say OK", "--model", "mockprov/not-a-model"],
    );
    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(stderr.contains("has no model"), "stderr: {stderr}");
}

#[test]
fn lcgn_json_mode_emits_events() {
    let server = MockServer::start(Vec::new(), completion("OK json"));
    let env = setup_env("json", &server.base_url);
    let (code, stdout, stderr) = run_lcgn(
        &env,
        &[
            "-p",
            "say OK json",
            "--model",
            "mockprov/mock-1",
            "--mode",
            "json",
            "--no-session",
        ],
    );
    assert_eq!(code, 0, "stderr: {stderr}");
    let types: Vec<String> = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|event| {
            event
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert!(types.contains(&"run_started".to_string()), "stdout: {stdout}");
    assert!(
        types.contains(&"run_completed".to_string()),
        "stdout: {stdout}"
    );
    // Every line is a JSON object.
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        let parsed: Value = serde_json::from_str(line).expect(line);
        assert!(parsed.get("type").is_some());
    }
}
