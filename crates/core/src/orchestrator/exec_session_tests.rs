//! QA-012: lines typed into a shell / REPL session are gated like commands;
//! input for other programs is plain data. Real (unsandboxed) piped
//! sessions, answered through the run's permission flow.

use super::agents::tests::{echo_handler, harness, Harness};
use super::*;
use locagens_execpolicy::processes::SpawnOptions;

/// The platform's interactive shell, started as an exec session would be.
const SHELL: &str = if cfg!(windows) { "cmd" } else { "sh" };

/// A program that reads stdin as data (not commands).
const DATA_READER: &str = if cfg!(windows) { "sort" } else { "cat" };

/// Starts an unsandboxed piped session for `command`, tracked for the
/// harness run like `exec_command` does.
fn start_session(h: &Harness, command: &str) -> String {
    let options = SpawnOptions { stdin_pipe: true, ..Default::default() };
    let info = h.state.processes.spawn_with_options(command, h.project.clone(), options);
    h.state.orchestrator.track_exec_session("run-test", &info.id, command);
    info.id
}

/// Answers the next permission prompt with `answer`; resolves to the
/// prompted command text.
fn answer_next_prompt(h: &Harness, answer: &'static str) -> tokio::task::JoinHandle<String> {
    let mut events = h.handle.events.subscribe();
    let handle = h.handle.clone();
    tokio::spawn(async move {
        loop {
            let event = events.recv().await.unwrap();
            if event["type"] == "permission_requested" {
                let sender = handle.permission.lock().await.sender.take().unwrap();
                sender.send(answer.to_string()).unwrap();
                return event["preview"]["command"].as_str().unwrap_or_default().to_string();
            }
        }
    })
}

/// `write_stdin` that must finish without any prompt.
async fn type_unprompted(parent: &AgentLoop, session: &str, chars: &str) -> String {
    let args = json!({ "session_id": session, "chars": chars, "yield_time_ms": 300 });
    tokio::time::timeout(Duration::from_secs(10), parent.write_stdin_tool("c", &args))
        .await
        .expect("write_stdin must not wait for a prompt")
        .unwrap()
}

async fn type_prompted(h: &Harness, parent: &AgentLoop, session: &str, chars: &str, answer: &'static str) -> (String, String) {
    let prompt = answer_next_prompt(h, answer);
    let args = json!({ "session_id": session, "chars": chars, "yield_time_ms": 300 });
    let result = tokio::time::timeout(Duration::from_secs(10), parent.write_stdin_tool("c", &args))
        .await
        .expect("prompt answered")
        .unwrap();
    (prompt.await.unwrap(), result)
}

async fn wait_for_dir(path: &Path) -> bool {
    for _ in 0..50 {
        if path.is_dir() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test]
async fn shell_session_lines_are_gated_like_commands() {
    let h = harness(echo_handler).await;
    let parent = h.parent("accept_edits");
    let session = start_session(&h, SHELL);

    // Known-safe lines run without a prompt.
    let output = type_unprompted(&parent, &session, "echo qa012-safe\n").await;
    assert!(!output.contains("denied"), "{output}");

    // Other lines prompt; a denial sends nothing.
    let (command, result) = type_prompted(&h, &parent, &session, "mkdir qa012-denied\n", "deny").await;
    assert_eq!(command, "mkdir qa012-denied");
    assert!(result.contains("denied"), "{result}");

    // A command typed in pieces is held, then checked whole.
    let held = type_unprompted(&parent, &session, "mkdir qa012-").await;
    assert!(held.contains("input_held"), "{held}");
    let (command, _) = type_prompted(&h, &parent, &session, "split\n", "allow_once").await;
    assert_eq!(command, "mkdir qa012-split");
    assert!(wait_for_dir(&h.project.join("qa012-split")).await, "approved line runs");
    assert!(!h.project.join("qa012-denied").exists(), "denied line never ran");

    // "Allow for Run" covers later lines (as it covers exec_command)…
    type_prompted(&h, &parent, &session, "mkdir qa012-run\n", "allow_run").await;
    assert!(wait_for_dir(&h.project.join("qa012-run")).await);
    type_unprompted(&parent, &session, "mkdir qa012-run2\n").await;
    assert!(wait_for_dir(&h.project.join("qa012-run2")).await);

    // …but never a dangerous one.
    let dangerous = if cfg!(windows) { "rd /s /q qa012-run" } else { "rm -rf qa012-run" };
    let (command, result) = type_prompted(&h, &parent, &session, &format!("{dangerous}\n"), "allow_run").await;
    assert_eq!(command, dangerous);
    assert!(!result.contains("denied"), "{result}");
    let (_, result) = type_prompted(&h, &parent, &session, &format!("{dangerous}2\n"), "deny").await;
    assert!(result.contains("denied"), "a dangerous line prompts again: {result}");

    // Interrupt keys pass without a prompt and drop held text.
    type_unprompted(&parent, &session, "mkdir qa012-dropped").await;
    type_unprompted(&parent, &session, "\u{3}").await;
    assert!(!h.state.orchestrator.has_held_input("run-test", &session));

    h.state.orchestrator.kill_exec_sessions(&h.state.processes, "run-test").await;
}

#[tokio::test]
async fn data_sessions_and_full_access_are_not_gated() {
    let h = harness(echo_handler).await;
    // A program reading data: typed lines are not commands.
    let parent = h.parent("accept_edits");
    let reader = start_session(&h, DATA_READER);
    assert!(!h.state.orchestrator.exec_session_reads_commands("run-test", &reader));
    type_unprompted(&parent, &reader, "mkdir qa012-data\n").await;
    type_unprompted(&parent, &reader, "\u{3}").await;

    // Full Access never prompts, even in a shell.
    let full = h.parent("auto");
    let shell = start_session(&h, SHELL);
    assert!(h.state.orchestrator.exec_session_reads_commands("run-test", &shell));
    type_unprompted(&full, &shell, "mkdir qa012-auto\n").await;
    assert!(wait_for_dir(&h.project.join("qa012-auto")).await);

    h.state.orchestrator.kill_exec_sessions(&h.state.processes, "run-test").await;
}

#[tokio::test]
async fn standing_grants_for_a_typed_line_match_exactly() {
    let h = harness(echo_handler).await;
    let parent = h.parent("accept_edits");
    let session = start_session(&h, SHELL);
    let project = h.project.display().to_string();
    h.state.db.upsert_permission("project", &project, "exec_command", "mkdir qa012-granted", Vec::new(), "allow_project");

    type_unprompted(&parent, &session, "mkdir qa012-granted\n").await;
    assert!(wait_for_dir(&h.project.join("qa012-granted")).await);
    let (command, _) = type_prompted(&h, &parent, &session, "mkdir qa012-granted-other\n", "allow_always").await;
    assert_eq!(command, "mkdir qa012-granted-other");
    // The answer persisted a grant for exactly that line.
    type_unprompted(&parent, &session, "mkdir qa012-granted-other\n").await;

    h.state.orchestrator.kill_exec_sessions(&h.state.processes, "run-test").await;
}
