# GEMINI.md

## Project

Locagens is a local-first AI orchestration tool. For architecture, project
map, and rules see `Claude.md`; for runtime behavior see `Agents.md`.

## Development & Agent Workflow (pi + Gemini)

How development work is executed in this repo:

- **Codebase exploration (read-only research & analysis)**: always use
  `gemini-4-hydrogen` (`Gemini 4 Hydrogen`), default effort `medium` (`low` for
  trivial lookups, `high`/`xhigh` for deep analysis), run as a read-only
  subagent or swarm worker pinned to a `gemini-4-hydrogen-*` model. Never burn a
  coding model on exploration.
- **All coding, development & file modifications**: always execute via the
  `pi` CLI using model `opencode-go/glm-5.3-flash`.

### Non-interactive `pi` CLI invocation (Windows PowerShell)

Always close stdin (`$null |`) and pass `--no-mcp --no-extensions --approve -p`.
For multi-line tasks on Windows, either pass a single-line prompt or pass a
`@prompt.md` file reference so Windows CLI argument parsing does not split on
newlines:

```powershell
$null | pi --model opencode-go/glm-5.3-flash --no-mcp --no-extensions --approve -p @path/to/prompt.md "Execute the instructions in the attached prompt file."
```

Or for short single-line prompts:

```powershell
$null | pi --model opencode-go/glm-5.3-flash --no-mcp --no-extensions --approve -p "Single line instruction"
```

### Locagens runtime architecture direction

Locagens is **100% pure Rust**: a modular workspace of `crates/*`
(`locagens-protocol`, `locagens-apply-patch`, `locagens-execpolicy`,
`locagens-config`, `locagens-context`, `locagens-core`) plus `apps/cli`
(`locagens-cli`, binary `lcgn`) and `apps/desktop-rs` (native Dioxus 0.7
desktop app, binary `locagens`), built directly on top of `pi` (in-process;
`pi auth` / `~/.pi/agent/auth.json`, `models.json`). Consumers call the typed
in-process facade `locagens_core::api` (see `docs/CORE_API.md`). There is no
Node backend, no Electron shell, no Vue web app, and no HTTP server.

## Phase 9 — Codex Harness Architecture (implemented)

Locagens now ships the Codex tool harness (ported from `codex-rs`):

- **`apply_patch`** (`crates/apply-patch`): full `*** Begin Patch` …
  `*** End Patch` parser (add / delete / update / move, multi-chunk updates,
  `@@ context`, `*** End of File`), the fuzzy `seek_sequence` matcher (exact →
  `trim_end` → `trim` → Unicode-punctuation normalization), workspace-safe
  application, `preview_patch` and Codex's exact
  `Success. Updated the following files:` summary. Every target/move path is
  validated through `safe_path`, so patches cannot escape the workspace.
- **`exec_command` / `write_stdin`** (unified exec): commands run through the
  ProcessManager with a secret-stripped environment; they return
  `{ exit_code, output, wall_time_ms }` when they finish within
  `yield_time_ms` (default 10000 ms, clamped 500..60000) and otherwise
  register an interactive session the model can drive with `write_stdin`.
- **Command safety & policy** (`crates/execpolicy`): Codex's
  `ApprovalPolicy` (`untrusted` / `on-request` / `on-failure` / `never`) and
  `SandboxMode` (`read-only` / `workspace-write` / `danger-full-access`)
  mapped from Locagens modes (`chat`/`plan` → read-only, `accept_edits` →
  workspace-write + on-request, `ask_permissions` → untrusted, `auto` →
  danger-full-access + never). Known-safe read-only commands (`git status`,
  `rg`, `cargo check`, …) run without prompting in `accept_edits`; dangerous
  commands (`rm -rf`, `git reset --hard`, `sudo`, …) always prompt.
- **Hierarchical `AGENTS.md` discovery** (`crates/context`): walks
  from the git root down to the cwd picking at most one doc per level
  (`AGENTS.override.md` > `AGENTS.md` > `CLAUDE.md` > `.agents.md`), joined
  with `

--- project-doc ---

` under a 32 KiB budget and injected into
  the system prompt.
- **`<environment_context>`** (`crates/context`): Codex-format
  XML block (cwd / shell / approval_policy / sandbox_mode / network_access /
  workspace_roots / current_date) seeded as the first user message of a run.
- **`TurnDiffTracker`** (`crates/context`): snapshots file
  baselines before mutations (`apply_patch`, `write_file`, `edit_file`,
  `delete_file`, `move_file`) and streams `turn_diff_updated` events with the
  aggregate unified diff and changed-file count.
- **Context compaction** (`locagens-core` orchestrator): automatic compaction
  when the estimated input tokens exceed 90k (also on demand via
  `compact_context`): older history is summarized with Codex's
  SUMMARIZATION_PROMPT and replaced by a `[Compacted context summary]` block
  while the last 8 messages stay intact.
- **Codex sub-agent tools**: `spawn_agent` (background agent with id),
  `wait_agent`, `send_input`, `close_agent`, `list_agents`, plus
  `request_user_input` (Codex question schema) and `view_image`.
- **Config & auth**: the primary agent directory is now `~/.locagens`
  (`$LOCAGENS_HOME`); `~/.codex` (`$CODEX_HOME`: `auth.json` OPENAI_API_KEY
  and `config.toml` `[model_providers.*]`) and the legacy `~/.pi/agent`
  directory are read as credential/config fallbacks.
- **`.vscode/tasks.json` + `.vscode/launch.json`**: w64devkit PATH, rustup-run
  GNU toolchain tasks (check/build/test/run) and cppdbg/gdb + CodeLLDB debug
  configurations for the desktop app and the `lcgn` CLI.
