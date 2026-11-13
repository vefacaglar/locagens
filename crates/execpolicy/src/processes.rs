//! Background process manager — port of `ProcessManager.ts` onto Tokio.
//!
//! Spawns shell commands inside a project directory, streams stdout/stderr into
//! a bounded in-memory log ring, tracks lifecycle state, and kills the whole
//! process tree on shutdown requests (`taskkill /T /F` on Windows, `SIGKILL`
//! on the process group on Unix). Interactive exec sessions can run piped or
//! on a real pseudo-terminal (PTY) so TTY-aware programs behave.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin};

/// Options controlling how a child process is spawned.
#[derive(Clone, Debug, Default)]
pub struct SpawnOptions {
    /// Pipe the child's stdin so interactive sessions can feed characters.
    pub stdin_pipe: bool,
    /// Allocate a pseudo-terminal (Unix) instead of separate pipes.
    pub tty: bool,
    /// Start the child with a cleared environment (secret-stripped exec
    /// sessions).
    pub env_clean: bool,
    /// Shell program to use (defaults to the platform shell).
    pub shell: Option<String>,
    /// Run the shell as a login shell (`-lc`).
    pub login: bool,
    /// Extra environment variables for the child.
    pub env: Option<HashMap<String, String>>,
    /// Native sandbox to run the child in (Seatbelt / Bubblewrap / Windows
    /// restricted token + network proxy).
    pub sandbox: Option<crate::sandbox::SandboxSpec>,
}

/// The writable stdin end of a managed child.
enum StdinSink {
    /// A plain piped stdin (`ChildStdin`).
    Pipe(ChildStdin),
    /// A PTY master or Windows sandbox pipe, written by a dedicated writer
    /// thread.
    Channel(tokio::sync::mpsc::UnboundedSender<Vec<u8>>),
}

/// Resolves the shell program and flag used to run `command`.
fn shell_for_options(options: &SpawnOptions, command: &str) -> (String, Vec<String>) {
    if cfg!(windows) {
        ("cmd".to_string(), vec!["/C".to_string(), command.to_string()])
    } else {
        let shell = options
            .shell
            .clone()
            .filter(|shell| !shell.trim().is_empty())
            .unwrap_or_else(|| "sh".to_string());
        let flag = if options.login { "-lc" } else { "-c" };
        (shell, vec![flag.to_string(), command.to_string()])
    }
}

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
    /// Bounded log ring; `logs[0]` has the absolute sequence `first_seq`.
    logs: VecDeque<ProcessLogEntry>,
    first_seq: u64,
    command: String,
    project_path: PathBuf,
    options: SpawnOptions,
    /// Writable stdin end (pipe or PTY writer) for interactive sessions.
    stdin: Option<StdinSink>,
    /// Absolute sequence of the next entry `poll_output` returns. Absolute
    /// so it stays valid once old entries fall out of the ring (QA-026).
    read_cursor: u64,
    /// Stream tag state of the last poll, so chunks continuing a line are
    /// not re-tagged mid-line.
    poll_tag: PollTag,
    /// Job Object + network proxy of a natively sandboxed (Windows) child.
    sandbox: Option<crate::sandbox::SandboxSession>,
}

/// `[stream] ` tags are written when the stream changes, never mid-line.
#[derive(Default)]
struct PollTag {
    stream: Option<String>,
    at_line_start: bool,
}

impl PollTag {
    /// Appends one log entry to `output`, tagging it only when it starts a
    /// different stream than the text before it.
    fn render(&mut self, output: &mut String, entry: &ProcessLogEntry) {
        if self.stream.as_deref() != Some(entry.stream.as_str()) {
            if !self.at_line_start && self.stream.is_some() {
                output.push('\n');
            }
            output.push_str(&format!("[{}] ", entry.stream));
            self.stream = Some(entry.stream.clone());
        }
        output.push_str(&entry.text);
        self.at_line_start = entry.text.ends_with('\n');
    }
}

