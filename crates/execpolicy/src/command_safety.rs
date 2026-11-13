//! Codex-style command safety classification and approval/sandbox policy.
//!
//! Ported from the Codex harness's `AskForApproval` / `SandboxMode` policy
//! model: known-safe read-only commands run without prompting under
//! `on-request` + `workspace-write`, while mutating, destructive or
//! network-bound commands require a standing grant or an explicit approval.

use serde_json::Value;

// The canonical policy enums live in `locagens-protocol`; re-exporting keeps
// existing `command_safety::{ApprovalPolicy, SandboxMode}` callers working.
pub use locagens_protocol::{ApprovalPolicy, SandboxMode};

/// Resolves the Locagens run `mode` into Codex approval/sandbox semantics.
pub fn resolve_policy(mode: &str) -> (ApprovalPolicy, SandboxMode) {
    locagens_protocol::from_run_mode(mode)
}

/// Known-safe read-only commands (first token(s) match).
const SAFE_READ_ONLY_PREFIXES: [&str; 36] = [
    "git status",
    "git diff",
    "git log",
    "git show",
    "git branch",
    "rg",
    "grep",
    "find",
    "ls",
    "dir",
    "pwd",
    "cat",
    "head",
    "tail",
    "wc",
    "which",
    "where",
    "echo",
    "Get-ChildItem",
    "Get-Content",
    "Select-String",
    "Get-Location",
    "cargo check",
    "cargo test --no-run",
    "cargo doc",
    "rustc --version",
    "cargo --version",
    "node --version",
    "npm ls",
    "python --version",
    "python3 --version",
    "dotnet --version",
    "go version",
    "rustup show",
    "tree",
    "type",
];

/// Destructive / dangerous command markers.
const DANGEROUS_MARKERS: [&str; 24] = [
    "rm -rf",
    "Remove-Item -Recurse",
    "Remove-Item -Force",
    "git reset --hard",
    "git checkout --",
    "git clean -f",
    "format",
    "mkfs",
    "dd ",
    "diskpart",
    "reg delete",
    "del /f /s /q",
    "rmdir /s /q",
    "sudo ",
    "runas ",
    "shutdown",
    "taskkill /f",
    "kill -9",
    "chmod 777",
    "> /dev/sda",
    "npx --yes",
    "curl | sh",
    "Invoke-Expression",
    "iex ",
];

/// Whether the raw command text contains a known dangerous marker
/// (privilege escalation, destructive wipe, forced kill, …).
pub(crate) fn has_dangerous_marker(command: &str) -> bool {
    let lowered = command.to_lowercase();
    DANGEROUS_MARKERS
        .iter()
        .any(|marker| lowered.contains(&marker.trim().to_lowercase()))
}

/// Classification result for a shell command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSafety {
    /// Read-only / known safe — may run without approval in `accept_edits`.
    KnownSafeReadOnly,
    /// Mutating or network-bound — requires approval / standing grant.
    RequiresApproval,
    /// Actively dangerous — always requires explicit approval.
    Dangerous,
}

/// Classifies a shell command string.
pub fn classify_command(command: &str) -> CommandSafety {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return CommandSafety::RequiresApproval;
    }
    if has_dangerous_marker(trimmed) {
        return CommandSafety::Dangerous;
    }

    // Compound commands: every segment must be classified; the strictest
    // segment wins. Unparseable (redirection / subshell / substitution)
    // commands always require approval.
    match crate::rules::split_shell_commands(trimmed) {
        Ok(segments) if !segments.is_empty() => {
            let mut result = CommandSafety::KnownSafeReadOnly;
            for segment in &segments {
                match classify_single_segment(&segment.join(" ")) {
                    CommandSafety::Dangerous => return CommandSafety::Dangerous,
                    CommandSafety::RequiresApproval => result = CommandSafety::RequiresApproval,
                    CommandSafety::KnownSafeReadOnly => {}
                }
            }
            result
        }
        _ => CommandSafety::RequiresApproval,
    }
}

/// Classifies a single (already split) command segment.
pub(crate) fn classify_single_segment(command: &str) -> CommandSafety {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return CommandSafety::RequiresApproval;
    }
    let lowered = trimmed.to_lowercase();

    // Privilege escalation and destructive markers always win.
    if has_dangerous_marker(trimmed) {
        return CommandSafety::Dangerous;
    }

    for prefix in SAFE_READ_ONLY_PREFIXES {
        let prefix = prefix.to_lowercase();
        if lowered == prefix || lowered.starts_with(&format!("{prefix} ")) {
            return CommandSafety::KnownSafeReadOnly;
        }
    }

    CommandSafety::RequiresApproval
}

/// Backwards-compatible helper used by the gating layer.
pub fn is_known_safe_read_only_command(command: &str) -> bool {
    classify_command(command) == CommandSafety::KnownSafeReadOnly
}

/// Whether a command is classified as actively dangerous.
pub fn is_dangerous_command(command: &str) -> bool {
    classify_command(command) == CommandSafety::Dangerous
}

/// Whether an `exec_command`/`run_command` call can bypass the interactive
/// permission prompt for the given run mode, consulting the loaded Starlark
/// `.rules` policy when one is available.
///
/// Returns `Err(reason)` when an execpolicy rule forbids the command outright.
pub fn command_bypasses_prompt_with_policy(
    mode: &str,
    command: &str,
    policy: Option<&crate::rules::Policy>,
) -> Result<bool, String> {
    let Some(policy) = policy else {
        return Ok(command_bypasses_prompt(mode, command));
    };
    let evaluation = policy.evaluate_command(command);
    if evaluation.decision == crate::rules::Decision::Forbidden {
        return Err(evaluation
            .justification
            .unwrap_or_else(|| "Command is forbidden by execpolicy rule.".to_string()));
    }
    match resolve_policy(mode) {
        // Never prompt.
        (ApprovalPolicy::Never, _) => Ok(true),
        // Read-only sandbox: commands are not part of the mutation gating.
        (_, SandboxMode::ReadOnly) => Ok(false),
        // workspace-write + on-request: allow-listed (rule or known-safe)
        // commands run now.
        (ApprovalPolicy::OnRequest, SandboxMode::WorkspaceWrite) => Ok(
            evaluation.decision == crate::rules::Decision::Allow && !is_dangerous_command(command),
        ),
        // untrusted / any remaining combination: everything prompts.
        _ => Ok(false),
    }
}

