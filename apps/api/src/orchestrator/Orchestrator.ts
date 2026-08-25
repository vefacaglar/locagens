import type { Run, RunMessage, ChatMessage } from "@locagens/shared";
import type { IRunRepository, IMessageRepository, IPlanRepository, IMemoryRepository, IUsageLogRepository } from "../database/repositories.js";
import { ProviderRegistry } from "../providers/ProviderRegistry.js";
import { eventBus } from "./eventBus.js";
import { buildSystemPrompt, formatMemoryContext, formatActivePlan, formatSkillCatalog, getModeStrategy } from "./systemPrompt.js";
import { DELEGATE_TASKS_TOOL, DELEGATE_UTILITY_TOOL } from "./workspaceTools.js";
import { availableSchemas } from "./tools/index.js";
import type { OrchestratorToolContext } from "./tools/index.js";
import { SkillRegistry, type DiscoveredSkill } from "./skills/index.js";
import { McpClientManager, McpConfigStore, adaptMcpToolsToSchemas } from "./mcp/index.js";
import { RunMessageStream } from "./RunMessageStream.js";
import { PermissionCoordinator, type PermissionDecision } from "./PermissionCoordinator.js";
import { QuestionCoordinator } from "./QuestionCoordinator.js";
import { AgentLoop } from "./AgentLoop.js";
import { DelegationCoordinator } from "./DelegationCoordinator.js";
import { rebuildHistory } from "./rebuildHistory.js";
import { randomId } from "./ids.js";

export class Orchestrator {
  private activeRuns: Set<string> = new Set();

  // Outbound persistence + SSE for this orchestrator's runs.
  private messages: RunMessageStream;
  // Tool-permission flow (standing grants, prompts, serialization).
  private permissions: PermissionCoordinator;
  // ask_user_question flow (pause the run, collect the user's answer).
  private questions: QuestionCoordinator;
  // The capability surface passed to orchestrator-native tool handlers
  // (set_chat_title / update_plan / ask_user_question / remember / load_skill).
  private toolContext: OrchestratorToolContext;
  // The shared model-agnostic generation loop (main agent + every sub-agent).
  private agentLoop: AgentLoop;
  // Runs the architect's delegate_tasks / delegate_to_utility sub-agent fan-out.
  private delegation: DelegationCoordinator;
  // Discovers SKILL.md packs for the main agent (catalog + load_skill).
  private skillRegistry: SkillRegistry;
  // Manages active MCP servers and tools.
  private mcpManager: McpClientManager;
  // Skills discovered for the in-flight drive(); empty when idle.
  private driveSkills: DiscoveredSkill[] = [];

  constructor(
    private runRepo: IRunRepository,
    private messageRepo: IMessageRepository,
    private registry: ProviderRegistry,
    private planRepo: IPlanRepository,
    private memoryRepo: IMemoryRepository,
    private usageLogRepo: IUsageLogRepository,
    skillRegistry?: SkillRegistry,
    mcpManager?: McpClientManager
  ) {
    this.skillRegistry = skillRegistry ?? new SkillRegistry();
    this.mcpManager = mcpManager ?? new McpClientManager(new McpConfigStore());
    this.messages = new RunMessageStream(this.runRepo, this.messageRepo);
    this.permissions = new PermissionCoordinator(this.activeRuns, this.messages);
    this.questions = new QuestionCoordinator(this.activeRuns, this.messages);
    this.toolContext = {
      runRepo: this.runRepo,
      planRepo: this.planRepo,
      memoryRepo: this.memoryRepo,
      eventBus,
      mcpManager: this.mcpManager,
      getSkills: () => this.driveSkills,
      requestUserAnswer: (runId, questions) => this.questions.request(runId, questions),
      isActive: (runId) => this.activeRuns.has(runId),
      setGenerating: (runId) => this.messages.emitStatus(runId, "generating")
    };
    // The loop and the delegation handler reference each other (loop runs the
    // sub-agents; delegation dispatches from inside the loop), so wire the
    // delegator after both exist.
    this.agentLoop = new AgentLoop(this.activeRuns, this.messages, this.permissions, this.toolContext, this.registry, this.usageLogRepo);
    this.delegation = new DelegationCoordinator(this.registry, this.agentLoop, this.memoryRepo);
    this.agentLoop.setDelegator(this.delegation);
  }

  // --- Permission / question pass-throughs (owned by the coordinators) -----

  resolvePermission(runId: string, decision: PermissionDecision): boolean {
    return this.permissions.resolve(runId, decision);
  }

  getPendingPermission(runId: string) {
    return this.permissions.getPending(runId);
  }

  resolveQuestion(runId: string, answer: { selections: string[][]; notes: string[] }): boolean {
    return this.questions.resolve(runId, answer);
  }

  getPendingQuestion(runId: string) {
    return this.questions.getPending(runId);
  }

  // --- Run lifecycle -------------------------------------------------------

