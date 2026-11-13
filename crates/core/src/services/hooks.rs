//! Lifecycle hooks engine — Codex/Claude-style hooks loaded from
//! `hooks.json` files (agent-dir chain + project `.locagens`) and from the
//! `hooks` field of enabled plugin manifests.
//!
//! Contract highlights (matching Codex's implementation):
//! - Event keys accept aliases: `SessionStart` / `onSessionStart` /
//!   `session_start`, `PreToolUse` / `preToolUse` / `pre_tool_use`,
//!   `PostToolUse` / `postToolUse` / `post_tool_use`, `Stop` / `stop` /
//!   `onStop`.
//! - Event values accept a single command string, an array of command
//!   strings, or `MatcherGroup` arrays
//!   (`[{ "matcher": "Bash|run_command", "hooks": [{ "type": "command",
//!   "command": "...", "timeout": 10 }] }]`).
//! - `"internal:*"` marker strings (seen in `plugins_templates.json`) are
//!   skipped cleanly.
//! - Commands run via `sh -c` (`cmd /C` on Windows) in the project folder
//!   with the event payload JSON on stdin, bounded by `timeout` (default
//!   10s).
//! - Exit code `2` on `PreToolUse`/`PostToolUse`/`Stop` blocks with the
//!   trimmed `stderr` (or `stdout`) as the reason.
//! - Exit code `0` parses stdout as JSON: `decision: "block"`,
//!   `continue: false` or `hookSpecificOutput.permissionDecision: "deny"`
//!   blocks; `hookSpecificOutput.additionalContext` / `systemMessage` become
//!   additional context; `hookSpecificOutput.updatedInput` replaces the tool
//!   input. Plain-text `SessionStart` stdout is added to context verbatim.

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

/// Canonicalizes supported event-key aliases.
fn canonical_event(key: &str) -> Option<&'static str> {
    match key {
        "SessionStart" | "onSessionStart" | "session_start" => Some("session_start"),
        "PreToolUse" | "preToolUse" | "pre_tool_use" => Some("pre_tool_use"),
        "PostToolUse" | "postToolUse" | "post_tool_use" => Some("post_tool_use"),
        "Stop" | "stop" | "onStop" => Some("stop"),
        _ => None,
    }
}

fn is_internal_marker(value: &str) -> bool {
    value.trim().starts_with("internal:")
}

fn hooks_files(project_path: Option<&Path>) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = crate::paths::agent_dir_chain()
        .iter()
        .map(|dir| dir.join("hooks.json"))
        .collect();
    if let Some(project) = project_path {
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
fn load_hooks(project_path: Option<&Path>) -> HashMap<String, Vec<HookCommand>> {
    let mut map: HashMap<String, Vec<HookCommand>> = HashMap::new();
    for file in hooks_files(project_path) {
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

/// Matches a MatcherGroup `matcher` against `payload["tool_name"]`: regex
/// when valid, exact-substring fallback otherwise. `None` / `""` / `"*"`
/// match everything.
fn matcher_matches(matcher: Option<&str>, payload: &Value) -> bool {
    let Some(matcher) = matcher.map(str::trim).filter(|m| !m.is_empty() && *m != "*") else {
        return true;
    };
    let tool_name = payload.get("tool_name").and_then(Value::as_str).unwrap_or_default();
    match regex::Regex::new(matcher) {
        Ok(re) => re.is_match(tool_name),
        Err(_) => tool_name.contains(matcher),
    }
}

/// Runs every hook registered for `event_name` (any alias spelling) in
/// order and folds the results into one `HookOutcome`.
pub async fn run_hook_event(event_name: &str, project_path: &Path, payload: &Value) -> HookOutcome {
    let Some(event) = canonical_event(event_name) else {
        return HookOutcome::default();
    };
    let hooks = load_hooks(Some(project_path));
    let registered = hooks.get(event).cloned().unwrap_or_default();
    let mut outcome = HookOutcome::default();
    for hook in registered {
        if !matcher_matches(hook.matcher.as_deref(), payload) {
            continue;
        }
        if let Err(err) = run_hook_command(&hook, payload, event, project_path, &mut outcome).await {
            tracing::warn!("Hook command for {event_name} failed: {err}");
        }
    }
    outcome
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
    if cfg!(windows) {
        cmd.args(["/C", &hook.command]);
    } else {
        cmd.args(["-c", &hook.command]);
    }
    cmd.current_dir(project_path)
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
        // Exit code 2 blocks PreToolUse / PostToolUse / Stop hooks.
        Some(2) if matches!(event, "pre_tool_use" | "post_tool_use" | "stop") => {
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
        if event == "session_start" {
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
    if decision == "block" || !cont || permission_decision == "deny" {
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

    fn temp_project(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "locagens-hooks-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(dir.join(".locagens")).expect("create temp project");
        dir
    }

    #[tokio::test]
    async fn hooks_pre_tool_use_blocks_on_exit_2_and_parses_json_decision() {
        let project = temp_project("pre-tool");
        let hooks = json!({
            "hooks": {
                "SessionStart": "echo hello-session-start",
                "PreToolUse": [
                    { "matcher": "write_file",
                      "hooks": [{ "type": "command",
                                  "command": "echo '{\"hookSpecificOutput\":{\"updatedInput\":{\"path\":\"rewritten.txt\",\"content\":\"hooked\"},\"additionalContext\":\"pre hook ran\"}}'",
                                  "timeout": 10 }] },
                    { "matcher": "run_command",
                      "hooks": [{ "type": "command",
                                  "command": "echo '{\"decision\":\"block\",\"reason\":\"no shell for you\"}'",
                                  "timeout": 10 }] },
                    { "matcher": "Bash|exec_command",
                      "hooks": [{ "type": "command",
                                  "command": "printf 'denied by policy' >&2; exit 2",
                                  "timeout": 10 }] }
                ]
            }
        });
        std::fs::write(
            project.join(".locagens").join("hooks.json"),
            serde_json::to_string(&hooks).unwrap(),
        )
        .unwrap();

        // SessionStart: plain non-JSON stdout becomes additional context.
        let outcome = run_hook_event(
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
        let outcome = run_hook_event(
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
        let outcome = run_hook_event(
            "PreToolUse",
            &project,
            &json!({ "hook_event_name": "PreToolUse", "tool_name": "run_command",
                     "tool_input": { "command": "ls" } }),
        )
        .await;
        assert!(outcome.blocked);
        assert_eq!(outcome.reason.as_deref(), Some("no shell for you"));

        // PreToolUse: exit code 2 with stderr as the reason.
        let outcome = run_hook_event(
            "PreToolUse",
            &project,
            &json!({ "hook_event_name": "PreToolUse", "tool_name": "exec_command",
                     "tool_input": { "cmd": "echo hi" } }),
        )
        .await;
        assert!(outcome.blocked);
        assert_eq!(outcome.reason.as_deref(), Some("denied by policy"));

        // Unmatched tools never trigger the hooks.
        let outcome = run_hook_event(
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

        let outcome = run_hook_event(
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
        let outcome = run_hook_event(
            "PostToolUse",
            &project,
            &json!({ "hook_event_name": "PostToolUse", "tool_name": "read_file" }),
        )
        .await;
        assert!(!outcome.blocked);
        assert!(outcome.additional_context.is_empty());

        // Unknown event names are clean no-ops too.
        let outcome = run_hook_event(
            "NotARealEvent",
            &project,
            &json!({ "hook_event_name": "NotARealEvent" }),
        )
        .await;
        assert!(!outcome.blocked);
        assert!(outcome.additional_context.is_empty());

        let _ = std::fs::remove_dir_all(&project);
    }
}
