//! Lifecycle hooks engine — Codex/Claude-style hooks loaded from
//! `hooks.json` files (agent-dir chain + project `.locagens`) and from the
//! `hooks` field of enabled plugin manifests.
//!
//! Contract highlights (matching Codex's implementation):
//! - Events: `SessionStart`, `SessionEnd`, `UserPromptSubmit`, `PreToolUse`,
//!   `PostToolUse`, `PermissionRequest`, `Notification`, `Stop`,
//!   `SubagentStart`, `SubagentStop`, `PreCompact`, `PostCompact`. Keys
//!   accept aliases (`SessionStart` / `onSessionStart` / `session_start` /
//!   `sessionStart`).
//! - Event values accept a single command string, an array of command
//!   strings, or `MatcherGroup` arrays
//!   (`[{ "matcher": "Bash|run_command", "hooks": [{ "type": "command",
//!   "command": "...", "timeout": 10 }] }]`).
//! - `"internal:*"` marker strings (seen in `plugins_templates.json`) are
//!   skipped cleanly.
//! - Commands run via `sh -c` (`cmd /C` on Windows) in the project folder
//!   with the event payload JSON on stdin, bounded by `timeout` (default
//!   10s).
//! - Exit code `2` on the blockable events (`PreToolUse`, `PostToolUse`,
//!   `UserPromptSubmit`, `PermissionRequest`, `Stop`, `SubagentStop`,
//!   `PreCompact`) blocks with the trimmed `stderr` (or `stdout`) as the
//!   reason; on the others it is only logged.
//! - Exit code `0` parses stdout as JSON: `decision: "block"`,
//!   `continue: false` or `hookSpecificOutput.permissionDecision: "deny"`
//!   blocks; `hookSpecificOutput.additionalContext` / `systemMessage` become
//!   additional context; `hookSpecificOutput.updatedInput` replaces the tool
//!   input; `hookSpecificOutput.decision.behavior` (`allow` / `deny`)
//!   answers a `PermissionRequest`. Plain-text `SessionStart` and
//!   `UserPromptSubmit` stdout is added to context verbatim.

use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// Result of running one hook event across all registered hooks.
#[derive(Debug, Default, Clone)]
pub struct HookOutcome {
    pub blocked: bool,
    pub reason: Option<String>,
    pub additional_context: Vec<String>,
    pub updated_input: Option<Value>,
    /// `PermissionRequest` answer: `Some(true)` allow, `Some(false)` deny.
    pub permission_allow: Option<bool>,
}

impl HookOutcome {
    pub fn context_joined(&self) -> Option<String> {
        if self.additional_context.is_empty() {
            None
        } else {
            Some(self.additional_context.join("\n\n"))
        }
    }
}

/// One executable hook command with its optional matcher.
#[derive(Debug, Clone)]
struct HookCommand {
    matcher: Option<String>,
    command: String,
    timeout_secs: u64,
}

const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 10;

/// Canonicalizes supported event-key aliases (case, `_` and a leading `on`
/// are ignored: `PreToolUse` = `pre_tool_use` = `onPreToolUse`).
fn canonical_event(key: &str) -> Option<&'static str> {
    let flat: String = key.chars().filter(|c| *c != '_').collect::<String>().to_lowercase();
    let flat = flat.strip_prefix("on").unwrap_or(&flat);
    Some(match flat {
        "sessionstart" => "session_start",
        "sessionend" => "session_end",
        "userpromptsubmit" => "user_prompt_submit",
        "pretooluse" => "pre_tool_use",
        "posttooluse" => "post_tool_use",
        "permissionrequest" => "permission_request",
        "notification" => "notification",
        "stop" => "stop",
        "subagentstart" => "subagent_start",
        "subagentstop" => "subagent_stop",
        "precompact" => "pre_compact",
        "postcompact" => "post_compact",
        _ => return None,
    })
}

/// Events whose exit code 2 / `decision: "block"` has an effect.
fn blockable(event: &str) -> bool {
    matches!(
        event,
        "pre_tool_use"
            | "post_tool_use"
            | "user_prompt_submit"
            | "permission_request"
            | "stop"
            | "subagent_stop"
            | "pre_compact"
    )
}

/// Events whose plain-text stdout (exit 0) becomes model context.
fn plain_stdout_is_context(event: &str) -> bool {
    matches!(event, "session_start" | "user_prompt_submit")
}

