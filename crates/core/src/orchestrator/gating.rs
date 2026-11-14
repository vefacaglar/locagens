//! Permission gating of workspace tools: exec policy, standing grants and
//! interactive prompts.

use super::*;
use locagens_execpolicy::stdin_input;

/// Pre-prompt verdict for one line typed into a shell / REPL session.
enum TypedLineGate {
    /// Known-safe, allow-listed or covered by a grant: send it.
    Send,
    /// Forbidden by an execpolicy rule.
    Blocked(String),
    /// Needs a prompt; `once_only` for dangerous lines (no standing grant).
    Prompt { once_only: bool },
}

impl AgentLoop {
    /// Loads the merged Starlark `.rules` exec policy for this run: every
    /// agent-directory `rules` folder plus, for a trusted project folder
    /// only, `<project>/.locagens/rules` (an untrusted repo could otherwise
    /// ship `allow` rules for any command).
    pub(super) fn load_exec_policy(&self) -> locagens_execpolicy::rules::Policy {
        let mut dirs: Vec<PathBuf> = crate::paths::agent_dir_chain()
            .into_iter()
            .map(|dir| dir.join("rules"))
            .collect();
        if crate::services::project_trust::is_trusted(&self.project_path) {
            dirs.push(self.project_path.join(".locagens").join("rules"));
        }
        locagens_execpolicy::rules::Policy::load_from_dirs(&dirs)
    }

    // -- Permission gating -------------------------------------------------

    /// Network allowlist + live domain approval for a sandboxed command.
    pub(super) fn command_network_policy(
        &self,
        args: &Value,
    ) -> (locagens_execpolicy::network_proxy::NetworkPolicy, network_access::ApprovalWindow) {
        network_access::command_network_policy(&self.state, &self.handle, &self.project_path, args)
    }

    pub(super) fn standing_grant(&self, name: &str, args: &Value) -> bool {
        let project_path = self.project_path.display().to_string();
        let key = grant_key(name, args);
        let domains = sorted_domains(args);
        self.state
            .db
            .list_permissions()
            .iter()
            .any(|rule| standing_grant_matches(rule, name, &key, &domains, &project_path))
    }

