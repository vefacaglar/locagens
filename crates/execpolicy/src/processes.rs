//! Background process manager — port of `ProcessManager.ts` onto Tokio.
//!
//! Spawns shell commands inside a project directory, streams stdout/stderr into
//! a bounded in-memory log ring, tracks lifecycle state, and kills the whole
//! process tree on shutdown requests (`taskkill /T /F` on Windows, `kill -9`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin};

const MAX_LOGS_PER_PROCESS: usize = 2000;

/// Current UTC time in the same shape as JS `new Date().toISOString()`
/// (same helper as `apps/backend/src/util.rs`).
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// One poll result of an exec session (`exec_command` / `write_stdin`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecPoll {
    pub output: String,
    /// `"running"` or `"exited"`.
    pub status: String,
    pub exit_code: Option<i32>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub id: String,
    pub command: String,
    pub project_path: String,
    pub status: String,
    pub started_at: String,
    pub stopped_at: Option<String>,
    pub exit_code: Option<i32>,
    pub pid: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProcessLogEntry {
    pub timestamp: String,
    pub stream: String,
    pub text: String,
}

struct Managed {
    info: ProcessInfo,
    logs: Vec<ProcessLogEntry>,
    command: String,
    project_path: PathBuf,
    env: HashMap<String, String>,
    /// Interactive stdin pipe (only for `spawn_with_stdin` sessions).
    stdin: Option<ChildStdin>,
    /// Whether this session was spawned with a piped stdin.
    stdin_pipe: bool,
    /// When true the child starts with a cleared environment and only the
    /// (secret-stripped) variables supplied in `env` — used by exec sessions.
    env_clean: bool,
    /// Read cursor into `logs` for incremental `poll_output` calls.
    read_cursor: usize,
}

pub struct ProcessManager {
    processes: Mutex<HashMap<String, Arc<Mutex<Managed>>>>,
    counter: AtomicU32,
}

impl ProcessManager {
    pub fn new() -> Self {
        Self {
            processes: Mutex::new(HashMap::new()),
            counter: AtomicU32::new(1),
        }
    }

