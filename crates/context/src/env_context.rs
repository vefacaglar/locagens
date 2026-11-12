//! Codex `<environment_context>` block.
//!
//! Serialized into the conversation exactly like `codex-rs` so models get the
//! same cwd / shell / approval / sandbox signal at the start of a run.

use serde_json::Value;

/// Builds the `<environment_context>` XML block for a run.
pub fn build_environment_context(args: &EnvironmentContextArgs) -> String {
    let shell = match args.shell.as_str() {
        "" => default_shell(),
        other => other,
    };
    let mut roots = String::new();
    for root in &args.workspace_roots {
        roots.push_str(&format!("    <root>{root}</root>\n"));
    }
    if roots.is_empty() {
        roots.push_str(&format!("    <root>{}</root>\n", args.cwd));
    }
    format!(
        "<environment_context>\n  <cwd>{cwd}</cwd>\n  <shell>{shell}</shell>\n  <approval_policy>{approval}</approval_policy>\n  <sandbox_mode>{sandbox}</sandbox_mode>\n  <network_access>{network}</network_access>\n  <workspace_roots>\n{roots}  </workspace_roots>\n  <current_date>{date}</current_date>\n</environment_context>",
        cwd = args.cwd,
        shell = shell,
        approval = args.approval_policy,
        sandbox = args.sandbox_mode,
        network = args.network_access,
        date = args.current_date,
    )
}

fn default_shell() -> &'static str {
    if cfg!(windows) {
        "powershell"
    } else if let Ok(shell) = std::env::var("SHELL") {
        if shell.ends_with("zsh") {
            "zsh"
        } else {
            "bash"
        }
    } else {
        "bash"
    }
}

/// Inputs for [`build_environment_context`].
pub struct EnvironmentContextArgs {
    pub cwd: String,
    pub shell: String,
    pub approval_policy: String,
    pub sandbox_mode: String,
    pub network_access: String,
    pub workspace_roots: Vec<String>,
    pub current_date: String,
}

impl EnvironmentContextArgs {
    /// Derives the context from a run's mode and project path (Codex defaults).
    pub fn for_run(mode: &str, project_path: &str, workspace_roots: Vec<String>) -> Self {
        let (approval, sandbox) = locagens_protocol::from_run_mode(mode);
        Self {
            cwd: project_path.to_string(),
            shell: default_shell().to_string(),
            approval_policy: approval.as_str().to_string(),
            sandbox_mode: sandbox.as_str().to_string(),
            network_access: "restricted".to_string(),
            workspace_roots,
            current_date: chrono::Utc::now().format("%Y-%m-%d").to_string(),
        }
    }
}

/// Convenience wrapper taking a run's mode + project path.
pub fn environment_context_for_run(mode: &str, project_path: &str) -> String {
    build_environment_context(&EnvironmentContextArgs::for_run(
        mode,
        project_path,
        vec![project_path.to_string()],
    ))
}

/// Parses an `<environment_context>` block back into a JSON object (used by
/// the desktop inspector to render the block).
pub fn context_to_json(block: &str) -> Value {
    let field = |name: &str| -> String {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        block
            .find(&open)
            .and_then(|start| block[start + open.len()..].find(&close).map(|end| block[start + open.len()..start + open.len() + end].to_string()))
            .unwrap_or_default()
    };
    serde_json::json!({
        "cwd": field("cwd"),
        "shell": field("shell"),
        "approval_policy": field("approval_policy"),
        "sandbox_mode": field("sandbox_mode"),
        "network_access": field("network_access"),
        "current_date": field("current_date"),
        "raw": block,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_contains_all_fields() {
        let block = build_environment_context(&EnvironmentContextArgs {
            cwd: "C:/repos/locagens".into(),
            shell: "powershell".into(),
            approval_policy: "on-request".into(),
            sandbox_mode: "workspace-write".into(),
            network_access: "restricted".into(),
            workspace_roots: vec!["C:/repos/locagens".into()],
            current_date: "2025-01-01".into(),
        });
        assert!(block.starts_with("<environment_context>"));
        assert!(block.contains("<cwd>C:/repos/locagens</cwd>"));
        assert!(block.contains("<shell>powershell</shell>"));
        assert!(block.contains("<approval_policy>on-request</approval_policy>"));
        assert!(block.contains("<sandbox_mode>workspace-write</sandbox_mode>"));
        assert!(block.contains("<network_access>restricted</network_access>"));
        assert!(block.contains("<root>C:/repos/locagens</root>"));
        assert!(block.contains("<current_date>2025-01-01</current_date>"));
        assert!(block.ends_with("</environment_context>"));
    }

    #[test]
    fn run_modes_map_to_codex_policies() {
        let block = environment_context_for_run("accept_edits", "/w");
        assert!(block.contains("<approval_policy>on-request</approval_policy>"));
        assert!(block.contains("<sandbox_mode>workspace-write</sandbox_mode>"));
        let block = environment_context_for_run("auto", "/w");
        assert!(block.contains("<approval_policy>never</approval_policy>"));
        assert!(block.contains("<sandbox_mode>danger-full-access</sandbox_mode>"));
        let block = environment_context_for_run("plan", "/w");
        assert!(block.contains("<sandbox_mode>read-only</sandbox_mode>"));
        let block = environment_context_for_run("ask_permissions", "/w");
        assert!(block.contains("<approval_policy>untrusted</approval_policy>"));
        // Date is today.
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        assert!(block.contains(&format!("<current_date>{today}</current_date>")));
    }

    #[test]
    fn roundtrip_to_json() {
        let block = environment_context_for_run("accept_edits", "/w");
        let json = context_to_json(&block);
        assert_eq!(json["cwd"], "/w");
        assert_eq!(json["approval_policy"], "on-request");
        assert_eq!(json["sandbox_mode"], "workspace-write");
        assert_eq!(json["network_access"], "restricted");
    }
}