fn is_internal_marker(value: &str) -> bool {
    value.trim().starts_with("internal:")
}

/// `hooks.json` of every agent dir, then the trusted project's.
fn hooks_files(agent_dirs: &[PathBuf], project_path: Option<&Path>) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = agent_dirs
        .iter()
        .map(|dir| dir.join("hooks.json"))
        .collect();
    // Project hooks are project-controlled code: only a trusted folder's
    // `.locagens/hooks.json` is loaded (see `project_trust`).
    if let Some(project) = project_path.filter(|project| crate::services::project_trust::is_trusted(project)) {
        files.push(project.join(".locagens").join("hooks.json"));
    }
    files
}

fn parse_hook_entry(value: &Value) -> Vec<HookCommand> {
    match value {
        Value::String(command) => {
            if is_internal_marker(command) {
                Vec::new()
            } else {
                vec![HookCommand {
                    matcher: None,
                    command: command.clone(),
                    timeout_secs: DEFAULT_HOOK_TIMEOUT_SECS,
                }]
            }
        }
        Value::Array(items) => items.iter().flat_map(parse_hook_item).collect(),
        _ => Vec::new(),
    }
}

fn parse_hook_item(item: &Value) -> Vec<HookCommand> {
    match item {
        Value::String(command) => {
            if is_internal_marker(command) {
                Vec::new()
            } else {
                vec![HookCommand {
                    matcher: None,
                    command: command.clone(),
                    timeout_secs: DEFAULT_HOOK_TIMEOUT_SECS,
                }]
            }
        }
        Value::Object(obj) => {
            // MatcherGroup: `{ "matcher": "...", "hooks": [{ "type":
            // "command", "command": "...", "timeout": 10 }] }`.
            if let Some(hooks) = obj.get("hooks").and_then(Value::as_array) {
                let matcher = obj
                    .get("matcher")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                return hooks
                    .iter()
                    .filter_map(|hook| {
                        let command = hook.get("command").and_then(Value::as_str)?;
                        if is_internal_marker(command) {
                            return None;
                        }
                        Some(HookCommand {
                            matcher: matcher.clone(),
                            command: command.to_string(),
                            timeout_secs: hook
                                .get("timeout")
                                .and_then(Value::as_u64)
                                .unwrap_or(DEFAULT_HOOK_TIMEOUT_SECS)
                                .max(1),
                        })
                    })
                    .collect();
            }
            // Bare command object: `{ "type": "command", "command": "...",
            // "timeout": 10, "matcher": "..." }`.
            let Some(command) = obj.get("command").and_then(Value::as_str) else {
                return Vec::new();
            };
            if is_internal_marker(command) {
                return Vec::new();
            }
            vec![HookCommand {
                matcher: obj.get("matcher").and_then(Value::as_str).map(str::to_string),
                command: command.to_string(),
                timeout_secs: obj
                    .get("timeout")
                    .and_then(Value::as_u64)
                    .unwrap_or(DEFAULT_HOOK_TIMEOUT_SECS)
                    .max(1),
            }]
        }
        _ => Vec::new(),
    }
}

fn collect_event(map: &mut HashMap<String, Vec<HookCommand>>, key: &str, value: &Value) {
    let Some(event) = canonical_event(key) else { return };
    let commands = parse_hook_entry(value);
    if commands.is_empty() {
        return;
    }
    map.entry(event.to_string()).or_default().extend(commands);
}

/// Loads every hook registered for this project: `hooks.json` files in the
/// agent-dir chain, `<project>/.locagens/hooks.json` and the `hooks` field
/// of every enabled plugin manifest.
fn load_hooks(agent_dirs: &[PathBuf], project_path: Option<&Path>) -> HashMap<String, Vec<HookCommand>> {
    let mut map: HashMap<String, Vec<HookCommand>> = HashMap::new();
    for file in hooks_files(agent_dirs, project_path) {
        let Ok(raw) = std::fs::read_to_string(&file) else { continue };
        let Ok(parsed) = serde_json::from_str::<Value>(&raw) else { continue };
        // Support both `{ "hooks": { ... } }` and a bare event map at root.
        let hooks_obj = parsed
            .get("hooks")
            .filter(|hooks| hooks.is_object())
            .cloned()
            .unwrap_or(parsed);
        if let Some(obj) = hooks_obj.as_object() {
            for (key, value) in obj {
                collect_event(&mut map, key, value);
            }
        }
    }
    if let Some(project) = project_path {
        for manifest in crate::services::plugins::enabled_manifests(project.to_str()) {
            if let Some(hooks) = manifest.get("hooks").and_then(Value::as_object) {
                for (key, value) in hooks {
                    collect_event(&mut map, key, value);
                }
            }
        }
    }
    map
}