    fn lock_processes(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Mutex<Managed>>>> {
        self.processes.lock().expect("process map poisoned")
    }

    fn get_arc(&self, id: &str) -> Option<Arc<Mutex<Managed>>> {
        self.lock_processes().get(id).cloned()
    }

    fn append_log(&self, id: &str, stream: &str, text: &str) {
        if let Some(managed) = self.get_arc(id) {
            let mut m = managed.lock().expect("managed process poisoned");
            m.logs.push(ProcessLogEntry {
                timestamp: now_iso(),
                stream: stream.to_string(),
                text: text.to_string(),
            });
            if m.logs.len() > MAX_LOGS_PER_PROCESS {
                m.logs.remove(0);
            }
        }
    }

    /// Spawns a background process and immediately returns its tracking info.
    pub fn spawn(
        self: &Arc<Self>,
        command: &str,
        project_path: PathBuf,
        env: Option<HashMap<String, String>>,
    ) -> ProcessInfo {
        let id = format!(
            "proc-{}-{}",
            chrono::Utc::now().timestamp_millis(),
            self.counter.fetch_add(1, Ordering::Relaxed)
        );
        let info = ProcessInfo {
            id: id.clone(),
            command: command.trim().to_string(),
            project_path: project_path.to_string_lossy().to_string(),
            status: "running".to_string(),
            started_at: now_iso(),
            stopped_at: None,
            exit_code: None,
            pid: None,
        };

        let managed = Arc::new(Mutex::new(Managed {
            info: info.clone(),
            logs: Vec::new(),
            command: command.trim().to_string(),
            project_path: project_path.clone(),
            env: env.unwrap_or_default(),
            stdin: None,
            stdin_pipe: false,
            env_clean: false,
            read_cursor: 0,
        }));
        self.lock_processes().insert(id.clone(), managed);
        self.start_child(&id);
        self.get(&id).unwrap_or(info)
    }

    /// Like [`spawn`](Self::spawn) but pipes the child's stdin so interactive
    /// exec sessions can feed characters via [`write_stdin`](Self::write_stdin).
    pub fn spawn_with_stdin(
        self: &Arc<Self>,
        command: &str,
        project_path: PathBuf,
        env: Option<HashMap<String, String>>,
    ) -> ProcessInfo {
        let id = format!(
            "proc-{}-{}",
            chrono::Utc::now().timestamp_millis(),
            self.counter.fetch_add(1, Ordering::Relaxed)
        );
        let info = ProcessInfo {
            id: id.clone(),
            command: command.trim().to_string(),
            project_path: project_path.to_string_lossy().to_string(),
            status: "running".to_string(),
            started_at: now_iso(),
            stopped_at: None,
            exit_code: None,
            pid: None,
        };
        let managed = Arc::new(Mutex::new(Managed {
            info: info.clone(),
            logs: Vec::new(),
            command: command.trim().to_string(),
            project_path,
            env: env.unwrap_or_default(),
            stdin: None,
            stdin_pipe: true,
            env_clean: true,
            read_cursor: 0,
        }));
        self.lock_processes().insert(id.clone(), managed);
        self.start_child(&id);
        self.get(&id).unwrap_or(info)
    }

    /// Writes characters to an interactive session's stdin.
    pub async fn write_stdin(&self, id: &str, chars: &str) -> bool {
        let Some(managed) = self.get_arc(id) else {
            return false;
        };
        let mut pipe = {
            let mut m = managed.lock().expect("managed process poisoned");
            m.stdin.take()
        };
        if pipe.is_none() {
            return false;
        }
        let result = match pipe.as_mut() {
            Some(stdin) => {
                let write = stdin.write_all(chars.as_bytes()).await;
                if write.is_ok() {
                    stdin.flush().await.is_ok()
                } else {
                    false
                }
            }
            None => false,
        };
        // Only put the pipe back when the write succeeded (broken pipes are
        // dropped so later writes fail fast).
        if result {
            managed.lock().expect("managed process poisoned").stdin = pipe;
        }
        result
    }

    /// Drains new output for `id` since the last poll and reports the current
    /// status (`running` / `exited` from the exec-session point of view).
    pub fn poll_output(&self, id: &str, max_chars: usize) -> Option<ExecPoll> {
        let managed = self.get_arc(id)?;
        let mut m = managed.lock().expect("managed process poisoned");
        let mut output = String::new();
        for entry in m.logs.iter().skip(m.read_cursor) {
            let text = format!("[{}] {}", entry.stream, entry.text);
            if output.len() + text.len() > max_chars {
                output.push_str("... (output truncated)\n");
                break;
            }
            output.push_str(&text);
        }
        m.read_cursor = m.logs.len();
        let status = if m.info.status == "running" { "running" } else { "exited" };
        Some(ExecPoll {
            output: output.trim_end().to_string(),
            status: status.to_string(),
            exit_code: m.info.exit_code,
        })
    }

    /// Removes a finished session from the registry.
    pub fn drop_session(&self, id: &str) -> bool {
        self.lock_processes().remove(id).is_some()
    }

    /// Starts (or restarts) the child process for an existing entry.
    fn start_child(self: &Arc<Self>, id: &str) {
        let Some(managed) = self.get_arc(id) else {
            return;
        };
        let (command, project_path, env) = {
            let m = managed.lock().expect("managed process poisoned");
            (m.command.clone(), m.project_path.clone(), m.env.clone())
        };

        let mut cmd = if cfg!(windows) {
            let mut c = tokio::process::Command::new("cmd");
            c.arg("/C").arg(&command);
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.arg("-c").arg(&command);
            c
        };
        let stdin_piped = managed.lock().expect("managed process poisoned").stdin_pipe;
        cmd.current_dir(&project_path)
            .env("FORCE_COLOR", "1")
            .stdin(if stdin_piped {
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            })
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(false);
        let env_clean = managed.lock().expect("managed process poisoned").env_clean;
        if env_clean {
            // Exec sessions start from a stripped environment: secrets never
            // reach child processes.
            cmd.env_clear();
        }
        for (key, value) in &env {
            cmd.env(key, value);
        }

        match cmd.spawn() {
            Ok(mut child) => {
                let pid = child.id();
                let stdout = child.stdout.take();
                let stderr = child.stderr.take();
                let stdin = if stdin_piped { child.stdin.take() } else { None };
                {
                    let mut m = managed.lock().expect("managed process poisoned");
                    m.info.pid = pid;
                    m.info.status = "running".to_string();
                    m.info.stopped_at = None;
                    m.info.exit_code = None;
                    m.stdin = stdin;
                }
                self.append_log(
                    id,
                    "system",
                    &format!(
                        "[Process started with PID {}]",
                        pid.map(|p| p.to_string())
                            .unwrap_or_else(|| "unknown".to_string())
                    ),
                );
                if let Some(out) = stdout {
                    self.spawn_reader(id.to_string(), out, "stdout");
                }
                if let Some(err) = stderr {
                    self.spawn_reader(id.to_string(), err, "stderr");
                }
                let this = Arc::clone(self);
                let id_owned = id.to_string();
                tokio::spawn(async move {
                    this.wait_for_exit(&id_owned, child).await;
                });
            }
            Err(err) => {
                let mut m = managed.lock().expect("managed process poisoned");
                m.info.status = "error".to_string();
                m.info.stopped_at = Some(now_iso());
                drop(m);
                self.append_log(id, "system", &format!("[Failed to spawn process: {err}]"));
            }
        }
    }

    /// Streams a child pipe into the log ring until EOF.
    fn spawn_reader(
        self: &Arc<Self>,
        id: String,
        mut stream: impl tokio::io::AsyncRead + Unpin + Send + 'static,
        stream_name: &'static str,
    ) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match stream.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buf[..n]).to_string();
                        this.append_log(&id, stream_name, &text);
                    }
                }
            }
        });
    }

    async fn wait_for_exit(self: Arc<Self>, id: &str, mut child: Child) {
        let status = child.wait().await.ok();
        let code = status.and_then(|s| s.code());
        if let Some(managed) = self.get_arc(id) {
            let mut m = managed.lock().expect("managed process poisoned");
            // A manual kill already marked the process stopped; only record the
            // natural exit when it is still considered running.
            if m.info.status == "running" {
                m.info.status = "stopped".to_string();
                m.info.exit_code = code;
                m.info.stopped_at = Some(now_iso());
                drop(m);
                self.append_log(
                    id,
                    "system",
                    &format!(
                        "[Process exited with code {}]",
                        code.map(|c| c.to_string())
                            .unwrap_or_else(|| "null".to_string())
                    ),
                );
            }
        }
    }

    pub fn list(&self, project_path: Option<&str>) -> Vec<ProcessInfo> {
        let mut list: Vec<ProcessInfo> = self
            .lock_processes()
            .values()
            .filter_map(|managed| {
                let m = managed.lock().expect("managed process poisoned");
                if project_path.map_or(true, |p| m.info.project_path == p) {
                    Some(m.info.clone())
                } else {
                    None
                }
            })
            .collect();
        list.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        list
    }

    pub fn get(&self, id: &str) -> Option<ProcessInfo> {
        self.get_arc(id).map(|managed| {
            managed.lock().expect("managed process poisoned").info.clone()
        })
    }

    pub fn get_logs(&self, id: &str) -> Vec<ProcessLogEntry> {
        self.get_arc(id)
            .map(|managed| {
                managed
                    .lock()
                    .expect("managed process poisoned")
                    .logs
                    .clone()
            })
            .unwrap_or_default()
    }

    /// Kills the whole process tree for `id`. Returns false when there is no
    /// running process to kill.
    pub async fn kill(&self, id: &str) -> bool {
        let Some(managed) = self.get_arc(id) else {
            return false;
        };
        let pid = {
            let m = managed.lock().expect("managed process poisoned");
            if m.info.status != "running" {
                return false;
            }
            m.info.pid
        };
        let Some(pid) = pid else {
            return false;
        };

        {
            let mut m = managed.lock().expect("managed process poisoned");
            m.info.status = "stopped".to_string();
            m.info.stopped_at = Some(now_iso());
            m.info.exit_code = None;
        }

        kill_tree(pid).await;
        self.append_log(id, "system", "[Process terminated]");
        true
    }

    /// Stops (if needed) and restarts the same command in the same project.
    pub async fn restart(self: &Arc<Self>, id: &str) -> Option<ProcessInfo> {
        let Some(managed) = self.get_arc(id) else {
            return None;
        };
        if self.get(id).map(|p| p.status == "running").unwrap_or(false) {
            self.kill(id).await;
        }

        {
            let mut m = managed.lock().expect("managed process poisoned");
            m.info.status = "running".to_string();
            m.info.started_at = now_iso();
            m.info.stopped_at = None;
            m.info.exit_code = None;
            m.info.pid = None;
        }
        self.append_log(id, "system", "[Process restarted]");
        self.start_child(id);
        self.get(id)
    }
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self::new()
    }
}

async fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .output()
            .await;
    }
    #[cfg(not(windows))]
    {
        let _ = tokio::process::Command::new("kill")
            .arg("-9")
            .arg(pid.to_string())
            .stdin(std::process::Stdio::null())
            .output()
            .await;
    }
}
