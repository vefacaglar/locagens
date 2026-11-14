# `lcgn` — the Locagens CLI

`lcgn` is the Locagens command line. It runs the **same Rust core as the
desktop app** (orchestrator, `PiProviderManager`, tools, permission flow,
file-based sessions) **in-process** — no HTTP server and no helper process. Sessions it creates are normal Locagens runs, so they show up in the
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

- Prompts are built the same way in `-p`, the REPL and the TUI
  (`cli/prompt.rs`): `@path` mentions of **existing files inside the project**
  are attached (max 256 KiB each); everything else, including `@alice`, a
  missing file or `@../outside`, stays plain text, and newlines are kept
  verbatim. Piped (non-TTY) stdin is the prompt when no message is given;
  with a message it is appended only when `--stdin` is passed (so an
  inherited open pipe cannot hang `lcgn -p "..."`).
- Text mode streams the main agent's answer to **stdout** and everything else
  (reasoning, tool lines like `✔ read_file src/main.rs`, sub-agent text tagged
  `[name]`, usage/cost) to **stderr** — so `lcgn -p "..." > out.txt` captures
  only the answer. Consecutive assistant messages are separated by a newline.
- `--mode json` prints one JSON event per line on stdout (the run-event
  objects of `locagens_core::api`, see `docs/CORE_API.md`).
- Exit codes: `0` success, `1` run failed/cancelled, `2` usage or
  configuration error (unknown model, provider not authenticated, …).
- `-p` runs in Build mode (`accept_edits`) unless config.toml or a flag says otherwise. Without `--approve`, every call
  that needs a permission prompt is **denied with a message** unless a
  standing grant exists in `permissions.json`. The gated tools are
  `run_command`, `exec_command`, `write_stdin` and `fetch_url`; known-safe
  commands (read-only ones such as `git status`) and commands a `.rules` file
  allows run without a prompt. Dangerous commands (`rm -rf`, `git reset --hard`,
  ...) and writes to protected paths (`.git`, `.locagens`, `.codex`,
  `.agents`) are always denied here, even with a standing grant, because they
  need a live per-call answer. Use `-a/--approve` (Full Access) to skip
  prompts, or drop `-p` to approve interactively. See the README for the full
  gating rules.

## Project trust

Project configuration under `<project>/.locagens/` (`hooks.json`, `mcp.json`,
`rules/*.rules`, `agents/*.toml`, `plugins/`) runs code or changes policy, so
it is only loaded for trusted folders. Trust is stored Codex-style in
`~/.locagens/config.toml`:

```toml
[projects."C:/repos/app"]
trust_level = "trusted"   # or "untrusted"
```

The nearest ancestor entry wins. When an untrusted folder ships such files,
the interactive UI asks once (`Trust <path> and load it? [y/N]`); `-p` runs print a
one-line notice on stderr and continue without them. `--trust` trusts the
folder for this invocation only.

```bash
lcgn trust [PATH]            # trust PATH (default: --cwd / cwd) and persist it
lcgn trust [PATH] --revoke   # mark it untrusted
lcgn trust [PATH] --status   # print trusted/untrusted and the ignored files
```

## Interactive terminal UI

```powershell
lcgn --model opencode-go/glm-5.3-flash
```

With a terminal on stdin and stdout, `lcgn` opens a Codex-style full-screen
UI (alternate screen): header with project path and run title, a scrollable
transcript (markdown, syntax-highlighted code, tool cells, diffs), a status
line while a run is active, a multi-line composer and a footer with
model, thinking level and mode on the left and context left and cost on
the right. The composer border names the mode: Build (cyan, ` Message `),
Plan (blue, ` Plan · read-only `) and Full Access (red, ` Full Access · no
permission prompts `). Setting changes (`/plan`, `/thinking`, `/model`) flash
in the status line instead of adding a transcript line. Messages queued while a
run is active wait in a strip above the composer. Use `--no-tui` (or `LCGN_NO_TUI=1`) for the plain line REPL below.
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
Ctrl+R                     search earlier prompts of this folder (Enter uses one)
?                          help overlay (empty composer)
Ctrl+O                     show details: reasoning, full tool output
                           (up to 200 lines), full diffs
Shift+Tab                  cycle Build -> Plan (-> Full Access once `--approve`
                           or `/auto` was used) -> Build
Up (run active, empty      pull the newest queued message back to edit it
  composer)
Esc                        close popup, otherwise interrupt the active run
Ctrl+C                     clear the composer; else interrupt the run; else
                           press twice within 2s to quit
