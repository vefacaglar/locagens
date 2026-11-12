//! Codex/Locagens shared protocol types.
//!
//! This crate is the single home for the data shapes shared between the
//! backend, the CLI and the desktop app: the Codex-style approval/sandbox
//! policy enums (`ApprovalPolicy`, `SandboxMode`) and the persisted data
//! models (`Project`, `Run`, `RunMessage`, `Plan`, `PlanTask`,
//! `PermissionRule`, `Memory`, `UsageLogEntry`).
//!
//! The serde shapes here mirror `apps/backend/src/db.rs` exactly
//! (`camelCase` keys, `default` on every field) so the JSON on disk keeps
//! round-tripping unchanged.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::str::FromStr;

// ----------------------------------------------------------------------
// Approval policy & sandbox mode (Codex semantics)
// ----------------------------------------------------------------------

/// Codex approval policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalPolicy {
    /// Require approval before running anything untrusted.
    Untrusted,
    /// Prompt only when the policy requires it (Locagens `accept_edits`).
    OnRequest,
    /// Prompt when a command fails (Codex's `on-failure`).
    OnFailure,
    /// Never prompt (Locagens `auto`).
    Never,
}

impl ApprovalPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::OnRequest => "on-request",
            Self::OnFailure => "on-failure",
            Self::Never => "never",
        }
    }
}

impl FromStr for ApprovalPolicy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "untrusted" => Ok(Self::Untrusted),
            "on-request" => Ok(Self::OnRequest),
            "on-failure" => Ok(Self::OnFailure),
            "never" => Ok(Self::Never),
            other => Err(format!("unknown approval policy: {other:?}")),
        }
    }
}

impl fmt::Display for ApprovalPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Codex sandbox mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    /// No writes, no network.
    ReadOnly,
    /// Writes inside the workspace, network restricted.
    WorkspaceWrite,
    /// Everything allowed.
    DangerFullAccess,
}

impl SandboxMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
}

impl FromStr for SandboxMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "read-only" => Ok(Self::ReadOnly),
            "workspace-write" => Ok(Self::WorkspaceWrite),
            "danger-full-access" => Ok(Self::DangerFullAccess),
            other => Err(format!("unknown sandbox mode: {other:?}")),
        }
    }
}

impl fmt::Display for SandboxMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Resolves the Locagens run `mode` into Codex approval/sandbox semantics.
pub fn from_run_mode(mode: &str) -> (ApprovalPolicy, SandboxMode) {
    match mode {
        "chat" => (ApprovalPolicy::OnRequest, SandboxMode::ReadOnly),
        "plan" => (ApprovalPolicy::OnRequest, SandboxMode::ReadOnly),
        "accept_edits" => (ApprovalPolicy::OnRequest, SandboxMode::WorkspaceWrite),
        "ask_permissions" => (ApprovalPolicy::Untrusted, SandboxMode::WorkspaceWrite),
        // auto
        _ => (ApprovalPolicy::Never, SandboxMode::DangerFullAccess),
    }
}

// ----------------------------------------------------------------------
// Persisted data models (serde shapes mirror `apps/backend/src/db.rs`)
// ----------------------------------------------------------------------

/// A registered project folder.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Project {
    pub path: String,
    pub name: String,
    pub created_at: String,
}

/// Full run metadata, persisted as `sessions/<run_id>/run.json`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Run {
    pub id: String,
    pub title: String,
    pub task: String,
    pub project_path: String,
    pub project_name: String,
    pub status: String,
    pub provider_id: String,
    pub provider_display_name: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub mode: String,
    /// Custom system prompt (`--system-prompt` in `lcgn`); replaces the base.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Extra system-prompt text (`--append-system-prompt` in `lcgn`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,
    pub error_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_active_at: Option<String>,
}

