//! Native OS sandboxing: macOS Seatbelt (`sandbox-exec`), Linux Bubblewrap
//! (`bwrap`) and a Windows restricted-token backend, ported from Codex's
//! sandbox policy model.
//!
//! On macOS/Linux, commands run through [`wrap_shell_command`] are wrapped
//! into a sandbox process when the run's [`SandboxMode`] is not
//! `danger-full-access` and a native backend is detected. Windows cannot be
//! expressed as an argv wrapper: callers check [`uses_native_spawn`] and
//! spawn through [`crate::windows_sandbox`] (see [`run_captured`] and the
//! `ProcessManager`), which also routes network traffic through the
//! per-domain [`NetworkProxy`].

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use locagens_protocol::SandboxMode;

use crate::network_proxy::{NetworkPolicy, NetworkProxy};

/// Available native sandbox backends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxBackend {
    /// macOS Seatbelt via `/usr/bin/sandbox-exec`.
    MacosSeatbelt,
    /// Linux Bubblewrap via `bwrap`.
    LinuxBubblewrap,
    /// Windows write-restricted token + capability-SID ACLs + Job Object.
    WindowsRestrictedToken,
    /// No native backend: commands run on the host (secrets stripped).
    HostEnvOnly,
}

/// The sandbox parameters for one command execution.
#[derive(Clone, Debug)]
pub struct SandboxSpec {
    pub mode: SandboxMode,
    pub project_path: PathBuf,
    /// macOS/Linux: allow all outbound network. Ignored by the Windows
    /// backend, which always routes sandboxed commands through the proxy.
    pub allow_network: bool,
    /// Windows: the per-domain allowlist (+ approval hook) enforced by the
    /// command's network proxy. `None` means deny every host.
    pub network: Option<NetworkPolicy>,
    /// Extra directories writable in `workspace-write` (config.toml
    /// `[sandbox_workspace_write] writable_roots`). Missing ones are skipped.
    pub writable_roots: Vec<PathBuf>,
}

/// Detects the best available sandbox backend on this platform.
pub fn detect_backend() -> SandboxBackend {
    #[cfg(target_os = "macos")]
    if Path::new("/usr/bin/sandbox-exec").is_file() {
        return SandboxBackend::MacosSeatbelt;
    }
    #[cfg(target_os = "linux")]
    if Path::new("/usr/bin/bwrap").is_file() || Path::new("/bin/bwrap").is_file() {
        return SandboxBackend::LinuxBubblewrap;
    }
    #[cfg(windows)]
    return SandboxBackend::WindowsRestrictedToken;
    #[allow(unreachable_code)]
    SandboxBackend::HostEnvOnly
}

/// Whether `spec` must be spawned in-process through the Windows backend
/// (restricted token) instead of an argv wrapper.
pub fn uses_native_spawn(spec: Option<&SandboxSpec>) -> bool {
    cfg!(windows) && spec.is_some_and(|spec| spec.mode != SandboxMode::DangerFullAccess)
}

/// Keeps a natively sandboxed child's Job Object and network proxy alive.
/// Dropping it while the command still runs kills the process tree.
pub struct SandboxSession {
    #[cfg(windows)]
    job: std::sync::Arc<crate::windows_sandbox::JobObject>,
    proxy: Option<NetworkProxy>,
}

impl SandboxSession {
    /// Kills the whole sandboxed process tree.
    pub fn terminate(&self) {
        #[cfg(windows)]
        let _ = self.job.terminate();
    }

    /// The root exited on its own: let background descendants keep running
    /// after the session is dropped (Codex `preserve_descendants`).
    pub fn release_descendants(&self) {
        #[cfg(windows)]
        let _ = self.job.preserve_descendants();
    }

    /// Hosts the network proxy refused so far.
    pub fn blocked_hosts(&self) -> Vec<String> {
        self.proxy.as_ref().map(NetworkProxy::blocked_hosts).unwrap_or_default()
    }
}