  async cancel(runId: string): Promise<boolean> {
    await this.messages.flushAllPendingDbUpdates();
    // Unblock any pending permission/question promise so the run loop can unwind.
    this.permissions.cancelPending(runId);
    this.questions.cancelPending(runId);

    if (this.activeRuns.has(runId)) {
      this.activeRuns.delete(runId);
      await this.messages.emitStatus(runId, "cancelled");
      eventBus.emit(`run:${runId}`, { type: "run_failed", errorMessage: "Chat generation cancelled by user." });
      console.log(`[Orchestrator] Run ${runId} has been cancelled by user.`);
      return true;
    }

    // Fallback for runs marked active in DB but not in memory (e.g. after restart).
    const run = this.runRepo.getById(runId);
    if (run && (run.status === "created" || run.status === "generating" || run.status === "awaiting_permission")) {
      await this.messages.emitStatus(runId, "cancelled");
      eventBus.emit(`run:${runId}`, { type: "run_failed", errorMessage: "Chat generation cancelled by user." });
      console.log(`[Orchestrator] Run ${runId} (inactive in memory) has been marked as cancelled.`);
      return true;
    }

    return false;
  }

  isRunning(runId: string): boolean {
    return this.activeRuns.has(runId);
  }

  /** Starts a brand new chat session for a freshly created run. */
  async run(runId: string): Promise<void> {
    const run = this.runRepo.getById(runId);
    if (!run) {
      console.error(`[Orchestrator] Run with id "${runId}" not found in database.`);
      return;
    }

    console.log(`[Orchestrator] Starting single-model chat session: ${runId} ("${run.title}")`);
    this.activeRuns.add(runId);

    eventBus.emit(`run:${runId}`, { type: "run_started", runId });
    eventBus.emit(`run:${runId}`, {
      type: "model_snapshot_locked",
      snapshot: {
        providerId: run.providerId,
        providerDisplayName: run.providerDisplayName,
        model: run.model,
        reasoningEffort: run.reasoningEffort
      }
    });

    await this.drive(runId, run, [{ role: "user", content: run.task }], true);
  }

  /** Continues an existing run with new user input, replaying stored history. */
  async continueRun(runId: string, _newUserMessage: string): Promise<void> {
    const run = this.runRepo.getById(runId);
    if (!run) {
      console.error(`[Orchestrator] Run with id "${runId}" not found for continuation.`);
      return;
    }

    console.log(`[Orchestrator] Continuing single-model chat session: ${runId}`);
    this.activeRuns.add(runId);

    const history = rebuildHistory(this.messageRepo.listByRunId(runId), run.task);
    await this.drive(runId, run, history, false);
  }

