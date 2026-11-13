//! End-to-end checks of the Windows sandbox backend on the host: real
//! restricted tokens, real ACLs, real Job Objects and the network proxy.
#![cfg(windows)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use locagens_execpolicy::network_proxy::{DomainAllowlist, NetworkPolicy};
use locagens_execpolicy::processes::{ProcessManager, SpawnOptions};
use locagens_execpolicy::sandbox::{run_captured, CapturedOutput, SandboxSpec};
use locagens_execpolicy::windows_sandbox::plan::normalize_path;
use locagens_execpolicy::windows_sandbox::revoke_workspace_access;
use locagens_protocol::SandboxMode;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A fresh workspace under the user temp dir (its parent is user-writable,
/// so a sibling directory is a realistic "outside the workspace" target).
struct Workspace {
    root: PathBuf,
    outside: PathBuf,
}

impl Workspace {
    /// Both paths are canonical long forms (CI runners' `%TEMP%` can be an
    /// 8.3 short path such as `C:\Users\RUNNER~1\…`), so they compare equal
    /// to the paths the sandbox plans and revokes.
    fn new(label: &str) -> Self {
        let id = uuid::Uuid::new_v4();
        let root = std::env::temp_dir().join(format!("locagens-sbx-it-{label}-{id}"));
        let outside = std::env::temp_dir().join(format!("locagens-sbx-it-{label}-{id}-outside"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        Self { root: normalize_path(&root), outside: normalize_path(&outside) }
    }

    fn spec(&self, mode: SandboxMode, network: Option<NetworkPolicy>) -> SandboxSpec {
        SandboxSpec {
            mode,
            project_path: self.root.clone(),
            allow_network: false,
            network,
            writable_roots: Vec::new(),
            secret_paths: Vec::new(),
        }
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        // Revoke the ACEs, then delete this workspace's private temp dir.
        let sandbox_temp = normalize_path(&std::env::temp_dir()).join("locagens-sandbox");
        for path in revoke_workspace_access(&self.root, &[], &[]).unwrap_or_default() {
            if path.starts_with(&sandbox_temp) {
                let _ = std::fs::remove_dir_all(path);
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_dir_all(&self.outside);
    }
}

fn base_env() -> HashMap<String, String> {
    ["SystemRoot", "SYSTEMROOT", "windir", "PATH", "PATHEXT", "COMSPEC", "USERPROFILE"]
        .iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_string(), value)))
        .collect()
}

async fn run(ws: &Workspace, spec: &SandboxSpec, script: &str) -> CapturedOutput {
    run_captured(
        "cmd",
        &["/C".to_string(), script.to_string()],
        &ws.root,
        base_env(),
        Some(spec),
        Duration::from_secs(60),
    )
    .await
    .expect("sandboxed command should start")
}

fn text(output: &CapturedOutput) -> String {
    format!(
        "exit={:?} stdout={} stderr={}",
        output.exit_code,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[tokio::test]
async fn workspace_write_allows_workspace_and_temp_but_not_outside() {
    let ws = Workspace::new("ww");
    let spec = ws.spec(SandboxMode::WorkspaceWrite, None);

    let inside = run(&ws, &spec, "echo inside> inside.txt").await;
    assert!(inside.success(), "{}", text(&inside));
    assert!(ws.root.join("inside.txt").exists());

    let nested = run(&ws, &spec, "mkdir sub\\deep && echo nested> sub\\deep\\n.txt").await;
    assert!(nested.success(), "{}", text(&nested));

    let temp = run(&ws, &spec, "echo t> \"%TEMP%\\sandbox-temp.txt\" && echo %TEMP%").await;
    assert!(temp.success(), "{}", text(&temp));
    assert!(String::from_utf8_lossy(&temp.stdout).contains("locagens-sandbox"));

    let sibling = ws.outside.join("outside.txt");
    let outside = run(&ws, &spec, &format!("echo outside> \"{}\"", sibling.display())).await;
    assert!(!outside.success(), "write outside the workspace must fail: {}", text(&outside));
    assert!(!sibling.exists());

    // Control: the identical command succeeds in full-access mode, so the
    // denial above comes from the sandbox, not from quoting or the path.
    let full = ws.spec(SandboxMode::DangerFullAccess, None);
    let control = run(&ws, &full, &format!("echo outside> \"{}\"", sibling.display())).await;
    assert!(control.success(), "{}", text(&control));
    assert!(sibling.exists());

    let home_target = PathBuf::from(std::env::var("USERPROFILE").unwrap())
        .join(format!("locagens-sbx-home-{}.txt", uuid::Uuid::new_v4()));
    let home = run(&ws, &spec, &format!("echo home> \"{}\"", home_target.display())).await;
    assert!(!home.success(), "write to the home dir must fail: {}", text(&home));
    assert!(!home_target.exists());
}

#[tokio::test]
async fn workspace_write_protects_git_metadata_and_locagens_dir() {
    let ws = Workspace::new("protect");
    std::fs::create_dir_all(ws.root.join(".git").join("hooks")).unwrap();
    std::fs::create_dir_all(ws.root.join(".git").join("objects")).unwrap();
    std::fs::write(ws.root.join(".git").join("config"), "[core]\n").unwrap();
    let spec = ws.spec(SandboxMode::WorkspaceWrite, None);

    let hook = run(&ws, &spec, "echo evil> .git\\hooks\\pre-commit").await;
    assert!(!hook.success(), "{}", text(&hook));
    assert!(!ws.root.join(".git").join("hooks").join("pre-commit").exists());

    let config = run(&ws, &spec, "echo evil>> .git\\config").await;
    assert!(!config.success(), "{}", text(&config));
    assert_eq!(std::fs::read_to_string(ws.root.join(".git").join("config")).unwrap(), "[core]\n");

    let rules = run(&ws, &spec, "mkdir .locagens\\rules & echo evil> .locagens\\rules\\default.rules").await;
    assert!(!ws.root.join(".locagens").join("rules").join("default.rules").exists(), "{}", text(&rules));

    // Ordinary git object writes stay possible.
    let objects = run(&ws, &spec, "echo blob> .git\\objects\\x").await;
    assert!(objects.success(), "{}", text(&objects));
}

#[tokio::test]
async fn read_only_mode_denies_workspace_writes() {
    let ws = Workspace::new("ro");
    let spec = ws.spec(SandboxMode::ReadOnly, None);
    std::fs::write(ws.root.join("readme.txt"), "hello").unwrap();

    let read = run(&ws, &spec, "type readme.txt").await;
    assert!(read.success(), "{}", text(&read));
    assert!(String::from_utf8_lossy(&read.stdout).contains("hello"));

    let write = run(&ws, &spec, "echo nope> new.txt").await;
    assert!(!write.success(), "{}", text(&write));
    assert!(!ws.root.join("new.txt").exists());

    // Redirecting to the null device keeps working without any ACL change.
    let null_device = run(&ws, &spec, "echo discarded> nul && echo null-ok").await;
    assert!(null_device.success(), "{}", text(&null_device));
    assert!(String::from_utf8_lossy(&null_device.stdout).contains("null-ok"));
}

fn curl_path() -> Option<PathBuf> {
    let root = std::env::var("SystemRoot").ok()?;
    let curl = Path::new(&root).join("System32").join("curl.exe");
    curl.exists().then_some(curl)
}

/// A local HTTP server answering every request with `body`.
async fn spawn_test_server(body: &'static str) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 4096];
                let _ = socket.read(&mut buffer).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    addr
}

#[tokio::test]
async fn proxy_blocks_non_allowlisted_hosts_and_forwards_allowed_ones() {
    let Some(curl) = curl_path() else {
        eprintln!("skipping: curl.exe not available");
        return;
    };
    let ws = Workspace::new("net");
    let server = spawn_test_server("allowed-body").await;
    let spec = ws.spec(
        SandboxMode::WorkspaceWrite,
        Some(NetworkPolicy::allowlist_only(DomainAllowlist::new(["127.0.0.1"]))),
    );

    let blocked = run(&ws, &spec, &format!("\"{}\" -sS http://blocked.example/", curl.display())).await;
    let blocked_text = text(&blocked);
    assert!(blocked_text.contains("network access to \"blocked.example\" is blocked"), "{blocked_text}");
    assert_eq!(blocked.blocked_hosts, vec!["blocked.example".to_string()]);

    let blocked_tls = run(&ws, &spec, &format!("\"{}\" -sS https://blocked.example/", curl.display())).await;
    assert!(!blocked_tls.success(), "{}", text(&blocked_tls));
    assert_eq!(blocked_tls.blocked_hosts, vec!["blocked.example".to_string()]);

    // `--noproxy ""` forces loopback through the proxy, where it is allowed.
    let allowed = run(
        &ws,
        &spec,
        &format!("\"{}\" -sS --noproxy \"\" http://127.0.0.1:{}/", curl.display(), server.port()),
    )
    .await;
    assert!(allowed.success(), "{}", text(&allowed));
    assert!(String::from_utf8_lossy(&allowed.stdout).contains("allowed-body"));
    assert!(allowed.blocked_hosts.is_empty());
}

/// Waits until the session's full log contains `needle`.
async fn wait_for_output(manager: &ProcessManager, id: &str, needle: &str) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let seen: String = manager.get_logs(id).into_iter().map(|entry| entry.text).collect();
        if seen.contains(needle) {
            return seen;
        }
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {needle:?}: {seen}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn interactive_sessions_work_and_kill_terminates_the_job() {
    let ws = Workspace::new("session");
    let manager = Arc::new(ProcessManager::new());
    let options = SpawnOptions {
        stdin_pipe: true,
        env_clean: true,
        env: Some(base_env()),
        sandbox: Some(ws.spec(SandboxMode::WorkspaceWrite, None)),
        ..SpawnOptions::default()
    };

    // `%line%` stays literal at parse time; `call` expands it after set /p.
    let info = manager.spawn_with_options(
        "set /p line=& call echo GOT-%line%& echo hi> session.txt",
        ws.root.clone(),
        options.clone(),
    );
    assert_eq!(info.status, "running", "{:?}", manager.get_logs(&info.id));
    wait_for_output(&manager, &info.id, "(sandboxed)").await;
    assert!(manager.write_stdin(&info.id, "hello\r\n").await);
    wait_for_output(&manager, &info.id, "GOT-hello").await;
    wait_for_output(&manager, &info.id, "exited with code 0").await;
    assert!(ws.root.join("session.txt").exists());
    manager.drop_session(&info.id);

    let long = manager.spawn_with_options(
        "\"%SystemRoot%\\System32\\ping.exe\" -n 6 127.0.0.1 >nul & echo late> late.txt",
        ws.root.clone(),
        options,
    );
    assert_eq!(long.status, "running");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(manager.kill(&long.id).await, "{:?}", manager.get_logs(&long.id));
    tokio::time::sleep(Duration::from_secs(7)).await;
    assert!(!ws.root.join("late.txt").exists(), "killed job must not keep running");
}

#[tokio::test]
async fn sandbox_setup_errors_fail_the_session_closed() {
    let manager = Arc::new(ProcessManager::new());
    let missing = std::env::temp_dir().join(format!("locagens-sbx-it-missing-{}", uuid::Uuid::new_v4()));
    let info = manager.spawn_with_options(
        "echo should-not-run",
        std::env::temp_dir(),
        SpawnOptions {
            env: Some(base_env()),
            sandbox: Some(SandboxSpec {
                mode: SandboxMode::WorkspaceWrite,
                project_path: missing,
                allow_network: false,
                network: None,
                writable_roots: Vec::new(),
                secret_paths: Vec::new(),
            }),
            ..SpawnOptions::default()
        },
    );
    assert_eq!(info.status, "error");
    let logs: String = manager.get_logs(&info.id).into_iter().map(|entry| entry.text).collect();
    assert!(logs.contains("Windows sandbox"), "{logs}");
    assert!(!logs.contains("should-not-run"));
}

/// The command runs at Low integrity, so files outside the writable roots
/// (unlabeled = Medium) cannot be deleted, renamed, moved into the
/// workspace, hard-linked, re-ACLed or modified — the rights
/// `WRITE_RESTRICTED` alone does not check against the capability SID.
#[tokio::test]
async fn workspace_write_cannot_delete_move_or_modify_files_outside() {
    let ws = Workspace::new("outside");
    let spec = ws.spec(SandboxMode::WorkspaceWrite, None);
    let victim = ws.outside.join("victim.txt");
    std::fs::write(&victim, "original").unwrap();
    let nested = ws.outside.join("dir");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("inner.txt"), "inner").unwrap();
    let victim_path = victim.display().to_string();

    for script in [
        format!("del /f /q \"{victim_path}\""),
        format!("move \"{victim_path}\" moved.txt"),
        format!("ren \"{victim_path}\" renamed.txt"),
        format!("rmdir /s /q \"{}\"", nested.display()),
        format!("echo pwned>> \"{victim_path}\""),
        format!("echo pwned> \"{victim_path}\""),
        format!("\"%SystemRoot%\\System32\\icacls.exe\" \"{victim_path}\" /grant *S-1-1-0:(F)"),
        format!("mklink /h linked.txt \"{victim_path}\""),
    ] {
        let output = run(&ws, &spec, &script).await;
        assert!(victim.exists(), "{script}: {}", text(&output));
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "original", "{script}: {}", text(&output));
    }
    assert_eq!(std::fs::read_to_string(nested.join("inner.txt")).unwrap(), "inner");
    for name in ["moved.txt", "linked.txt"] {
        assert!(!ws.root.join(name).exists(), "{name}");
    }
    assert!(!ws.outside.join("renamed.txt").exists());
    let acl = std::process::Command::new("icacls").arg(&victim).output().unwrap();
    assert!(!String::from_utf8_lossy(&acl.stdout).contains("Everyone"), "the DACL is unchanged");

    // Control: the same delete succeeds in full-access mode.
    let full = ws.spec(SandboxMode::DangerFullAccess, None);
    let control = run(&ws, &full, &format!("del /f /q \"{victim_path}\"")).await;
    assert!(!victim.exists(), "{}", text(&control));
}

/// Credential locations of an injected temp "home" (never the real one)
/// are labeled Medium + no-read-up: the Low-integrity command can neither
/// read, list, copy, delete nor rename them, while ordinary files next to
/// them stay readable. Revoking removes the labels again.
#[tokio::test]
async fn workspace_write_cannot_read_planted_credentials() {
    let ws = Workspace::new("readsecret");
    let home = ws.outside.join("home");
    let ssh = home.join(".ssh");
    let agent_dir = home.join(".locagens");
    std::fs::create_dir_all(&ssh).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(ssh.join("id_ed25519"), "PRIVATE-KEY-MARKER").unwrap();
    std::fs::write(agent_dir.join("auth.json"), "API-KEY-MARKER").unwrap();
    std::fs::write(home.join("notes.txt"), "public-notes").unwrap();
    let secrets = locagens_execpolicy::secret_paths::credential_paths(&home, std::slice::from_ref(&agent_dir));
    assert_eq!(secrets, vec![ssh.clone(), agent_dir.join("auth.json")]);
    let mut spec = ws.spec(SandboxMode::WorkspaceWrite, None);
    spec.secret_paths = secrets.clone();

    let key = ssh.join("id_ed25519").display().to_string();
    let auth = agent_dir.join("auth.json").display().to_string();
    for script in [
        format!("type \"{key}\""),
        format!("type \"{auth}\""),
        format!("copy \"{auth}\" stolen.json"),
        format!("findstr /s MARKER \"{}\\*\"", home.display()),
        format!("del /f /q \"{key}\" & ren \"{auth}\" gone.json"),
    ] {
        let output = run(&ws, &spec, &script).await;
        assert!(!text(&output).contains("MARKER"), "{script}: {}", text(&output));
    }
    let listing = run(&ws, &spec, &format!("dir /b \"{}\"", ssh.display())).await;
    assert!(!String::from_utf8_lossy(&listing.stdout).contains("id_ed25519"), "{}", text(&listing));
    assert!(!ws.root.join("stolen.json").exists());
    assert_eq!(std::fs::read_to_string(ssh.join("id_ed25519")).unwrap(), "PRIVATE-KEY-MARKER");
    assert_eq!(std::fs::read_to_string(agent_dir.join("auth.json")).unwrap(), "API-KEY-MARKER");

    let notes = run(&ws, &spec, &format!("type \"{}\"", home.join("notes.txt").display())).await;
    assert!(String::from_utf8_lossy(&notes.stdout).contains("public-notes"), "{}", text(&notes));

    let revoked = revoke_workspace_access(&ws.root, &[], &secrets).unwrap();
    assert!(revoked.contains(&ssh) && revoked.contains(&agent_dir.join("auth.json")), "{revoked:?}");
    let unlabeled = run(&ws, &ws.spec(SandboxMode::WorkspaceWrite, None), &format!("type \"{auth}\"")).await;
    assert!(String::from_utf8_lossy(&unlabeled.stdout).contains("API-KEY-MARKER"), "{}", text(&unlabeled));
}

/// Low integrity does not break ordinary read-and-run commands.
#[tokio::test]
async fn ordinary_commands_still_work_at_low_integrity() {
    let ws = Workspace::new("ordinary");
    let spec = ws.spec(SandboxMode::WorkspaceWrite, None);
    std::fs::create_dir_all(ws.root.join("src")).unwrap();
    std::fs::write(ws.root.join("src").join("lib.txt"), "workspace-content").unwrap();

    let checks = [
        ("echo sandbox-echo", "sandbox-echo"),
        ("where cmd", "cmd.exe"),
        ("type src\\lib.txt", "workspace-content"),
        ("type \"%SystemRoot%\\System32\\drivers\\etc\\hosts\" >nul && echo hosts-ok", "hosts-ok"),
        ("dir /b \"%SystemRoot%\\System32\\cmd.exe\"", "cmd.exe"),
        ("echo edited>> src\\lib.txt && type src\\lib.txt", "edited"),
        ("ren src\\lib.txt lib2.txt && del src\\lib2.txt && rmdir src && echo cleanup-ok", "cleanup-ok"),
        ("\"%SystemRoot%\\System32\\whoami.exe\" /groups", "S-1-16-4096"),
    ];
    for (script, expected) in checks {
        let output = run(&ws, &spec, script).await;
        assert!(output.success(), "{script}: {}", text(&output));
        assert!(String::from_utf8_lossy(&output.stdout).contains(expected), "{script}: {}", text(&output));
    }

    let git_on_host = std::process::Command::new("git").arg("--version").output().is_ok_and(|out| out.status.success());
    if git_on_host {
        let git = run(&ws, &spec, "git --version && git init -q repo && cd repo && git -c user.name=t -c user.email=t@t commit -q --allow-empty -m init && git log --oneline").await;
        assert!(git.success(), "{}", text(&git));
        assert!(String::from_utf8_lossy(&git.stdout).contains("git version"), "{}", text(&git));
        assert!(String::from_utf8_lossy(&git.stdout).contains("init"), "{}", text(&git));
    } else {
        eprintln!("skipping the git check: git is not installed");
    }
}

/// QA-020: credentials inside a writable root (here a temp "home" granted
/// as an extra writable root, never the real home) cannot be overwritten,
/// appended to or added to, while the rest of that root stays writable.
#[tokio::test]
async fn credentials_inside_writable_roots_are_write_protected() {
    let ws = Workspace::new("secrets");
    let home = ws.outside.join("home");
    let ssh = home.join(".ssh");
    let agent_dir = home.join(".locagens");
    std::fs::create_dir_all(&ssh).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(ssh.join("id_ed25519"), "PRIVATE KEY").unwrap();
    std::fs::write(agent_dir.join("auth.json"), "{}").unwrap();
    let secrets = locagens_execpolicy::secret_paths::credential_paths(&home, std::slice::from_ref(&agent_dir));
    assert_eq!(secrets, vec![ssh.clone(), agent_dir.join("auth.json")]);

    let mut spec = ws.spec(SandboxMode::WorkspaceWrite, None);
    spec.writable_roots = vec![home.clone()];
    spec.secret_paths = secrets.clone();

    let other = run(&ws, &spec, &format!("echo ok> \"{}\"", home.join("notes.txt").display())).await;
    assert!(other.success(), "the writable root itself stays writable: {}", text(&other));
    for script in [
        format!("echo pwned> \"{}\"", ssh.join("id_ed25519").display()),
        format!("echo pwned> \"{}\"", ssh.join("authorized_keys").display()),
        format!("echo pwned>> \"{}\"", agent_dir.join("auth.json").display()),
    ] {
        let output = run(&ws, &spec, &script).await;
        assert!(!output.success(), "{script}: {}", text(&output));
    }
    assert_eq!(std::fs::read_to_string(ssh.join("id_ed25519")).unwrap(), "PRIVATE KEY");
    assert!(!ssh.join("authorized_keys").exists());
    assert_eq!(std::fs::read_to_string(agent_dir.join("auth.json")).unwrap(), "{}");

    let revoked = revoke_workspace_access(&ws.root, std::slice::from_ref(&home), &secrets).unwrap();
    assert!(revoked.contains(&ssh) && revoked.contains(&agent_dir.join("auth.json")), "{revoked:?}");
}

/// QA-015: `allow_network` (network_access = true) is honoured on Windows:
/// no proxy variables; without it the allowlist proxy is injected.
#[tokio::test]
async fn allow_network_skips_the_proxy_and_default_uses_it() {
    let ws = Workspace::new("net");
    let mut spec = ws.spec(SandboxMode::WorkspaceWrite, Some(NetworkPolicy::default()));
    let proxied = run(&ws, &spec, "echo [%LOCAGENS_NETWORK_PROXY_ACTIVE%]").await;
    assert!(String::from_utf8_lossy(&proxied.stdout).contains("[1]"), "{}", text(&proxied));
    spec.allow_network = true;
    let direct = run(&ws, &spec, "echo [%LOCAGENS_NETWORK_PROXY_ACTIVE%]").await;
    assert!(
        String::from_utf8_lossy(&direct.stdout).contains("[%LOCAGENS_NETWORK_PROXY_ACTIVE%]"),
        "{}",
        text(&direct)
    );
}
