//! Network policy for sandboxed commands (Codex network approval).
//!
//! A sandboxed command may reach, through its per-command proxy:
//! - `[sandbox.network] allowed_domains` from config.toml (agent-dir chain)
//!   (empty by default, like Codex),
//! - the command's own approved `network_domains`,
//! - hosts with a standing `network_access` grant (Allow in Project /
//!   Always Allow on an earlier network prompt),
//! - hosts approved with "Allow for Run" earlier in the same run.
//!
//! Any other host pauses the run with a regular permission request
//! (`awaiting_permission`, tool `network_access`, `networkDomains: [host]`)
//! while the tool call that started the command is still in flight; later
//! (e.g. from a backgrounded exec session) it is refused with a clear error.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use locagens_execpolicy::network_proxy::{DomainAllowlist, NetworkDecider, NetworkPolicy, NetworkRequest};
use serde_json::{json, Value};

use crate::orchestrator::{PermissionPrompt, RunHandle};
use crate::AppState;

/// Tool name of network approval prompts and their standing grants.
pub const NETWORK_ACCESS_TOOL: &str = "network_access";

/// `[sandbox.network] allowed_domains` from the merged config.toml chain.
pub fn configured_allowed_domains() -> Vec<String> {
    crate::config_runtime::sandbox_allowed_domains()
}

/// The `network_domains` argument of a command tool call.
pub fn requested_domains(args: &Value) -> Vec<String> {
    args.get("network_domains")
        .or_else(|| args.get("networkDomains"))
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Every host the command may reach without a prompt. "Allow for Run" hosts
/// live on the drive's `RunHandle`, so they vanish with it (QA-032).
pub fn command_allowlist(state: &AppState, handle: &RunHandle, project_path: &Path, args: &Value) -> DomainAllowlist {
    let project = project_path.display().to_string();
    let mut allowlist = DomainAllowlist::new(configured_allowed_domains());
    for domain in requested_domains(args) {
        allowlist.insert(&domain);
    }
    for rule in state.db.list_permissions() {
        let in_scope = rule.scope == "global" || (rule.scope == "project" && rule.project_path == project);
        if rule.tool == NETWORK_ACCESS_TOOL && rule.status.starts_with("allow") && in_scope {
            for domain in &rule.network_domains {
                allowlist.insert(domain);
            }
        }
    }
    for host in handle.approved_hosts.lock().unwrap_or_else(|p| p.into_inner()).iter() {
        allowlist.insert(host);
    }
    allowlist
}

/// Disarms the network prompt when dropped (the tool call is over).
pub struct ApprovalWindow(Arc<AtomicBool>);

impl Drop for ApprovalWindow {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// The network policy of one command plus the window during which unknown
/// hosts may prompt the user. Keep the window alive for the tool call.
pub fn command_network_policy(
    state: &Arc<AppState>,
    handle: &Arc<RunHandle>,
    project_path: &Path,
    args: &Value,
) -> (NetworkPolicy, ApprovalWindow) {
    let armed = Arc::new(AtomicBool::new(true));
    let approval = Arc::new(NetworkApproval {
        state: Arc::clone(state),
        handle: Arc::clone(handle),
        project_path: project_path.display().to_string(),
        armed: Arc::clone(&armed),
    });
    let decider: NetworkDecider = Arc::new(move |request: NetworkRequest| {
        let approval = Arc::clone(&approval);
        Box::pin(async move { approval.ask(request).await })
    });
    let policy = NetworkPolicy {
        allowlist: command_allowlist(state, handle, project_path, args),
        decider: Some(decider),
    };
    (policy, ApprovalWindow(armed))
}

/// Asks the user about one host through the run's shared permission prompt
/// (`RunHandle::ask_permission`), so it queues behind tool prompts instead of
/// racing them for the slot (QA-028).
struct NetworkApproval {
    state: Arc<AppState>,
    handle: Arc<RunHandle>,
    project_path: String,
    armed: Arc<AtomicBool>,
}

impl NetworkApproval {
    async fn ask(&self, request: NetworkRequest) -> bool {
        if !self.armed.load(Ordering::SeqCst) || self.handle.is_cancelled() {
            return false;
        }
        let tool_call = json!({
            "id": format!("network-{}", uuid::Uuid::new_v4()),
            "function": {
                "name": NETWORK_ACCESS_TOOL,
                "arguments": json!({ "network_domains": [request.host] }).to_string(),
            }
        });
        let preview = json!({
            "tool": NETWORK_ACCESS_TOOL,
            "action": "network",
            "path": "",
            "absolutePath": "",
            "oldContent": Value::Null,
            "newContent": Value::Null,
            "url": format!("{}:{}", request.host, request.port),
            "networkDomains": [request.host],
        });
        let prompt = PermissionPrompt {
            tool_call,
            preview: Some(preview),
            agent_role: None,
            agent_name: None,
            armed: Some(Arc::clone(&self.armed)),
        };
        let Some(decision) = self.handle.ask_permission(&self.state.db, prompt).await else {
            return false;
        };
        match decision.as_str() {
            "allow_run" => {
                self.handle
                    .approved_hosts
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(request.host.clone());
            }
            "allow_project" | "allow_always" => {
                // The one place network grants are persisted (QA-031).
                self.state.db.upsert_permission(
                    if decision == "allow_project" { "project" } else { "global" },
                    &self.project_path,
                    NETWORK_ACCESS_TOOL,
                    "",
                    vec![request.host.clone()],
                    &decision,
                );
            }
            "allow_once" => {}
            // `deny` and unknown answers deny (QA-006).
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_allowed_domains_are_read_from_sandbox_network_table() {
        let parse = |text: &str| -> Vec<String> {
            let config: locagens_config::config_toml::Config = toml::from_str(text).unwrap();
            config.sandbox.map(|sandbox| sandbox.network.allowed_domains).unwrap_or_default()
        };
        let text = "[sandbox.network]\nallowed_domains = [\"registry.npmjs.org\", \"**.crates.io\"]\n";
        assert_eq!(parse(text), vec!["registry.npmjs.org", "**.crates.io"]);
        assert!(parse("[model_providers.x]\nname = \"x\"\n").is_empty());
    }

    /// `run_command` in Build mode runs sandboxed on Windows: workspace
    /// writes work and a non-allowlisted host is refused and reported.
    #[cfg(windows)]
    #[tokio::test]
    async fn run_command_is_sandboxed_and_reports_blocked_hosts() {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let curl = Path::new(&root).join("System32").join("curl.exe");
        if !curl.exists() {
            eprintln!("skipping: curl.exe not available");
            return;
        }
        let ws = std::env::temp_dir().join(format!("locagens-netaccess-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&ws).unwrap();
        let args = json!({
            "command": format!("\"{}\" -sS http://blocked.example/ & echo done> marker.txt", curl.display())
        });
        let output = crate::orchestrator::tools::run_command_with_network(&ws, "accept_edits", &args, None)
            .await
            .unwrap();
        assert!(output.contains("Sandbox blocked network access to: blocked.example"), "{output}");
        assert!(ws.join("marker.txt").exists(), "{output}");
        let _ = locagens_execpolicy::windows_sandbox::revoke_workspace_access(&ws, &[], &[]);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn requested_domains_accept_both_spellings() {
        assert_eq!(requested_domains(&json!({ "network_domains": ["a.com"] })), vec!["a.com"]);
        assert_eq!(requested_domains(&json!({ "networkDomains": ["b.com"] })), vec!["b.com"]);
        assert!(requested_domains(&json!({})).is_empty());
    }
}