/// Whether an `exec_command`/`run_command` call can bypass the interactive
/// permission prompt for the given run mode.
pub fn command_bypasses_prompt(mode: &str, command: &str) -> bool {
    match resolve_policy(mode) {
        // Never prompt.
        (ApprovalPolicy::Never, _) => true,
        // Read-only sandbox: commands are not part of the mutation gating.
        (_, SandboxMode::ReadOnly) => false,
        // workspace-write + on-request: known-safe read-only commands run now.
        (ApprovalPolicy::OnRequest, SandboxMode::WorkspaceWrite) => {
            is_known_safe_read_only_command(command)
        }
        // untrusted / any remaining combination: everything prompts.
        _ => false,
    }
}

/// Extracts the `command` string from tool arguments.
pub fn command_from_args(args: &Value) -> String {
    args.get("command")
        .or_else(|| args.get("cmd"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_safe_read_only_classification() {
        for command in [
            "git status",
            "git diff HEAD",
            "git log --oneline -5",
            "git show",
            "git branch -a",
            "rg pattern src",
            "grep -rn todo .",
            "find . -name '*.rs'",
            "ls -la",
            "dir",
            "pwd",
            "cat README.md",
            "head -n 10 file",
            "tail file",
            "wc -l src",
            "which cargo",
            "where cargo",
            "echo hello",
            "Get-ChildItem src",
            "Get-Content file.txt",
            "Select-String -Path x",
            "Get-Location",
            "cargo check",
            "cargo check --workspace",
            "cargo test --no-run",
        ] {
            assert_eq!(
                classify_command(command),
                CommandSafety::KnownSafeReadOnly,
                "{command} should be known safe"
            );
        }
    }

    #[test]
    fn mutating_commands_require_approval() {
        for command in ["npm test", "cargo build", "git commit -m x", "node script.js"] {
            assert_eq!(
                classify_command(command),
                CommandSafety::RequiresApproval,
                "{command} should require approval"
            );
        }
    }

    #[test]
    fn dangerous_commands_are_flagged() {
        for command in [
            "rm -rf /",
            "Remove-Item -Recurse -Force C:\\data",
            "git reset --hard",
            "git checkout -- .",
            "git clean -fd",
            "format C:",
            "mkfs.ext4 /dev/sda1",
            "dd if=/dev/zero of=/dev/sda",
            "sudo apt install x",
            "runas /user:admin cmd",
        ] {
            assert_eq!(
                classify_command(command),
                CommandSafety::Dangerous,
                "{command} should be dangerous"
            );
        }
    }

    #[test]
    fn compound_commands_never_bypass_when_second_segment_is_unsafe() {
        // A safe first segment never masks an unsafe second one.
        assert_eq!(
            classify_command("git status && rm -rf /"),
            CommandSafety::Dangerous
        );
        assert_eq!(
            classify_command("cargo check && npm test"),
            CommandSafety::RequiresApproval
        );
        assert_eq!(
            classify_command("echo hi && git status"),
            CommandSafety::KnownSafeReadOnly
        );
        // Unparseable constructs always require approval.
        assert_eq!(classify_command("echo hi > out.txt"), CommandSafety::RequiresApproval);
        assert_eq!(classify_command("echo $(whoami)"), CommandSafety::RequiresApproval);

        // The policy-aware gate agrees.
        assert!(!command_bypasses_prompt_with_policy(
            "accept_edits",
            "git status && rm -rf /",
            None
        )
        .unwrap());
        assert!(command_bypasses_prompt_with_policy(
            "accept_edits",
            "git status && echo hi",
            None
        )
        .unwrap());
    }

    #[test]
    fn policy_resolution_maps_locagens_modes() {
        assert_eq!(resolve_policy("accept_edits"), (ApprovalPolicy::OnRequest, SandboxMode::WorkspaceWrite));
        assert_eq!(resolve_policy("plan"), (ApprovalPolicy::OnRequest, SandboxMode::ReadOnly));
        assert_eq!(resolve_policy("chat"), (ApprovalPolicy::OnRequest, SandboxMode::ReadOnly));
        assert_eq!(resolve_policy("ask_permissions"), (ApprovalPolicy::Untrusted, SandboxMode::WorkspaceWrite));
        assert_eq!(resolve_policy("auto"), (ApprovalPolicy::Never, SandboxMode::DangerFullAccess));
    }

    #[test]
    fn prompt_bypass_matrix() {
        // accept_edits: safe commands bypass, mutating ones do not.
        assert!(command_bypasses_prompt("accept_edits", "git status"));
        assert!(command_bypasses_prompt("accept_edits", "cargo check"));
        assert!(!command_bypasses_prompt("accept_edits", "npm test"));
        assert!(!command_bypasses_prompt("accept_edits", "rm -rf /"));
        // ask_permissions: everything prompts.
        assert!(!command_bypasses_prompt("ask_permissions", "git status"));
        // auto: nothing prompts.
        assert!(command_bypasses_prompt("auto", "anything at all"));
        // read-only modes: no direct command execution bypass.
        assert!(!command_bypasses_prompt("plan", "git status"));
    }
}
