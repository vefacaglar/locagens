import type { Run, RunMessage, ChatMessage, ReasoningEffort, CompletionResponse } from "@locagens/shared";
import type { ModelProvider } from "../providers/ModelProvider.js";
import type { ProviderRegistry } from "../providers/ProviderRegistry.js";
import { appendFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { getModeStrategy, type ModeStrategy } from "./systemPrompt.js";
import { executeWorkspaceToolAsync, DANGEROUS_TOOLS, READONLY_TOOLS, MODIFYING_TOOLS, UTILITY_TOOL_NAMES } from "./workspaceTools.js";
import { getOrchestratorTool } from "./tools/index.js";
import type { OrchestratorToolContext } from "./tools/index.js";
import type { RunMessageStream } from "./RunMessageStream.js";
import type { PermissionCoordinator } from "./PermissionCoordinator.js";
import { randomId } from "./ids.js";
import type { IUsageLogRepository } from "../database/repositories.js";
import { calculateCost } from "./pricing.js";
import { estimateTokens, compactHistory, COMPACT_TRIGGER_RATIO, COMPACT_TARGET_RATIO, DEFAULT_CONTEXT_LIMIT } from "./contextWindow.js";

/** The model + toolset + prompt a single agent loop runs with. */
export interface AgentRunOptions {
  providerId: string;
  providerDisplayName: string;
  model: string;
  reasoningEffort?: ReasoningEffort;
  systemPrompt: string;
  tools: any[];
  agentRole?: RunMessage["agentRole"];
  agentName?: string;
  /**
   * Weak architects sometimes announce that they will start ("I'll begin now")
   * and then end the turn with NO tool call, which the loop would treat as a
   * finished run. When this nudge text is set and the agent ends the run having
   * made ZERO tool calls, the loop injects this reminder (invisibly) and lets the
   * model try again, up to maxIdleNudges times. Tying it to zero tool calls caps
   * the extra turns hard and prevents an infinite loop.
   */
  idleNudge?: string;
  maxIdleNudges?: number;
  /**
   * When the architect delegates tasks and then tries to end the run without
   * verifying the results (no read_file/inspect calls after delegation), this
   * nudge is injected to remind it to verify. Capped at maxPostDelegationNudges.
   */
  postDelegationNudge?: string;
  maxPostDelegationNudges?: number;
}

/**
 * Handles the delegate_tasks / delegate_to_utility tool calls. Injected into the
 * AgentLoop (rather than imported) so the loop depends only on this small
 * interface — breaking the loop ↔ delegation recursion cycle. Implemented by
 * DelegationCoordinator.
 */
export interface Delegator {
  executeDelegateTasks(runId: string, run: Run, toolCall: any): Promise<string>;
  executeUtilityTasks(runId: string, run: Run, toolCall: any): Promise<string>;
}

// One-shot reminder injected when the agent ends its turn mid-run with unchecked
// <task_list> items and no tool call — the classic weak-model "I'll do X next"
// stall, after it has already started working (the idle nudge only covers the
// zero-tool-call case). Capped at one per run to avoid loops.
const OPEN_TASK_LIST_NUDGE =
  "Your <task_list> still has unchecked '- [ ]' items, but you ended your turn without calling any tool. " +
  "Either continue the work with your tools NOW, or — if you are blocked or the remaining items are intentionally out of scope — say so explicitly and update the list.";
const MAX_TASK_LIST_NUDGES = 1;

/**
 * When a utility tier is configured, the architect should explore the workspace
 * through delegate_to_utility, not by reading it into its own expensive context.
 * The prompt alone does not hold weak models to this, so — like the mutating-tool
 * trap — it is enforced in-loop: the architect gets a small per-turn budget of
 * direct read_file/list_directory/search_files calls (enough for guidance files
 * and a short, decisive read), and further read-only calls are rejected with a
 * redirect to delegate_to_utility.
 */
const ARCHITECT_DIRECT_INSPECT_BUDGET = 4;

/**
 * Minimum gap between message_updated SSE emissions during token streaming.
 * Every emission carries the message's FULL accumulated content, so emitting on
 * every chunk makes total stream traffic grow quadratically with response
 * length. The DB write is already throttled (RunMessageStream); this throttles
 * the wire. The final message emitted after completion always carries the full
 * text, so trailing chunks are never lost.
 */
const STREAM_EMIT_INTERVAL_MS = 80;

/**
 * The tools an architect may call directly. Everything else it is advertised is
 * a trap that redirects to delegation (see gateWorkspaceCall). Read-only tools
 * are also allowed, but budgeted when a utility tier exists.
 */
const ARCHITECT_DIRECT_TOOLS = new Set([
  "search_web",
  "fetch_url",
  "delegate_tasks",
  "delegate_to_utility",
  "update_plan",
  "set_chat_title",
  "ask_user_question",
  "remember"
]);

/** Wraps a gating refusal in the standard tool-result error shape. */
function deny(error: string): string {
  return JSON.stringify({ success: false, error });
}

/** True when the reply carries a <task_list> with at least one unchecked item. */
function hasOpenTaskListItems(text: string): boolean {
  const match = text.match(/<task_list>([\s\S]*?)(?:<\/task_list>|$)/);
  return !!match && /-\s*\[\s\]/.test(match[1]);
}

function usageLogPath(): string | null {
  if (process.env.LOCAGENS_USAGE_LOG_PATH) {
    return resolve(process.env.LOCAGENS_USAGE_LOG_PATH);
  }
  if (process.env.LOCAGENS_DB_PATH) {
    return join(dirname(resolve(process.env.LOCAGENS_DB_PATH)), "usage.log");
  }
  return null;
}

/**
 * Tracks whether the architect verified its delegated work. Fed one executed
 * tool call at a time; asked for the right verification nudge when the model
 * tries to end its turn.
 */
class VerificationTracker {
  private delegated = false;
  private verified = false;
  private filesToVerify: string[] = [];
  private filesVerified = new Set<string>();

  observe(toolCall: any, result: string): void {
    const toolName = toolCall.function?.name;
    if (toolName === "delegate_tasks") {
      this.delegated = true;
      this.verified = false;
      this.observeDelegationResult(result);
    } else if (toolName === "delegate_to_utility") {
      // The architect offloaded verification to the cheap utility tier (one
      // call covers all changed files). We can't match individual files through
      // it, so count it as full verification.
      this.markFullyVerified();
    } else if (READONLY_TOOLS.has(toolName)) {
      this.verified = true;
      if (toolName === "read_file") this.observeFileRead(toolCall);
    }
  }

  /**
   * The nudge to inject when the architect tries to finish: the base nudge when
   * it never verified, a completeness nudge when it verified only SOME files,
   * or null when verification is satisfied (or never owed).
   */
  pendingNudge(baseNudge: string | undefined): string | null {
    if (!baseNudge || !this.delegated) return null;
    if (!this.verified) return baseNudge;
    const remaining = this.filesToVerify.filter(f => !this.filesVerified.has(f));
    if (this.filesToVerify.length === 0 || remaining.length === 0) return null;
    return `You verified ${this.filesVerified.size}/${this.filesToVerify.length} files. Please also verify: ${remaining.join(", ")} before marking tasks complete.`;
  }

  private observeDelegationResult(result: string): void {
    try {
      const delegationResult = JSON.parse(result);
      if (delegationResult._is_verification === true) {
        // The call WAS the verification (verify:true tasks on the coder, read-only).
        this.markFullyVerified();
      } else if (Array.isArray(delegationResult._files_to_verify)) {
        this.filesToVerify = delegationResult._files_to_verify;
        this.filesVerified = new Set();
      }
    } catch {
      // If parsing fails, skip tracking
    }
  }

  /** Matches a read_file call against the files awaiting verification. */
  private observeFileRead(toolCall: any): void {
    if (this.filesToVerify.length === 0) return;
    try {
      const args = JSON.parse(toolCall.function?.arguments || "{}");
      if (typeof args.path !== "string") return;
      const baseOf = (p: string) => p.split("/").pop() ?? p;
      const verifiedBase = baseOf(args.path);
      for (const f of this.filesToVerify) {
        if (baseOf(f) === verifiedBase) this.filesVerified.add(f);
      }
    } catch {
      // If parsing fails, skip tracking
    }
  }

  private markFullyVerified(): void {
    this.verified = true;
    this.filesToVerify = [];
    this.filesVerified = new Set();
  }
}

/** Mutable per-run loop state: action counts and nudge/inspection budgets. */
interface LoopState {
  toolCallsMade: number;
  idleNudgesUsed: number;
  postDelegationNudgesUsed: number;
  taskListNudgesUsed: number;
  directInspections: number;
  verification: VerificationTracker;
}

/** The fixed per-run inputs every helper of the loop needs. */
interface TurnContext {
  runId: string;
  opts: AgentRunOptions;
  provider: ModelProvider;
  contextLimit: number;
  checkCancelled: () => void;
}

/**
 * The model-agnostic generation loop and per-tool-call gating, shared by the
 * main/architect agent and every coder/utility sub-agent. It calls the model,
 * streams + persists its messages, gates and executes each tool call, and repeats
 * until the model returns a final answer with no tool calls. It owns no run-level
 * status transitions — the caller (Orchestrator.drive) owns generating/done/failed.
 */
export class AgentLoop {
  private delegator!: Delegator;

  constructor(
    private activeRuns: Set<string>,
    private messages: RunMessageStream,
    private permissions: PermissionCoordinator,
    private toolContext: OrchestratorToolContext,
    private registry: ProviderRegistry,
    private usageLogRepo: IUsageLogRepository
  ) {}

  /** Wires the delegation handler. Called once during orchestrator setup. */
  setDelegator(delegator: Delegator): void {
    this.delegator = delegator;
  }

  /**
   * Runs the generation loop with the given model/tools, tagging persisted
   * messages with the supplied agentRole. Returns the model's final text.
   */
  async run(runId: string, run: Run, messages: ChatMessage[], opts: AgentRunOptions): Promise<string> {
    const turn: TurnContext = {
      runId,
      opts,
      provider: this.registry.getProvider(opts.providerId),
      // Models without a configured limit fall back to a conservative default —
      // no model runs unprotected.
      contextLimit: this.registry.resolveContextLimit(opts.providerId, opts.model) || DEFAULT_CONTEXT_LIMIT,
      checkCancelled: () => {
        if (!this.activeRuns.has(runId)) {
          throw new Error("ORCHESTRATION_CANCELLED");
        }
      }
    };
    const state: LoopState = {
      toolCallsMade: 0,
      idleNudgesUsed: 0,
      postDelegationNudgesUsed: 0,
      taskListNudgesUsed: 0,
      directInspections: 0,
      verification: new VerificationTracker()
    };

    let lastText = "";
    // The REAL token count of the previous prompt, from the provider's usage
    // report. 0 = unknown (cold start / right after compaction) — the context
    // check then falls back to a chars-based estimate.
    let lastPromptTotal = 0;

    while (true) {
      turn.checkCancelled();
      lastPromptTotal = this.enforceContextWindow(turn, messages, lastPromptTotal);

      const { response, promptTotal } = await this.streamCompletion(turn, messages);
      lastPromptTotal = promptTotal;
      if (response.content) lastText = response.content;

      if (response.toolCalls && response.toolCalls.length > 0) {
        await this.executeToolCalls(turn, run, messages, response, state);
        continue;
      }

      // No tool call: the model thinks it is done. Give the relevant corrective
      // nudge a shot (invisibly — nudges are not persisted, so they stay out of
      // the UI); if none applies, the run is genuinely finished.
      const nudge = this.chooseNudge(response.content || "", opts, state);
      if (nudge === null) return lastText;
      messages.push({ role: "assistant", content: response.content || "" });
      messages.push({ role: "user", content: nudge });
    }
  }

  /**
   * Compacts the history in chunks when the prompt nears the model's context
   * limit. Returns the (possibly reset) lastPromptTotal: 0 right after a
   * compaction, since the previous usage report no longer describes the history.
   */
  private enforceContextWindow(turn: TurnContext, messages: ChatMessage[], lastPromptTotal: number): number {
    const estimated = lastPromptTotal || estimateTokens(messages, turn.opts.systemPrompt);
    if (estimated < COMPACT_TRIGGER_RATIO * turn.contextLimit) return lastPromptTotal;

    const compacted = compactHistory(messages, Math.floor(COMPACT_TARGET_RATIO * turn.contextLimit));
    if (!compacted) return lastPromptTotal;

    // In place: the array reference is shared with the caller and pushed to
    // throughout the loop.
    messages.length = 0;
    messages.push(...compacted.messages);
    console.log(`[context] compacted run=${turn.runId} role=${turn.opts.agentRole ?? "main"} evicted=${compacted.evictedCount} clipped=${compacted.clippedCount} limit=${turn.contextLimit}`);
    return 0;
  }

  /**
   * One model call: streams the response into a live RunMessage, records usage,
   * and emits the final message. Returns the response plus the prompt's real
   * token total (estimated when the provider omitted usage).
   */
  private async streamCompletion(
    turn: TurnContext,
    messages: ChatMessage[]
  ): Promise<{ response: CompletionResponse; promptTotal: number }> {
    const { runId, opts } = turn;
    const msgId = randomId("msg-res");
    let accumulatedContent = "";
    let accumulatedReasoning = "";
    let hasCreatedMessage = false;

    const emitStreamingMessage = () => {
      const assistantMsg = this.buildAssistantMessage(turn, msgId, {
        content: accumulatedContent,
        reasoningContent: accumulatedReasoning || undefined
      });
      if (!hasCreatedMessage) {
        this.messages.emitMessage(runId, assistantMsg);
        hasCreatedMessage = true;
      } else {
        this.messages.updateMessage(runId, assistantMsg);
      }
    };

    const startTime = Date.now();
    let lastEmitTime = 0;
    const response = await turn.provider.complete({
      model: opts.model,
      reasoningEffort: opts.reasoningEffort,
      reasoning: this.registry.resolveReasoning(opts.providerId, opts.model, opts.reasoningEffort),
      temperature: this.registry.resolveTemperature(opts.providerId, opts.model),
      systemPrompt: opts.systemPrompt,
      messages,
      tools: opts.tools
    }, (chunk) => {
      turn.checkCancelled();
      if (chunk.content) accumulatedContent += chunk.content;
      if (chunk.reasoningContent) accumulatedReasoning += chunk.reasoningContent;
      const now = Date.now();
      if (!hasCreatedMessage || now - lastEmitTime >= STREAM_EMIT_INTERVAL_MS) {
        lastEmitTime = now;
        emitStreamingMessage();
      }
    });

    turn.checkCancelled();
    await this.messages.flushMessageDbUpdate(msgId);

    const promptTotal = this.recordUsage(turn, messages, response, startTime);

    const finalMsg = this.buildAssistantMessage(turn, msgId, {
      content: response.content || "",
      reasoningContent: response.reasoningContent,
      rawResponse: response.toolCalls ? JSON.stringify(response.toolCalls) : undefined
    });
    if (!hasCreatedMessage) {
      this.messages.emitMessage(runId, finalMsg);
    } else {
      this.messages.updateMessage(runId, finalMsg);
    }

    return { response, promptTotal };
  }

  /** Assembles an assistant RunMessage carrying this loop's agent/model tags. */
  private buildAssistantMessage(
    turn: TurnContext,
    msgId: string,
    body: Pick<RunMessage, "content" | "reasoningContent" | "rawResponse">
  ): RunMessage {
    const { opts } = turn;
    return {
      id: msgId,
      runId: turn.runId,
      role: "assistant",
      agentRole: opts.agentRole,
      agentName: opts.agentName,
      providerId: opts.providerId,
      providerDisplayName: opts.providerDisplayName,
      model: opts.model,
      createdAt: new Date().toISOString(),
      ...body
    };
  }

  /**
   * Logs token/cache usage (console + local file) and writes the usage_logs row
   * with the locally computed cost. Returns the prompt's total token count, so
   * the loop can use the REAL size for the next context-window check. Falls back
   * to a chars-based estimate when the provider omitted usage.
   */
  private recordUsage(
    turn: TurnContext,
    messages: ChatMessage[],
    response: CompletionResponse,
    startTime: number
  ): number {
    const { runId, opts } = turn;
    let usage = response.usage;
    if (!usage) {
      // Fallback estimation (e.g. for streaming OpenAI calls where usage chunk is omitted)
      const promptChars = (opts.systemPrompt || "").length +
        messages.reduce((sum, m) => sum + (typeof m.content === "string" ? m.content.length : 0), 0);
      const estIn = Math.round(promptChars / 3.5) || 1;
      const estOut = Math.round((response.content || "").length / 3.5) || 1;
      usage = { inputTokens: estIn, outputTokens: estOut, totalTokens: estIn + estOut };
    }

    const fresh = usage.inputTokens ?? 0;
    const cacheRead = usage.cacheReadInputTokens ?? 0;
    const cacheWrite = usage.cacheWriteInputTokens ?? 0;
    const promptTotal = fresh + cacheRead + cacheWrite;
    const hitPct = promptTotal > 0 ? Math.round((cacheRead / promptTotal) * 100) : 0;

    // Prompt-cache measurement: one line per model call so cache hit-rate is
    // observable live in the server console during a run. inputTokens is the
    // fresh (full-rate) prompt; cacheRead is served cheaply from cache.
    const line =
      `[usage] ${new Date().toISOString()} run=${runId} role=${opts.agentRole ?? "main"} model=${opts.model} ` +
      `in=${fresh} cacheRead=${cacheRead} cacheWrite=${cacheWrite} out=${usage.outputTokens ?? 0} cacheHit=${hitPct}%`;
    console.log(line);
    // Also append to a local log file so token/cache usage can be inspected
    // after the fact (the console stream is otherwise lost in the dev terminal).
    try {
      const logPath = usageLogPath();
      if (logPath) appendFileSync(logPath, line + "\n");
    } catch {
      // Best-effort only; never let logging break a run.
    }

    // Cost comes solely from the model's user-entered pricing (no built-in
    // table, no name-based guessing). Unpriced models cost 0.
    const pricing = this.registry.resolvePricing(opts.providerId, opts.model);
    const cost = calculateCost(pricing, fresh, usage.outputTokens ?? 0, cacheRead, cacheWrite);

    this.usageLogRepo.create({
      runId,
      agentRole: opts.agentRole ?? "main",
      providerId: opts.providerId,
      model: opts.model,
      inputTokens: fresh,
      outputTokens: usage.outputTokens ?? 0,
      cacheReadTokens: cacheRead,
      cacheWriteTokens: cacheWrite,
      cacheHitRate: hitPct,
      cost,
      createdAt: new Date().toISOString(),
      durationMs: Date.now() - startTime
    }).catch(err => {
      console.error(`[Database] Failed to write usage log to database: ${err.message}`);
    });

    return promptTotal;
  }

  /** Executes a response's tool calls in order, appending each result to the history. */
  private async executeToolCalls(
    turn: TurnContext,
    run: Run,
    messages: ChatMessage[],
    response: CompletionResponse,
    state: LoopState
  ): Promise<void> {
    const toolCalls = response.toolCalls ?? [];
    state.toolCallsMade += toolCalls.length;
    messages.push({
      role: "assistant",
      content: response.content || "",
      toolCalls
    });

    for (const tc of toolCalls) {
      turn.checkCancelled();
      const result = await this.runToolCall(turn.runId, run, tc, turn.opts.agentRole, state);

      this.messages.emitMessage(turn.runId, {
        id: randomId("msg-tool"),
        runId: turn.runId,
        role: "tool",
        agentRole: turn.opts.agentRole,
        agentName: turn.opts.agentName,
        content: result,
        createdAt: new Date().toISOString()
      });
      messages.push({
        role: "tool",
        content: result,
        tool_call_id: tc.id,
        name: tc.function.name
      });

      state.verification.observe(tc, result);
    }
  }

  /**
   * Picks the corrective nudge for a turn that ended with no tool call, spending
   * the matching budget. Priority: never acted at all > delegated but did not
   * (fully) verify > stopped with open task-list items. Null = let the run finish.
   */
  private chooseNudge(responseText: string, opts: AgentRunOptions, state: LoopState): string | null {
    if (opts.idleNudge && state.toolCallsMade === 0 && state.idleNudgesUsed < (opts.maxIdleNudges ?? 0)) {
      state.idleNudgesUsed++;
      return opts.idleNudge;
    }

    const verificationNudge = state.verification.pendingNudge(opts.postDelegationNudge);
    if (verificationNudge && state.postDelegationNudgesUsed < (opts.maxPostDelegationNudges ?? 0)) {
      state.postDelegationNudgesUsed++;
      return verificationNudge;
    }

    if (state.toolCallsMade > 0 && state.taskListNudgesUsed < MAX_TASK_LIST_NUDGES && hasOpenTaskListItems(responseText)) {
      state.taskListNudgesUsed++;
      return OPEN_TASK_LIST_NUDGE;
    }

    return null;
  }

  /**
   * Resolves a single tool call: dispatches orchestrator-native tools and
   * delegation, enforces the mode's gating policy, then runs workspace tools
   * (prompting for permission when required).
   */
  private async runToolCall(runId: string, run: Run, toolCall: any, agentRole?: string, state?: LoopState): Promise<string> {
    // This run's mode strategy: it owns the gating policy (which tools are
    // allowed, what must be approved) so the checks below read its flags instead
    // of branching on run.mode directly.
    const strategy = getModeStrategy(run.mode);
    const toolName = toolCall.function?.name;

    // Orchestrator-native tools (set_chat_title / update_plan / ask_user_question
    // / remember / load_skill) run silently here — no permission gating. Each is
    // a registered handler that owns its own per-mode rules (e.g. update_plan
    // rejects outside plan mode). See orchestrator/tools.
    const nativeTool = getOrchestratorTool(toolName);
    if (nativeTool) {
      return nativeTool.execute(this.toolContext, runId, run, toolCall);
    }

    // Delegation fans out to coder/utility sub-agents; the orchestrator runs
    // their loops itself (the sub-agents' own tool calls are gated normally).
    // Gated by mode FIRST: runs can switch modes mid-thread, so a model
    // continuing a build-mode thread in plan mode may imitate the delegate calls
    // in its history — without this check those would actually launch sub-agents.
    if (toolName === "delegate_tasks" || toolName === "delegate_to_utility") {
      if (!strategy.allowsDelegation) {
        return deny(`Blocked: "${toolName}" is not available in ${run.mode} mode. Delegation only runs in build-type modes. Ask the user to switch to Build mode to implement.`);
      }
      return toolName === "delegate_tasks"
        ? this.delegator.executeDelegateTasks(runId, run, toolCall)
        : this.delegator.executeUtilityTasks(runId, run, toolCall);
    }

    const denial = this.gateWorkspaceCall(run, toolName, strategy, agentRole, state);
    if (denial) return denial;

    const isDangerous = DANGEROUS_TOOLS.has(toolName);
    // run_command / search_web / fetch_url require approval in most modes — but NOT in Full
    // Access mode, where the user has explicitly opted in to autonomous operation
    // with no interruptions. A matching standing grant also lets them run silently
    // in any mode. Other tools gate only in ask_permissions mode (gatesEveryTool).
    let requestsCommandNetwork = false;
    if (toolName === "run_command") {
      try {
        const args = JSON.parse(toolCall.function.arguments || "{}");
        requestsCommandNetwork = Array.isArray(args.network_domains) && args.network_domains.length > 0;
      } catch {
        requestsCommandNetwork = false;
      }
    }
    // Full Access skips ordinary command prompts, but never grants fresh
    // outbound network access. Declared domains still require an exact grant.
    const mustGate = requestsCommandNetwork || (!strategy.bypassDangerousGating && (isDangerous || strategy.gatesEveryTool));
    const needsPermission = mustGate && !this.permissions.check(run, toolCall);

    if (needsPermission) {
      const decision = await this.permissions.request(runId, run, toolCall);
      if (decision === "deny") {
        return deny("Permission denied by user.");
      }
      await this.messages.emitStatus(runId, "generating");
    }
    return executeWorkspaceToolAsync(run, toolCall);
  }

  /**
   * The role/mode gating policy for a workspace tool call. Returns the refusal
   * to send back to the model, or null when the call may proceed to the
   * permission check + execution.
   */
  private gateWorkspaceCall(
    run: Run,
    toolName: string,
    strategy: ModeStrategy,
    agentRole?: string,
    state?: LoopState
  ): string | null {
    const isDelegated = !!(run.coderModel && run.coderProviderId);
    const hasUtilityTier = !!(run.utilityModel && run.utilityProviderId);
    const isArchitect = isDelegated && agentRole !== "coder" && agentRole !== "utility";

    // Architect exploration trap: with a utility tier available, the architect
    // gets only a small per-turn budget of direct read-only calls; after that,
    // exploration is redirected to delegate_to_utility (in-loop correction —
    // the prompt rule alone does not hold weak models to this).
    if (isArchitect && hasUtilityTier && strategy.allowsDelegation && READONLY_TOOLS.has(toolName) && state) {
      if (state.directInspections >= ARCHITECT_DIRECT_INSPECT_BUDGET) {
        return deny(`You have used all ${ARCHITECT_DIRECT_INSPECT_BUDGET} direct inspection calls for this turn. Delegate exploration and summarization to the utility tier instead — it is cheaper and keeps your context lean. Example: delegate_to_utility({ tasks: [{ title: "Map project structure", instructions: "List the key directories and files under <path>, and summarize what each contains. Report back briefly." }] }).`);
      }
      state.directInspections++;
    }

    // Architect mutation trap: every advertised mutating tool is rejected with a
    // redirect to delegation — the only way the architect can change the project.
    if (isArchitect && !READONLY_TOOLS.has(toolName) && !ARCHITECT_DIRECT_TOOLS.has(toolName)) {
      return deny(`You are the ARCHITECT and cannot run "${toolName}" directly — this is by design, not a missing permission. Delegate it to a coder with delegate_tasks. Example: delegate_tasks({ tasks: [{ title: "<short title>", instructions: "<self-contained English instructions; include any shell command to run>" }] }). For a tiny read-only lookup use delegate_to_utility instead.`);
    }

    // The Utility sub-agent is held to its read-only(+move) tool set.
    if (agentRole === "utility" && !UTILITY_TOOL_NAMES.has(toolName)) {
      return deny(`Blocked: Utility sub-agent cannot run "${toolName}". Utility is read-only (read_file/list_directory/search_files) plus move_file. Deletes, edits, writes, and shell commands must be delegated to a coder via delegate_tasks — not to utility.`);
    }

    // HARD RULE: non-build modes are read-only. No file mutation, no run_command —
    // even if the user approves. The model must switch to Build mode before
    // anything changes. We block here regardless of any grant.
    if (!strategy.allowsMutation && MODIFYING_TOOLS.has(toolName)) {
      return deny(`Blocked: "${toolName}" is not allowed in ${run.mode} mode. ${run.mode === "chat" ? "Chat" : "Plan"} mode makes NO changes to the workspace and runs NO commands. Ask the user to switch to Build mode to implement.`);
    }

    // Plan mode additionally allows ONLY read-only tools (it offers update_plan,
    // handled by the native-tool registry, plus inspection) — block anything else.
    if (strategy.allowsPlanTool && !READONLY_TOOLS.has(toolName)) {
      return deny(`Blocked: "${toolName}" is not allowed in Plan mode. Plan mode makes NO changes and runs NO commands. Ask the user to switch to Build mode to implement.`);
    }

    return null;
  }
}
