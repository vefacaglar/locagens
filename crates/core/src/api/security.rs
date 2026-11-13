//! Security status & sandbox setup facade.

use crate::api::error::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};

/// API listener part of the security status.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SecurityApi {
    pub bind_address: String,
    pub authenticated: bool,
}

/// Command sandbox part of the security status.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SecuritySandbox {
    pub platform: String,
    /// Detected sandbox backend (`macos-seatbelt`, `linux-bwrap`,
    /// `windows-restricted-token` or `host-env-only`).
    #[serde(default)]
    pub backend: String,
    pub status: String,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub can_install: bool,
}

/// Workspace policy part of the security status.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SecurityPolicy {
    pub command_network: String,
    pub workspace_writes_only: bool,
}

/// Full security status payload.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SecurityStatus {
    pub api: SecurityApi,
    pub sandbox: SecuritySandbox,
    pub policy: SecurityPolicy,
}

/// Mirrors `CommandSandbox.status()` in compatibility mode: commands run on the
/// host with secrets removed from their environment, so the sandbox reports
/// ready. Native sandboxing arrives with the pi-based orchestrator phase.
async fn status_value() -> SecurityStatus {
    SecurityStatus {
        api: SecurityApi {
            bind_address: "127.0.0.1".to_string(),
            authenticated: true,
        },
        sandbox: sandbox_status(),
        policy: SecurityPolicy {
            command_network: "deny_by_default".to_string(),
            workspace_writes_only: true,
        },
    }
}

/// Builds the sandbox part of the status from the detected native backend.
fn sandbox_status() -> SecuritySandbox {
    use locagens_execpolicy::sandbox::{detect_backend, SandboxBackend};
    let platform = std::env::consts::OS.to_string();
    let unix_network_note = "Network is all-or-nothing in Build mode: offline unless a command names network_domains (or network_access is enabled), which opens the whole network for that command - there is no per-domain proxy on this platform.";
    match detect_backend() {
        SandboxBackend::MacosSeatbelt => SecuritySandbox {
            platform,
            backend: "macos-seatbelt".to_string(),
            status: "ready".to_string(),
            errors: Vec::new(),
            warnings: vec![unix_network_note.to_string()],
            can_install: false,
        },
        SandboxBackend::LinuxBubblewrap => SecuritySandbox {
            platform,
            backend: "linux-bwrap".to_string(),
            status: "ready".to_string(),
            errors: Vec::new(),
            warnings: vec![unix_network_note.to_string()],
            can_install: false,
        },
        SandboxBackend::WindowsRestrictedToken => SecuritySandbox {
            platform,
            backend: "windows-restricted-token".to_string(),
            status: "ready".to_string(),
            errors: Vec::new(),
            warnings: vec![
                "Windows network approval is best-effort: the allowlist is an HTTP proxy that programs can bypass (curl --noproxy, raw sockets), and local services stay reachable. Do not rely on it to contain a hostile command."
                    .to_string(),
                "Windows Build mode hides only the known credential files (~/.locagens/auth.json, ~/.ssh, cloud and registry credentials) from sandboxed commands; they can still read everything else you can, such as .env files and other projects."
                    .to_string(),
                "Windows Build mode blocks writes and deletes outside the workspace, except in low-integrity folders: a sandboxed command can still delete (not modify) files in other workspaces you used with the sandbox and in folders such as AppData\\LocalLow."
                    .to_string(),
                "Windows cannot stop a sandboxed command from renaming .git, .git/hooks, .git/config or .locagens and creating a replacement; Locagens detects this when the command (and its background processes) end or before it runs git itself, moves the replacement to a *.locagens-quarantine-* folder, restores the original and flags the command. Your own git runs in between are not covered."
                    .to_string(),
            ],
            can_install: false,
        },
        SandboxBackend::HostEnvOnly => SecuritySandbox {
            platform,
            backend: "host-env-only".to_string(),
            status: "unavailable".to_string(),
            errors: vec![locagens_execpolicy::sandbox::unavailable_backend_message()],
            warnings: Vec::new(),
            can_install: false,
        },
    }
}

/// Returns the current security status.
pub async fn security_status() -> ApiResult<SecurityStatus> {
    Ok(status_value().await)
}

/// Runs the Windows sandbox setup after explicit user confirmation and returns
/// the resulting status.
pub async fn setup_sandbox(confirmed: bool) -> ApiResult<SecurityStatus> {
    if !confirmed {
        return Err(ApiError::bad_request(
            "Explicit user confirmation is required.",
        ));
    }
    if !cfg!(windows) {
        return Err(ApiError::bad_request(
            "Sandbox setup is only required on Windows.",
        ));
    }
    // The Rust backend runs commands in compatibility mode; nothing to
    // install until the native sandbox runtime is wired up.
    Ok(status_value().await)
}
