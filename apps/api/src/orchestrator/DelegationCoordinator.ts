import type { Run, RunMessage, ReasoningEffort } from "@locagens/shared";
import type { ProviderRegistry } from "../providers/ProviderRegistry.js";
import type { IMemoryRepository } from "../database/repositories.js";
import { buildCoderSystemPrompt, buildUtilitySystemPrompt, buildVerifierSystemPrompt, buildExplorerSystemPrompt, formatCoderMemoryContext, getModeStrategy } from "./systemPrompt.js";
import { WORKSPACE_TOOLS, UTILITY_TOOLS, READONLY_TOOLS, MAX_SPAWNED_AGENTS, type SubAgentType } from "./workspaceTools.js";
import type { AgentLoop, Delegator } from "./AgentLoop.js";

/**
 * Common framework/library tokens that look like file paths (extension match)
 * but are prose mentions, not files. Excluded when scanning coder summaries for
 * files to verify. Compared lowercased against the matched token's basename.
 */
const NON_FILE_TOKENS = new Set([
  "node.js",
  "vue.js",
  "react.js",
  "next.js",
  "nuxt.js",
  "express.js",
  "three.js",
  "d3.js"
]);

/**
 * Hard cap on how much of a sub-agent's report is fed back into the (expensive)
 * architect context. The coder/utility prompt already asks for a compact report,
 * but a non-compliant model can still dump a wall of text; this guarantees the
 * architect's input stays lean regardless. File-path scanning runs on the FULL
 * summary before trimming, so verification targets are never lost to truncation.
 */
const MAX_SUMMARY_CHARS = 1200;

function trimSummary(summary: string): string {
  if (summary.length <= MAX_SUMMARY_CHARS) return summary;
  const dropped = summary.length - MAX_SUMMARY_CHARS;
  return `${summary.slice(0, MAX_SUMMARY_CHARS)}\n…[truncated ${dropped} chars — the sub-agent's report was long. Verify the changed files if you need detail instead of asking it to repeat.]`;
}

/**
 * Parses the coder's structured `FILES_CHANGED: ["a.ts","b.ts"]` line (the last
 * one, if the model emits several). Returns null when absent or malformed so the
 * caller can fall back to the heuristic prose scan.
 */
export function parseFilesChanged(summary: string): string[] | null {
  const matches = [...summary.matchAll(/^\s*FILES_CHANGED:\s*(\[.*\])\s*$/gm)];
  if (matches.length === 0) return null;
  try {
    const parsed = JSON.parse(matches[matches.length - 1][1]);
    if (!Array.isArray(parsed)) return null;
    return parsed.filter((f): f is string => typeof f === "string" && f.trim().length > 0).map(f => f.trim());
  } catch {
    return null;
  }
}

/** A delegated sub-task after validation/normalization. */
interface DelegatedTask {
  title: string;
  instructions: string;
  verify: boolean;
}

/** A sub-agent's report back to the architect. */
interface TaskResult {
  title: string;
  summary: string;
}

/** Validates and normalizes a delegate_* call's task list, capped at `limit`. */
function normalizeTasks(args: any, limit: number, fallbackTitle: string): DelegatedTask[] {
  const rawTasks = Array.isArray(args.tasks) ? args.tasks : [];
  return rawTasks
    .filter((t: any) => t && typeof t.instructions === "string" && t.instructions.trim())
    .slice(0, limit)
    .map((t: any, i: number) => ({
      title: typeof t.title === "string" && t.title.trim() ? t.title.trim() : `${fallbackTitle} ${i + 1}`,
      instructions: String(t.instructions),
      verify: t.verify === true
    }));
}

/** File paths the architect explicitly attached to tasks (their `files` arrays). */
function explicitTaskFiles(args: any): string[] {
  const files: string[] = [];
  if (!Array.isArray(args.tasks)) return files;
  for (const t of args.tasks) {
    if (!t || !Array.isArray(t.files)) continue;
    for (const f of t.files) {
      if (typeof f === "string" && f.trim()) files.push(f.trim());
    }
  }
  return files;
}