Ctrl+D                     quit (empty composer)
```

Up / Down also recall prompts from earlier sessions of the folder (the last 100
from `history.jsonl`). An interrupted or failed run puts its undelivered queued
messages back into the composer.

Permission requests open a modal that says why it appeared. `y`/`o` allow
once, `r` this run, `d`/`n`/Esc deny, Up/Down scroll the diff. `p` (project)
and `a` (always) are standing grants: press the key twice (the footer reads
`p×2 project` and `a×2 always`; after the first press it says "press a again
to always allow"), and grant keys are ignored for the first 600 ms. The line
under the options shows what a standing grant covers. Dangerous commands and
protected paths show only `y` and `d`. `apply_patch` requests list the files
and the diff; network requests name the host. Questions
from the agent open a list: Up/Down, Space (multi-select), Enter to confirm,
Tab for a free-text note, Esc to skip. `@path` mentions of existing project
files are attached to the prompt like in the REPL.

Slash commands (TUI and REPL):

```txt
/model [provider/id]   show or switch models
/thinking <level>      off|minimal|low|medium|high|xhigh|max
/plan                  plan mode (read-only tools)
/build                 build mode (edits applied directly)
/auto                  full access (no permission prompts)
/chat                  chat mode (no tools)
/new                   start a fresh session on the next message
/fork                  fork the current session
/undo                  undo the last turn (git snapshot restore)
/diff                  working-tree diff
/mcp, /skills          list MCP servers / skills
/review [target]       code review (see "Code review" below); `/review ` opens a
                       popup of targets: uncommitted changes, branches, commits
/resume                pick a session of this folder to continue (alias /sessions;
                       `lcgn -r` opens this picker at start)
/login [provider]      connect a provider; /logout <provider> removes it
/copy [code]           copy the last answer (or its last code block)
/usage                 usage/cost of the current session
/help                  commands and keys overlay
/exit                  (alias /quit)
```

`/clear` is an alias of `/new`. `/help` (or `?`) opens an overlay with the
commands and key bindings.

`/login` opens a provider picker (`Enter` connect, `Tab` use an API key instead).
API-key providers show one masked input; OAuth providers show the code and
URL and wait (`Enter` opens the browser, `c` copies the code, `Esc` cancels).
Credentials are stored like `lcgn auth login` does. Anthropic is API key only.
With no provider connected the welcome text says so and `Enter` opens `/login`.

`/copy` uses OSC 52 plus the platform clipboard command. Mouse capture is on,
so hold Shift while dragging to select text with the terminal.

`[tui]` in `config.toml`: `theme = "dark" | "light"` (default: from
`COLORFGBG`), `animations = false` (also `LCGN_REDUCED_MOTION=1`) for a static
status line. `NO_COLOR` removes all colours. For a screen reader use `--no-tui`.

`/new`, `/fork`, `/undo` and `/review` are refused while a run is generating
(Esc interrupts it first). The commands are parsed and executed by one shared
module, `cli/commands.rs`, used by both the TUI and the REPL.

If the event stream lags (a slow terminal during a burst of output), `lcgn`
re-syncs from the store instead of dropping the gap: persisted messages are
re-applied and a permission/question prompt or terminal event that was lost is
replayed (`cli/events.rs`).

## Code review

`lcgn review` reviews code changes on the selected model in a read-only run
(chat mode: the model gets `read_file`, `list_directory`, `search_files`,
`view_image`, `load_skill`, `remember`, `set_chat_title`, `compact_context`,
the question tools, the MCP resource tools, `get_context_remaining` and
`host_executable`, but no edits, commands or network; the diff is gathered
with read-only `git` and handed to the model). The result is
structured: findings with title, priority (`P0`..`P3`), confidence, file and
line range, plus an overall verdict.

```powershell
lcgn review --model opencode-go/glm-5.3-flash                 # uncommitted changes (default)
lcgn review --uncommitted                                     # staged + unstaged + untracked
lcgn review --base main                                       # changes since the merge base with main
lcgn review --commit 1b27d87                                  # one commit
lcgn review "focus on error handling in the parser"           # custom instructions
lcgn review --base main --mode json                           # events as JSON lines + a final review_completed line
```

Findings print to stdout (progress on stderr); the exit code is 0 when the
review ran, whatever it found. A model is required: without `--model` (or a
default in `config.toml`) it exits 2, as it does for an unauthenticated
provider or an invalid target (not a git repo, no changes, unknown branch or
commit). A failed or cancelled review exits 1 and prints no findings; in
`--mode json` the final `review_completed` line is emitted only when the run
succeeded. `--model`, `--provider`, `--thinking`, `--cwd` and `--mode` work
before or after `review`. The review is stored as a normal
session titled `Review: ...`; the last assistant message carries the
structured result in `rawResponse` (`{"kind":"review_output", ...}`), so it
reopens after a restart. In the REPL/TUI, `/review` starts a review the same
way (`/review base <branch>`, `/review commit <sha>`, `/review <text>`); in the
TUI the review becomes the current session.

## Plain REPL (`--no-tui`)

A line-editor REPL with streaming output and the same slash commands (shared
with the TUI) and the same `@file` handling.
`Ctrl+C` cancels the running run (or exits at an idle prompt).

## Sessions

```powershell
lcgn -c "keep going"                     # continue the most recent session
lcgn --session run-882c "and now?"       # open a session by (partial) id
lcgn -r                                  # pick a session interactively
lcgn sessions                            # list sessions for this directory
lcgn sessions --json
lcgn --no-session -p "quick question" …  # ephemeral: no rollout kept, and the
                                         # prompt is not written to history.jsonl
                                         # or session_index.jsonl
