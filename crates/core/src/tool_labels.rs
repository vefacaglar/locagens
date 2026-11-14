//! Frontend-neutral tool-call labels (UX-008): what a call did, as a verb and
//! a target ("Ran" `cargo test`), and how it ended (ok / failed / denied).
//! The TUI and the desktop render these the same way; the desktop translates
//! the verb by [`ToolVerb::key`].

use serde_json::Value;

/// What a tool call did, as a past-tense verb.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolVerb {
    Ran,
    SentInput,
    Read,
    Listed,
    Searched,
    Wrote,
    Edited,
    AppliedPatch,
    Deleted,
    CreatedDir,
    Moved,
    Fetched,
    SpawnedAgents,
    SpawnedAgent,
    WaitedForAgents,
    LoadedSkill,
    Asked,
    TitledChat,
    UpdatedPlan,
    Remembered,
    /// Agent tools (`wait_agent` …) and unknown tools: the tool name.
    Tool(String),
}

impl ToolVerb {
    /// Stable id of the verb (`ran`, `edited` …); `None` for [`ToolVerb::Tool`].
    pub fn key(&self) -> Option<&'static str> {
        Some(match self {
            ToolVerb::Ran => "ran",
            ToolVerb::SentInput => "sent_input",
            ToolVerb::Read => "read",
            ToolVerb::Listed => "listed",
            ToolVerb::Searched => "searched",
            ToolVerb::Wrote => "wrote",
            ToolVerb::Edited => "edited",
            ToolVerb::AppliedPatch => "applied_patch",
            ToolVerb::Deleted => "deleted",
            ToolVerb::CreatedDir => "created_dir",
            ToolVerb::Moved => "moved",
            ToolVerb::Fetched => "fetched",
            ToolVerb::SpawnedAgents => "spawned_agents",
            ToolVerb::SpawnedAgent => "spawned_agent",
            ToolVerb::WaitedForAgents => "waited_agents",
            ToolVerb::LoadedSkill => "loaded_skill",
            ToolVerb::Asked => "asked",
            ToolVerb::TitledChat => "titled_chat",
            ToolVerb::UpdatedPlan => "updated_plan",
            ToolVerb::Remembered => "remembered",
            ToolVerb::Tool(_) => return None,
        })
    }

    /// English text ("Ran", "Applied patch"; agent tools read "wait agent").
    pub fn english(&self) -> String {
        match self {
            ToolVerb::Ran => "Ran",
            ToolVerb::SentInput => "Sent input to",
            ToolVerb::Read => "Read",
            ToolVerb::Listed => "Listed",
            ToolVerb::Searched => "Searched",
            ToolVerb::Wrote => "Wrote",
            ToolVerb::Edited => "Edited",
            ToolVerb::AppliedPatch => "Applied patch",
            ToolVerb::Deleted => "Deleted",
            ToolVerb::CreatedDir => "Created dir",
            ToolVerb::Moved => "Moved",
            ToolVerb::Fetched => "Fetched",
            ToolVerb::SpawnedAgents => "Spawned agents",
            ToolVerb::SpawnedAgent => "Spawned agent",
            ToolVerb::WaitedForAgents => "Waited for agents",
            ToolVerb::LoadedSkill => "Loaded skill",
            ToolVerb::Asked => "Asked",
            ToolVerb::TitledChat => "Titled chat",
            ToolVerb::UpdatedPlan => "Updated plan",
            ToolVerb::Remembered => "Remembered",
            ToolVerb::Tool(name) => return name.replace('_', " "),
        }
        .to_string()
    }
}

