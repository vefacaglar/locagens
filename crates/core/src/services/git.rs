//! Thin `git` process runners shared by the project facade and the
//! snapshot store.

use std::path::Path;

/// Repairs git / `.locagens` metadata a still-running sandboxed command in
/// `project_path` replaced (Windows; see `locagens_execpolicy::sandbox::
/// repair_workspace_metadata`), so Locagens' own unsandboxed git never
/// runs planted hooks or config. A repair is an error carrying the note.
pub fn check_sandbox_metadata(project_path: &Path) -> Result<(), String> {
    let reports = locagens_execpolicy::sandbox::repair_workspace_metadata(project_path);
    if reports.is_empty() {
        return Ok(());
    }
    Err(locagens_execpolicy::sandbox::sandbox_violation_note(&reports))
}

/// Config overrides applied to every git Locagens runs itself. The project's
/// own `.git/config` is repository-controlled (an archive or copied folder
/// can set `core.fsmonitor`, `diff.external`, a textconv driver, hooks or a
/// pager that git would execute), so those are neutralised before any trust
/// decision (FR-029).
const HARDENING_ARGS: &[&str] = &[
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.untrackedCache=false",
    "-c",
    "diff.external=",
    "-c",
    "core.pager=cat",
    "-c",
    "core.editor=true",
    "-c",
    "core.askPass=",
    "-c",
    "protocol.ext.allow=never",
    "--no-pager",
];

/// The null device, used as an empty hooks directory.
const NULL_DEVICE: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

/// The one place a git command is built: hardened config, no system config,
/// no prompts, no optional locks. Callers add the subcommand and its args
/// (diff-like subcommands should also pass `--no-ext-diff --no-textconv`).
pub fn hardened_git_command(project_path: &Path) -> std::process::Command {
    let mut command = std::process::Command::new("git");
    crate::util::hide_console_window(&mut command);
    command
        .args(HARDENING_ARGS)
        .arg("-c")
        .arg(format!("core.hooksPath={NULL_DEVICE}"))
        .current_dir(project_path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_EXTERNAL_DIFF")
        .env_remove("GIT_PAGER")
        .env_remove("GIT_ASKPASS")
        .env_remove("GIT_SSH_COMMAND");
    command
}

/// Builds the hardened command for `args` (subcommand first); diff-producing
/// subcommands also get `--no-ext-diff --no-textconv`.
pub fn git_command(project_path: &Path, args: &[&str]) -> std::process::Command {
    let mut command = hardened_git_command(project_path);
    if let Some((subcommand, rest)) = args.split_first() {
        command.arg(subcommand);
        if *subcommand == "push" {
            // Pushing needs the user's system credential helper and may
            // prompt; the rest of the hardening still applies.
            command.env_remove("GIT_CONFIG_NOSYSTEM").env_remove("GIT_TERMINAL_PROMPT");
        }
        if matches!(*subcommand, "diff" | "show" | "log") {
            command.args(["--no-ext-diff", "--no-textconv"]);
        }
        command.args(rest);
    }
    command
}

/// Runs `git` in `project_path` and returns stdout; stderr (or the spawn
/// failure) becomes the error message.
pub async fn git(project_path: &Path, args: &[&str]) -> Result<String, String> {
    check_sandbox_metadata(project_path)?;
    let mut command = tokio::process::Command::from(git_command(project_path, args));
    command.kill_on_drop(true);
    let output = command
        .output()
        .await
        .map_err(|err| format!("Failed to run git: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("git exited with status {}", output.status)
        } else {
            stderr
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Runs `git` with optional extra env and stdin, returning raw stdout bytes.
/// Stdin is written from its own task while stdout is read, so a command that
/// answers while it reads (`hash-object --stdin-paths`) cannot fill both pipes
/// and deadlock (FR-006).
pub async fn git_bytes(
    project_path: &Path,
    args: &[&str],
    env: &[(&str, &Path)],
    stdin: Option<Vec<u8>>,
) -> Result<Vec<u8>, String> {
    use tokio::io::AsyncWriteExt;
    check_sandbox_metadata(project_path)?;
    let mut command = tokio::process::Command::from(git_command(project_path, args));
    command
        .stdin(if stdin.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command.spawn().map_err(|err| format!("Failed to run git: {err}"))?;
    let writer = match (stdin, child.stdin.take()) {
        (Some(input), Some(mut pipe)) => Some(tokio::spawn(async move {
            let result = pipe.write_all(&input).await;
            drop(pipe);
            result
        })),
        _ => None,
    };
    let output = child.wait_with_output().await.map_err(|err| format!("git: {err}"))?;
    if let Some(writer) = writer {
        // A broken pipe means git exited early; its status reports why.
        if let Ok(Err(err)) = writer.await {
            if output.status.success() && err.kind() != std::io::ErrorKind::BrokenPipe {
                return Err(format!("git stdin: {err}"));
            }
        }
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("git {}: {stderr}", args.first().unwrap_or(&"")));
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("locagens-git-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn init(root: &Path) {
        let status = std::process::Command::new("git").args(["init", "-q"]).current_dir(root).status().unwrap();
        assert!(status.success());
    }

    /// FR-006: the path list is far larger than any pipe buffer; writing it
    /// all before reading stdout deadlocked.
    #[tokio::test]
    async fn hash_object_stdin_paths_with_a_huge_list_does_not_deadlock() {
        let root = temp_dir("deadlock");
        init(&root);
        std::fs::write(root.join("a.txt"), "x").unwrap();
        // About 3 MB of input: one real file listed many times with a long
        // detour prefix.
        let line = format!("{}a.txt", "./".repeat(50));
        let input = vec![line; 30_000].join("\n").into_bytes();
        let run = git_bytes(&root, &["hash-object", "--no-filters", "--stdin-paths"], &[], Some(input));
        let output = tokio::time::timeout(std::time::Duration::from_secs(60), run)
            .await
            .expect("git_bytes must not deadlock")
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output).lines().count(), 30_000);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// FR-029: a repository-controlled `core.fsmonitor` must not run.
    #[tokio::test]
    async fn repository_fsmonitor_config_is_not_executed() {
        let root = temp_dir("fsmonitor");
        init(&root);
        let marker = root.join("pwned.marker");
        let script = root.join("evil.sh");
        let marker_path = marker.display().to_string().replace('\\', "/");
        std::fs::write(&script, format!("#!/bin/sh\ntouch '{marker_path}'\n")).unwrap();
        let script_path = script.display().to_string().replace('\\', "/");
        let status = std::process::Command::new("git")
            .args(["config", "core.fsmonitor", &format!("sh '{script_path}'")])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(status.success());
        git(&root, &["status", "--porcelain"]).await.unwrap();
        git(&root, &["ls-files", "-co", "--exclude-standard"]).await.unwrap();
        assert!(!marker.exists(), "core.fsmonitor must be neutralised");
        let _ = std::fs::remove_dir_all(&root);
    }
}