/// The payload field a MatcherGroup `matcher` is tested against.
fn matcher_subject<'a>(event: &str, payload: &'a Value) -> &'a str {
    let field = match event {
        "pre_tool_use" | "post_tool_use" | "permission_request" => "tool_name",
        "session_start" => "source",
        "session_end" => "reason",
        "notification" => "notification_type",
        "pre_compact" | "post_compact" => "trigger",
        "subagent_start" | "subagent_stop" => "agent_type",
        _ => return "",
    };
    payload.get(field).and_then(Value::as_str).unwrap_or_default()
}

/// Matches a MatcherGroup `matcher` against the event's subject (tool name,
/// session source, ...): regex when valid, substring fallback otherwise.
/// `None` / `""` / `"*"` match everything.
fn matcher_matches(matcher: Option<&str>, event: &str, payload: &Value) -> bool {
    let Some(matcher) = matcher.map(str::trim).filter(|m| !m.is_empty() && *m != "*") else {
        return true;
    };
    let subject = matcher_subject(event, payload);
    match regex::Regex::new(matcher) {
        Ok(re) => re.is_match(subject),
        Err(_) => subject.contains(matcher),
    }
}

/// Runs every hook registered for `event_name` (any alias spelling) in
/// order and folds the results into one `HookOutcome`.
pub async fn run_hook_event(event_name: &str, project_path: &Path, payload: &Value) -> HookOutcome {
    run_hook_event_in(&crate::paths::agent_dir_chain(), event_name, project_path, payload).await
}

/// [`run_hook_event`] with an explicit agent-dir chain (the `hooks.json`
/// roots), so tests never load the developer's own hooks (QA-071).
pub async fn run_hook_event_in(agent_dirs: &[PathBuf], event_name: &str, project_path: &Path, payload: &Value) -> HookOutcome {
    let Some(event) = canonical_event(event_name) else {
        return HookOutcome::default();
    };
    let hooks = load_hooks(agent_dirs, Some(project_path));
    let registered = hooks.get(event).cloned().unwrap_or_default();
    let mut outcome = HookOutcome::default();
    for hook in registered {
        if !matcher_matches(hook.matcher.as_deref(), event, payload) {
            continue;
        }
        if let Err(err) = run_hook_command(&hook, payload, event, project_path, &mut outcome).await {
            tracing::warn!("Hook command for {event_name} failed: {err}");
        }
    }
    outcome
}

/// Builds the common payload (`session_id`, `cwd`, `hook_event_name`) plus
/// `extra` fields and runs the event. The one-line entry point used by the
/// orchestrator.
pub async fn fire(event_name: &str, project_path: &Path, session_id: &str, extra: Value) -> HookOutcome {
    run_hook_event(event_name, project_path, &event_payload(event_name, project_path, session_id, extra)).await
}

/// Common payload fields merged with `extra`.
pub fn event_payload(event_name: &str, project_path: &Path, session_id: &str, extra: Value) -> Value {
    let mut payload = serde_json::json!({
        "session_id": session_id,
        "cwd": project_path.display().to_string(),
        "hook_event_name": event_name,
    });
    if let (Some(obj), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            obj.insert(key.clone(), value.clone());
        }
    }
    payload
}