const FILE_TOKEN_PATTERN = /[\w\-./]+\.\w{1,5}/g;
const SOURCE_FILE_EXTENSIONS =
  /\.(ts|js|vue|go|json|tsx|jsx|css|scss|sass|less|md|py|rb|rs|java|kt|swift|yaml|yml|toml|xml|sh|bash|zsh|fish|env|gitignore|editorconfig|prettierrc|eslintrc|npmrc|nvmrc)$/i;
const ERROR_KEYWORDS = /\b(error|failed|could not|unable|exception|bug|broken|crash|regression)\b/i;

/**
 * Collects each result's changed files. The structured FILES_CHANGED line is
 * authoritative when present; the prose scan is only a fallback for models that
 * ignore the output format. Runs on the FULL summary (before trimming).
 */
function reportedChangedFiles(results: TaskResult[], tasks: DelegatedTask[]): string[] {
  const files: string[] = [];
  for (let i = 0; i < results.length; i++) {
    const r = results[i];
    // Verify-task reports describe files that were checked, not changed.
    if (!r.summary || tasks[i]?.verify) continue;

    const structured = parseFilesChanged(r.summary);
    if (structured) {
      files.push(...structured);
      continue;
    }

    const matches = r.summary.match(FILE_TOKEN_PATTERN);
    if (!matches) continue;
    for (const m of matches) {
      const basename = (m.split("/").pop() ?? m).toLowerCase();
      if (SOURCE_FILE_EXTENSIONS.test(m) && !NON_FILE_TOKENS.has(basename)) {
        files.push(m);
      }
    }
  }
  return files;
}

/** Flags summaries whose wording suggests the sub-agent hit problems. */
function summaryWarnings(results: TaskResult[]): string[] {
  return results
    .filter(r => r.summary && ERROR_KEYWORDS.test(r.summary))
    .map(r => `Task "${r.title}" may have issues — review the summary carefully.`);
}

/** Runs one sub-agent per task — concurrently only when the architect asked for it. */
async function runTasks<T extends DelegatedTask>(
  tasks: T[],
  parallel: boolean,
  runOne: (task: T) => Promise<TaskResult>
): Promise<TaskResult[]> {
  if (parallel) return Promise.all(tasks.map(runOne));
  const results: TaskResult[] = [];
  for (const task of tasks) {
    results.push(await runOne(task));
  }
  return results;
}

/** One execution attempt for a delegated task: a model paired with its toolset. */
interface SubAgentTier {
  providerId: string;
  providerDisplayName: string;
  model: string;
  reasoningEffort?: ReasoningEffort;
  tools: any[];
  agentRole: RunMessage["agentRole"];
  systemPrompt: string;
  agentName: string;
}

/**
 * Runs the architect's delegate_tasks / delegate_to_utility calls: spins up
 * coder / utility sub-agents (each its own AgentLoop pass in the same workspace),
 * optionally with a resilience fallback chain (utility -> coder -> architect),
 * and returns their result summaries to the architect. Owns no run state — it
 * borrows the shared AgentLoop to execute each sub-agent.
 */
export class DelegationCoordinator implements Delegator {
  constructor(
    private registry: ProviderRegistry,
    private agentLoop: AgentLoop,
    private memoryRepo: IMemoryRepository
  ) {}

  /** Condensed project memories injected into every coder prompt for this run. */
  private coderProjectContext(run: Run): string {
    try {
      return formatCoderMemoryContext(this.memoryRepo.listForContext(run.projectPath));
    } catch {
      return ""; // memory is a quality booster, never a delegation blocker
    }
  }

  /** Resolves how many coder sub-agents this run may launch (preset-bound, 1..3). */
  maxSubAgentsFor(run: Run): number {
    const preset = run.agentPreset ? this.registry.getAgentPreset(run.agentPreset) : undefined;
    return Math.min(3, Math.max(1, preset?.maxSubAgents ?? 3));
  }