```

## Models

```powershell
lcgn --list-models            # table: provider, model, context, max-out, thinking, images
lcgn --list-models glm
lcgn --model glm-5.3-flash    # bare id works when unambiguous
lcgn --provider opencode-go --model glm-5.3-flash
```

## Auth

Credentials are written to `~/.locagens/auth.json` (`$LOCAGENS_HOME`), shared
with the desktop app; `~/.codex` and `~/.pi/agent` are read-only fallbacks.
For an API-key provider the key is prompted with no echo when `--api-key` is
omitted; OAuth providers (below) start a browser or device flow instead.
Anthropic is API key only.

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
credential is written to `~/.locagens/auth.json`
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
provider's own env var, or an entry in `models.json` / `config.toml`
`[model_providers]`) and that have a base URL appear in `/model` and
`--list-models`. A vendor env var (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, ...)
is only used for that vendor's own built-in provider; any other provider uses
an env key only when it names it (`"$VAR"` in `auth.json`, or `envKey` /
`env_key`).

Model lists are fetched live: the TUI (and the desktop app) re-fetch each
signed-in provider's `GET /models` in the background when the cached list is
missing or older than 6 hours (`~/.locagens/models-live.json`). Once fetched,
that list decides which models a provider offers (models you declared in
`models.json` stay), so retired models drop out. Models switched off in the
desktop's Settings → Providers are hidden from `/model` too.

New chats are named by the model: on the first turn of a TUI or desktop chat a
small extra call (no tools, logged under the `title` usage role) writes a
2–6 word title, unless one was set meanwhile. Headless `lcgn -p` runs keep the
first line of the prompt as the title.

## MCP

```bash
lcgn mcp list              # configured servers, status, tool counts
lcgn mcp login <server>    # OAuth sign-in for a remote (http) MCP server
lcgn mcp logout <server>   # forget the stored token
```

`login` discovers the authorization server
(`/.well-known/oauth-protected-resource` then authorization-server metadata),
registers a client dynamically, opens the browser for the PKCE authorization
code flow and receives the redirect on `127.0.0.1`. Tokens are stored in
`~/.locagens/mcp-oauth.json` (`$LOCAGENS_HOME`) and refreshed automatically.
For static auth use `bearer_token_env_var`, `http_headers` and
`env_http_headers` in the server entry (see `docs/CORE_API.md`, "MCP server
configuration").

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
  --trust                      Trust the project folder for this invocation
  --list-models [search]       Table of authenticated models
  --profile <name>             Config profile from config.toml [profiles.<name>]
  config [show]                Print the effective config.toml (secrets masked)
  trust [PATH] [--revoke|--status]  Persist or inspect project trust
  mcp list|login|logout        MCP servers and their OAuth sign-in
  review [--uncommitted|--base B|--commit SHA] [text]  Read-only code review
  sessions [--json]            List sessions for this directory
  auth [login|logout|check]    Provider credentials
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
disk under the lock, so interleaved writes lose nothing. The usage id counter
holds the lock only while an id is allocated. Session rollouts under
`sessions/` are private to the run that owns them and append-only.

Changes made by the other process show up without a restart: grants in
`permissions.json`, `projects.json` and memories are re-read when their
modification time changes, and the run list picks up runs created elsewhere.
A run that a live process is driving holds an owner lock under `run_locks/`;
starting another `lcgn` or the desktop app never marks such a run failed (only
runs whose owner process is gone are failed at startup). If `<root>/.lock`
cannot be opened or taken within 10 s the store logs a warning and continues
without it instead of crashing.

`config.toml` edits are picked up at the start of the next run of a long-lived
process (the desktop app); `lcgn config` prints the chain without opening the
store. A broken `~/.codex` / `~/.pi/agent` `config.toml` is skipped with a
warning rather than failing `lcgn`; a broken `~/.locagens/config.toml` still
fails and names the file. `--provider X` without `--model` no longer borrows the
`model` of another provider from `config.toml`.

## Credential fallbacks

Reads fall back from `~/.locagens` to Codex's `~/.codex/auth.json`
(`OPENAI_API_KEY`, `config.toml` `[model_providers.*]`) and
`~/.pi/agent/auth.json` / `models.json`, plus OpenCode's `auth.json` (API
keys only, lowest precedence), so existing credentials on the machine keep
working. Only `~/.locagens` is ever written.