/// Starts the per-command network proxy for a natively sandboxed spec and
/// returns it with the environment variables pointing at it.
pub fn start_network_proxy(spec: &SandboxSpec) -> std::io::Result<(NetworkProxy, Vec<(String, String)>)> {
    let proxy = NetworkProxy::start(spec.network.clone().unwrap_or_default())?;
    let vars = proxy.env_vars();
    Ok((proxy, vars))
}

/// A natively spawned sandboxed child plus the session that owns it.
#[cfg(windows)]
pub struct NativeChild {
    pub child: crate::windows_sandbox::SandboxedChild,
    pub session: SandboxSession,
}

/// Spawns `program args` inside the Windows sandbox with its own network
/// proxy. `env` is the complete child environment. Errors mean the command
/// did not run (fail closed).
#[cfg(windows)]
pub fn spawn_native(
    spec: &SandboxSpec,
    program: &str,
    args: &[String],
    cwd: &Path,
    mut env: HashMap<String, String>,
    stdin_pipe: bool,
) -> Result<NativeChild, String> {
    let (proxy, proxy_env) = start_network_proxy(spec)
        .map_err(|err| format!("Windows sandbox: cannot start the network proxy: {err}"))?;
    env.extend(proxy_env);
    let request = crate::windows_sandbox::SpawnRequest {
        program,
        args,
        cwd,
        env: &env,
        stdin_pipe,
    };
    let child = crate::windows_sandbox::spawn_sandboxed(spec, &request)?;
    let session = SandboxSession {
        job: std::sync::Arc::clone(&child.job),
        proxy: Some(proxy),
    };
    Ok(NativeChild { child, session })
}

/// Output of [`run_captured`].
#[derive(Clone, Debug, Default)]
pub struct CapturedOutput {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Hosts the network proxy refused (Windows sandbox only).
    pub blocked_hosts: Vec<String>,
}

impl CapturedOutput {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Runs `program args` to completion with exactly `env` as its environment,
/// sandboxed per `spec`, killing the process tree after `timeout`
/// (`ErrorKind::TimedOut`). On Windows a sandbox that cannot be set up is an
/// error — the command never runs unsandboxed outside full-access mode.
pub async fn run_captured(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: HashMap<String, String>,
    spec: Option<&SandboxSpec>,
    timeout: Duration,
) -> std::io::Result<CapturedOutput> {
    #[cfg(windows)]
    if let Some(spec) = spec.filter(|spec| uses_native_spawn(Some(spec))) {
        return run_captured_native(spec, program, args, cwd, env, timeout).await;
    }
    let (program, prog_args) = wrap_shell_command(program, args, spec);
    let mut cmd = tokio::process::Command::new(&program);
    set_command_args(&mut cmd, &program, &prog_args);
    cmd.current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .env_clear()
        .envs(env);
    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "command timed out"))??;
    Ok(CapturedOutput {
        exit_code: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
        blocked_hosts: Vec::new(),
    })
}

/// Sets `args` on `cmd`. On Windows a `cmd /C <script>` pair is passed
/// verbatim as `/s /c "<script>"` (Codex `cmd_script_command_line`): MSVC
/// argument quoting would turn the script's own quotes into `\"`, which
/// `cmd.exe` does not understand.
pub fn set_command_args(cmd: &mut tokio::process::Command, program: &str, args: &[String]) {
    #[cfg(windows)]
    if let Some(script) = cmd_script(program, args) {
        cmd.raw_arg(format!("/s /c \"{script}\""));
        return;
    }
    let _ = program;
    cmd.args(args);
}

/// The script of a `cmd /C <script>` invocation, if `program args` is one.
pub fn cmd_script<'a>(program: &str, args: &'a [String]) -> Option<&'a str> {
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program).to_ascii_lowercase();
    let is_cmd = name == "cmd" || name == "cmd.exe";
    match args {
        [switch, script] if is_cmd && switch.eq_ignore_ascii_case("/c") => Some(script.as_str()),
        _ => None,
    }
}