/// One chat message of a run, persisted as a line of
/// `sessions/<run_id>/messages.jsonl`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RunMessage {
    pub id: String,
    pub run_id: String,
    pub role: String,
    pub agent_role: Option<String>,
    pub agent_name: Option<String>,
    pub provider_id: Option<String>,
    pub provider_display_name: Option<String>,
    pub model: Option<String>,
    pub content: String,
    pub reasoning_content: Option<String>,
    pub raw_response: Option<String>,
    pub created_at: String,
}

/// One task inside a [`Plan`].
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct PlanTask {
    pub id: String,
    pub label: String,
    pub status: String,
}

/// The active plan of a run, persisted as `sessions/<run_id>/plan.json`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Plan {
    pub id: String,
    pub run_id: String,
    pub title: String,
    pub body: Option<String>,
    pub tasks: Value,
    pub status: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// A persisted standing approval for workspace tool calls.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct PermissionRule {
    pub id: i64,
    pub scope: String,
    pub project_path: String,
    pub tool: String,
    pub command: String,
    pub network_domains: Vec<String>,
    pub status: String,
}

/// A durable fact remembered via the `remember` tool / Settings → Memory.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Memory {
    pub id: i64,
    pub scope: String,
    pub project_path: String,
    pub category: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

/// One usage entry, persisted as a line of `usage_logs.jsonl`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageLogEntry {
    pub id: i64,
    pub run_id: String,
    pub agent_role: Option<String>,
    pub provider_id: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cache_hit_rate: f64,
    pub cost: f64,
    pub created_at: String,
    pub duration_ms: Option<i64>,
}

