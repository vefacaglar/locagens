# `lcgn` — the Locagens CLI

`lcgn` is a pi-like command line for Locagens. It runs the **same Rust core as
the desktop app** (orchestrator, `PiProviderManager` tools, permission flow,
file-based sessions) **in-process** — no HTTP server and no spawned `pi`
process. Sessions it creates are normal Locagens runs, so they show up in the
desktop app and can be continued from there.

## Install / build

From the repo root (Windows, w64devkit toolchain):

```powershell
$env:PATH = "$env:USERPROFILE\w64devkit\w64devkit\bin;$env:PATH"
rustup run stable-x86_64-pc-windows-gnu cargo build --bin lcgn --release
# binary at target/release/lcgn(.exe)
```

## One-shot runs (`-p`)

```powershell
lcgn -p "say OK" --model opencode-go/glm-5.3-flash --no-tools
lcgn -p "list the files here" --model opencode-go/glm-5.3-flash --approve
lcgn -p "explain @src/main.rs" --model opencode-go/glm-5.3-flash
type error.log | lcgn -p "what happened here?" --model opencode-go/glm-5.3-flash
```

- `@path` arguments are attached as files; piped (non-TTY) stdin is appended
  to the prompt, like `pi`.
- Text mode streams the assistant answer to **stdout** and everything else
  (reasoning, tool lines like `✔ read_file src/main.rs`, usage/cost) to
  **stderr** — so `lcgn -p "..." > out.txt` captures only the answer.
- `--mode json` prints one JSON event per line on stdout (the same run-event
  objects the SSE endpoint emits).
- Exit codes: `0` success, `1` run failed/cancelled, `2` usage or
  configuration error (unknown model, provider not authenticated, …).
- Without `--approve`, dangerous tool calls (`run_command`, `fetch_url`, file
  mutations in ask-permissions flows) are **denied with a message** unless a
  standing grant exists in `permissions.json`. Use `-a/--approve` for full
  access, or drop `-p` to approve interactively.

## Interactive terminal UI

```powershell
lcgn --model opencode-go/glm-5.3-flash
```

With a terminal on stdin and stdout, `lcgn` opens a Codex-style full-screen
UI (alternate screen): header with project path and run title, a scrollable
transcript (markdown, syntax-highlighted code, tool cells, diffs), a status
line while a run is active, a multi-line composer and a footer with
provider/model, thinking level, mode, session id, token usage and context
left. Use `--no-tui` (or `LCGN_NO_TUI=1`) for the plain line REPL below.
`-p` and `--mode json` never start the UI.

Keys:

```txt
Enter                      send (while a run is active: queue the message)
Shift+Enter, Ctrl+J,       newline (also: end a line with \ then Enter)
  Alt+Enter
Up / Down                  move the cursor; on the first/last row recall history
Ctrl+A / Ctrl+E            line start / end (Home / End also work)
Ctrl+W, Ctrl+Backspace     delete previous word; Alt+D deletes the next word
Ctrl+U / Ctrl+K            delete to line start / end
Ctrl+Left / Ctrl+Right     move by word
/                          command popup (Up/Down, Tab or Enter to complete)
@                          file popup (fuzzy project files, inserts @path)
PageUp / PageDown, wheel   scroll the transcript; End follows the output again
Ctrl+O                     show / hide reasoning
Shift+Tab                  toggle Plan / Build mode
Esc                        close popup, otherwise interrupt the active run
Ctrl+C                     clear the composer; else interrupt the run; else
                           press twice within 2s to quit
Ctrl+D                     quit (empty composer)
```

Permission requests open a modal: `o`/`y` allow once, `r` this run, `p`
project, `a` always, `d`/`n`/Esc deny, Up/Down scroll the diff. Questions
from the agent open a list: Up/Down, Space (multi-select), Enter to confirm,
Tab for a free-text note, Esc to skip. `@path` mentions of existing project
files are attached to the prompt like in the REPL.

Slash commands (TUI and REPL):

```txt
/model [provider/id]   show or switch models
/thinking <level>      off|minimal|low|medium|high|xhigh|max
/plan                  plan mode (read-only tools)
/build                 build mode (edits applied directly)
/new                   start a fresh session on the next message
/fork                  fork the current session
/undo                  undo the last turn (git snapshot restore)
/diff                  working-tree diff
/mcp, /skills          list MCP servers / skills
/sessions              list sessions for this directory
/usage                 usage/cost of the current session
/help, /exit
```

## Plain REPL (`--no-tui`)

A line-editor REPL with streaming output and the same slash commands.
`Ctrl+C` cancels the running run (or exits at an idle prompt).

## Sessions

```powershell
lcgn -c "keep going"                     # continue the most recent session
lcgn --session run-882c "and now?"       # open a session by (partial) id
lcgn -r                                  # pick a session interactively
lcgn sessions                            # list sessions for this directory
lcgn sessions --json
lcgn --no-session -p "quick question" …  # ephemeral, not persisted
```

## Models

```powershell
lcgn --list-models            # table: provider, model, context, max-out, thinking, images
lcgn --list-models glm
lcgn --model glm-5.3-flash    # bare id works when unambiguous
lcgn --provider opencode-go --model glm-5.3-flash
```

## Auth

