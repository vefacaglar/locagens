# Locagens

Locagens is a local-first AI coding agent written in pure Rust (no Node.js,
Electron, web frontend, SQLite or HTTP server). You pick one provider/model and
a project folder, send a task, and the model works it with workspace tools
until it returns a final answer. Everything runs in-process on your machine;
there is no telemetry.

It ships as:

- a native desktop app (Dioxus 0.7, binary `locagens`),
- `lcgn`, a command line with a full-screen terminal UI (ratatui), a plain
  line REPL, one-shot `-p` runs and `lcgn review`.

Both use the same core (`locagens_core`) and the same on-disk store, so a
session started in one shows up in the other.

```txt
User task -> selected model -> workspace tool calls (permission-gated)
          -> optional sub-agents (explorer / worker)
          -> tool results back to the model -> final answer
```

Runs are single-model: one provider/model drives the whole run, and the main
agent can fan work out to sub-agents on the same model. There are no agent
presets and no coder/utility model tiers.

## Layout

```txt
crates/protocol      locagens-protocol: shared data types, ApprovalPolicy, SandboxMode
crates/apply-patch   locagens-apply-patch: Codex `*** Begin Patch` parser/applier
crates/execpolicy    locagens-execpolicy: command safety, .rules policy, sandboxes,
                     network proxy, ProcessManager
crates/config        locagens-config: paths, config.toml, auth.json, provider/model catalog
crates/context       locagens-context: AGENTS.md discovery, <environment_context>,
                     TurnDiffTracker
crates/core          locagens-core (lib locagens_core):
  src/api/             typed in-process facade (docs/CORE_API.md)
  src/orchestrator/    run driver, streaming LLM client, tools, compaction, agents
  src/services/        processes, plugins, hooks, skills, symbols, MCP, project trust
  src/cli/             lcgn implementation (args, REPL, TUI, runner, review, auth)
  src/db.rs            file-based JSON/JSONL store
apps/cli             locagens-cli, binary `lcgn` (thin entry into locagens_core::cli)
apps/desktop-rs      locagens-desktop, binary `locagens`
docs/                CORE_API.md, CLI.md, CONFIG.md, HOOKS.md, SANDBOX.md,
                     database-schema.md, RELEASING.md
```

## Build and run

```bash
cargo check --workspace
cargo test --workspace
cargo run -p locagens-desktop
cargo run -p locagens-cli --bin lcgn -- -p "say OK" --model <provider/model>
cargo build --release -p locagens-cli -p locagens-desktop
```

On Windows with the GNU toolchain, put `w64devkit` on `PATH` and use
`rustup run stable-x86_64-pc-windows-gnu cargo ...` (see `state.md`).

## Configuration, providers and logins

The agent directory chain is, primary first: `~/.locagens` (`$LOCAGENS_HOME`),
then `~/.codex` (`$CODEX_HOME`) and `~/.pi/agent` (`$PI_CODING_AGENT_DIR`) as
**read-only fallbacks**. New credentials and settings are written to
`~/.locagens`; nothing is stored in the project or in app data.

```txt
~/.locagens/auth.json         provider id -> credential
                              (literal key, "!command", "$ENV_VAR", or OAuth tokens)
~/.locagens/models.json       custom providers and model overrides
~/.locagens/config.toml       Codex-compatible subset, see docs/CONFIG.md
~/.locagens/hooks.json        lifecycle hooks, see docs/HOOKS.md
~/.locagens/mcp-oauth.json    MCP OAuth tokens
```

Secret files are written atomically with mode 0600 on unix. API keys are never
logged or returned by settings (the UI gets a preserve marker,
`__LOCAGENS_PRESERVE_API_KEY__`, instead).

Providers are OpenAI-compatible Chat Completions, OpenAI Responses and
Anthropic Messages endpoints. Sign in with `lcgn auth login <provider>` or the
desktop Settings:

| Provider | Login |
| --- | --- |
| most providers | API key (`auth.json`, or the provider's env var such as `OPENAI_API_KEY`) |
| `openai` | API key, or "Sign in with ChatGPT" (browser + loopback redirect; uses the Responses API) |
| `github-copilot` | GitHub device login (Enterprise domains supported) |
| `xai` | xAI / Grok device login |
| `openrouter` | Sign in with OpenRouter (browser + loopback redirect; stores a permanent key) |
| `kimi-coding` | Kimi Code device login |
| `anthropic` | **API key only.** No Claude subscription OAuth. |

OAuth tokens are refreshed automatically. Only providers you have configured
appear in the model list. There is no built-in pricing table: set `pricing` on
a model in `models.json` to get cost in the usage logs; unpriced models cost 0.

## Modes and permissions

The UI offers three modes; the core also keeps `chat` and `ask_permissions`.

| Mode | Codex policy | Behaviour |
| --- | --- | --- |
| Build (`accept_edits`) | on-request + workspace-write | file edits apply directly; commands and `fetch_url` prompt (known-safe commands excepted) |
| Plan (`plan`) | on-request + read-only | read-only tools, `update_plan`, research and sub-agents; no mutations or commands |
| Full Access (`auto`) | never + danger-full-access | no prompts, no sandbox |
| `chat` | read-only | conversation with read-only tools |
| `ask_permissions` | untrusted + workspace-write | everything that mutates or executes prompts |

What prompts (`orchestrator::tools::needs_permission` plus the gate in the
orchestrator), outside Full Access:

- `run_command`, `exec_command`, `write_stdin` and `fetch_url` prompt in every
  mode that offers them. In Build, **known-safe** commands (read-only ones
  such as `git status`, `rg`, `cargo check`) and commands an execpolicy
  `.rules` file allows run without a prompt, unless the call asks for
  `network_domains`. A `.rules` `forbidden` match blocks the command outright.
- File mutations (`write_file`, `edit_file`, `delete_file`, `move_file`,
  `create_directory`, `apply_patch`) and MCP tool calls prompt in
  `ask_permissions`, not in Build.
- **Dangerous** commands (`rm -rf`, `git reset --hard`, `sudo`, ...) and writes
  to **protected paths** (`.git`, `.locagens`, `.codex`, `.agents`) always
  prompt. No standing grant or "allow for run" covers them, and the answer
  approves that one call only.
- Answers are `allow_once | allow_project | allow_run | allow_always | deny`;
  unknown answers deny. Standing grants persist in `permissions.json`.
- **Project trust.** `<project>/.locagens/` (hooks, `mcp.json`, plugins,
  `.rules`, `agents/*.toml`) loads only for trusted folders: `lcgn trust`,
  `--trust` for one invocation, or the desktop trust banner. Trust is stored in
  `~/.locagens/config.toml` under `[projects."<path>"]`.

All file paths resolve inside the project folder (symlink-aware containment).

## Sandboxing

Commands run inside a native sandbox chosen from the mode (`workspace-write`
in Build and `ask_permissions`, none in Full Access):

- **Windows:** restricted token (write-restricted, capability-SID ACLs) with a
  Job Object, and a per-domain allowlist network proxy.
- **macOS:** Seatbelt (`sandbox-exec`).
- **Linux:** Bubblewrap (`bwrap`).

On macOS/Linux a missing backend fails closed (commands refuse to run outside
Full Access), and approving `network_domains` opens the whole network for that
command because no per-domain proxy exists there. Details and limits are in
`docs/SANDBOX.md`.

## Tools

The main agent's toolset depends on the mode (`chat` is the smallest, Plan adds
planning and delegation, Build and Full Access get everything).

```txt
Files        read_file, write_file, edit_file, apply_patch, delete_file, move_file,
             create_directory, list_directory, search_files, view_image
Commands     exec_command (+ write_stdin for running sessions), run_command,
             host_executable
Network      fetch_url
Agents       spawn_agent, send_message, followup_task, wait_agent, interrupt_agent,
             close_agent, list_agents (persistent named agents, max 4 live),
             spawn_agents (one-shot parallel batch)
Interaction  ask_user_question, request_user_input, update_plan, set_chat_title
Context      compact_context, get_context_remaining, remember, load_skill
MCP          list_mcp_resources, read_mcp_resource, mcp__<server>__<tool>,
             tool_search (only when more than 8 MCP tools are connected)
```

Sub-agent roles: `explorer` (read-only, the default) and `worker` (full
tools), plus custom roles from `agents/*.toml` in `<project>/.locagens/` and
the agent directories. Children share the parent's run (cancel and permission
prompts go through the parent), get no agent tools (depth 1), and Plan/chat
force read-only.

Context is auto-compacted above about 90k estimated input tokens (last 8
messages kept). `AGENTS.md` files are discovered from the git root down to the
working directory and injected into the system prompt, and a Codex-style
`<environment_context>` block seeds each run.

## Extensions

- **MCP** servers over stdio and streamable HTTP (`http`, `streamable-http`,
  `sse` entries), configured in `mcp_servers.json` or `<project>/.locagens/mcp.json`.
  Remote servers can use bearer tokens, custom headers or OAuth
  (`lcgn mcp login <server>`: discovery, dynamic registration, PKCE).
- **Skills**: `SKILL.md` packs loaded with `load_skill`.
- **Plugins**: registry of MCP servers, custom tools and hooks.
- **Hooks**: Codex/Claude-style lifecycle hooks (`PreToolUse`, `Stop`, ...) in
  `hooks.json`; see `docs/HOOKS.md`.

## Command line

```bash
lcgn --model <provider/model>              # full-screen TUI (or --no-tui for the line REPL)
lcgn -p "say OK" --model <provider/model>  # one-shot, answer on stdout
lcgn -c "keep going"                       # continue the most recent session
lcgn auth login openai                     # sign in (see the table above)
lcgn review --base main                    # read-only code review
lcgn trust .                               # trust this project's .locagens config
lcgn mcp list | login <server> | logout <server>
lcgn config                                # effective config.toml, secrets masked
```

Slash commands (`/model`, `/thinking`, `/plan`, `/build`, `/auto`, `/chat`,
`/new`, `/fork`, `/undo`, `/diff`, `/review`, `/mcp`, `/skills`, `/sessions`,
`/usage`, `/help`, `/exit`) work in the TUI and the REPL. See `docs/CLI.md`.

## Storage

Codex-style files under `~/.locagens` (`$LOCAGENS_HOME`; `LOCAGENS_DATA_DIR`
overrides the root for tests): one append-only JSONL rollout per run at
`sessions/YYYY/MM/DD/rollout-<ts>-<run_id>.jsonl`, plus `session_index.jsonl`,
`history.jsonl`, `memories/*.md`, `projects.json` and `permissions.json`.
Every message is persisted and runs reopen after a restart; startup marks runs
that were active when a process died as `failed`. The desktop app and `lcgn`
can run at the same time (advisory lock on `<root>/.lock`). Layout:
`docs/database-schema.md`.

## Not implemented

A reviewer/acceptance loop after sub-agent fan-out, extra approval checkpoints
beyond the permission flow, multi-user auth and cloud deployment.
