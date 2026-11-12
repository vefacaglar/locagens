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

Locagens is **100% pure Rust**: `apps/backend` (`locagens_core` library +
`lcgn` CLI) and `apps/desktop-rs` (native Dioxus 0.7 desktop app, binary
`locagens`), built directly on top of `pi` (in-process; `pi auth` /
`~/.pi/agent/auth.json`, `models.json`). Consumers call the typed in-process
facade `locagens_core::api` (see `docs/CORE_API.md`). There is no Node
backend, no Electron shell, no Vue web app, and no HTTP server.
