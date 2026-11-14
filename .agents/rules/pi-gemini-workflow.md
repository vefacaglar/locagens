# Rule: Development & Agent Workflow (Gemini)

How development work is executed in the Locagens repo.

## Model split

- **Codebase exploration (read-only research & analysis)**: always use
  `gemini-4-hydrogen` (`Gemini 4 Hydrogen`), default effort `medium` (`low` for
  trivial lookups, `high`/`xhigh` for deep analysis), run as a read-only
  subagent or swarm worker pinned to a `gemini-4-hydrogen-*` model.
- **All coding, development & file modifications (executioner)**: always use
  `gemini-4-hydrogen` (`Gemini 4 Hydrogen`), executed via the Orchestrator
  plan/execute handoff or `gemini-4-hydrogen-*` workers (default effort
  `medium`, `low` for trivial changes, `high`/`xhigh` for complex work). Do not
  use `pi` / `opencode-go/glm-5.3-flash` for coding.

## Locagens runtime architecture direction

Locagens is migrating its internal custom provider/API-key storage and agent
runtime to be built directly on top of `pi` (`pi auth` /
`~/.pi/agent/auth.json`, `pi --list-models`, and `pi` execution modes). New
provider/auth work should target the `pi`-backed model.