#[cfg(windows)]
async fn run_captured_native(
    spec: &SandboxSpec,
    program: &str,
    args: &[String],
    cwd: &Path,
    env: HashMap<String, String>,
    timeout: Duration,
) -> std::io::Result<CapturedOutput> {
    use std::io::Read;

    let NativeChild { child, session } =
        spawn_native(spec, program, args, cwd, env, false).map_err(std::io::Error::other)?;
    let waiter = child.waiter()?;
    let crate::windows_sandbox::SandboxedChild { mut stdout, mut stderr, .. } = child;
    let read_stdout = tokio::task::spawn_blocking(move || {
        let mut buffer = Vec::new();
        let _ = stdout.read_to_end(&mut buffer);
        buffer
    });
    let read_stderr = tokio::task::spawn_blocking(move || {
        let mut buffer = Vec::new();
        let _ = stderr.read_to_end(&mut buffer);
        buffer
    });
    let wait = tokio::task::spawn_blocking(move || waiter.wait());
    let finished = tokio::time::timeout(timeout, async {
        let code = wait.await.map_err(std::io::Error::other)??;
        let stdout = read_stdout.await.unwrap_or_default();
        let stderr = read_stderr.await.unwrap_or_default();
        Ok::<_, std::io::Error>((code, stdout, stderr))
    })
    .await;
    match finished {
        Ok(result) => {
            let (code, stdout, stderr) = result?;
            session.release_descendants();
            Ok(CapturedOutput {
                exit_code: Some(code),
                stdout,
                stderr,
                blocked_hosts: session.blocked_hosts(),
            })
        }
        Err(_) => {
            session.terminate();
            Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "command timed out"))
        }
    }
}