/// Decodes a byte stream chunk by chunk without turning a multibyte UTF-8
/// character split across two reads into `U+FFFD`.
#[derive(Default)]
struct Utf8Chunks {
    pending: Vec<u8>,
}

impl Utf8Chunks {
    fn decode(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let keep = incomplete_utf8_tail(&self.pending);
        let tail = self.pending.split_off(self.pending.len() - keep);
        let text = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending = tail;
        text
    }

    fn finish(&mut self) -> String {
        let text = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        text
    }
}

/// Number of trailing bytes that start a UTF-8 sequence not yet complete.
fn incomplete_utf8_tail(bytes: &[u8]) -> usize {
    for back in 1..=bytes.len().min(3) {
        let byte = bytes[bytes.len() - back];
        if byte & 0xC0 == 0x80 {
            continue; // continuation byte: keep looking for the lead byte
        }
        let needed = match byte {
            0xF0..=0xFF => 4,
            0xE0..=0xEF => 3,
            0xC0..=0xDF => 2,
            _ => 1,
        };
        return if needed > back { back } else { 0 };
    }
    0
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
            m.logs.push_back(ProcessLogEntry {
                timestamp: now_iso(),
                stream: stream.to_string(),
                text: text.to_string(),
            });
            if m.logs.len() > MAX_LOGS_PER_PROCESS {
                m.logs.pop_front();
                m.first_seq += 1;
            }
        }
    }

    /// Appends decoded reader output (empty text is skipped).
    fn append_decoded(&self, id: &str, stream: &str, text: String) {
        if !text.is_empty() {
            self.append_log(id, stream, &text);
        }
    }

    /// Spawns a background process and immediately returns its tracking info.
    pub fn spawn(
        self: &Arc<Self>,
        command: &str,
        project_path: PathBuf,
        env: Option<HashMap<String, String>>,
    ) -> ProcessInfo {
        self.spawn_with_options(
            command,
            project_path,
            SpawnOptions {
                env,
                ..SpawnOptions::default()
            },
        )
    }

    /// Like [`spawn`](Self::spawn) but pipes the child's stdin so interactive
    /// exec sessions can feed characters via [`write_stdin`](Self::write_stdin).
    pub fn spawn_with_stdin(
        self: &Arc<Self>,
        command: &str,
        project_path: PathBuf,
        env: Option<HashMap<String, String>>,
    ) -> ProcessInfo {
        self.spawn_with_options(
            command,
            project_path,
            SpawnOptions {
                stdin_pipe: true,
                env_clean: true,
                env,
                ..SpawnOptions::default()
            },
        )
    }

    /// Spawns a process with full control over the spawn options (piped or
    /// TTY stdio, shell, login shell, environment, native sandbox).
    pub fn spawn_with_options(
        self: &Arc<Self>,
        command: &str,
        project_path: PathBuf,
        options: SpawnOptions,
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
            logs: VecDeque::new(),
            first_seq: 0,
            command: command.trim().to_string(),
            project_path,
            options,
            stdin: None,
            read_cursor: 0,
            poll_tag: PollTag { stream: None, at_line_start: true },
            sandbox: None,
        }));
        self.lock_processes().insert(id.clone(), managed);
        self.start_child(&id);
        self.get(&id).unwrap_or(info)
    }

    /// Writes characters to an interactive session's stdin (pipe or PTY).
    pub async fn write_stdin(&self, id: &str, chars: &str) -> bool {
        let Some(managed) = self.get_arc(id) else {
            return false;
        };
        let sink = {
            let mut m = managed.lock().expect("managed process poisoned");
            m.stdin.take()
        };
        let Some(sink) = sink else {
            return false;
        };
        let (result, sink) = match sink {
            StdinSink::Pipe(mut pipe) => {
                let ok = pipe.write_all(chars.as_bytes()).await.is_ok() && pipe.flush().await.is_ok();
                (ok, StdinSink::Pipe(pipe))
            }
            StdinSink::Channel(sender) => {
                let ok = sender.send(chars.as_bytes().to_vec()).is_ok();
                (ok, StdinSink::Channel(sender))
            }
        };
        // Only put the sink back when the write succeeded (broken pipes are
        // dropped so later writes fail fast).
        if result {
            managed.lock().expect("managed process poisoned").stdin = Some(sink);
        }
        result
    }

    /// Drains new output for `id` since the last poll and reports the current
    /// status (`running` / `exited` from the exec-session point of view).
    ///
    /// The cursor only advances past what was returned: output beyond
    /// `max_chars` stays queued for the next poll (a single oversized entry
    /// is returned cut, so polling always makes progress).
    pub fn poll_output(&self, id: &str, max_chars: usize) -> Option<ExecPoll> {
        let managed = self.get_arc(id)?;
        let mut guard = managed.lock().expect("managed process poisoned");
        let m = &mut *guard;
        let mut output = String::new();
        if m.read_cursor < m.first_seq {
            let dropped = m.first_seq - m.read_cursor;
            output.push_str(&format!("... ({dropped} earlier output chunks were dropped from the log buffer)\n"));
            m.read_cursor = m.first_seq;
            m.poll_tag = PollTag { stream: None, at_line_start: true };
        }
        let start = (m.read_cursor - m.first_seq) as usize;
        let mut consumed = 0u64;
        for entry in m.logs.iter().skip(start) {
            let mut piece = String::new();
            let mut tag = PollTag { stream: m.poll_tag.stream.clone(), at_line_start: m.poll_tag.at_line_start };
            tag.render(&mut piece, entry);
            if output.len() + piece.len() > max_chars {
                if consumed == 0 {
                    // One entry larger than the budget: return it cut.
                    let cut = locagens_protocol::text::floor_char_boundary(&piece, max_chars.saturating_sub(output.len()));
                    output.push_str(&piece[..cut]);
                    output.push('\n');
                    m.poll_tag = PollTag { stream: tag.stream, at_line_start: true };
                    consumed = 1;
                }
                output.push_str("... (more output pending: poll again)\n");
                break;
            }
            output.push_str(&piece);
            m.poll_tag = tag;
            consumed += 1;
        }
        m.read_cursor += consumed;
        // `exited` only once the output is drained too, so callers that drop
        // the session on exit never lose its tail.
        let drained = m.read_cursor >= m.first_seq + m.logs.len() as u64;
        let status = if m.info.status == "running" || !drained { "running" } else { "exited" };
        Some(ExecPoll {
            output: output.trim_end().to_string(),
            status: status.to_string(),
            exit_code: m.info.exit_code,
        })
    }

    /// Whether the session's process is still running, without consuming
    /// any output (`None` for an unknown session).
    pub fn is_running(&self, id: &str) -> Option<bool> {
        let managed = self.get_arc(id)?;
        let m = managed.lock().expect("managed process poisoned");
        Some(m.info.status == "running")
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
        let (command, project_path, options) = {
            let m = managed.lock().expect("managed process poisoned");
            (m.command.clone(), m.project_path.clone(), m.options.clone())
        };

        #[cfg(unix)]
        {
            if options.tty {
                return self.start_pty_child(id, &command, project_path, &options);
            }
        }
        #[cfg(windows)]
        {
            if crate::sandbox::uses_native_spawn(options.sandbox.as_ref()) {
                return self.start_sandboxed_child(id, &command, project_path, &options);
            }
        }
        self.start_pipe_child(id, &command, project_path, &options);
    }

    /// Marks a spawn attempt as failed and logs why.
    fn record_spawn_error(&self, id: &str, message: &str) {
        if let Some(managed) = self.get_arc(id) {
            let mut m = managed.lock().expect("managed process poisoned");
            m.info.status = "error".to_string();
            m.info.stopped_at = Some(now_iso());
        }
        self.append_log(id, "system", message);
    }

    /// Windows sandbox spawn: restricted token + Job Object + network proxy
    /// (see [`crate::windows_sandbox`]). Stdio uses pipes served by blocking
    /// threads; a sandbox that cannot be set up fails the spawn (closed).
    #[cfg(windows)]
    fn start_sandboxed_child(
        self: &Arc<Self>,
        id: &str,
        command: &str,
        project_path: PathBuf,
        options: &SpawnOptions,
    ) {
        let Some(spec) = options.sandbox.as_ref() else {
            return;
        };
        let (shell_prog, shell_args) = shell_for_options(options, command);
        let env = sandboxed_child_env(options);
        let native = match crate::sandbox::spawn_native(
            spec,
            &shell_prog,
            &shell_args,
            &project_path,
            env,
            options.stdin_pipe,
        ) {
            Ok(native) => native,
            Err(err) => return self.record_spawn_error(id, &format!("[Failed to spawn process: {err}]")),
        };
        let crate::sandbox::NativeChild { child, session } = native;
        let waiter = match child.waiter() {
            Ok(waiter) => waiter,
            Err(err) => {
                session.terminate();
                return self.record_spawn_error(id, &format!("[Failed to spawn process: {err}]"));
            }
        };
        let pid = child.pid;
        let crate::windows_sandbox::SandboxedChild { stdin, stdout, stderr, .. } = child;
        let stdin = stdin.map(|file| StdinSink::Channel(spawn_stdin_writer(file)));
        {
            let managed_arc = self.get_arc(id).unwrap();
            let mut m = managed_arc.lock().expect("managed process poisoned");
            m.info.pid = Some(pid);
            m.info.status = "running".to_string();
            m.info.stopped_at = None;
            m.info.exit_code = None;
            m.stdin = stdin;
            m.sandbox = Some(session);
        }
        self.append_log(id, "system", &format!("[Process started with PID {pid} (sandboxed)]"));
        self.spawn_blocking_reader(id.to_string(), stdout, "stdout");
        self.spawn_blocking_reader(id.to_string(), stderr, "stderr");
        let this = Arc::clone(self);
        let id_owned = id.to_string();
        std::thread::spawn(move || {
            let code = waiter.wait().ok();
            this.finish_sandboxed_exit(&id_owned, code);
        });
    }

    /// Natural exit of a sandboxed root: keep background descendants, tell
    /// the model about refused hosts, then record the exit.
    #[cfg(windows)]
    fn finish_sandboxed_exit(self: &Arc<Self>, id: &str, code: Option<i32>) {
        let blocked = self.get_arc(id).and_then(|managed| {
            let m = managed.lock().expect("managed process poisoned");
            let session = m.sandbox.as_ref()?;
            if m.info.status == "running" {
                session.release_descendants();
            }
            Some(session.blocked_hosts())
        });
        if let Some(hosts) = blocked.filter(|hosts| !hosts.is_empty()) {
            self.append_log(id, "system", &blocked_hosts_note(&hosts));
        }
        self.finish_exit(id, code);
    }

    /// Streams a blocking reader (Windows sandbox pipe) into the log ring.
    #[cfg(windows)]
    fn spawn_blocking_reader(
        self: &Arc<Self>,
        id: String,
        mut stream: std::fs::File,
        stream_name: &'static str,
    ) {
        let this = Arc::clone(self);
        std::thread::spawn(move || {
            use std::io::Read as _;
            let mut buf = [0u8; 4096];
            let mut decoder = Utf8Chunks::default();
            loop {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => this.append_decoded(&id, stream_name, decoder.decode(&buf[..n])),
                }
            }
            this.append_decoded(&id, stream_name, decoder.finish());
        });
    }

    /// Pipe-based spawn: shell → wrapped sandbox → separate stdin/stdout/
    /// stderr pipes.
    fn start_pipe_child(
        self: &Arc<Self>,
        id: &str,
        command: &str,
        project_path: PathBuf,
        options: &SpawnOptions,
    ) {
        let (shell_prog, shell_args) = shell_for_options(options, command);
        let (program, prog_args) = match crate::sandbox::try_wrap_shell_command(
            &shell_prog,
            &shell_args,
            options.sandbox.as_ref(),
        ) {
            Ok(wrapped) => wrapped,
            // No sandbox backend: fail closed, never run unsandboxed.
            Err(err) => return self.record_spawn_error(id, &format!("[Failed to spawn process: {err}]")),
        };

        let mut cmd = tokio::process::Command::new(&program);
        crate::sandbox::set_command_args(&mut cmd, &program, &prog_args);
        #[cfg(unix)]
        {
            // The child leads its own process group so tree kills work.
            cmd.process_group(0);
        }
        cmd.current_dir(&project_path)
            .env("FORCE_COLOR", "1")
            .stdin(if options.stdin_pipe {
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            })
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(false);
        // Exec sessions start from a stripped environment (secrets never
        // reach child processes); `[shell_environment_policy]` refines it.
        let (clear_env, child_vars) = crate::env_policy::child_env(options.env_clean, options.env.as_ref());
        if clear_env {
            cmd.env_clear();
        }
        for (key, value) in child_vars {
            cmd.env(key, value);
        }

        match cmd.spawn() {
            Ok(mut child) => {
                let pid = child.id();
                let stdout = child.stdout.take();
                let stderr = child.stderr.take();
                let stdin = if options.stdin_pipe {
                    child.stdin.take().map(StdinSink::Pipe)
                } else {
                    None
                };
                {
                    let managed_arc = self.get_arc(id).unwrap();
                let mut m = managed_arc
                        .lock()
                        .expect("managed process poisoned");
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
                let managed_arc = self.get_arc(id).unwrap();
                let mut m = managed_arc
                    .lock()
                    .expect("managed process poisoned");
                m.info.status = "error".to_string();
                m.info.stopped_at = Some(now_iso());
                drop(m);
                self.append_log(id, "system", &format!("[Failed to spawn process: {err}]"));
            }
        }
    }

    /// PTY spawn (Unix): the shell runs with its stdio attached to a real
    /// pseudo-terminal so TTY-aware programs behave. Falls back gracefully
    /// to piped stdio when `openpty` fails.
    #[cfg(unix)]
    fn start_pty_child(
        self: &Arc<Self>,
        id: &str,
        command: &str,
        project_path: PathBuf,
        options: &SpawnOptions,
    ) {
        use std::os::unix::io::FromRawFd;
        use std::os::unix::process::CommandExt;

        let (shell_prog, shell_args) = shell_for_options(options, command);
        let (program, prog_args) = match crate::sandbox::try_wrap_shell_command(
            &shell_prog,
            &shell_args,
            options.sandbox.as_ref(),
        ) {
            Ok(wrapped) => wrapped,
            Err(err) => return self.record_spawn_error(id, &format!("[Failed to spawn process: {err}]")),
        };

        let mut master_fd: libc::c_int = -1;
        let mut slave_fd: libc::c_int = -1;
        let mut winsize = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let opened = unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut winsize,
            )
        };
        if opened != 0 {
            // Graceful fallback: no PTY available, use piped stdio.
            return self.start_pipe_child(id, command, project_path, options);
        }

        let mut cmd = std::process::Command::new(&program);
        cmd.args(&prog_args).current_dir(&project_path);
        let (clear_env, child_vars) = crate::env_policy::child_env(options.env_clean, options.env.as_ref());
        if clear_env {
            cmd.env_clear();
        }
        for (key, value) in child_vars {
            cmd.env(key, value);
        }
        unsafe {
            // The slave end becomes the child's stdio; each dup is owned by
            // the resulting `Stdio` from here on.
            cmd.stdin(std::process::Stdio::from_raw_fd(libc::dup(slave_fd)));
            cmd.stdout(std::process::Stdio::from_raw_fd(libc::dup(slave_fd)));
            cmd.stderr(std::process::Stdio::from_raw_fd(libc::dup(slave_fd)));
        }
        unsafe {
            cmd.pre_exec(|| {
                // New session + controlling terminal so the child behaves
                // like a foreground TTY program.
                libc::setsid();
                libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, std::ptr::null::<libc::c_void>());
                Ok::<(), std::io::Error>(())
            });
        }

        match cmd.spawn() {
            Ok(mut child) => {
                let pid = child.id();
                unsafe { libc::close(slave_fd) };

                // Writer task: stdin writes flow through an unbounded channel
                // into the PTY master from a dedicated blocking thread.
                let (stdin_tx, mut stdin_rx) =
                    tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
                let writer_fd = unsafe { libc::dup(master_fd) };
                std::thread::spawn(move || {
                    let mut file = unsafe { std::fs::File::from_raw_fd(writer_fd) };
                    use std::io::Write as _;
                    while let Some(bytes) = stdin_rx.blocking_recv() {
                        if file.write_all(&bytes).is_err() || file.flush().is_err() {
                            break;
                        }
                    }
                });

                // Reader thread: PTY master → log ring.
                let reader_fd = unsafe { libc::dup(master_fd) };
                let this = Arc::clone(self);
                let id_owned = id.to_string();
                std::thread::spawn(move || {
                    let mut file = unsafe { std::fs::File::from_raw_fd(reader_fd) };
                    let mut buf = [0u8; 4096];
                    let mut decoder = Utf8Chunks::default();
                    use std::io::Read as _;
                    loop {
                        match file.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => this.append_decoded(&id_owned, "stdout", decoder.decode(&buf[..n])),
                        }
                    }
                    this.append_decoded(&id_owned, "stdout", decoder.finish());
                });

                {
                    let managed_arc = self.get_arc(id).unwrap();
                    let mut m = managed_arc
                        .lock()
                        .expect("managed process poisoned");
                    m.info.pid = Some(pid);
                    m.info.status = "running".to_string();
                    m.info.stopped_at = None;
                    m.info.exit_code = None;
                    m.stdin = Some(StdinSink::Channel(stdin_tx));
                }
                self.append_log(
                    id,
                    "system",
                    &format!("[Process started with PID {pid} (tty)]"),
                );

                // Exit monitor thread (uniform bookkeeping with pipe spawns).
                let this = Arc::clone(self);
                let id_owned = id.to_string();
                std::thread::spawn(move || {
                    let code = child.wait().ok().and_then(|status| status.code());
                    this.finish_exit(&id_owned, code);
                });

                // The master fd is kept alive by the reader/writer threads;
                // drop our own handle in the parent now.
                unsafe { libc::close(master_fd) };
            }
            Err(err) => {
                unsafe {
                    libc::close(slave_fd);
                    libc::close(master_fd);
                }
                let managed_arc = self.get_arc(id).unwrap();
                let mut m = managed_arc
                    .lock()
                    .expect("managed process poisoned");
                m.info.status = "error".to_string();
                m.info.stopped_at = Some(now_iso());
                drop(m);
                self.append_log(id, "system", &format!("[Failed to spawn process: {err}]"));
            }
        }
    }

    /// Records a natural (or killed-then-waited) exit for a session.
    fn finish_exit(self: &Arc<Self>, id: &str, code: Option<i32>) {
        if let Some(managed) = self.get_arc(id) {
            let mut m = managed.lock().expect("managed process poisoned");
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
            let mut decoder = Utf8Chunks::default();
            loop {
                match stream.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => this.append_decoded(&id, stream_name, decoder.decode(&buf[..n])),
                }
            }
            this.append_decoded(&id, stream_name, decoder.finish());
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
                    .iter()
                    .cloned()
                    .collect()
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

        let killed_job = {
            let mut m = managed.lock().expect("managed process poisoned");
            m.info.status = "stopped".to_string();
            m.info.stopped_at = Some(now_iso());
            m.info.exit_code = None;
            // Sandboxed children die with their Job Object.
            m.sandbox.as_ref().map(crate::sandbox::SandboxSession::terminate).is_some()
        };

        if !killed_job {
            kill_tree(pid).await;
        }
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

/// The environment of a natively sandboxed child: the same
/// `[shell_environment_policy]`-aware resolution the pipe / PTY spawns use
/// (QA-016), plus `FORCE_COLOR` for non-clean sessions.
#[cfg(windows)]
fn sandboxed_child_env(options: &SpawnOptions) -> HashMap<String, String> {
    let mut env = crate::env_policy::full_child_env(options.env_clean, options.env.as_ref());
    if !options.env_clean {
        env.entry("FORCE_COLOR".to_string()).or_insert_with(|| "1".to_string());
    }
    env
}

/// Writer thread feeding a sandboxed child's stdin pipe from a channel.
#[cfg(windows)]
fn spawn_stdin_writer(mut file: std::fs::File) -> tokio::sync::mpsc::UnboundedSender<Vec<u8>> {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    std::thread::spawn(move || {
        use std::io::Write as _;
        while let Some(bytes) = receiver.blocking_recv() {
            if file.write_all(&bytes).is_err() || file.flush().is_err() {
                break;
            }
        }
    });
    sender
}

/// Log line telling the model which hosts the sandbox proxy refused.
#[cfg(windows)]
fn blocked_hosts_note(hosts: &[String]) -> String {
    format!("[{}]", crate::network_proxy::blocked_hosts_note(hosts))
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
        // SIGKILL the whole process group first, then the pid itself; the
        // children are spawned as process-group leaders (or new PTY sessions).
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A registered session with no child process, for log-ring tests.
    fn fake_session(manager: &ProcessManager, id: &str) {
        let info = ProcessInfo {
            id: id.to_string(),
            command: "fake".into(),
            project_path: String::new(),
            status: "running".into(),
            started_at: now_iso(),
            stopped_at: None,
            exit_code: None,
            pid: None,
        };
        let managed = Managed {
            info,
            logs: VecDeque::new(),
            first_seq: 0,
            command: "fake".into(),
            project_path: PathBuf::new(),
            options: SpawnOptions::default(),
            stdin: None,
            read_cursor: 0,
            poll_tag: PollTag { stream: None, at_line_start: true },
            sandbox: None,
        };
        manager.lock_processes().insert(id.to_string(), Arc::new(Mutex::new(managed)));
    }

    /// QA-026: once the ring is full, new output must still be returned.
    #[test]
    fn polling_keeps_working_after_the_log_ring_wraps() {
        let manager = ProcessManager::new();
        fake_session(&manager, "s");
        for n in 0..MAX_LOGS_PER_PROCESS {
            manager.append_log("s", "stdout", &format!("{n}\n"));
        }
        let first = manager.poll_output("s", usize::MAX).unwrap();
        assert!(first.output.ends_with(&format!("{}", MAX_LOGS_PER_PROCESS - 1)));
        for round in 0..3 {
            manager.append_log("s", "stdout", &format!("late-{round}\n"));
            let poll = manager.poll_output("s", usize::MAX).unwrap();
            assert_eq!(poll.output, format!("late-{round}"), "round {round}");
        }
        // Entries that fell out of the ring unread are reported, not hidden.
        for n in 0..(MAX_LOGS_PER_PROCESS + 5) {
            manager.append_log("s", "stdout", &format!("x{n}\n"));
        }
        let poll = manager.poll_output("s", usize::MAX).unwrap();
        assert!(poll.output.starts_with("... (5 earlier output chunks were dropped"), "{}", &poll.output[..80]);
    }

    /// Output over `max_chars` stays queued for the next poll.
    #[test]
    fn truncated_polls_do_not_drop_unread_output() {
        let manager = ProcessManager::new();
        fake_session(&manager, "s");
        for n in 0..10 {
            manager.append_log("s", "stdout", &format!("line-{n}\n"));
        }
        let mut seen = String::new();
        for _ in 0..20 {
            let poll = manager.poll_output("s", 30).unwrap();
            seen.push_str(&poll.output.replace("... (more output pending: poll again)", ""));
            seen.push('\n');
        }
        for n in 0..10 {
            assert!(seen.contains(&format!("line-{n}")), "line-{n} lost: {seen}");
        }
        // A finished process only reports `exited` once its output is drained.
        manager.append_log("s", "stdout", &"y".repeat(30));
        manager.append_log("s", "stdout", &"z".repeat(30));
        manager.get_arc("s").unwrap().lock().unwrap().info.status = "stopped".into();
        assert_eq!(manager.poll_output("s", 40).unwrap().status, "running");
        assert_eq!(manager.poll_output("s", 400).unwrap().status, "exited");
    }

    /// Stream tags mark stream switches, never the middle of a line.
    #[test]
    fn stream_tags_are_not_inserted_mid_line() {
        let manager = ProcessManager::new();
        fake_session(&manager, "s");
        manager.append_log("s", "stdout", "Compiling foo");
        manager.append_log("s", "stdout", " v1.0\nDone\n");
        manager.append_log("s", "stderr", "warn: x");
        manager.append_log("s", "stdout", "ok\n");
        let poll = manager.poll_output("s", usize::MAX).unwrap();
        assert_eq!(poll.output, "[stdout] Compiling foo v1.0\nDone\n[stderr] warn: x\n[stdout] ok");
        manager.append_log("s", "stdout", "more\n");
        assert_eq!(manager.poll_output("s", usize::MAX).unwrap().output, "more");
    }

    #[test]
    fn utf8_chunks_survive_split_reads() {
        let bytes = "ğ→🙂".as_bytes();
        let mut decoder = Utf8Chunks::default();
        let mut text = String::new();
        for byte in bytes {
            text.push_str(&decoder.decode(std::slice::from_ref(byte)));
        }
        text.push_str(&decoder.finish());
        assert_eq!(text, "ğ→🙂");
        // Invalid bytes still decode (lossy) instead of stalling.
        let mut decoder = Utf8Chunks::default();
        assert_eq!(decoder.decode(&[b'a', 0xFF, b'b']), "a\u{FFFD}b");
    }

    /// Spawns a TTY session whose script proves stdin is a terminal, feeds a
    /// line via `write_stdin` and verifies the output roundtrip.
    #[cfg(unix)]
    #[tokio::test]
    async fn pty_session_roundtrip_with_stdin() {
        let manager = Arc::new(ProcessManager::new());
        let mut env = HashMap::new();
        env.insert("PATH".to_string(), std::env::var("PATH").unwrap_or_default());
        let info = manager.spawn_with_options(
            "sh -c 'test -t 0 && echo TTY-OK; read line; echo GOT-$line'",
            std::env::temp_dir(),
            SpawnOptions {
                stdin_pipe: true,
                tty: true,
                env_clean: true,
                env: Some(env),
                ..SpawnOptions::default()
            },
        );
        assert_eq!(info.status, "running");

        // Wait for the TTY check to pass.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let poll = manager
                .poll_output(&info.id, 4000)
                .expect("session should exist");
            if poll.output.contains("TTY-OK") {
                break;
            }
            assert!(poll.status == "running", "exited early: {poll:?}");
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for TTY-OK: {poll:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // Feed a line on the PTY and read the echo back.
        assert!(manager.write_stdin(&info.id, "hello\n").await);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let poll = manager.poll_output(&info.id, 4000).unwrap();
            if poll.output.contains("GOT-hello") {
                break;
            }
            assert!(poll.status == "running", "exited early: {poll:?}");
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for GOT-hello: {poll:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // The script exits on its own; verify the exit is recorded cleanly
        // (and that the PTY session lifecycle bookkeeping is intact).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let poll = manager.poll_output(&info.id, 4000).unwrap();
            if poll.status == "exited" {
                assert_eq!(poll.exit_code, Some(0), "exit poll was: {poll:?}");
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for exit: {poll:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        manager.drop_session(&info.id);
    }
}