async fn run_hook_command(
    hook: &HookCommand,
    payload: &Value,
    event: &str,
    project_path: &Path,
    outcome: &mut HookOutcome,
) -> Result<(), String> {
    let input = serde_json::to_vec(payload).map_err(|err| err.to_string())?;

    let mut cmd = tokio::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" });
    #[cfg(windows)]
    {
        // `cmd` does not understand Rust's backslash-quote escaping, so the
        // command line is passed verbatim; `/S` strips exactly the outer quotes.
        cmd.raw_arg(format!("/S /C \"{}\"", hook.command));
    }
    #[cfg(not(windows))]
    {
        cmd.args(["-c", &hook.command]);
    }
    // Hooks run outside the command sandbox: give them the same
    // secret-free environment as exec sessions, never the parent's API keys.
    cmd.env_clear()
        .envs(crate::orchestrator::tools::exec_session_env())
        .current_dir(project_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|err| format!("failed to spawn hook command: {err}"))?;

    let run = async {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&input).await;
            let _ = stdin.flush().await;
            // Dropping stdin closes it so the hook's reader sees EOF.
            drop(stdin);
        }
        child.wait_with_output().await.map_err(|err| err.to_string())
    };
    let output = tokio::time::timeout(Duration::from_secs(hook.timeout_secs), run)
        .await
        .map_err(|_| format!("hook timed out after {}s", hook.timeout_secs))?
        .map_err(|err| err.to_string())?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    match output.status.code() {
        // Exit code 2 blocks the blockable events (stderr = reason).
        Some(2) if blockable(event) => {
            outcome.blocked = true;
            let reason = non_empty_trim(&stderr).or_else(|| non_empty_trim(&stdout));
            if let Some(reason) = reason {
                outcome.reason = Some(reason);
            }
        }
        Some(0) => apply_success_output(&stdout, event, outcome),
        _ => {
            // Any other exit code is a hook failure: never blocks, just logs.
            tracing::debug!(
                "Hook exited with {:?}: {}",
                output.status.code(),
                non_empty_trim(&stderr).unwrap_or_default()
            );
        }
    }
    Ok(())
}

