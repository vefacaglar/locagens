//! Exec sessions (`exec_command` / `write_stdin`) and their per-run tracking.

use super::*;
use locagens_execpolicy::stdin_input;

/// Typed-input state of one exec session (QA-012).
#[derive(Debug, Default)]
pub(super) struct ExecSessionInput {
    /// The session is a shell / REPL: every submitted line is a command
    /// (`stdin_input::reads_commands_from_stdin` of the approved command).
    reads_commands: bool,
    /// Typed text after the last line break, not written yet.
    held: String,
}

impl Orchestrator {
    /// Tracks an exec session started by `run_id` with the approved
    /// `command` (which decides whether typed input is gated).
    pub(super) fn track_exec_session(&self, run_id: &str, session_id: &str, command: &str) {
        let input = ExecSessionInput {
            reads_commands: stdin_input::reads_commands_from_stdin(command),
            held: String::new(),
        };
        lock_ignoring_poison(&self.exec_sessions)
            .entry(run_id.to_string())
            .or_default()
            .insert(session_id.to_string(), input);
    }

    fn with_exec_session<T>(&self, run_id: &str, session_id: &str, f: impl FnOnce(&mut ExecSessionInput) -> T) -> Option<T> {
        lock_ignoring_poison(&self.exec_sessions)
            .get_mut(run_id)
            .and_then(|sessions| sessions.get_mut(session_id))
            .map(f)
    }

    /// Whether typed input to this session is a stream of commands.
    pub(super) fn exec_session_reads_commands(&self, run_id: &str, session_id: &str) -> bool {
        self.with_exec_session(run_id, session_id, |input| input.reads_commands).unwrap_or(false)
    }

    /// Takes the held (unsubmitted) text of a session.
    pub(super) fn take_held_input(&self, run_id: &str, session_id: &str) -> String {
        self.with_exec_session(run_id, session_id, |input| std::mem::take(&mut input.held)).unwrap_or_default()
    }

    pub(super) fn has_held_input(&self, run_id: &str, session_id: &str) -> bool {
        self.with_exec_session(run_id, session_id, |input| !input.held.is_empty()).unwrap_or(false)
    }

    pub(super) fn set_held_input(&self, run_id: &str, session_id: &str, held: String) {
        self.with_exec_session(run_id, session_id, |input| input.held = held);
    }

    pub(super) fn untrack_exec_session(&self, run_id: &str, session_id: &str) {
        let mut sessions = lock_ignoring_poison(&self.exec_sessions);
        if let Some(ids) = sessions.get_mut(run_id) {
            ids.remove(session_id);
            if ids.is_empty() {
                sessions.remove(run_id);
            }
        }
    }