Credentials live in `~/.pi/agent/auth.json`, shared with `pi` and the desktop
app. Keys are prompted with no echo when `--api-key` is omitted.

```powershell
lcgn auth                       # provider table (id, name, sign-in method, status) + usage hint
lcgn auth login                 # numbered table; on a terminal, prompts for number or id
lcgn auth login opencode-go [--api-key <key>]
lcgn auth login xai             # OAuth device login (SuperGrok / X Premium)
lcgn auth login openai          # Sign in with ChatGPT (browser + loopback redirect)
lcgn auth login github-copilot  # GitHub device login (asks for an Enterprise domain on a terminal)
lcgn auth login openrouter      # Sign in with OpenRouter (browser + loopback redirect)
lcgn auth login kimi-coding     # Kimi Code device login
lcgn auth logout opencode-go
lcgn auth check                 # exit 0 ready / 1 not ready
lcgn auth check --provider opencode-go --json
```

Status shows `signed in (oauth)`, `signed in (api key)`, `env $VAR` or `-`
(never key or token values). Without a terminal, `lcgn auth login` only prints
the table and exits 2.

`lcgn auth login xai` prints a verification URL and a code, opens the URL in
your browser (https only), and waits for approval (Ctrl+C cancels). The OAuth
credential is written to `~/.locagens/auth.json` in pi's format
(`type: "oauth"`, `access`, `refresh`, `expires`) and refreshed automatically
before it expires. Pass `--api-key` to store an API key instead.

Two OAuth flows exist, and the `sign-in` column of `lcgn auth` lists every
provider that has one:

- **Device code** (`xai`, `github-copilot`, `kimi-coding`): `lcgn` prints a URL
  and a code and waits for approval.
- **Authorization URL** (`openai`, `openrouter`): `lcgn` prints a URL, opens it,
  and waits for the browser to be redirected to a loopback server on
  `127.0.0.1` (`openai` needs port 1455 free; `openrouter` picks a free port).
  On a terminal you can instead paste the final redirect URL (or, for
  OpenRouter, the code) and press Enter, for a browser on another machine.
  Ctrl+C cancels.

Notes per provider: `openai` ("Sign in with ChatGPT") switches the `openai`
provider to the Responses API with the ChatGPT model list; an API key set with
`--api-key` keeps the normal behavior. `github-copilot` stores the Copilot token
plus the GitHub token used to renew it, offers only the models your account may
use, and picks the API host from the token. `openrouter` stores a permanent API
key (never refreshed). OpenCode's `~/.local/share/opencode/auth.json`
(`$XDG_DATA_HOME/opencode/auth.json`) is also read, API keys only, below every
other source and never written.
`lcgn auth check` reports OAuth credentials as valid/expired (never the
tokens). Only providers you have configured (credential in `auth.json`, the
provider's own env var, or an entry in `models.json` / Codex `config.toml`)
appear in `/model` and `--list-models`.

## Flags reference

```txt
lcgn [options] [@files...] [messages...]
  -p, --print                  Non-interactive: run, print, exit
  --model <provider/id>        e.g. opencode-go/glm-5.3-flash
  --provider <name>            Provider for a bare --model id
  --thinking <level>           off|minimal|low|medium|high|xhigh|max
  --mode <text|json>           Output mode (text default)
  --plan                       Plan mode (read-only tools)
  --approve, -a                Full access, no permission prompts
  -c, --continue               Continue the most recent session here
  -r, --resume                 Pick a session to resume
  --session <id>               Open a session by (partial) id
  --no-session                 Ephemeral: do not persist the run
  --no-tools                   Chat only
  --no-tui                     Plain line REPL instead of the full-screen UI (or LCGN_NO_TUI=1)
  --system-prompt <text>       Replace the built-in system prompt
  --append-system-prompt <t>   Append text to the system prompt
  --cwd <dir>                  Project directory (default: cwd)
  --list-models [search]       Table of authenticated models
  --profile <name>             Config profile from config.toml [profiles.<name>]
  config [show]                Print the effective config.toml (secrets masked)
  -v, --version, -h, --help
```

## Configuration and hooks

`config.toml` (Codex-compatible subset) is documented in
[CONFIG.md](CONFIG.md); lifecycle hooks (`hooks.json`) in [HOOKS.md](HOOKS.md).
Flags override config, config overrides the built-in defaults.

## Concurrency with the desktop app

`lcgn` and the desktop app share the same store (`~/.locagens`). Mutations of
shared files (`projects.json`, `permissions.json`, memory `.md` files, usage id
counter) are serialized with an advisory lock on `<root>/.lock` and re-read from
disk under the lock, so interleaved writes lose nothing. Session rollouts under
`sessions/` are private to the run that owns them and append-only.

## Auth & providers (Codex / Locagens compatible)

`lcgn auth login` stores credentials in `~/.locagens/auth.json`
(`$LOCAGENS_HOME`). Reads fall back to Codex's `~/.codex/auth.json`
(`OPENAI_API_KEY`, `config.toml` `[model_providers.*]`) and the legacy
`~/.pi/agent/auth.json` / `models.json`, plus OpenCode's `auth.json` (API
keys only, lowest precedence), so existing credentials on the machine keep
working out of the box.
