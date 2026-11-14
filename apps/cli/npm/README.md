# Locagens (`lcgn`)

**Locagens** is a local-first AI coding agent written in pure Rust. Pick a model provider and a project folder, send a task, and `lcgn` works inside your workspace with tools, permission gating, OS sandboxing, MCP servers, and persistent sessions — everything runs in-process on your machine with zero telemetry.

This npm package installs the prebuilt **`lcgn`** CLI binary from [GitHub Releases](https://github.com/vefacaglar/locagens/releases) for **macOS** (universal Apple Silicon + Intel, signed & notarized) and **Windows** (`x64`).

## Install

```bash
npm install -g locagens
# or
pnpm add -g locagens
```

Or run without installing globally:

```bash
npx locagens --help
```

> **Note for `pnpm` users:** `pnpm` skips `postinstall` scripts by default; the `lcgn` launcher automatically downloads and caches the matching binary under `~/.locagens/bin/` on first run.

## Quick Start

```bash
# 1. Connect a provider (interactive picker, OAuth, or API key)
lcgn auth login

# 2. List authenticated models
lcgn --list-models

# 3. Launch the full-screen terminal UI (TUI) in your project directory
lcgn

# Or run a one-shot task
lcgn -p "explain @src/main.rs"
lcgn -p "fix the failing tests" --approve
```

Attach existing project files in any prompt with `@path` (up to 256 KiB per file), or pipe input to `lcgn`:

```bash
git diff | lcgn -p "summarize these changes"
```

## Features

- **Full-Screen Terminal UI (TUI) & REPL:** Rich markdown rendering, syntax-highlighted code, live tool diffs, queued messages, prompt history (`Ctrl+R`), and a plain line REPL (`--no-tui` / `LCGN_NO_TUI=1`).
- **One-Shot & JSON Modes (`-p`):** Stream final assistant output to `stdout` (progress/tools on `stderr`) or emit structured JSON lines with `--mode json`.
- **Code Review (`lcgn review`):** Read-only review runs against uncommitted changes, branches (`--base main`), or specific commits (`--commit <sha>`) producing prioritized findings (`P0`–`P3`).
- **Workspace Safety & Native Sandboxing:** Workspace-confined file tools plus OS-level sandboxing (macOS Seatbelt `sandbox-exec`, Windows restricted token + Job Object + network proxy, Linux Bubblewrap `bwrap`).
- **Sub-Agent Delegation:** Single-model runs can fan out to `explorer` (read-only) and `worker` sub-agents or custom `agents/*.toml` roles.
- **MCP, Skills & Hooks:** Supports stdio and streamable HTTP MCP servers (including OAuth PKCE via `lcgn mcp login <server>`), `SKILL.md` skills, and lifecycle hooks (`hooks.json`).
- **Shared Store with Desktop App:** Sessions, permissions, and configs live under `~/.locagens` (`$LOCAGENS_HOME`), shared seamlessly with the native Rust desktop app (`locagens`).

## Provider Authentication

Credentials live in `~/.locagens/auth.json` (with read-only fallbacks to `~/.codex/auth.json`, `~/.pi/agent/auth.json`, and OpenCode's `auth.json`).

| Provider | Sign-in Command | Method |
| --- | --- | --- |
| `openai` | `lcgn auth login openai` | Sign in with ChatGPT (browser OAuth) or API key |
| `github-copilot` | `lcgn auth login github-copilot` | GitHub device code login (Enterprise supported) |
| `xai` | `lcgn auth login xai` | xAI / Grok device login |
| `openrouter` | `lcgn auth login openrouter` | Browser OAuth (stores permanent key) |
| `kimi-coding` | `lcgn auth login kimi-coding` | Kimi Code device login |
| `anthropic` | `lcgn auth login anthropic` | API key (`ANTHROPIC_API_KEY`) |
| `opencode-go` / others | `lcgn auth login <provider>` | API key or provider env var |
| `tavily` / `brave-search` / `exa` | `lcgn auth login <backend>` | Optional `web_search` API keys (fallback: DuckDuckGo) |

Check readiness anytime with `lcgn auth` or `lcgn auth check`.

## Interactive TUI Keys & Slash Commands

### Keybindings

| Key | Action |
| --- | --- |
| `Enter` | Send prompt (queues message while a run is active) |
| `Shift+Enter` / `Ctrl+J` | Insert newline |
| `/` | Open slash-command popup |
| `@` | Fuzzy-insert `@path` project file |
| `Shift+Tab` | Cycle mode: `Build` → `Plan` → `Full Access` |
| `Ctrl+O` | Expand reasoning, full tool output, and diffs |
| `Ctrl+R` | Search prompt history for this folder |
| `Esc` / `Ctrl+C` | Interrupt active run / clear composer (press `Ctrl+C` twice to quit) |
| `?` | Open help overlay (when composer is empty) |

### Slash Commands

- `/model [provider/id]` — Switch model
- `/thinking [level]` — Set reasoning level (`off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`)
- `/plan` · `/build` · `/auto` · `/chat` · `/permissions [mode]` — Switch permission & sandbox mode
- `/review [target]` — Start a structured read-only code review
- `/undo` · `/diff` — Revert last turn via snapshot or inspect working-tree diff
- `/new` · `/fork` · `/resume` — Manage or resume persisted sessions
- `/mcp` · `/skills` · `/login` · `/usage` · `/copy` · `/help` · `/exit`

## CLI Flags Reference

```txt
lcgn [options] [@files...] [messages...]
  -p, --print                  Non-interactive: run, print, exit
  --model <provider/id>        Select model (bare id accepted when unambiguous)
  --provider <name>            Provider for a bare --model id
  --thinking <level>           off|minimal|low|medium|high|xhigh|max
  --mode <text|json>           Output mode (default: text)
  --plan                       Plan mode (read-only tools)
  --approve, -a                Full access (no permission prompts)
  -c, --continue               Continue the most recent session here
  -r, --resume                 Pick a session to resume interactively
  --session <id>               Open a session by (partial) id
  --no-session                 Ephemeral run (do not persist rollout)
  --no-tools                   Chat only
  --no-tui                     Plain line REPL instead of full-screen TUI
  --system-prompt <text>       Replace the built-in system prompt
  --append-system-prompt <t>   Append text to the system prompt
  --cwd <dir>                  Project directory (default: cwd)
  --trust                      Trust <project>/.locagens for this invocation
  --list-models [search]       List authenticated models
  --profile <name>             Config profile from config.toml [profiles.<name>]

Subcommands:
  lcgn auth [login|logout|check]
  lcgn review [--uncommitted|--base <branch>|--commit <sha>] [instructions]
  lcgn sessions [--json]
  lcgn trust [PATH] [--revoke|--status]
  lcgn mcp list|login|logout
  lcgn config [show]
```

## Documentation & Desktop App

- [CLI Reference (`docs/CLI.md`)](https://github.com/vefacaglar/locagens/blob/main/docs/CLI.md)
- [Configuration (`docs/CONFIG.md`)](https://github.com/vefacaglar/locagens/blob/main/docs/CONFIG.md)
- [Hooks (`docs/HOOKS.md`)](https://github.com/vefacaglar/locagens/blob/main/docs/HOOKS.md)
- [Sandboxing (`docs/SANDBOX.md`)](https://github.com/vefacaglar/locagens/blob/main/docs/SANDBOX.md)
- [Core API (`docs/CORE_API.md`)](https://github.com/vefacaglar/locagens/blob/main/docs/CORE_API.md)
- [Desktop App & Prebuilt Releases](https://github.com/vefacaglar/locagens/releases)

## License

MIT