    /// Exec session ids started by a run that are still tracked.
    pub fn exec_sessions(&self, run_id: &str) -> Vec<String> {
        lock_ignoring_poison(&self.exec_sessions)
            .get(run_id)
            .map(|ids| ids.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Kills (process tree / Job Object) and forgets every exec session the
    /// run started. Used on cancel, failure, panic and run deletion.
    pub async fn kill_exec_sessions(&self, processes: &locagens_execpolicy::processes::ProcessManager, run_id: &str) {
        let ids = lock_ignoring_poison(&self.exec_sessions).remove(run_id).unwrap_or_default();
        for id in ids.into_keys() {
            processes.kill(&id).await;
            processes.drop_session(&id);
        }
    }
}

impl AgentLoop {
    /// Runs one exec session (`exec_command`): spawns via the ProcessManager,
    /// waits up to `yield_time_ms` for completion and either returns the
    /// result or hands back an interactive session id.
    pub(super) async fn exec_session(&self, args: &Value) -> Result<String, String> {
        let command = args
            .get("cmd")
            .or_else(|| args.get("command"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if command.is_empty() {
            return Err("exec_command: cmd is empty.".to_string());
        }
        let workdir = match str_arg(args, "workdir") {
            Some(workdir) if !workdir.trim().is_empty() => {
                let path = tools::safe_path(&self.project_path, workdir)?;
                if !path.is_dir() {
                    return Err(format!("exec_command: workdir {workdir:?} is not a directory."));
                }
                path
            }
            _ => self.project_path.clone(),
        };
        let yield_ms = args
            .get("yield_time_ms")
            .and_then(Value::as_u64)
            .unwrap_or(10_000)
            .clamp(500, 60_000);
        let max_chars = args
            .get("max_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(tools::MAX_OUTPUT_CHARS as u64 / 4)
            .clamp(1_000, tools::MAX_OUTPUT_CHARS as u64 / 4) as usize * 4;

        let tty = args.get("tty").and_then(Value::as_bool).unwrap_or(false);
        let shell = str_arg(args, "shell")
            .map(str::trim)
            .filter(|shell| !shell.is_empty())
            .map(str::to_string);
        // The safety checks and grants look only at `cmd`, so `shell` must
        // never name an arbitrary program (FR-030).
        if let Some(shell) = shell.as_deref().filter(|shell| !locagens_execpolicy::processes::is_known_shell(shell)) {
            return Err(format!(
                "exec_command: unknown shell {shell:?}; use sh, bash, zsh, fish, dash or ksh, or omit it for the platform shell."
            ));
        }
        let login =args.get("login").and_then(Value::as_bool).unwrap_or(false);
        // Unknown hosts may prompt only while this call waits (see network_access).
        let (network, _approval_window) = self.command_network_policy(args);
        let options = locagens_execpolicy::processes::SpawnOptions {
            stdin_pipe: true,
            tty,
            env_clean: true,
            shell,
            login,
            env: Some(tools::exec_session_env()),
            sandbox: Some(tools::command_sandbox_spec(
                &self.mode,
                &self.project_path,
                network,
                &network_access::requested_domains(args),
            )),
        };
        let info = self
            .state
            .processes
            .spawn_with_options(&command, workdir, options);
        // Tracked per run so cancel / failure kills it (QA-027).
        self.state.orchestrator.track_exec_session(&self.run_id, &info.id, &command);
        let started = std::time::Instant::now();
        self.wait_exec_session(&info.id, yield_ms).await.map_err(|_| "cancelled".to_string())?;
        let poll = self.state.processes.poll_output(&info.id, max_chars).unwrap_or(
            locagens_execpolicy::processes::ExecPoll { output: String::new(), status: "exited".into(), exit_code: None },
        );
        if poll.status == "exited" {
            self.forget_exec_session(&info.id);
            return Ok(serde_json::json!({
                "exit_code": poll.exit_code.unwrap_or(-1),
                "output": poll.output,
                "wall_time_ms": started.elapsed().as_millis() as u64,
            })
            .to_string());
        }
        Ok(serde_json::json!({
            "session_id": info.id,
            "status": "running",
            "output": poll.output,
        })
        .to_string())
    }

    /// Waits until the session's process exits or `yield_ms` passes, without
    /// consuming output (it is read once afterwards, so nothing is lost).
    /// Checks for cancellation / interrupt every 100 ms (QA-023).
    pub(super) async fn wait_exec_session(&self, session_id: &str, yield_ms: u64) -> Result<(), AgentError> {
        let started = std::time::Instant::now();
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            self.check_cancel()?;
            let running = self.state.processes.is_running(session_id).unwrap_or(false);
            if !running {
                // Let the pipe readers flush the last chunks after the exit.
                tokio::time::sleep(Duration::from_millis(50)).await;
                return Ok(());
            }
            if started.elapsed().as_millis() as u64 >= yield_ms {
                return Ok(());
            }
        }
    }

    /// Drops a finished exec session and stops tracking it for the run.
    pub(super) fn forget_exec_session(&self, session_id: &str) {
        self.state.processes.drop_session(session_id);
        self.state.orchestrator.untrack_exec_session(&self.run_id, session_id);
    }

    /// `write_stdin`: feeds characters to a running exec session and polls
    /// its latest output. Only sessions this run started are reachable.
    /// Lines typed into a shell / REPL session are gated like commands
    /// outside Full Access (QA-012, [`Self::gate_stdin_input`]).
    pub(super) async fn write_stdin_tool(&self, call_id: &str, args: &Value) -> Result<String, AgentError> {
        let session_id = str_arg(args, "session_id").unwrap_or_default().to_string();
        if session_id.is_empty() {
            return Err(AgentError::Failed("write_stdin: session_id is empty.".into()));
        }
        if !self.state.orchestrator.exec_sessions(&self.run_id).contains(&session_id) {
            return Err(AgentError::Failed(format!("write_stdin: unknown session {session_id}.")));
        }
        let mut held = false;
        if let Some(chars) = str_arg(args, "chars") {
            let to_send = match self.gate_stdin_input(call_id, &session_id, chars).await? {
                Ok(to_send) => to_send,
                Err(refusal) => return Ok(refusal),
            };
            held = self.state.orchestrator.has_held_input(&self.run_id, &session_id);
            if !to_send.is_empty() && !self.state.processes.write_stdin(&session_id, &to_send).await {
                return Err(AgentError::Failed(format!(
                    "write_stdin: session {session_id} has no writable stdin (unknown or already exited)."
                )));
            }
        }
        let yield_ms = args
            .get("yield_time_ms")
            .and_then(Value::as_u64)
            .unwrap_or(2_000)
            .clamp(100, 60_000);
        let max_chars = args
            .get("max_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(tools::MAX_OUTPUT_CHARS as u64 / 4)
            .clamp(1_000, tools::MAX_OUTPUT_CHARS as u64 / 4) as usize * 4;
        if self.state.processes.is_running(&session_id).is_none() {
            return Err(AgentError::Failed(format!("write_stdin: unknown session {session_id}.")));
        }
        self.wait_exec_session(&session_id, yield_ms).await?;
        let Some(poll) = self.state.processes.poll_output(&session_id, max_chars) else {
            return Err(AgentError::Failed(format!("write_stdin: unknown session {session_id}.")));
        };
        if poll.status == "exited" {
            self.forget_exec_session(&session_id);
            return Ok(serde_json::json!({
                "session_id": session_id,
                "status": "exited",
                "exit_code": poll.exit_code.unwrap_or(-1),
                "output": poll.output,
            })
            .to_string());
        }
        let mut response = serde_json::json!({
            "session_id": session_id,
            "status": "running",
            "output": poll.output,
        });
        if held {
            response["input_held"] = json!(
                "Text after the last line break was not sent yet: this session reads commands, and a line is sent (and approved) once a later write ends it with a newline."
            );
        }
        Ok(response.to_string())
    }
}

#[cfg(test)]
#[path = "exec_session_tests.rs"]
mod tests;