  /**
   * Whether this run's preset opts into the sub-agent fallback chain. When on, a
   * delegated task that errors out escalates (utility -> coder -> architect), and
   * the architect can take over directly as a last resort. Resolved live from the
   * preset; absent/deleted preset => off.
   */
  private fallbackEnabled(run: Run): boolean {
    const preset = run.agentPreset ? this.registry.getAgentPreset(run.agentPreset) : undefined;
    return !!preset?.fallback;
  }

  /**
   * Runs a delegated task through an ordered list of execution tiers, escalating
   * to the next tier only when one throws a provider/network error (cancellation
   * is never swallowed). This is the resilience chain: utility -> coder ->
   * architect. If a cheap model is unavailable the work retries on a stronger
   * one, and as a last resort the architect executes it directly with the full
   * workspace toolset. Normal tool failures are NOT thrown — they come back
   * inside the sub-agent's summary — so they never trigger escalation here.
   */
  private async runTaskWithFallback(
    runId: string,
    run: Run,
    task: { title: string; instructions: string },
    tiers: SubAgentTier[]
  ): Promise<{ title: string; summary: string }> {
    let lastError: any;
    for (let t = 0; t < tiers.length; t++) {
      const tier = tiers[t];
      try {
        const summary = await this.agentLoop.run(runId, run, [{ role: "user", content: task.instructions }], {
          providerId: tier.providerId,
          providerDisplayName: tier.providerDisplayName,
          model: tier.model,
          reasoningEffort: tier.reasoningEffort,
          systemPrompt: tier.systemPrompt,
          tools: tier.tools,
          agentRole: tier.agentRole,
          agentName: tier.agentName
        });
        return { title: task.title, summary: summary || "(no summary returned)" };
      } catch (err: any) {
        // Cancellation must unwind the whole run — never swallow it as a tier failure.
        if (err?.message === "ORCHESTRATION_CANCELLED") throw err;
        lastError = err;
        const next = t + 1 < tiers.length ? `escalating to "${tiers[t + 1].agentName}"` : "no tiers left";
        console.warn(`[Orchestrator] Run ${runId} - tier "${tier.agentName}" failed (${err?.message}); ${next}.`);
      }
    }
    return {
      title: task.title,
      summary: `(could not complete — all execution tiers failed: ${lastError?.message ?? "unknown error"})`
    };
  }

  /** Resolves a provider's display name from safe metadata, with fallbacks. */
  private providerDisplayName(providerId: string | undefined, fallback: string): string {
    const meta = this.registry.getSafeMetadata().find(p => p.id === providerId);
    return meta?.displayName ?? providerId ?? fallback;
  }

  /**
   * One coder sub-task: the coder tier, plus the architect as last resort when
   * the preset enables fallback. verify tasks run the coder model READ-ONLY with
   * a verdict-shaped prompt — cheap verification of earlier changes without the
   * architect reading the files into its own expensive context — and never
   * escalate to the architect (a failed verification must not turn into the
   * architect doing expensive reads).
   */
  private runCoderTask(runId: string, run: Run, task: DelegatedTask): Promise<TaskResult> {
    const projectContext = this.coderProjectContext(run);
    const readonlyWorkspaceTools = WORKSPACE_TOOLS.filter(t => READONLY_TOOLS.has(t.function.name));
    const tiers: SubAgentTier[] = [
      {
        providerId: run.coderProviderId!,
        providerDisplayName: this.providerDisplayName(run.coderProviderId, "Coder"),
        model: run.coderModel!,
        reasoningEffort: run.coderReasoningEffort,
        tools: task.verify ? [...readonlyWorkspaceTools] : [...WORKSPACE_TOOLS],
        agentRole: "coder",
        systemPrompt: task.verify
          ? buildVerifierSystemPrompt(run.projectName, run.projectPath, task.title)
          : buildCoderSystemPrompt(run.projectName, run.projectPath, task.title, projectContext),
        // Tag every message with the sub-task title so the UI can render each
        // coder sub-agent in its own window instead of merging them.
        agentName: task.title
      }
    ];
    if (this.fallbackEnabled(run) && !task.verify) {
      tiers.push(this.architectTier(run, task, projectContext));
    }
    return this.runTaskWithFallback(runId, run, task, tiers);
  }