    pub(super) async fn gated_execute(&self, call_id: &str, name: &str, args: &Value) -> Result<String, AgentError> {
        // Codex command safety + Starlark `.rules` execpolicy: under
        // accept_edits (on-request + workspace-write) allow-listed commands
        // run without a prompt; forbidden rules block outright. Requested
        // network domains are only granted through a prompt (or a matching
        // standing grant), never by the known-safe bypass.
        let is_command = name == "run_command" || name == "exec_command";
        let mut dangerous = false;
        if is_command {
            let cmd_str = command_safety::command_from_args(args);
            dangerous = command_safety::is_dangerous_command(&cmd_str);
            let policy = self.load_exec_policy();
            let requests_network = !sorted_domains(args).is_empty();
            match command_safety::command_bypasses_prompt_with_policy(&self.mode, &cmd_str, Some(&policy)) {
                Ok(true) if !requests_network => {
                    return self.execute_tool(name, args).await.map_err(AgentError::Failed)
                }
                Ok(_) => {}
                Err(reason) => {
                    return Ok(json!({
                        "ok": false,
                        "error": format!("Command blocked by execpolicy rule: {reason}")
                    })
                    .to_string());
                }
            }
        }
        // Outside Full Access, writes to protected paths (`.git`, `.locagens`,
        // … — what the command sandbox also protects) and dangerous commands
        // always prompt: no standing grant or "Allow for Run" covers them,
        // and the answer approves this one call only.
        let protected = if self.mode == "auto" {
            None
        } else {
            tools::protected_mutation_target(&self.project_path, name, args)
        };
        let must_prompt = protected.is_some() || (dangerous && self.mode != "auto");
        if !must_prompt
            && (!tools::needs_permission(&self.mode, name)
                || self.handle.tool_allowed_for_run(name)
                || self.standing_grant(name, args))
        {
            return self.execute_with_turn_diff(name, args).await;
        }

        // Pause for an interactive decision: status → awaiting_permission,
        // permission_requested event, then wait for `api::runs::permission_decision`.
        let mut preview = tools::build_preview(&self.project_path, name, args);
        if let (Some(path), Some(Value::Object(fields))) = (protected.as_ref(), preview.as_mut()) {
            fields.insert("protectedPath".into(), json!(path));
        }
        // Why it asks and what a standing answer would cover, so frontends
        // can say it in words (UX-014) and hide grants that do not apply.
        let why = PromptReason::of(protected.as_deref(), dangerous, &sorted_domains(args));
        if preview.is_none() {
            preview = Some(json!({}));
        }
        if let Some(Value::Object(fields)) = preview.as_mut() {
            fields.insert("reason".into(), json!({ "code": why.code(), "text": why.text(&self.mode) }));
            fields.insert("oneTimeOnly".into(), json!(must_prompt));
            fields.insert("grantLabel".into(), json!(grant_label(name, args)));
        }
        let decision = match self.ask_tool_permission(call_id, name, args, preview).await? {
            Ok(decision) => decision,
            Err(reason) => return Ok(format!("Denied by PermissionRequest hook: {reason}")),
        };

        // Standing grants are persisted here only (QA-031), never by the facade.
        // Unknown answers deny (QA-006); protected / dangerous calls are
        // approved once, never as a standing or run-wide grant.
        let decision = if must_prompt && decision.allows() { PermissionDecision::AllowOnce } else { decision };
        match decision {
            PermissionDecision::Deny => Ok("The user denied permission for this tool call.".to_string()),
            PermissionDecision::AllowRun => {
                self.handle.allow_tool_for_run(name);
                self.execute_with_turn_diff(name, args).await
            }
            PermissionDecision::AllowProject | PermissionDecision::AllowAlways => {
                // Standing grants live in `permissions.json` under the
                // Locagens home and match the exact command + domains
                // (`standing_grant_matches`); nothing is written into the
                // project folder.
                self.state.db.upsert_permission(
                    decision.standing_scope().unwrap_or("project"),
                    &self.project_path.display().to_string(),
                    name,
                    &grant_key(name, args),
                    sorted_domains(args),
                    decision.as_str(),
                );
                self.execute_with_turn_diff(name, args).await
            }
            PermissionDecision::AllowOnce => self.execute_with_turn_diff(name, args).await,
        }
    }

    /// Asks before a `remember` call stores a memory when the mode requires
    /// it (see `tools::remember_needs_permission`). `Ok(Some(response))` is
    /// the tool result of a denial; any allow answer runs this one call.
    pub(super) async fn gate_remember(
        &self,
        call_id: &str,
        scope: &str,
        category: &str,
        content: &str,
    ) -> Result<Option<String>, AgentError> {
        if !tools::remember_needs_permission(&self.mode, scope) {
            return Ok(None);
        }
        let args = json!({ "scope": scope, "category": category, "content": content });
        match self.ask_tool_permission(call_id, "remember", &args, None).await? {
            Ok(decision) if decision.allows() => Ok(None),
            Ok(_) => Ok(Some("The user denied permission to store this memory.".to_string())),
            Err(reason) => Ok(Some(format!("Denied by PermissionRequest hook: {reason}"))),
        }
    }

    /// The one prompt path of a gated call: the PermissionRequest hook first,
    /// then the shared permission queue (it queues behind other sub-agents'
    /// and network prompts, and its slot/status are restored even if this
    /// future is aborted, e.g. `close_agent` mid-prompt). `Ok(Err(reason))`
    /// is a hook denial; a hook approval counts as "allow once".
    pub(super) async fn ask_tool_permission(
        &self,
        call_id: &str,
        name: &str,
        args: &Value,
        preview: Option<Value>,
    ) -> Result<Result<PermissionDecision, String>, AgentError> {
        match self.permission_hook(call_id, name, args).await {
            hook_calls::PermissionVerdict::Allow => return Ok(Ok(PermissionDecision::AllowOnce)),
            hook_calls::PermissionVerdict::Deny(reason) => return Ok(Err(reason)),
            hook_calls::PermissionVerdict::Ask => {}
        }
        self.notification_hook("permission_prompt", &format!("Locagens needs permission to run {name}")).await;
        let prompt = PermissionPrompt {
            tool_call: json!({
                "id": call_id,
                "function": {
                    "name": name,
                    "arguments": serde_json::to_string(args).unwrap_or_else(|_| "{}".into()),
                }
            }),
            preview,
            agent_role: self.agent_role.clone(),
            agent_name: self.agent_name.clone(),
            armed: None,
        };
        let Some(decision) = self.handle.ask_permission(&self.state.db, prompt).await else {
            return Err(AgentError::Cancelled);
        };
        if self.stop_requested() {
            return Err(AgentError::Cancelled);
        }
        Ok(Ok(PermissionDecision::parse_or_deny(&decision)))
    }

