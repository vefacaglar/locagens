# Locagens

Locagens is a local-first workspace assistant for local projects.

The user selects one provider/model, chooses a project folder, sends a task, and
the backend orchestrator runs a tool-calling chat loop. The assistant can read,
edit, create, move, delete, and search files inside the selected workspace. It
can also ask for permission to run commands or fetch URLs.

The default product is a **single-model workspace agent**. Optionally, the user
can select an agent preset that splits the run into an architect model plus coder
sub-agents, with an optional utility model for small read/search/rename tasks.

```txt
User Task
  -> Selected Model or Preset Architect
  -> Optional Workspace Tool Calls
  -> Optional Coder / Utility Delegation
  -> Tool Results Back To Model
  -> Final Assistant Response
```

## Current Features

```txt
Vue 3 frontend
Fastify backend
SQLite persistence
OpenAI-compatible provider adapter
Anthropic provider adapter
committed config/providers.json catalog with writable user overlay
API keys kept in the OS keychain, never in JSON
per-model context limits and tiered pricing -> local cost / usage logs
reasoning-effort conversion per model (reasoningWire)
single provider/model run creation
optional architect/coder/utility agent presets
build, plan, and full-access modes
workspace filesystem + web search tools
ask_user_question multiple-choice flow
  two-tier persistent memory (global + project)
  skill loading (SKILL.md packs via load_skill; Settings → Skills)
  permission flow for commands and URL fetches
SSE live updates
run/message/project/permission/memory/usage persistence
right-hand plan panel
```

## Tech Stack

```txt
Frontend: Vue 3 + Vite + TypeScript
Backend: Node.js + TypeScript + Fastify
Database: SQLite via node:sqlite
Realtime: SSE
Package manager: pnpm
Repository style: monorepo
```

## Project Structure

```txt
locagens/
  apps/
    api/
      src/
        database/
        orchestrator/
        providers/
        routes/
    web/
      src/
        api/
        components/
        composables/
        lib/
  packages/
    shared/
      src/
  config/
    providers.json
  Agents.md
  Claude.md
  README.md
```

## Provider Configuration

Provider settings live in a committed, version-controlled catalog at
`config/providers.json` (providers, models, per-model settings, pricing, and
agent presets — never secrets), so every clone shares the same setup. API keys
are never written to JSON: on macOS the key is stored in the Keychain and the
file keeps only a non-secret `apiKeyRef` pointer.

```txt
config/providers.json   committed catalog (NO secrets)
OS keychain             holds the actual API key, referenced by apiKeyRef
```

`LOCAGENS_PROVIDER_CONFIG_PATH` overrides the catalog path (used by tests). When
`LOCAGENS_PROVIDER_USER_CONFIG_PATH` is set, the registry treats
`config/providers.json` as a read-only base and merges a writable user overlay on
top (the user's custom providers, edits, and removal tombstones); saves write
only the overlay, so app updates can refresh the base catalog without losing the
user's own providers. In the packaged desktop app the overlay lives at
`<userData>/providers.user.json`.

Example:

```json
{
  "providers": {
    "openai": {
      "type": "openai-compatible",
      "displayName": "OpenAI",
      "baseUrl": "https://api.openai.com/v1",
      "apiKeyRef": "macos-keychain:openai",
      "models": ["gpt-5.5"]
    },
    "anthropic": {
      "type": "anthropic",
      "displayName": "Anthropic",
      "baseUrl": "https://api.anthropic.com",
      "apiKeyRef": "macos-keychain:anthropic",
      "models": ["claude-sonnet-4.5"]
    }
  }
}
```

Public provider metadata omits API keys. The local settings API returns editable
provider config with the API key masked by a preserve marker, so the frontend can
edit other fields without receiving or re-posting the secret.

Each provider's `modelSettings[model]` block may carry a `reasoning` effort, a
`contextLimit`, and `pricing` (tiered input/output/cache rates per 1M tokens).
Cost is computed locally from these rates and the provider's reported token
counts, then stored per call in `usage_logs`. A model with no configured pricing
costs 0.

`config/providers.json` may also include `agentPresets`, which define an
architect provider/model, coder provider/model, `maxSubAgents`, and optionally a
utility provider/model.

## Modes

```txt
Build         applies file edits directly through workspace tools
Plan          writes a stable plan to the plan panel; no file mutations
Full Access   autonomous build; still gates dangerous tools
```

The backend also keeps `chat`, `ask_permissions`, and `auto` modes for older
persisted runs or direct API use, but the UI only exposes the three above.
`run_command`, `fetch_url`, and `search_web` always require permission unless a
matching standing grant exists.

When an agent preset is active in Build mode, the architect receives read-only
workspace tools and delegates mutations through `delegate_tasks`. Coder
sub-agents receive the normal workspace tools. Utility sub-agents receive read,
list, search, and move tools.

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
delegate_tasks         (preset build modes)
delegate_to_utility    (preset build modes)
```

All file paths are resolved relative to the selected project folder and cannot
escape it.

## Development

```bash
pnpm install
pnpm dev
```

Useful checks:

```bash
pnpm --filter @locagens/api test
pnpm --filter @locagens/api build
pnpm --filter @locagens/web build
```
