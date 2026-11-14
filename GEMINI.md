# GEMINI.md

## Project

Locagens is a local-first AI orchestration tool. For architecture, project
map, and rules see `Claude.md`; for runtime behavior see `Agents.md`.

## Development & Agent Workflow (Gemini)

How development work is executed in this repo:

- **Codebase exploration (read-only research & analysis)**: always use
  `gemini-4-hydrogen` (`Gemini 4 Hydrogen`), default effort `medium` (`low` for
  trivial lookups, `high`/`xhigh` for deep analysis), run as a read-only
  subagent or swarm worker pinned to a `gemini-4-hydrogen-*` model.
- **All coding, development & file modifications (executioner)**: always use
  `gemini-4-hydrogen` (`Gemini 4 Hydrogen`), executed via the Orchestrator
  plan/execute handoff or `gemini-4-hydrogen-*` workers (default effort
  `medium`, `low` for trivial changes, `high`/`xhigh` for complex work). Do not
  use `pi` / `opencode-go/glm-5.3-flash` for coding.