// ----------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_run_mode_mapping() {
        assert_eq!(
            from_run_mode("chat"),
            (ApprovalPolicy::OnRequest, SandboxMode::ReadOnly)
        );
        assert_eq!(
            from_run_mode("plan"),
            (ApprovalPolicy::OnRequest, SandboxMode::ReadOnly)
        );
        assert_eq!(
            from_run_mode("accept_edits"),
            (ApprovalPolicy::OnRequest, SandboxMode::WorkspaceWrite)
        );
        assert_eq!(
            from_run_mode("ask_permissions"),
            (ApprovalPolicy::Untrusted, SandboxMode::WorkspaceWrite)
        );
        assert_eq!(
            from_run_mode("auto"),
            (ApprovalPolicy::Never, SandboxMode::DangerFullAccess)
        );
        // Unknown modes fall back to the autonomous default.
        assert_eq!(from_run_mode("whatever"), from_run_mode("auto"));
    }

    #[test]
    fn policy_as_str_and_from_str_roundtrip() {
        for policy in [
            ApprovalPolicy::Untrusted,
            ApprovalPolicy::OnRequest,
            ApprovalPolicy::OnFailure,
            ApprovalPolicy::Never,
        ] {
            assert_eq!(policy.as_str().parse::<ApprovalPolicy>().unwrap(), policy);
        }
        for mode in [
            SandboxMode::ReadOnly,
            SandboxMode::WorkspaceWrite,
            SandboxMode::DangerFullAccess,
        ] {
            assert_eq!(mode.as_str().parse::<SandboxMode>().unwrap(), mode);
        }
        assert!("bogus".parse::<ApprovalPolicy>().is_err());
        assert!("bogus".parse::<SandboxMode>().is_err());
    }

    #[test]
    fn serde_camel_case_and_roundtrip() {
        let run = Run {
            id: "r1".into(),
            title: "Fix build".into(),
            task: "make it compile".into(),
            project_path: "C:/repos/locagens".into(),
            project_name: "locagens".into(),
            status: "generating".into(),
            provider_id: "openai".into(),
            provider_display_name: "OpenAI".into(),
            model: "gpt-5".into(),
            reasoning_effort: Some("high".into()),
            mode: "accept_edits".into(),
            system_prompt: None,
            append_system_prompt: Some("be terse".into()),
            error_message: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:01:00Z".into(),
            last_active_at: Some("2026-01-01T00:01:00Z".into()),
        };
        let json = serde_json::to_value(&run).unwrap();
        // camelCase keys on the wire.
        assert!(json.get("projectId").is_none());
        assert!(json.get("projectPath").is_some());
        assert!(json.get("providerDisplayName").is_some());
        assert!(json.get("reasoningEffort").is_some());
        // `None` with `skip_serializing_if` is absent.
        assert!(json.get("systemPrompt").is_none());
        assert!(json.get("appendSystemPrompt").is_some());
        assert_eq!(serde_json::from_value::<Run>(json).unwrap(), run);
    }

    #[test]
    fn models_default_and_roundtrip() {
        // Every model implements Default and survives a JSON roundtrip.
        macro_rules! roundtrip {
            ($ty:ty) => {{
                let value: $ty = Default::default();
                let json = serde_json::to_value(&value).unwrap();
                assert_eq!(serde_json::from_value::<$ty>(json).unwrap(), value);
            }};
        }
        roundtrip!(Project);
        roundtrip!(Run);
        roundtrip!(RunMessage);
        roundtrip!(Plan);
        roundtrip!(PlanTask);
        roundtrip!(PermissionRule);
        roundtrip!(Memory);
        roundtrip!(UsageLogEntry);
    }

    #[test]
    fn project_permission_memory_roundtrip_camel_case() {
        let project = Project {
            path: "C:/repos/locagens".into(),
            name: "locagens".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_value(&project).unwrap();
        assert_eq!(json["createdAt"], "2026-01-01T00:00:00Z");
        assert_eq!(serde_json::from_value::<Project>(json).unwrap(), project);

        let rule = PermissionRule {
            id: 7,
            scope: "project".into(),
            project_path: "C:/repos/locagens".into(),
            tool: "run_command".into(),
            command: "cargo test".into(),
            network_domains: vec!["crates.io".into()],
            status: "always".into(),
        };
        let json = serde_json::to_value(&rule).unwrap();
        assert_eq!(json["networkDomains"][0], "crates.io");
        assert_eq!(json["projectPath"], "C:/repos/locagens");
        assert_eq!(serde_json::from_value::<PermissionRule>(json).unwrap(), rule);

        let entry = UsageLogEntry {
            id: 3,
            run_id: "r1".into(),
            agent_role: Some("explorer".into()),
            provider_id: "openai".into(),
            model: "gpt-5".into(),
            input_tokens: 1200,
            output_tokens: 300,
            cache_read_tokens: 100,
            cache_write_tokens: 50,
            cache_hit_rate: 0.25,
            cost: 0.0125,
            created_at: "2026-01-01T00:00:00Z".into(),
            duration_ms: Some(1500),
        };
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["runId"], "r1");
        assert_eq!(json["cacheHitRate"], 0.25);
        assert_eq!(json["durationMs"], 1500);
        assert_eq!(serde_json::from_value::<UsageLogEntry>(json).unwrap(), entry);
    }

    #[test]
    fn plan_tasks_are_free_form_json() {
        let plan = Plan {
            id: "p1".into(),
            run_id: "r1".into(),
            title: "Refactor workspace".into(),
            body: Some("crates/ first".into()),
            tasks: serde_json::json!([
                { "id": "t1", "label": "protocol crate", "status": "done" },
                { "id": "t2", "label": "apply-patch crate", "status": "in_progress" }
            ]),
            status: "active".into(),
            version: 2,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:05:00Z".into(),
        };
        let json = serde_json::to_value(&plan).unwrap();
        assert_eq!(json["tasks"][1]["status"], "in_progress");
        assert_eq!(json["runId"], "r1");
        assert_eq!(serde_json::from_value::<Plan>(json.clone()).unwrap(), plan);
        // `PlanTask` itself roundtrips against the same shape.
        let task: PlanTask = serde_json::from_value(json["tasks"][0].clone()).unwrap();
        assert_eq!(task.label, "protocol crate");
    }
}
