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

/// Runs `git` in `project_path` and returns stdout; stderr (or the spawn
/// failure) becomes the error message.
pub async fn git(project_path: &Path, args: &[&str]) -> Result<String, String> {
    check_sandbox_metadata(project_path)?;
    let mut command = tokio::process::Command::new("git");
    crate::util::hide_console_window(command.as_std_mut());
    let output = command
        .args(args)
        .current_dir(project_path)
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
pub async fn git_bytes(
    project_path: &Path,
    args: &[&str],
    env: &[(&str, &Path)],
    stdin: Option<Vec<u8>>,
) -> Result<Vec<u8>, String> {
    use tokio::io::AsyncWriteExt;
    check_sandbox_metadata(project_path)?;
    let mut command = tokio::process::Command::new("git");
    crate::util::hide_console_window(command.as_std_mut());
    command
        .args(args)
        .current_dir(project_path)
        .stdin(if stdin.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command.spawn().map_err(|err| format!("Failed to run git: {err}"))?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(&input).await.map_err(|err| format!("git stdin: {err}"))?;
        drop(pipe);
    }
    let output = child.wait_with_output().await.map_err(|err| format!("git: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("git {}: {stderr}", args.first().unwrap_or(&"")));
    }
    Ok(output.stdout)
}