fn canonical_or(path: &Path) -> String {
    std::fs::canonicalize(path)
        .map(|canonical| canonical.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
}

/// Builds a Seatbelt `.sbpl` profile plus the `-D<name>=<value>` parameters
/// it references.
pub fn build_macos_seatbelt_profile_and_params(
    spec: &SandboxSpec,
) -> (String, Vec<(String, String)>) {
    let root = canonical_or(&spec.project_path);
    let mut profile = String::from(
        r#"(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow signal (target same-sandbox))
(allow process-info* (target same-sandbox))
(allow sysctl-read)
(allow mach-lookup)
(allow ipc-posix-sem)
(allow ipc-posix-shm)
(allow pseudo-tty)
(allow file-read*)
(allow file-write-data (literal "/dev/null"))
(allow file-write-data (literal "/dev/dtracehelper"))
(allow file-ioctl)
"#,
    );

    let mut params: Vec<(String, String)> = Vec::new();
    if spec.mode == SandboxMode::WorkspaceWrite {
        profile.push_str(
            r#"(allow file-write* (subpath (param "PROJECT_ROOT")))
(allow file-write* (subpath "/private/tmp"))
(allow file-write* (subpath "/tmp"))
(allow file-write* (subpath "/private/var/folders"))
(deny file-write* (subpath (param "GIT_HOOKS_DIR")))
(deny file-write* (literal (param "GIT_CONFIG_FILE")))
(deny file-write* (subpath (param "LOCAGENS_CONFIG_DIR")))
"#,
        );
        if let Ok(tmp) = std::env::var("TMPDIR") {
            if let Ok(canonical) = std::fs::canonicalize(&tmp) {
                profile.push_str("(allow file-write* (subpath (param \"DARWIN_TMP_DIR\")))\n");
                params.push(("DARWIN_TMP_DIR".to_string(), canonical.to_string_lossy().to_string()));
            }
        }
        for extra in existing_roots(&spec.writable_roots) {
            profile.push_str(&format!("(allow file-write* (subpath {}))\n", seatbelt_string(&extra)));
        }
        params.push(("PROJECT_ROOT".to_string(), root.clone()));
        params.push(("GIT_HOOKS_DIR".to_string(), format!("{root}/.git/hooks")));
        params.push(("GIT_CONFIG_FILE".to_string(), format!("{root}/.git/config")));
        params.push(("LOCAGENS_CONFIG_DIR".to_string(), format!("{root}/.locagens")));
    }

    if spec.allow_network {
        profile.push_str(
            "(allow network-outbound)\n(allow network-inbound)\n(allow system-socket)\n",
        );
    } else {
        // Unix-socket outbound stays allowed (system agents, ssh-agent, …);
        // all TCP/UDP outbound network is denied by `(deny default)`.
        profile.push_str("(allow network-outbound (remote unix-socket))\n");
    }

    (profile, params)
}

/// Canonical string forms of the extra writable roots that exist.
fn existing_roots(roots: &[PathBuf]) -> Vec<String> {
    roots
        .iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .map(|root| root.to_string_lossy().to_string())
        .collect()
}

/// A Seatbelt (SBPL) string literal.
fn seatbelt_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Builds the `bwrap` command line (`program` + `args` appended after `--`).
pub fn build_linux_bwrap_args(
    spec: &SandboxSpec,
    program: &str,
    args: &[String],
) -> (String, Vec<String>) {
    let bwrap = if Path::new("/usr/bin/bwrap").is_file() {
        "/usr/bin/bwrap"
    } else {
        "/bin/bwrap"
    };
    let mut out: Vec<String> = vec![
        "--ro-bind".to_string(),
        "/".to_string(),
        "/".to_string(),
        "--dev".to_string(),
        "/dev".to_string(),
        "--proc".to_string(),
        "/proc".to_string(),
        "--tmpfs".to_string(),
        "/tmp".to_string(),
        "--die-with-parent".to_string(),
    ];
    if !spec.allow_network {
        out.push("--unshare-net".to_string());
    }
    if spec.mode == SandboxMode::WorkspaceWrite {
        let root = canonical_or(&spec.project_path);
        out.push("--bind".to_string());
        out.push(root.clone());
        out.push(root.clone());
        for extra in existing_roots(&spec.writable_roots) {
            out.extend(["--bind".to_string(), extra.clone(), extra]);
        }
        // Carve-outs stay read-only so hooks/config/tool state cannot be
        // rewritten by sandboxed commands.
        for carve_out in [
            ".git/hooks",
            ".git/config",
            ".locagens",
        ] {
            let path = format!("{root}/{carve_out}");
            if Path::new(&path).exists() {
                out.push("--ro-bind".to_string());
                out.push(path.clone());
                out.push(path);
            }
        }
    }
    out.push("--".to_string());
    out.push(program.to_string());
    out.extend(args.iter().cloned());
    (bwrap.to_string(), out)
}

/// Wraps a shell command into its sandboxed equivalent, dispatching on the
/// detected backend. `None` spec or `DangerFullAccess` runs unchanged.
pub fn wrap_shell_command(
    program: &str,
    args: &[String],
    spec: Option<&SandboxSpec>,
) -> (String, Vec<String>) {
    let Some(spec) = spec else {
        return (program.to_string(), args.to_vec());
    };
    if spec.mode == SandboxMode::DangerFullAccess {
        return (program.to_string(), args.to_vec());
    }
    match detect_backend() {
        SandboxBackend::MacosSeatbelt => {
            let (profile, params) = build_macos_seatbelt_profile_and_params(spec);
            let mut out: Vec<String> = vec!["-p".to_string(), profile];
            for (key, value) in params {
                out.push(format!("-D{key}={value}"));
            }
            out.push("--".to_string());
            out.push(program.to_string());
            out.extend(args.iter().cloned());
            ("/usr/bin/sandbox-exec".to_string(), out)
        }
        SandboxBackend::LinuxBubblewrap => build_linux_bwrap_args(spec, program, args),
        SandboxBackend::WindowsRestrictedToken => refuse_unsandboxed_windows_command(),
        SandboxBackend::HostEnvOnly => (program.to_string(), args.to_vec()),
    }
}

/// Fail-closed stand-in for callers that try to argv-wrap a sandboxed
/// Windows command instead of using [`uses_native_spawn`]: the replacement
/// command only prints an error and exits non-zero.
fn refuse_unsandboxed_windows_command() -> (String, Vec<String>) {
    (
        "cmd".to_string(),
        vec![
            "/C".to_string(),
            "echo Locagens: sandboxed Windows commands must be spawned through the Windows sandbox backend; refusing to run unsandboxed. 1>&2 & exit /b 126".to_string(),
        ],
    )
}

// ----------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_spec(label: &str) -> (PathBuf, SandboxSpec) {
        let dir = std::env::temp_dir().join(format!(
            "locagens-sbx-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let canonical = std::fs::canonicalize(&dir).unwrap();
        let spec = SandboxSpec {
            mode: SandboxMode::WorkspaceWrite,
            project_path: canonical.clone(),
            allow_network: false,
            network: None,
            writable_roots: Vec::new(),
        };
        (canonical, spec)
    }

    #[test]
    fn native_spawn_is_windows_only_and_skips_full_access() {
        let (root, mut spec) = temp_spec("native");
        assert_eq!(uses_native_spawn(Some(&spec)), cfg!(windows));
        assert!(!uses_native_spawn(None));
        spec.mode = SandboxMode::ReadOnly;
        assert_eq!(uses_native_spawn(Some(&spec)), cfg!(windows));
        spec.mode = SandboxMode::DangerFullAccess;
        assert!(!uses_native_spawn(Some(&spec)));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A stray argv-wrapping caller on Windows gets a refusing command, never
    /// the original one.
    #[cfg(windows)]
    #[test]
    fn windows_argv_wrapping_fails_closed() {
        let (root, spec) = temp_spec("wrap");
        let original = vec!["/C".to_string(), "echo hi > x.txt".to_string()];
        let (program, args) = wrap_shell_command("cmd", &original, Some(&spec));
        assert_eq!(program, "cmd");
        assert_ne!(args, original);
        assert!(args[1].contains("refusing to run unsandboxed"));
        assert!(args[1].contains("exit /b 126"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// When the sandbox cannot be set up the command must not run at all.
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_sandbox_setup_failure_fails_closed() {
        let missing = std::env::temp_dir().join(format!("locagens-sbx-missing-{}", uuid::Uuid::new_v4()));
        let marker = std::env::temp_dir().join(format!("locagens-sbx-marker-{}", uuid::Uuid::new_v4()));
        let spec = SandboxSpec {
            mode: SandboxMode::WorkspaceWrite,
            project_path: missing.clone(),
            allow_network: false,
            network: None,
            writable_roots: Vec::new(),
        };
        let script = format!("echo ran > \"{}\"", marker.display());
        let result = run_captured(
            "cmd",
            &["/C".to_string(), script],
            &std::env::temp_dir(),
            HashMap::new(),
            Some(&spec),
            Duration::from_secs(30),
        )
        .await;
        let err = result.expect_err("setup failure must be an error, not an unsandboxed run");
        assert!(err.to_string().contains("Windows sandbox"), "{err}");
        assert!(!marker.exists(), "the command must not have run");
    }

    #[test]
    fn macos_seatbelt_profile_protects_git_and_locagens_dirs() {
        let (root, spec) = temp_spec("profile");
        let (profile, params) = build_macos_seatbelt_profile_and_params(&spec);
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(allow file-write* (subpath (param \"PROJECT_ROOT\")))"));
        assert!(profile.contains("(deny file-write* (subpath (param \"GIT_HOOKS_DIR\")))"));
        assert!(profile.contains("(deny file-write* (literal (param \"GIT_CONFIG_FILE\")))"));
        assert!(profile.contains("(deny file-write* (subpath (param \"LOCAGENS_CONFIG_DIR\")))"));
        assert!(profile.contains("(allow network-outbound (remote unix-socket))"));
        assert!(!profile.contains("(allow network-outbound)\n"));

        let params_map: std::collections::HashMap<String, String> = params.iter().cloned().collect();
        let root_str = root.to_string_lossy().to_string();
        assert_eq!(params_map.get("PROJECT_ROOT").unwrap(), &root_str);
        assert_eq!(
            params_map.get("GIT_HOOKS_DIR").unwrap(),
            &format!("{root_str}/.git/hooks")
        );
        assert_eq!(
            params_map.get("GIT_CONFIG_FILE").unwrap(),
            &format!("{root_str}/.git/config")
        );
        assert_eq!(
            params_map.get("LOCAGENS_CONFIG_DIR").unwrap(),
            &format!("{root_str}/.locagens")
        );

        // The wrapped argv carries the profile and every parameter (only
        // where Seatbelt is the active backend).
        if detect_backend() == SandboxBackend::MacosSeatbelt {
            let (program, args) =
                wrap_shell_command("/bin/sh", &["-c".to_string(), "true".to_string()], Some(&spec));
            assert_eq!(program, "/usr/bin/sandbox-exec");
            assert!(args.contains(&profile));
            assert!(args.iter().any(|arg| arg.starts_with("-DPROJECT_ROOT=")));
            assert!(args.iter().any(|arg| arg.starts_with("-DGIT_CONFIG_FILE=")));
        }

        // Full-access mode runs unchanged.
        let (program, args) = wrap_shell_command(
            "/bin/sh",
            &["-c".to_string(), "true".to_string()],
            Some(&SandboxSpec {
                mode: SandboxMode::DangerFullAccess,
                project_path: root.clone(),
                allow_network: true,
                network: None,
                writable_roots: Vec::new(),
            }),
        );
        assert_eq!(program, "/bin/sh");
        assert_eq!(args, vec!["-c".to_string(), "true".to_string()]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_seatbelt_blocks_writes_outside_workspace() {
        use std::process::Command;

        if detect_backend() != SandboxBackend::MacosSeatbelt {
            eprintln!("skipping: no sandbox-exec backend");
            return;
        }
        let (root, spec) = temp_spec("exec");
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let outside_target = format!("{home}/locagens-sbx-outside-{}.txt", uuid::Uuid::new_v4());

        let run_wrapped = |shell_script: String| {
            let (program, args) =
                wrap_shell_command("/bin/sh", &["-c".to_string(), shell_script], Some(&spec));
            let mut command = Command::new(&program);
            command.args(&args).current_dir(&root);
            command.output().expect("sandbox-exec must be runnable")
        };

        let inside = run_wrapped("echo hi > inside.txt".to_string());
        let inside_log = format!(
            "{}{}",
            String::from_utf8_lossy(&inside.stdout),
            String::from_utf8_lossy(&inside.stderr)
        );
        // When the test process is itself nested inside an existing sandbox,
        // `sandbox-exec` cannot apply a nested profile; treat that specific
        // restriction gracefully and only assert profile/args construction.
        if inside_log.contains("sandbox_apply") && inside_log.contains("Operation not permitted") {
            eprintln!("skipping: nested sandbox detected");
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        assert!(inside.status.success(), "inside write failed: {inside_log}");
        assert!(root.join("inside.txt").exists());

        let outside = run_wrapped(format!("echo hi > {outside_target}"));
        assert!(
            !outside.status.success(),
            "outside-workspace write must be denied"
        );
        assert!(std::fs::metadata(&outside_target).is_err());

        let _ = std::fs::remove_dir_all(&root);
    }
}