  /**
   * One utility sub-task: the cheap utility tier, escalating (when the preset
   * enables fallback) to the coder with the full toolset, then to the architect
   * doing it directly.
   */
  private runUtilityTask(runId: string, run: Run, task: DelegatedTask): Promise<TaskResult> {
    const projectContext = this.coderProjectContext(run);
    const tiers: SubAgentTier[] = [
      {
        providerId: run.utilityProviderId!,
        providerDisplayName: this.providerDisplayName(run.utilityProviderId, "Utility"),
        model: run.utilityModel!,
        reasoningEffort: run.utilityReasoningEffort,
        tools: [...UTILITY_TOOLS],
        agentRole: "utility",
        systemPrompt: buildUtilitySystemPrompt(run.projectName, run.projectPath, task.title),
        agentName: task.title
      }
    ];
    if (this.fallbackEnabled(run)) {
      if (run.coderModel && run.coderProviderId) {
        tiers.push({
          providerId: run.coderProviderId,
          providerDisplayName: this.providerDisplayName(run.coderProviderId, "Coder"),
          model: run.coderModel,
          reasoningEffort: run.coderReasoningEffort,
          tools: [...WORKSPACE_TOOLS],
          agentRole: "coder",
          systemPrompt: buildCoderSystemPrompt(run.projectName, run.projectPath, task.title, projectContext),
          agentName: `${task.title} (coder fallback)`
        });
      }
      tiers.push(this.architectTier(run, task, projectContext));
    }
    return this.runTaskWithFallback(runId, run, task, tiers);
  }

  /** Builds the architect "last resort" tier: the run's own model + full toolset. */
  private architectTier(run: Run, task: { title: string }, projectContext?: string): SubAgentTier {
    return {
      providerId: run.providerId,
      providerDisplayName: run.providerDisplayName,
      model: run.model,
      reasoningEffort: run.reasoningEffort,
      tools: [...WORKSPACE_TOOLS],
      agentRole: "coder",
      systemPrompt: buildCoderSystemPrompt(run.projectName, run.projectPath, task.title, projectContext),
      agentName: `${task.title} (architect fallback)`
    };
  }

