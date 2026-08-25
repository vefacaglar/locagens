# API Endpoint Map

_Generated from `apps/api/src/routes/` — grouped by route file. `/ping` is public;
every `/api/*` route requires the process-local bearer token._

---

## `index.ts` (base)

| Method | Path | Description | Repo / Orchestrator Call |
|--------|------|-------------|--------------------------|
| GET | `/ping` | Health check (status + timestamp) | inline return |

---

## `runs.ts` — `registerRunRoutes`

| Method | Path | Description | Repo / Orchestrator Call |
|--------|------|-------------|--------------------------|
| POST | `/api/runs` | Create and trigger a new orchestration run | `ctx.runRepo.create(run)`, `ctx.orchestrator.run(runId)` |
| POST | `/api/runs/:id/cancel` | Cancel a running orchestration job | `ctx.orchestrator.cancel(id)` |
| POST | `/api/runs/:id/continue` | Continue a run with follow-up instructions in the same thread | `ctx.runRepo.update(id, updates)`, `ctx.messageRepo.create(userMsg)`, `ctx.runRepo.update(id, {status: "generating"})`, `ctx.orchestrator.continueRun(id, task)` |
| POST | `/api/runs/:id/permission` | Resolve a pending permission request for a running job | `ctx.orchestrator.resolvePermission(id, decision)`, `ctx.permissionRepo.allowGlobal(tool, command)` / `ctx.permissionRepo.allowProject(path, tool, command)` |
| POST | `/api/runs/:id/answer` | Resolve a pending `ask_user_question` with user's selections | `ctx.orchestrator.resolveQuestion(id, {selections, notes})` |
| GET | `/api/runs` | Run history list | `ctx.runRepo.list()` |
| GET | `/api/runs/:id` | Single run details | `ctx.runRepo.getById(id)` |
| GET | `/api/runs/:id/messages` | Messages for a single run | `ctx.messageRepo.listByRunId(id)` |
| GET | `/api/runs/:id/usage` | Aggregated token/cost totals for a run, broken down by agent role | `ctx.usageLogRepo.getRunSummary(id)` |
| GET | `/api/runs/:id/plan` | Active plan for a single run (drives plan side panel) | `ctx.planRepo.getActive(id)` |
| GET | `/api/runs/:id/pending` | Pending user-facing request (permission / question) | `ctx.orchestrator.getPendingPermission(id)`, `ctx.orchestrator.getPendingQuestion(id)` |
| GET | `/api/runs/:id/events` | SSE event stream for a specific run | event bus: `eventBus.on(\`run:${id}\`, listener)` |
| GET | `/api/usage-logs` | Paginated/filterable usage log history | `ctx.usageLogRepo.listPaginated(...)` |

---

## `providers.ts` — `registerProviderRoutes`

| Method | Path | Description | Repo / Orchestrator Call |
|--------|------|-------------|--------------------------|
| GET | `/api/providers` | Safe provider metadata (never exposes API keys) | `ctx.registry.reload()`, `ctx.registry.getSafeMetadata()` |
| POST | `/api/providers/test` | Test a provider/model with a one-off completion | `ctx.registry.getProvider(providerId)`, `provider.complete({model, messages})` |
| GET | `/api/providers/config` | Get editable provider configurations (API keys masked) | `ctx.registry.reload()`, `ctx.registry.getEditableConfigs()` |
| POST | `/api/providers/config` | Save provider configurations | `ctx.registry.saveConfigs(configs)` |
| GET | `/api/agent-presets` | Dual-model agent presets (architect + coder) | `ctx.registry.reload()`, `ctx.registry.getAgentPresets()` |
| PUT | `/api/agent-presets` | Save the full agent-preset set | `ctx.registry.saveAgentPresets(presets)` |
| POST | `/api/providers/fetch-models` | Fetch available models dynamically from a provider endpoint | inline fetch to provider API (`/models` or `/model`) |

---

## `projects.ts` — `registerProjectRoutes`

| Method | Path | Description | Repo / Orchestrator Call |
|--------|------|-------------|--------------------------|
| GET | `/api/projects` | List all projects | `ctx.projectRepo.list()` |
| POST | `/api/projects` | Create/add a project manually | `ctx.projectRepo.create(project)` |
| DELETE | `/api/projects` | Remove a project from the list | `ctx.projectRepo.delete(projectPath)` |
| POST | `/api/projects/select-dir` | Trigger macOS native folder picker (returns canonical path + name) | `execFile` (osascript), no repo call |
| GET | `/api/projects/git/status` | Git branch and dirty-state for a registered canonical project | `execFile("git", args)` |
| GET | `/api/projects/git/diff-details` | File-level diff details without mutating the Git index | `execFile("git", args)`, guarded disk reads |
| POST | `/api/projects/git/generate-message` | Generate a commit message for a run's registered project | provider completion + `execFile("git", args)` |
| POST | `/api/projects/git/commit` | Commit and optionally push using literal argument arrays; repository hooks are disabled | `execFile("git", args)` |

---

## `permissions.ts` — `registerPermissionRoutes`

| Method | Path | Description | Repo / Orchestrator Call |
|--------|------|-------------|--------------------------|
| GET | `/api/permissions` | List all standing permissions | `ctx.permissionRepo.list()` |
| DELETE | `/api/permissions/:id` | Revoke a single permission by id | `ctx.permissionRepo.deleteById(numericId)` |
| DELETE | `/api/permissions` | Revoke all permissions | `ctx.permissionRepo.clear()` |

---

## `memory.ts` — `registerMemoryRoutes`

| Method | Path | Description | Repo / Orchestrator Call |
|--------|------|-------------|--------------------------|
| GET | `/api/memories` | List every saved memory (global + per-project) | `ctx.memoryRepo.list()` |
| POST | `/api/memories` | Manually add a memory from the UI | `ctx.memoryRepo.create({scope, projectPath, category, content})` |
| PUT | `/api/memories/:id` | Edit a memory's content | `ctx.memoryRepo.update(numericId, content)` |
| DELETE | `/api/memories/:id` | Delete a single memory | `ctx.memoryRepo.deleteById(numericId)` |
| DELETE | `/api/memories` | Clear all memories | `ctx.memoryRepo.clear()` |

---

## `skills.ts` — `registerSkillRoutes`

| Method | Path | Description | Repo / Orchestrator Call |
|--------|------|-------------|--------------------------|
| GET | `/api/skills` | List discovered skills + folder roots (optional `projectPath`) | `ctx.skillRegistry.listSummaries`, `userRoot`, `projectRoot` |
| POST | `/api/skills/open-folder` | Resolve/create allowlisted skills dir (`user` \| `project`) | `ctx.skillRegistry.ensureUserRoot` / `ensureProjectRoot` |

---

## `settings.ts` — `registerSettingsRoutes`

| Method | Path | Description | Repo / Orchestrator Call |
|--------|------|-------------|--------------------------|
| GET | `/api/settings` | Current local app settings (backend port) | `ctx.settingsStore.read()` |
| PUT | `/api/settings` | Update app settings (port change requires restart) | `ctx.settingsStore.save({port})` |

---

## `security.ts` — `registerSecurityRoutes`

| Method | Path | Description | Repo / Orchestrator Call |
|--------|------|-------------|--------------------------|
| GET | `/api/security/status` | Report local API and command-sandbox readiness | `commandSandbox.status()` |
| POST | `/api/security/sandbox/setup` | Install the Windows sandbox runtime after explicit UI confirmation | `commandSandbox.installWindows()` |
