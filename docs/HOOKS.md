# Lifecycle hooks

Hooks are shell commands run at fixed points of a run. They follow the Codex /
Claude Code contract: the event payload is JSON on stdin, the decision comes
from the exit code and optional JSON on stdout. Engine: `crates/core/src/services/hooks.rs`;
call sites: `crates/core/src/orchestrator/hook_calls.rs`.

## Where hooks live

`hooks.json` in `~/.locagens`, `~/.codex`, `~/.pi/agent`, `<project>/.locagens/hooks.json`,
and the `hooks` field of enabled plugin manifests. All matching hooks run in
that order, in the project folder, with a timeout (default 10 s).

```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "run_command|exec_command",
        "hooks": [{ "type": "command", "command": "python guard.py", "timeout": 10 }] }
    ],
    "UserPromptSubmit": "python add_context.py",
    "Stop": ["python check_tests.py"]
  }
}
```

A value is a command string, an array of strings, or matcher groups. Event keys
accept `SessionStart`, `sessionStart`, `session_start` and `onSessionStart`.
`"internal:*"` marker strings are skipped. On Windows commands run through
`cmd /S /C`, elsewhere `sh -c`.

## Events

Every payload carries `session_id` (the run id), `cwd` and `hook_event_name`.

| Event | Fired | Extra payload | Matcher tests | Can block |
| --- | --- | --- | --- | --- |
| `SessionStart` | when a run turn starts | `source` (`startup`/`resume`), `model`, `permission_mode` | `source` | no |
| `UserPromptSubmit` | before the model sees the latest prompt | `prompt`, `model`, `permission_mode` | - | yes: the run fails with the reason |
| `PreToolUse` | before a tool executes | `tool_name`, `tool_input`, `tool_use_id` | `tool_name` | yes: the call is skipped, reason returned to the model |
| `PermissionRequest` | before the user is asked to approve a tool | `tool_name`, `tool_input`, `tool_use_id`, `permission_mode` | `tool_name` | answers: allow or deny |
| `Notification` | the run waits on the user | `notification_type` (`permission_prompt`/`elicitation_dialog`), `message` | `notification_type` | no |
| `PostToolUse` | after a tool returns | `tool_name`, `tool_input`, `tool_response`, `tool_use_id` | `tool_name` | feedback: reason is appended to the tool result |
| `Stop` | main agent produced a final answer | `stop_hook_active`, `last_assistant_message`, `model`, `permission_mode` | - | yes: the reason is fed back and the agent continues (max 3 times) |
| `SubagentStart` | a sub-agent begins | `agent_type` (`explorer`/`worker`), `agent_name` | `agent_type` | no |
| `SubagentStop` | a sub-agent finishes | `agent_type`, `agent_name`, `last_assistant_message` | `agent_type` | parsed, not acted on |
| `PreCompact` | before context compaction | `trigger` (`auto`/`manual`) | `trigger` | yes: compaction is skipped |
| `PostCompact` | after compaction | `trigger` | `trigger` | no |
| `SessionEnd` | the run's drive task finished | `reason` (`done`/`failed`/`cancelled`) | `reason` | no |

Matchers are regexes (substring fallback when invalid); `""` and `*` match all.

## Decisions

- Exit `0`: stdout is parsed as JSON when possible.
  - `{"decision":"block","reason":"..."}`, `{"continue":false,"stopReason":"..."}`
    or `hookSpecificOutput.permissionDecision: "deny"` block (blockable events only).
  - `hookSpecificOutput.additionalContext` and `systemMessage` become extra context
    (system prompt for `SessionStart`/`UserPromptSubmit`, appended to the tool
    result for `PostToolUse`).
  - `hookSpecificOutput.updatedInput` (PreToolUse) replaces the tool arguments.
  - `PermissionRequest`: `{"hookSpecificOutput":{"decision":{"behavior":"allow"}}}` or
    `{"behavior":"deny","message":"..."}`.
  - Plain (non-JSON) stdout of `SessionStart` and `UserPromptSubmit` is added as context.
- Exit `2` on a blockable event: blocks, stderr (or stdout) is the reason. On other
  events it is logged only.
- Any other exit code or a timeout: logged, never blocks.

## Example

```python
# guard.py (PreToolUse): refuse recursive deletes
import json, sys
event = json.load(sys.stdin)
cmd = event["tool_input"].get("command", "") or event["tool_input"].get("cmd", "")
if "rm -rf" in cmd:
    print("rm -rf is not allowed here", file=sys.stderr)
    sys.exit(2)
```