  /**
   * Executes a delegate_tasks call from the architect: spins up 1..maxSubAgents
   * coder sub-agents (each its own AgentLoop pass on the coder model, in the same
   * workspace) and returns their result summaries to the architect. Runs them in
   * parallel only when the architect explicitly requested it; otherwise sequential.
   * If the coder model is unavailable, each task falls back to the architect.
   */
  async executeDelegateTasks(runId: string, run: Run, toolCall: any): Promise<string> {
    if (!run.coderModel || !run.coderProviderId) {
      return JSON.stringify({ success: false, error: "No coder model is configured for this run, so tasks cannot be delegated." });
    }
    // Safety net: delegation is implementation, which is forbidden in plan/chat
    // mode. The tool is normally not even advertised in those modes, but guard
    // here too in case the model calls it from replayed history.
    if (!getModeStrategy(run.mode).allowsDelegation) {
      return JSON.stringify({ success: false, error: `Cannot delegate tasks in ${run.mode} mode. Switch to Build mode to implement.` });
    }

    let args: any;
    try {
      args = JSON.parse(toolCall.function.arguments || "{}");
    } catch (e: any) {
      return JSON.stringify({ success: false, error: `Could not parse delegate_tasks arguments (${e.message}). This usually means the arguments were too large and got cut off. Do NOT paste file contents or large code into 'instructions' — the coder reads files itself with read_file. Keep each task's instructions short: describe what to change and cite file paths, then retry.` });
    }

    const tasks = normalizeTasks(args, this.maxSubAgentsFor(run), "Subtask");
    if (tasks.length === 0) {
      return JSON.stringify({ success: false, error: "No valid tasks to delegate (each task needs non-empty instructions)." });
    }

    const parallel = !!args.parallel && tasks.length > 1;
    const results = await runTasks(tasks, parallel, task => this.runCoderTask(runId, run, task));

    const uniqueFilesToVerify = [...new Set([
      ...explicitTaskFiles(args),
      ...reportedChangedFiles(results, tasks)
    ])];
    const warnings = summaryWarnings(results);

    // A call consisting purely of verify tasks IS the verification — don't ask
    // the architect to verify the verification.
    const allVerify = tasks.every((t: { verify: boolean }) => t.verify);
    if (allVerify) {
      return JSON.stringify({
        success: true,
        parallel,
        results: results.map(r => ({ title: r.title, summary: trimSummary(r.summary) })),
        _verification_required: false,
        _is_verification: true,
        _reminder: "Review the verdicts above. If issues were found, delegate a fix; otherwise the work is verified.",
        _warnings: warnings.length > 0 ? warnings : undefined
      });
    }

    // Verification is always offloaded to a cheaper model instead of the
    // architect reading each file into its expensive context: the utility tier
    // when configured, otherwise the coder running read-only (verify: true).
    const verifyViaUtility = !!(run.utilityModel && run.utilityProviderId);
    const reminder = verifyViaUtility
      ? "You MUST now verify these changes via delegate_to_utility: ask the utility model to read each file in _files_to_verify and confirm correctness, returning a SHORT verdict. Do NOT read the files yourself. If something is wrong, delegate a fix."
      : "You MUST now verify these changes by calling delegate_tasks with ONE task with verify: true, instructing it to read each file in _files_to_verify and return a SHORT verdict. Do NOT read the files yourself — that bloats your expensive context. If something is wrong, delegate a fix.";

    return JSON.stringify({
      success: true,
      parallel,
      results: results.map(r => ({ title: r.title, summary: trimSummary(r.summary) })),
      _verification_required: true,
      _files_to_verify: uniqueFilesToVerify,
      _reminder: reminder,
      _parallel_advice: !parallel && tasks.length > 1
        ? `You ran ${tasks.length} tasks sequentially. If they touch disjoint files, use parallel: true next time for faster execution.`
        : undefined,
      _warnings: warnings.length > 0 ? warnings : undefined
    });
  }

  /**
   * Executes a delegate_to_utility call: spins up 1..3 cheap "utility" sub-agents
   * (each its own AgentLoop pass on the utility model) restricted to read/list/search
   * + move_file, and returns their short summaries to the architect. Mirrors
   * executeDelegateTasks but with the utility model and a lighter toolset.
   */
  async executeUtilityTasks(runId: string, run: Run, toolCall: any): Promise<string> {
    if (!run.utilityModel || !run.utilityProviderId) {
      return JSON.stringify({ success: false, error: "No utility model is configured for this run." });
    }
    if (!getModeStrategy(run.mode).allowsDelegation) {
      return JSON.stringify({ success: false, error: `Cannot delegate in ${run.mode} mode. Switch to Build mode.` });
    }

    let args: any;
    try {
      args = JSON.parse(toolCall.function.arguments || "{}");
    } catch (e: any) {
      return JSON.stringify({ success: false, error: `Could not parse delegate_to_utility arguments (${e.message}). This usually means they were too large and got cut off. Do NOT paste file contents into 'instructions' — utility reads files itself. Keep each task short: say what to look up and cite file paths, then retry.` });
    }

    const tasks = normalizeTasks(args, 3, "Lookup");
    if (tasks.length === 0) {
      return JSON.stringify({ success: false, error: "No valid tasks to delegate (each task needs non-empty instructions)." });
    }

    const parallel = !!args.parallel && tasks.length > 1;
    const results = await runTasks(tasks, parallel, task => this.runUtilityTask(runId, run, task));

    return JSON.stringify({
      success: true,
      parallel,
      results: results.map(r => ({ title: r.title, summary: trimSummary(r.summary) })),
      _verification_required: false,
      _reminder:
        "Review the utility results above and proceed with implementation if the information is sufficient."
    });
  }