    /// The pre-prompt verdict for one line typed into a shell / REPL
    /// session: the same classification as an `exec_command` of that line
    /// (execpolicy rules, known-safe commands, dangerous commands, standing
    /// grants and "Allow for Run" of `exec_command`).
    fn typed_line_gate(&self, policy: &locagens_execpolicy::rules::Policy, line: &str) -> TypedLineGate {
        match command_safety::command_bypasses_prompt_with_policy(&self.mode, line, Some(policy)) {
            Err(reason) => return TypedLineGate::Blocked(reason),
            Ok(true) => return TypedLineGate::Send,
            Ok(false) => {}
        }
        if command_safety::is_dangerous_command(line) {
            return TypedLineGate::Prompt { once_only: true };
        }
        let as_command = json!({ "cmd": line });
        if self.handle.tool_allowed_for_run("exec_command") || self.standing_grant("exec_command", &as_command) {
            return TypedLineGate::Send;
        }
        TypedLineGate::Prompt { once_only: false }
    }

    /// QA-012: what of `chars` may be written to exec session `session_id`
    /// now. Outside Full Access, input to a session that reads commands
    /// (see `locagens_execpolicy::stdin_input`) is split into submitted
    /// lines and each line is gated like a command; one prompt covers every
    /// line of the write that needs approval. Text after the last line break
    /// is held back until a later write ends the line. Input to any other
    /// program, and interrupt / end-of-input keys, pass unchanged.
    ///
    /// Returns the text to write, or `Err(response)` when nothing is sent.
    pub(super) async fn gate_stdin_input(
        &self,
        call_id: &str,
        session_id: &str,
        chars: &str,
    ) -> Result<Result<String, String>, AgentError> {
        let orchestrator = &self.state.orchestrator;
        if self.mode == "auto" || !orchestrator.exec_session_reads_commands(&self.run_id, session_id) {
            return Ok(Ok(chars.to_string()));
        }
        if stdin_input::is_signal_input(chars) {
            orchestrator.set_held_input(&self.run_id, session_id, String::new());
            return Ok(Ok(chars.to_string()));
        }
        // The held text is dropped unless this write is allowed.
        let input = stdin_input::split_submitted_input(&orchestrator.take_held_input(&self.run_id, session_id), chars);
        let policy = self.load_exec_policy();
        let mut prompted: Vec<String> = Vec::new();
        let mut once_only = false;
        for line in input.lines.iter().map(|line| line.trim()).filter(|line| !line.is_empty()) {
            match self.typed_line_gate(&policy, line) {
                TypedLineGate::Send => {}
                TypedLineGate::Blocked(reason) => {
                    return Ok(Err(json!({
                        "ok": false,
                        "error": format!("Input blocked by execpolicy rule: {reason}. Nothing was sent.")
                    })
                    .to_string()))
                }
                TypedLineGate::Prompt { once_only: dangerous } => {
                    once_only |= dangerous;
                    prompted.push(line.to_string());
                }
            }
        }
        if !prompted.is_empty() {
            let commands = prompted.join("\n");
            let args = json!({ "session_id": session_id, "chars": commands });
            let preview = json!({
                "tool": "write_stdin",
                "action": "command",
                "path": "",
                "absolutePath": self.project_path.display().to_string(),
                "oldContent": Value::Null,
                "newContent": Value::Null,
                "command": commands,
                "sessionId": session_id,
                "networkDomains": [],
            });
            let decision = match self.ask_tool_permission(call_id, "write_stdin", &args, Some(preview)).await? {
                Ok(decision) => decision,
                Err(reason) => return Ok(Err(format!("Denied by PermissionRequest hook: {reason}. Nothing was sent."))),
            };
            match decision {
                PermissionDecision::Deny => {
                    return Ok(Err("The user denied permission for this input; nothing was sent.".to_string()))
                }
                // Dangerous lines are approved for this write only.
                _ if once_only => {}
                PermissionDecision::AllowOnce => {}
                PermissionDecision::AllowRun => self.handle.allow_tool_for_run("exec_command"),
                PermissionDecision::AllowProject | PermissionDecision::AllowAlways => {
                    for line in &prompted {
                        self.state.db.upsert_permission(
                            decision.standing_scope().unwrap_or("project"),
                            &self.project_path.display().to_string(),
                            "exec_command",
                            line,
                            Vec::new(),
                            decision.as_str(),
                        );
                    }
                }
            }
        }
        orchestrator.set_held_input(&self.run_id, session_id, input.held);
        Ok(Ok(input.to_send))
    }