fn non_empty_trim(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Applies the exit-0 contract: JSON stdout decides blocking/context/input;
/// plain-text `SessionStart` stdout becomes context.
fn apply_success_output(stdout: &str, event: &str, outcome: &mut HookOutcome) {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return;
    }
    let parsed = serde_json::from_str::<Value>(trimmed).ok();
    let Some(value) = parsed else {
        if plain_stdout_is_context(event) {
            outcome.additional_context.push(trimmed.to_string());
        }
        return;
    };

    let decision = value.get("decision").and_then(Value::as_str).unwrap_or_default();
    let cont = value.get("continue").and_then(Value::as_bool).unwrap_or(true);
    let hso = value.get("hookSpecificOutput");
    let permission_decision = hso
        .and_then(|h| h.get("permissionDecision"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    // PermissionRequest answer: `decision.behavior` = allow | deny.
    if event == "permission_request" {
        let answer = hso.and_then(|h| h.get("decision"));
        match answer.and_then(|d| d.get("behavior")).and_then(Value::as_str).unwrap_or_default() {
            "allow" => outcome.permission_allow = Some(true),
            "deny" => {
                outcome.permission_allow = Some(false);
                outcome.blocked = true;
                outcome.reason = answer
                    .and_then(|d| d.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            _ => {}
        }
    }
    if blockable(event) && (decision == "block" || !cont || permission_decision == "deny") {
        outcome.blocked = true;
        let reason = value
            .get("reason")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| value.get("stopReason").and_then(Value::as_str).map(str::to_string))
            .or_else(|| {
                hso.and_then(|h| h.get("permissionDecisionReason"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        if let Some(reason) = reason {
            outcome.reason = Some(reason);
        }
    }
    if let Some(context) = hso
        .and_then(|h| h.get("additionalContext"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        outcome.additional_context.push(context.to_string());
    }
    if let Some(message) = value
        .get("systemMessage")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        outcome.additional_context.push(message.to_string());
    }
    if let Some(updated) = hso.and_then(|h| h.get("updatedInput")) {
        if updated.is_object() {
            outcome.updated_input = Some(updated.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Hook tests load only the hooks they write (QA-071).
    const NO_AGENT_DIRS: &[PathBuf] = &[];

    fn temp_project(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "locagens-hooks-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(dir.join(".locagens")).expect("create temp project");
        // Project hooks only load for trusted folders.
        crate::services::project_trust::trust_for_session(&dir);
        dir
    }

    #[tokio::test]
    async fn untrusted_project_hooks_never_run() {
        let project = std::env::temp_dir().join(format!("locagens-hooks-untrusted-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(project.join(".locagens")).unwrap();
        write_hooks(&project, json!({ "SessionStart": "echo untrusted-hook-ran" }));
        let plugin_dir = project.join(".locagens").join("plugins").join("evil");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.json"),
            json!({ "id": "evil", "name": "Evil", "enabled": true, "hooks": { "SessionStart": "echo untrusted-plugin-ran" } }).to_string(),
        )
        .unwrap();
        let outcome = run_hook_event_in(NO_AGENT_DIRS, "SessionStart", &project, &json!({ "hook_event_name": "SessionStart" })).await;
        assert!(
            !outcome.additional_context.iter().any(|c| c.contains("untrusted-")),
            "{:?}",
            outcome.additional_context
        );
        crate::services::project_trust::trust_for_session(&project);
        let outcome = run_hook_event_in(NO_AGENT_DIRS, "SessionStart", &project, &json!({ "hook_event_name": "SessionStart" })).await;
        let context = outcome.context_joined().unwrap_or_default();
        assert!(context.contains("untrusted-hook-ran") && context.contains("untrusted-plugin-ran"), "{context}");
        let _ = std::fs::remove_dir_all(&project);
    }

    #[tokio::test]
    async fn hooks_do_not_inherit_secret_environment_variables() {
        let project = temp_project("env");
        let var = format!("LOCAGENS_TEST_API_KEY_{}", uuid::Uuid::new_v4().simple());
        let _env = crate::ENV_TEST_LOCK.lock().await;
        std::env::set_var(&var, "hook-secret-value");
        let print = if cfg!(windows) { format!("echo [%{var}%]") } else { format!("echo \"[${var}]\"") };
        write_hooks(&project, json!({ "SessionStart": print }));
        let outcome = run_hook_event_in(NO_AGENT_DIRS, "SessionStart", &project, &json!({ "hook_event_name": "SessionStart" })).await;
        std::env::remove_var(&var);
        let context = outcome.context_joined().unwrap_or_default();
        assert!(context.contains('['), "hook ran: {context}");
        assert!(!context.contains("hook-secret-value"), "{context}");
        let _ = std::fs::remove_dir_all(&project);
    }

    #[tokio::test]
    async fn agent_dir_hooks_load_from_the_injected_chain_only() {
        let project = temp_project("agent-dir");
        let agent_dir = project.join("agent-home");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("hooks.json"), json!({ "SessionStart": "echo agent-dir-hook-ran" }).to_string()).unwrap();
        let payload = json!({ "hook_event_name": "SessionStart" });
        let outcome = run_hook_event_in(&[agent_dir.clone()], "SessionStart", &project, &payload).await;
        assert!(outcome.context_joined().unwrap_or_default().contains("agent-dir-hook-ran"));
        let outcome = run_hook_event_in(NO_AGENT_DIRS, "SessionStart", &project, &payload).await;
        assert!(outcome.context_joined().is_none(), "{:?}", outcome.additional_context);
        let _ = std::fs::remove_dir_all(&project);
    }

    /// Portable "print this JSON" command (`cmd` keeps single quotes literally).
    fn echo_json(json: &str) -> String {
        if cfg!(windows) {
            format!("echo {json}")
        } else {
            format!("echo '{json}'")
        }
    }

    /// Portable "write `message` to stderr and exit 2".
    fn deny_with(message: &str) -> String {
        if cfg!(windows) {
            format!("echo {message} 1>&2 & exit 2")
        } else {
            format!("printf '{message}' >&2; exit 2")
        }
    }

    /// Portable "copy stdin to a file".
    fn capture_stdin(file: &str) -> String {
        if cfg!(windows) {
            format!("more > {file}")
        } else {
            format!("cat > {file}")
        }
    }

    fn write_hooks(project: &Path, hooks: Value) {
        std::fs::write(
            project.join(".locagens").join("hooks.json"),
            serde_json::to_string(&json!({ "hooks": hooks })).unwrap(),
        )
        .unwrap();
    }

    fn group(matcher: &str, command: &str) -> Value {
        json!([{ "matcher": matcher, "hooks": [{ "type": "command", "command": command, "timeout": 10 }] }])
    }

    #[tokio::test]
    async fn hooks_pre_tool_use_blocks_on_exit_2_and_parses_json_decision() {
        let project = temp_project("pre-tool");
        let hooks = json!({
            "SessionStart": "echo hello-session-start",
            "PreToolUse": [
                { "matcher": "write_file",
                  "hooks": [{ "type": "command",
                              "command": echo_json(r#"{"hookSpecificOutput":{"updatedInput":{"path":"rewritten.txt","content":"hooked"},"additionalContext":"pre hook ran"}}"#),
                              "timeout": 10 }] },
                { "matcher": "run_command",
                  "hooks": [{ "type": "command",
                              "command": echo_json(r#"{"decision":"block","reason":"no shell for you"}"#),
                              "timeout": 10 }] },
                { "matcher": "Bash|exec_command",
                  "hooks": [{ "type": "command",
                              "command": deny_with("denied by policy"),
                              "timeout": 10 }] }
            ]
        });
        write_hooks(&project, hooks);

        // SessionStart: plain non-JSON stdout becomes additional context.
        let outcome = run_hook_event_in(NO_AGENT_DIRS, 
            "SessionStart",
            &project,
            &json!({ "hook_event_name": "SessionStart", "session_id": "s1" }),
        )
        .await;
        assert!(!outcome.blocked);
        assert!(outcome
            .additional_context
            .iter()
            .any(|c| c.contains("hello-session-start")));

        // PreToolUse: updatedInput replaces the tool args and adds context.
        let outcome = run_hook_event_in(NO_AGENT_DIRS, 
            "PreToolUse",
            &project,
            &json!({ "hook_event_name": "PreToolUse", "tool_name": "write_file",
                     "tool_input": { "path": "a.txt" }, "tool_use_id": "c1" }),
        )
        .await;
        assert!(!outcome.blocked);
        assert!(outcome.additional_context.iter().any(|c| c.contains("pre hook ran")));
        let updated = outcome.updated_input.expect("updatedInput captured");
        assert_eq!(updated["path"], "rewritten.txt");
        assert_eq!(updated["content"], "hooked");

        // PreToolUse: JSON `decision: "block"` with a reason.
        let outcome = run_hook_event_in(NO_AGENT_DIRS, 
            "PreToolUse",
            &project,
            &json!({ "hook_event_name": "PreToolUse", "tool_name": "run_command",
                     "tool_input": { "command": "ls" } }),
        )
        .await;
        assert!(outcome.blocked);
        assert_eq!(outcome.reason.as_deref(), Some("no shell for you"));

        // PreToolUse: exit code 2 with stderr as the reason.
        let outcome = run_hook_event_in(NO_AGENT_DIRS, 
            "PreToolUse",
            &project,
            &json!({ "hook_event_name": "PreToolUse", "tool_name": "exec_command",
                     "tool_input": { "cmd": "echo hi" } }),
        )
        .await;
        assert!(outcome.blocked);
        assert_eq!(outcome.reason.as_deref(), Some("denied by policy"));

        // Unmatched tools never trigger the hooks.
        let outcome = run_hook_event_in(NO_AGENT_DIRS, 
            "PreToolUse",
            &project,
            &json!({ "hook_event_name": "PreToolUse", "tool_name": "read_file",
                     "tool_input": {} }),
        )
        .await;
        assert!(!outcome.blocked);
        assert!(outcome.updated_input.is_none());

        let _ = std::fs::remove_dir_all(&project);
    }

    #[tokio::test]
    async fn plugin_camel_case_hooks_and_internal_markers_are_handled() {
        let project = temp_project("plugin-hooks");

        // An enabled plugin with camelCase `onSessionStart` and a
        // `systemPrompt` (the hooks engine only consumes `hooks`, but the
        // manifest must be picked up through enabled_manifests).
        let plugin_dir = project.join(".locagens").join("plugins").join("demo-hooks");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.json"),
            serde_json::to_vec(&json!({
                "id": "demo-hooks",
                "name": "Demo Hooks",
                "version": "1.0.0",
                "enabled": true,
                "hooks": {
                    "onSessionStart": "echo hi-plugin",
                    "postToolUse": "internal:demo-marker"
                },
                "systemPrompt": "Demo plugin is active."
            }))
            .unwrap(),
        )
        .unwrap();

        // hooks.json with internal markers: they must neither error nor run
        // as shell commands.
        std::fs::write(
            project.join(".locagens").join("hooks.json"),
            serde_json::to_string(&json!({
                "PreToolUse": "internal:context-mode-sandbox",
                "onSessionStart": "internal:context-mode-init"
            }))
            .unwrap(),
        )
        .unwrap();

        let outcome = run_hook_event_in(NO_AGENT_DIRS, 
            "SessionStart",
            &project,
            &json!({ "hook_event_name": "SessionStart", "session_id": "s2" }),
        )
        .await;
        assert!(!outcome.blocked, "internal markers must not block");
        assert!(
            outcome.additional_context.iter().any(|c| c.contains("hi-plugin")),
            "camelCase plugin hook must run; got {:?}",
            outcome.additional_context
        );
        assert!(
            !outcome.additional_context.iter().any(|c| c.contains("internal:")),
            "internal markers must not be executed or echoed"
        );

        // PostToolUse with only an internal marker must be a clean no-op.
        let outcome = run_hook_event_in(NO_AGENT_DIRS, 
            "PostToolUse",
            &project,
            &json!({ "hook_event_name": "PostToolUse", "tool_name": "read_file" }),
        )
        .await;
        assert!(!outcome.blocked);
        assert!(outcome.additional_context.is_empty());

        // Unknown event names are clean no-ops too.
        let outcome = run_hook_event_in(NO_AGENT_DIRS, 
            "NotARealEvent",
            &project,
            &json!({ "hook_event_name": "NotARealEvent" }),
        )
        .await;
        assert!(!outcome.blocked);
        assert!(outcome.additional_context.is_empty());

        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn event_aliases_cover_every_event() {
        for (alias, canonical) in [
            ("SessionStart", "session_start"),
            ("onSessionEnd", "session_end"),
            ("userPromptSubmit", "user_prompt_submit"),
            ("pre_tool_use", "pre_tool_use"),
            ("PostToolUse", "post_tool_use"),
            ("PermissionRequest", "permission_request"),
            ("Notification", "notification"),
            ("onStop", "stop"),
            ("SubagentStart", "subagent_start"),
            ("subagent_stop", "subagent_stop"),
            ("PreCompact", "pre_compact"),
            ("PostCompact", "post_compact"),
        ] {
            assert_eq!(canonical_event(alias), Some(canonical), "{alias}");
        }
        assert_eq!(canonical_event("Bogus"), None);
    }

    #[tokio::test]
    async fn user_prompt_submit_gets_prompt_adds_context_and_blocks() {
        let project = temp_project("prompt");
        write_hooks(
            &project,
            json!({ "UserPromptSubmit": [
                { "hooks": [{ "type": "command", "command": capture_stdin("prompt-payload.json") }] },
                { "hooks": [{ "type": "command", "command": echo_json(r#"{"hookSpecificOutput":{"additionalContext":"extra-context"}}"#) }] }
            ] }),
        );
        let outcome = fire(
            "UserPromptSubmit",
            &project,
            "run-1",
            json!({ "prompt": "fix the bug", "model": "m", "permission_mode": "auto" }),
        )
        .await;
        assert!(!outcome.blocked);
        assert_eq!(outcome.context_joined().as_deref(), Some("extra-context"));
        let raw = std::fs::read_to_string(project.join("prompt-payload.json")).unwrap();
        let payload: Value = serde_json::from_str(raw.trim()).unwrap();
        assert_eq!(payload["hook_event_name"], "UserPromptSubmit");
        assert_eq!(payload["session_id"], "run-1");
        assert_eq!(payload["prompt"], "fix the bug");
        assert_eq!(payload["cwd"], project.display().to_string());

        // Plain stdout is context, exit 2 blocks with the stderr reason.
        write_hooks(&project, json!({ "UserPromptSubmit": ["echo plain-note"] }));
        let outcome = fire("UserPromptSubmit", &project, "run-1", json!({ "prompt": "x" })).await;
        assert_eq!(outcome.context_joined().as_deref(), Some("plain-note"));
        write_hooks(&project, json!({ "UserPromptSubmit": [deny_with("prompt rejected")] }));
        let outcome = fire("UserPromptSubmit", &project, "run-1", json!({ "prompt": "x" })).await;
        assert!(outcome.blocked);
        assert_eq!(outcome.reason.as_deref(), Some("prompt rejected"));
        let _ = std::fs::remove_dir_all(&project);
    }

    #[tokio::test]
    async fn session_end_and_notification_fire_but_never_block() {
        let project = temp_project("lifecycle");
        write_hooks(
            &project,
            json!({
                "SessionEnd": [deny_with("ignored")],
                "Notification": group("permission_prompt", &capture_stdin("notify-payload.json")),
            }),
        );
        let outcome = fire("SessionEnd", &project, "run-2", json!({ "reason": "done" })).await;
        assert!(!outcome.blocked, "SessionEnd exit 2 is only logged");

        // The notification_type matcher selects the permission prompt only.
        fire("Notification", &project, "run-2", json!({ "notification_type": "elicitation_dialog", "message": "q?" })).await;
        assert!(!project.join("notify-payload.json").exists());
        fire(
            "Notification",
            &project,
            "run-2",
            json!({ "notification_type": "permission_prompt", "message": "Allow run_command?" }),
        )
        .await;
        let raw = std::fs::read_to_string(project.join("notify-payload.json")).unwrap();
        let payload: Value = serde_json::from_str(raw.trim()).unwrap();
        assert_eq!(payload["hook_event_name"], "Notification");
        assert_eq!(payload["message"], "Allow run_command?");
        let _ = std::fs::remove_dir_all(&project);
    }

    #[tokio::test]
    async fn stop_and_subagent_stop_block_with_reason() {
        let project = temp_project("stop");
        write_hooks(
            &project,
            json!({
                "Stop": echo_json(r#"{"decision":"block","reason":"keep going: tests not run"}"#),
                "SubagentStop": [deny_with("worker incomplete")],
                "SubagentStart": group("explorer", "echo started-explorer"),
            }),
        );
        let outcome = fire("Stop", &project, "r", json!({ "stop_hook_active": false, "last_assistant_message": "done" })).await;
        assert!(outcome.blocked);
        assert_eq!(outcome.reason.as_deref(), Some("keep going: tests not run"));
        let outcome = fire("SubagentStop", &project, "r", json!({ "agent_type": "worker" })).await;
        assert!(outcome.blocked);
        assert_eq!(outcome.reason.as_deref(), Some("worker incomplete"));

        // SubagentStart matches on agent_type; its output is not context.
        let outcome = fire("SubagentStart", &project, "r", json!({ "agent_type": "worker" })).await;
        assert!(outcome.additional_context.is_empty());
        let _ = std::fs::remove_dir_all(&project);
    }

    #[tokio::test]
    async fn compact_hooks_block_pre_only() {
        let project = temp_project("compact");
        write_hooks(
            &project,
            json!({
                "PreCompact": group("auto", &deny_with("do not compact")),
                "PostCompact": [deny_with("ignored")],
            }),
        );
        let outcome = fire("PreCompact", &project, "r", json!({ "trigger": "auto" })).await;
        assert!(outcome.blocked);
        assert_eq!(outcome.reason.as_deref(), Some("do not compact"));
        let outcome = fire("PreCompact", &project, "r", json!({ "trigger": "manual" })).await;
        assert!(!outcome.blocked, "matcher auto does not apply to manual");
        let outcome = fire("PostCompact", &project, "r", json!({ "trigger": "auto" })).await;
        assert!(!outcome.blocked);
        let _ = std::fs::remove_dir_all(&project);
    }

    #[tokio::test]
    async fn permission_request_hook_can_allow_or_deny() {
        let project = temp_project("perm");
        write_hooks(
            &project,
            json!({ "PermissionRequest": [
                group("run_command", &echo_json(r#"{"hookSpecificOutput":{"decision":{"behavior":"allow"}}}"#))[0].clone(),
                group("fetch_url", &echo_json(r#"{"hookSpecificOutput":{"decision":{"behavior":"deny","message":"no network"}}}"#))[0].clone(),
            ] }),
        );
        let allow = fire("PermissionRequest", &project, "r", json!({ "tool_name": "run_command", "tool_input": {} })).await;
        assert_eq!(allow.permission_allow, Some(true));
        assert!(!allow.blocked);
        let deny = fire("PermissionRequest", &project, "r", json!({ "tool_name": "fetch_url", "tool_input": {} })).await;
        assert_eq!(deny.permission_allow, Some(false));
        assert!(deny.blocked);
        assert_eq!(deny.reason.as_deref(), Some("no network"));
        let none = fire("PermissionRequest", &project, "r", json!({ "tool_name": "write_file", "tool_input": {} })).await;
        assert_eq!(none.permission_allow, None);
        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn event_payload_merges_common_fields() {
        let payload = event_payload("Stop", Path::new("/p"), "sid", json!({ "x": 1 }));
        assert_eq!(payload["session_id"], "sid");
        assert_eq!(payload["hook_event_name"], "Stop");
        assert_eq!(payload["x"], 1);
    }
}
