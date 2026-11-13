# `config.toml`

Locagens reads the Codex `config.toml` format from the agent directory chain,
primary first:

1. `~/.locagens/config.toml` (`$LOCAGENS_HOME`)
2. `~/.codex/config.toml` (`$CODEX_HOME`)
3. `~/.pi/agent/config.toml` (`$PI_CODING_AGENT_DIR`)

Files are deep-merged table by table; on a conflict the earlier file wins
(arrays and scalars are replaced, not concatenated). Unknown keys are ignored.
Malformed TOML, or a known key with the wrong type, in the primary
`~/.locagens/config.toml` is an error that names the file
(`lcgn: Invalid config file C:\Users\me\.locagens\config.toml: ...`). The
read-only fallbacks (`~/.codex`, `~/.pi/agent`) are parsed leniently: an
unreadable file is skipped and a key with the wrong type is dropped, each with
a warning, so a broken Codex config cannot break Locagens.

The effective config is reloaded when any file of the chain changes (checked at
the start of every run), so a running desktop app sees edits without a restart;
a config that no longer parses keeps the previous one active. The selected
profile is kept across reloads.

Precedence: CLI flags > selected profile > top-level keys > built-in defaults.
`lcgn config` (alias `lcgn config show`) prints the effective config with
secrets masked (values under `env`, `http_headers`, `shell_environment_policy.set`
and any key containing `key`, `token`, `secret`, `password`, `authorization`;
in list values such as `args` / `notify`: `--token=...`, `NAME=value` with a
secret-looking name and the item after a bare `--api-key`-style flag; in URLs:
`user:password@` and secret-looking query parameters). It does not open the
session store.

Typed implementation: `crates/config/src/config_toml.rs` (`Config`);
process-wide access: `locagens_core::config_runtime::current()`.

## Supported keys

| Key | Effect |
| --- | --- |
| `model`, `model_provider` | Default `--model` / `--provider` for `lcgn` when no flag is given |
| `model_reasoning_effort` | Default `--thinking` (`none` = `off`); default for new runs without an explicit effort |
| `model_reasoning_summary` | `reasoning.summary` on OpenAI Responses requests (`auto`, `concise`, `detailed`; `none` omits it) |
| `model_verbosity` | `text.verbosity` on OpenAI Responses requests |
| `model_context_window` | Auto-compaction triggers at 90% of the window |
| `model_auto_compact_token_limit` | Explicit compaction threshold (wins over the window); default is ~90k estimated tokens |
| `approval_policy`, `sandbox_mode` | Default run mode when none is chosen: `never` or `danger-full-access` = `auto`; `read-only` = `plan`; `untrusted` = `ask_permissions`; otherwise `accept_edits` |
| `[sandbox_workspace_write]` | `network_access = true` lets sandboxed `run_command` and `exec_command` use the network (all modes and platforms; on Windows it skips the proxy). `writable_roots` (`~` expanded; missing dirs skipped) become extra writable directories in workspace-write mode on every backend (Windows: an allow-write ACE for the workspace capability SID; macOS: a Seatbelt `subpath`; Linux: a bwrap `--bind`). Codex's `exclude_tmpdir_env_var` / `exclude_slash_tmp` are ignored |
| `[sandbox.network]` | `allowed_domains = ["registry.npmjs.org", "**.crates.io"]`: hosts sandboxed commands may reach through the per-command proxy (see docs/SANDBOX.md); read from the whole agent-dir chain |
| `[shell_environment_policy]` | Environment of spawned commands: `inherit` (`all`/`core`/`none`), `ignore_default_excludes`, `exclude`, `set`, `include_only` (case-insensitive globs). Default excludes drop `*KEY*`, `*SECRET*`, `*TOKEN*` once a policy table exists. Applies to every command path (`run_command`, `exec_command` pipe / PTY / Windows sandbox spawn); without a policy a built-in safe allowlist (PATH, HOME, TEMP, ...) with secrets removed is used |
| `[model_providers.<id>]` | Custom providers (unchanged: read by the provider engine from every chain file) |
| `[mcp_servers.<name>]` | Parsed into `McpServerConfig` (`command`, `args`, `env`, `url`, `bearer_token_env_var`, `http_headers`, `enabled`, `enabled_tools`, `disabled_tools`) and merged into the MCP server list (user scope, lowest precedence: a same-named entry in `mcp_servers.json` / project `.locagens/mcp.json` wins). `env_vars` are forwarded from the environment. `enabled_tools` (allowlist) and `disabled_tools` (always wins) hide tools from the model and refuse calls to them, also when the server itself is defined in `mcp.json`. `cwd`, `startup_timeout_sec` and `tool_timeout_sec` are ignored |
| `profile`, `[profiles.<name>]` | Profile overlay of the model, reasoning, verbosity, context, approval and sandbox keys; select with `--profile <name>` |
| `project_doc_max_bytes` | AGENTS.md byte budget (default 32768, `0` disables project docs) |
| `project_doc_fallback_filenames` | Extra doc filenames tried after `AGENTS.override.md`, `AGENTS.md`, `CLAUDE.md`, `.agents.md` |
| `notify` | Command run when a turn completes; the JSON event is appended as the last argument |
| `hide_agent_reasoning` | Do not render reasoning in `lcgn` (explicit `--thinking` re-enables) |
| `[history]` `persistence` | `none` stops writing `history.jsonl`; `save-all` (default) keeps it |

Codex keys without an lcgn equivalent (`show_raw_agent_reasoning`,
`file_opener`, `[tui]`, `history.max_bytes`) are ignored like any other unknown key.

## Example

```toml
model = "gpt-5"
model_provider = "openai"
model_reasoning_effort = "medium"
model_auto_compact_token_limit = 60000
approval_policy = "on-request"
sandbox_mode = "workspace-write"
project_doc_max_bytes = 65536
project_doc_fallback_filenames = ["TEAM_GUIDE.md"]
notify = ["python", "C:/tools/notify.py"]
profile = "daily"

[sandbox_workspace_write]
network_access = false

[shell_environment_policy]
inherit = "core"
exclude = ["AWS_*"]
[shell_environment_policy.set]
CI = "1"

[history]
persistence = "none"

[profiles.daily]
model = "glm-5.3-flash"
model_provider = "opencode-go"

[profiles.careful]
approval_policy = "untrusted"
model_reasoning_effort = "high"

[mcp_servers.docs]
command = "npx"
args = ["-y", "@acme/docs-mcp"]
[mcp_servers.docs.env]
DOCS_TOKEN = "..."   # masked by `lcgn config`
```

```txt
lcgn -p "refactor module x" --profile careful      # profile overlay
lcgn -p "hi" --model opencode-go/glm-5.3-flash      # flag beats config
lcgn config                                         # effective config, secrets masked
```

The `notify` payload (Codex contract):

```json
{ "type": "agent-turn-complete", "thread-id": "<run id>", "cwd": "<project>", "last-assistant-message": "..." }
```