    /// Executes a gated tool, snapshotting turn-diff baselines for mutating
    /// tools first and emitting `turn_diff_updated` afterwards.
    pub(super) async fn execute_with_turn_diff(&self, name: &str, args: &Value) -> Result<String, AgentError> {
        self.snapshot_turn_diff(name, args);
        let result = self.execute_tool(name, args).await;
        if result.is_ok() {
            self.emit_turn_diff();
        }
        result.map_err(AgentError::Failed)
    }
}

/// The command string of a `run_command`/`exec_command` call: `command`
/// first, Codex's `cmd` as fallback.
pub(super) fn extract_command_str(args: &Value) -> &str {
    args.get("command")
        .or_else(|| args.get("cmd"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

pub(super) fn sorted_domains(args: &Value) -> Vec<String> {
    args.get("network_domains")
        .or_else(|| args.get("networkDomains"))
        .and_then(Value::as_array)
        .map(|items| {
            let mut domains: Vec<String> = items.iter().filter_map(Value::as_str).map(str::to_string).collect();
            domains.sort();
            domains
        })
        .unwrap_or_default()
}

/// Whether a persisted permission rule covers this exact tool call.
pub(super) fn standing_grant_matches(
    rule: &crate::db::PermissionRule,
    tool: &str,
    command: &str,
    domains: &[String],
    project_path: &str,
) -> bool {
    if !rule.status.starts_with("allow") || rule.tool != tool {
        return false;
    }
    let scope_ok = rule.scope == "global" || (rule.scope == "project" && rule.project_path == project_path);
    if !scope_ok {
        return false;
    }
    if grant_is_keyed(tool) {
        // An empty rule key used to be saved for exec_command (its args
        // use `cmd`) and fetch_url; never let it act as a wildcard.
        if rule.command.is_empty() || command.is_empty() || rule.command != command {
            return false;
        }
        // The saved host list must equal the call's: a grant saved without
        // domains does not cover a later call that adds hosts (FR-032).
        let mut rule_domains = rule.network_domains.clone();
        rule_domains.sort();
        if rule_domains != domains {
            return false;
        }
    }
    true
}

/// Tools whose standing grant is bound to a key (command or host) and a
/// domain list instead of covering the whole tool.
fn grant_is_keyed(tool: &str) -> bool {
    matches!(tool, "run_command" | "exec_command" | "fetch_url")
}

/// What a standing grant of this call is bound to: the command string, or
/// for `fetch_url` the URL's host (so "Allow always" covers one site, not
/// every URL). Empty for tools whose grant covers the whole tool.
/// Why a tool call asks for permission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PromptReason {
    /// It changes a protected path (`.git`, `.locagens`, …).
    Protected(String),
    /// The command looks destructive (`rm -rf`, `git push --force`, …).
    Dangerous,
    /// The command asks for network access to these hosts.
    Network(Vec<String>),
    /// The mode asks before this kind of tool.
    Mode,
}

impl PromptReason {
    pub(super) fn of(protected: Option<&str>, dangerous: bool, domains: &[String]) -> Self {
        match (protected, dangerous) {
            (Some(path), _) => Self::Protected(path.to_string()),
            (None, true) => Self::Dangerous,
            _ if !domains.is_empty() => Self::Network(domains.to_vec()),
            _ => Self::Mode,
        }
    }

    pub(super) fn code(&self) -> &'static str {
        match self {
            Self::Protected(_) => "protected_path",
            Self::Dangerous => "dangerous_command",
            Self::Network(_) => "network",
            Self::Mode => "mode",
        }
    }

    /// One sentence for the prompt.
    pub(super) fn text(&self, mode: &str) -> String {
        match self {
            Self::Protected(path) => {
                format!("It changes `{path}`, which is protected. This is always asked and can only be allowed once.")
            }
            Self::Dangerous => {
                "The command can delete or overwrite data. This is always asked and can only be allowed once.".to_string()
            }
            Self::Network(hosts) => format!("The command wants network access to {}.", hosts.join(", ")),
            Self::Mode => {
                let mode = match mode {
                    "plan" => "Plan",
                    "chat" => "Chat",
                    "ask_permissions" => "Ask",
                    _ => "Build",
                };
                format!("{mode} mode asks before this kind of action.")
            }
        }
    }
}

/// What "always allow" for this call would cover, in words.
pub(super) fn grant_label(tool: &str, args: &Value) -> String {
    let domains = sorted_domains(args);
    match tool {
        "run_command" | "exec_command" => {
            let command = extract_command_str(args);
            if domains.is_empty() {
                format!("exactly this command: {command}")
            } else {
                format!("exactly this command with network access to {}: {command}", domains.join(", "))
            }
        }
        "fetch_url" => match grant_key(tool, args) {
            host if host.is_empty() => "fetching this URL".to_string(),
            host => format!("fetching any page on {host}"),
        },
        "web_search" => "searching the web".to_string(),
        "write_file" | "edit_file" | "apply_patch" | "delete_file" | "move_file" => {
            format!("every {tool} call in this project")
        }
        name if name.starts_with("mcp__") => format!("every call to the MCP tool {name}"),
        name => format!("every {name} call"),
    }
}

pub(super) fn grant_key(tool: &str, args: &Value) -> String {
    match tool {
        "fetch_url" => args
            .get("url")
            .and_then(Value::as_str)
            .and_then(|raw| url::Url::parse(raw).ok())
            .and_then(|parsed| parsed.host_str().map(str::to_lowercase))
            .unwrap_or_default(),
        _ => extract_command_str(args).to_string(),
    }
}

#[cfg(test)]
mod prompt_reason_tests {
    use super::*;

    #[test]
    fn prompts_say_why_and_what_a_grant_covers() {
        let domains = vec!["crates.io".to_string()];
        assert_eq!(PromptReason::of(Some(".git/config"), true, &domains).code(), "protected_path");
        assert_eq!(PromptReason::of(None, true, &domains).code(), "dangerous_command");
        assert_eq!(PromptReason::of(None, false, &domains).code(), "network");
        assert_eq!(PromptReason::of(None, false, &[]).code(), "mode");
        assert!(PromptReason::Mode.text("accept_edits").starts_with("Build mode"));
        assert!(PromptReason::Network(domains).text("plan").contains("crates.io"));

        let command = json!({ "command": "cargo test", "network_domains": ["crates.io"] });
        assert_eq!(grant_label("exec_command", &command), "exactly this command with network access to crates.io: cargo test");
        assert_eq!(grant_label("fetch_url", &json!({ "url": "https://Docs.rs/serde" })), "fetching any page on docs.rs");
        assert_eq!(grant_label("web_search", &json!({ "query": "rust" })), "searching the web");
        assert_eq!(grant_key("web_search", &json!({ "query": "rust" })), "");
        assert_eq!(grant_label("write_file", &json!({})), "every write_file call in this project");
        assert_eq!(grant_label("mcp__gh__search", &json!({})), "every call to the MCP tool mcp__gh__search");
    }
}