  /**
   * Executes a spawn_agents call from a single-model run's main agent: launches
   * 1..MAX_SPAWNED_AGENTS sub-agents on the run's OWN model — 'explore' read-only,
   * 'general' with the full workspace toolset — and returns their compact
   * reports. The types allowed come from the mode strategy (plan: explore only).
   */
  async executeSpawnAgents(runId: string, run: Run, toolCall: any): Promise<string> {
    if (run.coderModel && run.coderProviderId) {
      return JSON.stringify({ success: false, error: "This run uses an agent preset; delegate with delegate_tasks instead of spawn_agents." });
    }
    const allowedTypes = getModeStrategy(run.mode).subAgentTypes;
    if (allowedTypes.length === 0) {
      return JSON.stringify({ success: false, error: `Cannot spawn sub-agents in ${run.mode} mode.` });
    }

    let args: any;
    try {
      args = JSON.parse(toolCall.function.arguments || "{}");
    } catch (e: any) {
      return JSON.stringify({ success: false, error: `Could not parse spawn_agents arguments (${e.message}). This usually means they were too large and got cut off. Do NOT paste file contents into 'instructions' — sub-agents read files themselves. Keep instructions short and cite file paths, then retry.` });
    }

    // Same filter normalizeTasks applies, so the two lists stay index-aligned.
    const valid = (Array.isArray(args.agents) ? args.agents : [])
      .filter((a: any) => a && typeof a.instructions === "string" && a.instructions.trim());
    const disallowed = valid.find((a: any) => !allowedTypes.includes(a.type));
    if (disallowed) {
      return JSON.stringify({ success: false, error: `Sub-agent type "${disallowed.type}" is not available in ${run.mode} mode. Allowed: ${allowedTypes.join(", ")}.` });
    }
    const agents = normalizeTasks({ tasks: valid }, MAX_SPAWNED_AGENTS, "Agent")
      .map((task, i) => ({ ...task, type: valid[i].type as SubAgentType }));
    if (agents.length === 0) {
      return JSON.stringify({ success: false, error: "No valid agents to spawn (each needs a type and non-empty instructions)." });
    }

    const parallel = !!args.parallel && agents.length > 1;
    const results = await runTasks(agents, parallel, agent => this.runSpawnedAgent(runId, run, agent));

    const warnings = summaryWarnings(results);
    return JSON.stringify({
      success: true,
      parallel,
      results: results.map((r, i) => ({ type: agents[i].type, title: r.title, summary: trimSummary(r.summary) })),
      _warnings: warnings.length > 0 ? warnings : undefined
    });
  }

  /** One spawn_agents sub-agent on the run's own model, tooled by its type. */
  private runSpawnedAgent(runId: string, run: Run, agent: DelegatedTask & { type: SubAgentType }): Promise<TaskResult> {
    const explore = agent.type === "explore";
    return this.runTaskWithFallback(runId, run, agent, [{
      providerId: run.providerId,
      providerDisplayName: run.providerDisplayName,
      model: run.model,
      reasoningEffort: run.reasoningEffort,
      tools: explore
        ? WORKSPACE_TOOLS.filter(t => READONLY_TOOLS.has(t.function.name))
        : [...WORKSPACE_TOOLS],
      agentRole: explore ? "explorer" : "worker",
      systemPrompt: explore
        ? buildExplorerSystemPrompt(run.projectName, run.projectPath, agent.title)
        : buildCoderSystemPrompt(run.projectName, run.projectPath, agent.title, this.coderProjectContext(run)),
      agentName: agent.title
    }]);
  }
}