/// `(verb, target)` of a call, e.g. `(Ran, "cargo test")`.
pub fn describe_tool(name: &str, args: &Value) -> (ToolVerb, String) {
    let text = |key: &str| args.get(key).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_string);
    let first = |keys: &[&str]| keys.iter().find_map(|key| text(key)).unwrap_or_default();
    match name {
        "run_command" | "exec_command" => (ToolVerb::Ran, first(&["command", "cmd"])),
        "write_stdin" => (ToolVerb::SentInput, first(&["session_id", "chars"])),
        "read_file" | "view_image" => (ToolVerb::Read, first(&["path"])),
        "list_directory" => (ToolVerb::Listed, first(&["path"])),
        "search_files" => (ToolVerb::Searched, first(&["query", "pattern", "path"])),
        "write_file" => (ToolVerb::Wrote, first(&["path"])),
        "edit_file" => (ToolVerb::Edited, first(&["path"])),
        "apply_patch" => (ToolVerb::AppliedPatch, String::new()),
        "delete_file" => (ToolVerb::Deleted, first(&["path"])),
        "create_directory" => (ToolVerb::CreatedDir, first(&["path"])),
        "move_file" => (ToolVerb::Moved, format!("{} -> {}", first(&["from", "path"]), first(&["to"]))),
        "fetch_url" => (ToolVerb::Fetched, first(&["url"])),
        "spawn_agents" => (ToolVerb::SpawnedAgents, String::new()),
        "spawn_agent" => (ToolVerb::SpawnedAgent, first(&["task_name", "name"])),
        "load_skill" => (ToolVerb::LoadedSkill, first(&["name"])),
        "ask_user_question" | "request_user_input" => (ToolVerb::Asked, String::new()),
        "set_chat_title" => (ToolVerb::TitledChat, first(&["title"])),
        "update_plan" => (ToolVerb::UpdatedPlan, first(&["title"])),
        "remember" => (ToolVerb::Remembered, String::new()),
        "wait_agent" => (ToolVerb::WaitedForAgents, first(&["target"])),
        "send_input" | "send_message" | "followup_task" | "interrupt_agent" | "close_agent" | "list_agents" => {
            (ToolVerb::Tool(name.to_string()), first(&["target", "agent_id", "id"]))
        }
        other => (ToolVerb::Tool(other.to_string()), first(&["path", "command", "url", "query", "name", "title"])),
    }
}

/// How a tool call ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolOutcome {
    /// No result yet.
    Running,
    Ok,
    /// The tool reported an error.
    Failed,
    /// The user (or the policy) refused it.
    Denied,
}

/// Outcome from the tool result text (`None` while the call runs). Mirrors
/// how the orchestrator words failures (`Tool error…`) and denials
/// (`The user denied permission…`).
pub fn tool_outcome(result: Option<&str>) -> ToolOutcome {
    match result {
        None => ToolOutcome::Running,
        Some(text) if text.starts_with("The user denied permission") => ToolOutcome::Denied,
        Some(text) if text.starts_with("Tool error") => ToolOutcome::Failed,
        Some(_) => ToolOutcome::Ok,
    }
}

/// Tools whose call counts as running a command.
pub fn is_command_tool(name: &str) -> bool {
    matches!(name, "run_command" | "exec_command" | "write_stdin")
}

/// Tools whose call changes files.
pub fn is_edit_tool(name: &str) -> bool {
    matches!(name, "write_file" | "edit_file" | "apply_patch" | "delete_file" | "move_file" | "create_directory")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn describes_common_tools() {
        assert_eq!(describe_tool("run_command", &json!({"command": "cargo test"})), (ToolVerb::Ran, "cargo test".into()));
        assert_eq!(describe_tool("edit_file", &json!({"path": "src/a.rs"})), (ToolVerb::Edited, "src/a.rs".into()));
        assert_eq!(describe_tool("move_file", &json!({"from": "a", "to": "b"})).1, "a -> b");
        let (verb, target) = describe_tool("wait_agent", &json!({"timeout_ms": 30000}));
        assert_eq!((verb.key(), verb.english(), target.as_str()), (Some("waited_agents"), "Waited for agents".into(), ""));
        let (verb, target) = describe_tool("close_agent", &json!({"target": "scan"}));
        assert_eq!((verb.key(), verb.english(), target.as_str()), (None, "close agent".into(), "scan"));
        assert_eq!(ToolVerb::AppliedPatch.key(), Some("applied_patch"));
    }

    #[test]
    fn outcome_tells_failures_and_denials_apart() {
        assert_eq!(tool_outcome(None), ToolOutcome::Running);
        assert_eq!(tool_outcome(Some("ok output")), ToolOutcome::Ok);
        assert_eq!(tool_outcome(Some("Tool error: boom")), ToolOutcome::Failed);
        assert_eq!(tool_outcome(Some("The user denied permission for run_command.")), ToolOutcome::Denied);
    }
}
