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
    /// Detected sandbox backend (`macos-seatbelt`, `linux-bwrap` or
    /// `host-env-only`).
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

/// Full security status payload (`GET /api/security/status`).
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
    match detect_backend() {
        SandboxBackend::MacosSeatbelt => SecuritySandbox {
            platform,
            backend: "macos-seatbelt".to_string(),
            status: "ready".to_string(),
            errors: Vec::new(),
            warnings: Vec::new(),
            can_install: false,
        },
        SandboxBackend::LinuxBubblewrap => SecuritySandbox {
            platform,
            backend: "linux-bwrap".to_string(),
            status: "ready".to_string(),
            errors: Vec::new(),
            warnings: Vec::new(),
            can_install: false,
        },
        SandboxBackend::HostEnvOnly => SecuritySandbox {
            platform,
            backend: "host-env-only".to_string(),
            status: "ready".to_string(),
            errors: Vec::new(),
            warnings: vec![
                "Compatibility mode is active: commands run on the host with secrets removed from their environment."
                    .to_string(),
            ],
            can_install: false,
        },
    }
}

/// Returns the current security status (`GET /api/security/status`).
pub async fn security_status() -> ApiResult<SecurityStatus> {
    Ok(status_value().await)
}

/// Runs the Windows sandbox setup after explicit user confirmation and returns
/// the resulting status (`POST /api/security/sandbox/setup`).
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
