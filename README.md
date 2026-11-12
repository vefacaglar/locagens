# Locagens

Locagens is a local-first workspace assistant for local projects — 100% pure
Rust, no Node.js, no Electron, no web frontend, no HTTP server.

The user selects one provider/model, chooses a project folder, sends a task, and
the in-process orchestrator runs a tool-calling chat loop. The assistant can
read, edit, create, move, delete, and search files inside the selected
workspace. It can also ask for permission to run commands or fetch URLs.

The product is a **single-model workspace agent**: one provider/model drives the
whole run, and the agent can fan work out to sub-agents with `spawn_agents`
(`explorer` read-only research / `worker` full workspace tools). The UI is a
native Rust desktop app built with Dioxus.

```txt
User Task
  -> Selected Model
  -> Optional Workspace Tool Calls
  -> Optional spawn_agents Fan-Out (explorer / worker)
  -> Tool Results Back To Model
  -> Final Assistant Response
```

## Current Features

```txt
Native Rust desktop UI (Dioxus 0.7, apps/desktop-rs, binary `locagens`)
Rust core (locagens_core + lcgn CLI, apps/backend) built on top of pi
in-process typed API facade (locagens_core::api, see docs/CORE_API.md)
file-based JSON/JSONL persistence under <app_support_dir>/data/ (no SQLite)
pi provider engine (~/.pi/agent/auth.json & models.json, models-store.json)
OpenAI-compatible provider adapter
Anthropic provider adapter
provider config in pi's agent directory (not the project)
API keys live in auth.json (literal, !command, $ENV_VAR, or OAuth), never in app data
per-model context limits and tiered pricing -> local cost / usage logs
reasoning-effort conversion per model (reasoningWire)
single provider/model run creation with `spawn_agents` fan-out
build, plan, and full-access modes
workspace filesystem + web search tools
ask_user_question multiple-choice flow
two-tier persistent memory (global + project)
skill loading (SKILL.md packs via load_skill; Settings → Skills)
permission flow for commands and URL fetches
live event stream (in-process channels, no SSE / HTTP)
run/message/project/permission/memory/usage persistence
right-hand plan panel
lcgn CLI (pi-like command line on the same Rust core)
```

## Tech Stack

```txt
Desktop: native Rust app (Dioxus 0.7, apps/desktop-rs, binary `locagens`)
Core:    Rust (Tokio) built directly on top of pi (apps/backend,
         `locagens_core` library + `lcgn` CLI binary)
API:     in-process typed facade (locagens_core::api, docs/CORE_API.md)
Storage: file-based JSON/JSONL (projects.json, sessions/<run_id>/…, no SQLite)
Providers: pi (~/.pi/agent/auth.json & models.json, models-store.json)
Package manager: cargo (backend/desktop)
Repository style: monorepo
```

## Project Structure

```txt
locagens/
  apps/
    backend/               Rust core: locagens_core library + lcgn CLI
      src/
        api/               typed in-process facade (docs/CORE_API.md)
        cli/               lcgn command line (args, repl, runner, render)
        orchestrator/      run lifecycle + workspace tools
        services/          processes, plugins, skills, symbols, mcp
        db.rs              file-based JSON/JSONL store
        paths.rs           settings/data dir resolution
        pi_providers.rs    ~/.pi/agent auth.json + models.json engine
    desktop-rs/            native Dioxus 0.7 desktop app (binary `locagens`)
      src/
  docs/                    CORE_API.md, CLI.md, RELEASING.md, database-schema.md
  Agents.md  Claude.md  GEMINI.md  README.md
```

## Provider Configuration

The core is built directly on top of `pi` and reads/writes `pi`'s configuration
files under the `pi` agent directory (`~/.pi/agent`, overridable via
`PI_CODING_AGENT_DIR`), not in the project:

```txt
~/.pi/agent/auth.json           provider id -> credential
                                ({ "type": "api_key", "key": "..." },
                                 "!command", "$ENV_VAR", or OAuth)
~/.pi/agent/models.json         custom provider configs & model overrides
~/.pi/agent/models-store.json   pi's cached dynamic model catalog
```

Locagens UI metadata lives under the OS app-support directory:

```txt
locagens-providers-meta.json    display names / model settings
```

API keys never enter the app data or session files — credentials live only in
`auth.json` (or environment variables via the fallback chain such as
`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, …).

Example `auth.json`:

```json
{
  "openai": { "type": "api_key", "key": "sk-..." },
  "anthropic": { "type": "api_key", "key": "$ANTHROPIC_API_KEY" }
}
```

Public provider metadata omits credentials. Provider settings return editable
config with the API key masked by a preserve marker
(`__LOCAGENS_PRESERVE_API_KEY__`), so the UI can edit other fields without
receiving or re-posting the secret.

Locagens model settings may carry a `reasoning` effort, a `contextLimit`, and
`pricing` (tiered input/output/cache rates per 1M tokens). Cost is computed
locally from these rates and the provider's reported token counts, then stored
per call in `usage_logs.jsonl`. A model with no configured pricing costs 0.

## Modes

```txt
Build         applies file edits directly through workspace tools
Plan          writes a stable plan to the plan panel; no file mutations
Full Access   autonomous build; still gates dangerous tools
```

The core also keeps `chat`, `ask_permissions`, and `auto` modes for older
persisted runs, but the UI only exposes the three above. `run_command`,
`fetch_url`, and `search_web` always require permission unless a matching
standing grant exists.

When the main agent fans work out with `spawn_agents`, `explorer` sub-agents get
read-only tools (read, list, search) and `worker` sub-agents get the full
workspace toolset. Sub-agents run with the run's own model and depth is capped
at 1 (sub-agents cannot spawn further agents).

## Workspace Tools

The orchestrator exposes:

```txt
write_file
edit_file
delete_file
read_file
list_directory
create_directory
move_file
search_files
run_command
fetch_url
search_web
set_chat_title
ask_user_question
remember
update_plan            (plan mode)
spawn_agents           (explorer / worker fan-out)
```

All file paths are resolved relative to the selected project folder and cannot
escape it.

## Command line (`lcgn`)

Locagens ships a pi-like CLI that runs the same Rust core in-process:

```bash
lcgn -p "say OK" --model opencode-go/glm-5.3-flash --no-tools
lcgn --model opencode-go/glm-5.3-flash        # interactive REPL
lcgn --list-models                            # table of authenticated models
lcgn auth login opencode-go                   # store a key in ~/.pi/agent/auth.json
lcgn -c "keep going"                          # continue the most recent session
```

Sessions are normal Locagens runs, so they also appear in the desktop app.
See `docs/CLI.md` for the full flag reference.

## Development

Everything builds with cargo alone — no Node.js, pnpm, or Electron:

```bash
cargo test -p locagens-backend
cargo build --release -p locagens-backend
cargo run -p locagens-backend --bin lcgn -- -p "say OK"
cargo run -p locagens-desktop
```