  /**
   * Top-level run driver used by both fresh runs and continuations. Sets up the
   * architect/main agent tools (advertising delegate_tasks when a coder model is
   * configured) and system prompt, runs the shared AgentLoop, then finalizes run
   * status. The loop itself (AgentLoop) is reused by every coder/utility sub-agent.
   */
  private async drive(
    runId: string,
    run: Run,
    initialMessages: ChatMessage[],
    shouldReadProjectGuidance: boolean
  ): Promise<void> {
    try {
      console.log(`[Orchestrator] Run ${runId} - Entering GENERATING state`);
      await this.messages.emitStatus(runId, "generating");

      // The mode strategy carries this run's full behavior: prompt section, base
      // tool set, and gating policy. All per-mode branching reads from it.
      const strategy = getModeStrategy(run.mode);

      // Dual-model: when a coder model is wired up, the main model is the
      // architect — it gets the delegate_tasks tool and architect instructions.
      // Delegation means implementation, so it is only offered in build-type
      // modes — never in plan mode (planning only) or chat mode (lightweight).
      const canDelegate = strategy.allowsDelegation;
      // An optional utility tier (cheap lookups/renames) rides alongside the
      // coder; it is only offered when the preset configured a utility model.
      const hasUtility = !!(run.utilityModel && run.utilityProviderId);
      const delegation = (canDelegate && run.coderModel && run.coderProviderId)
        ? {
            coderModel: run.coderModel,
            maxSubAgents: this.delegation.maxSubAgentsFor(run),
            utilityModel: hasUtility ? run.utilityModel : undefined
          }
        : undefined;

      // The strategy selects the base tools for this mode. Plan mode gets
      // read-only tools plus update_plan; chat gets non-mutating tools; build
      // modes get the full toolset, except when a coder is configured — then the
      // architect is held to read-only tools so the only way it can implement is
      // to delegate (weak models otherwise ignore the soft "prefer delegating"
      // instruction and do all the work directly).
      const baseTools = strategy.selectBaseTools(!!delegation);
      const withDelegate = delegation ? [...baseTools, DELEGATE_TASKS_TOOL] : baseTools;
      // The utility-delegation tool is offered only when a utility model is
      // configured (and the run can delegate at all).
      const withUtility = delegation?.utilityModel ? [...withDelegate, DELEGATE_UTILITY_TOOL] : withDelegate;
      // The orchestrator-native tools the strategy allows: update_plan (plan
      // mode only) plus set_chat_title / ask_user_question / remember (every
      // mode). Sub-agents never get these. See orchestrator/tools.
      const nativeTools = [...withUtility, ...availableSchemas(strategy)];

      // Active MCP tools from enabled MCP servers for this context.
      const mcpTools = await this.mcpManager.getAllActiveTools(run.projectPath);
      const mcpSchemas = adaptMcpToolsToSchemas(mcpTools);
      const tools = [...nativeTools, ...mcpSchemas];

      // Durable memories for this run's context (all global + this project's),
      // injected into the system prompt so the model honors them from the start.
      const memoryContext = formatMemoryContext(this.memoryRepo.listForContext(run.projectPath));

      // In implementation modes, inject the run's active plan (created in plan
      // mode via update_plan) so the model seeds and maintains its live
      // <task_list> from it. Plan/chat modes skip this: plan mode owns the plan
      // already, chat does no proactive work.
      const activePlan = strategy.allowsMutation ? this.planRepo.getActive(runId) : null;
      const planContext = formatActivePlan(activePlan);

      // Skills: name+description catalog in the prompt; full body via load_skill.
      // Fresh discover per drive so new SKILL.md files appear between turns.
      this.driveSkills = this.skillRegistry.discover(run.projectPath);
      const skillCatalog = formatSkillCatalog(this.driveSkills);

      const systemPrompt = buildSystemPrompt({
        projectName: run.projectName,
        projectPath: run.projectPath,
        mode: run.mode,
        shouldReadProjectGuidance: run.mode !== "chat" && shouldReadProjectGuidance,
        delegation,
        memoryContext,
        planContext,
        skillCatalog
      });

      // When an approved plan exists, implementation is clearly expected. Weak
      // architects sometimes just say "I'll start now" and end the turn without
      // calling a tool, which would finish the run prematurely. This nudge makes
      // the loop give them one more shot to actually act (capped — see AgentLoop).
      const idleNudge = planContext
        ? (delegation
            ? "You ended your turn without calling any tool, but there is an approved plan to implement and you have not started yet. Do not merely announce that you will begin — call delegate_tasks NOW to implement the first step(s). Only stop without a tool call if the work is genuinely already complete, and then say so explicitly."
            : "You ended your turn without calling any tool, but there is an approved plan to implement and you have not started yet. Do not merely announce that you will begin — use the workspace tools NOW to implement the first step(s). Only stop without a tool call if the work is genuinely already complete, and then say so explicitly.")
        : undefined;

      const postDelegationNudge = delegation
        ? (delegation.utilityModel
            ? "You delegated tasks to a coder but did not verify the results. Do NOT read the changed files yourself — that bloats your context. Instead call delegate_to_utility with one task asking the utility model to read each changed file and confirm the changes are correct, returning a SHORT verdict. Only after that verdict can you mark the task complete and finish."
            : "You delegated tasks to a coder but did not verify the results. Do NOT read the changed files yourself — that bloats your context. Instead call delegate_tasks with ONE task with verify: true, instructing it to read each changed file and return a SHORT verdict. Only after that verdict can you mark the task complete and finish.")
        : undefined;

      const finalText = await this.agentLoop.run(runId, run, [...initialMessages], {
        providerId: run.providerId,
        providerDisplayName: run.providerDisplayName,
        model: run.model,
        reasoningEffort: run.reasoningEffort,
        systemPrompt,
        tools,
        agentRole: delegation ? "planner" : undefined,
        idleNudge,
        maxIdleNudges: idleNudge ? 1 : 0,
        postDelegationNudge,
        maxPostDelegationNudges: postDelegationNudge ? 1 : 0
      });

      await this.messages.emitStatus(runId, "done");
      eventBus.emit(`run:${runId}`, { type: "run_completed", finalOutput: finalText });
      console.log(`[Orchestrator] Run ${runId} - Completed successfully.`);
    } catch (error: any) {
      if (error.message === "ORCHESTRATION_CANCELLED") {
        console.log(`[Orchestrator] Run ${runId} - Process safely stopped by cancellation.`);
      } else {
        console.error(`[Orchestrator] Run ${runId} - Failed with error:`, error.message);

        const errorMsg: RunMessage = {
          id: randomId("msg-err"),
          runId,
          role: "system",
          content: error.message || "Failed to query provider.",
          createdAt: new Date().toISOString()
        };
        this.messages.emitMessage(runId, errorMsg);

        await this.messages.emitStatus(runId, "failed", { errorMessage: error.message });
        eventBus.emit(`run:${runId}`, { type: "run_failed", errorMessage: error.message });
      }
    } finally {
      await this.messages.flushAllPendingDbUpdates();
      this.activeRuns.delete(runId);
      this.permissions.clear(runId);
      this.questions.cancelPending(runId);
      this.driveSkills = [];
    }
  }
}
