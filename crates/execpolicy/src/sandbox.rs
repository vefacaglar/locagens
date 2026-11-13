//! Native OS sandboxing: macOS Seatbelt (`sandbox-exec`) and Linux
//! Bubblewrap (`bwrap`), ported from Codex's sandbox policy model.
//!
//! Commands run through [`wrap_shell_command`] are wrapped into a sandbox
//! process when the run's [`SandboxMode`] is not `danger-full-access` and a
//! native backend is detected; otherwise the command runs unchanged (host
//! with a secret-stripped environment only).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use locagens_protocol::SandboxMode;

/// Available native sandbox backends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxBackend {
    /// macOS Seatbelt via `/usr/bin/sandbox-exec`.
    MacosSeatbelt,
    /// Linux Bubblewrap via `bwrap`.
    LinuxBubblewrap,
    /// No native backend: commands run on the host (secrets stripped).
    HostEnvOnly,
}

/// The sandbox parameters for one command execution.
#[derive(Clone, Debug)]
pub struct SandboxSpec {
    pub mode: SandboxMode,
    pub project_path: PathBuf,
    pub allow_network: bool,
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
    #[allow(unreachable_code)]
    SandboxBackend::HostEnvOnly
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
        SandboxBackend::HostEnvOnly => (program.to_string(), args.to_vec()),
    }
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
        };
        (canonical, spec)
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

        // The wrapped argv carries the profile and every parameter.
        let (program, args) = wrap_shell_command("/bin/sh", &["-c".to_string(), "true".to_string()], Some(&spec));
        assert_eq!(program, "/usr/bin/sandbox-exec");
        assert!(args.contains(&profile));
        assert!(args.iter().any(|arg| arg.starts_with("-DPROJECT_ROOT=")));
        assert!(args.iter().any(|arg| arg.starts_with("-DGIT_CONFIG_FILE=")));

        // Full-access mode runs unchanged.
        let (program, args) = wrap_shell_command(
            "/bin/sh",
            &["-c".to_string(), "true".to_string()],
            Some(&SandboxSpec {
                mode: SandboxMode::DangerFullAccess,
                project_path: root.clone(),
                allow_network: true,
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
