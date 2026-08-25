import type { Run, UserQuestion } from "@locagens/shared";
import type { ModeStrategy, ToolDef } from "../prompts/index.js";
import type { IRunRepository, IPlanRepository, IMemoryRepository } from "../../database/repositories.js";
import type { eventBus } from "../eventBus.js";
import type { DiscoveredSkill } from "../skills/index.js";

import type { McpClientManager } from "../mcp/index.js";
import type { PluginRegistry, PluginHookRunner } from "../plugins/index.js";

/** The user's reply to an ask_user_question request, aligned to the questions order. */
export interface QuestionAnswerInput {
  selections: string[][];  // chosen option labels per question
  notes: string[];         // free-text comment per question ("" if none)
}

/**
 * The capabilities an orchestrator-native tool handler may use. Deliberately
 * small: it exposes the repositories the lightweight tools persist to, the event
 * bus, and the few run-lifecycle hooks (active check, status flip, pausing for a
 * user answer) that ask_user_question needs — nothing more.
 */
export interface OrchestratorToolContext {
  runRepo: IRunRepository;
  planRepo: IPlanRepository;
  memoryRepo: IMemoryRepository;
  eventBus: typeof eventBus;
  mcpManager?: McpClientManager;
  pluginRegistry?: PluginRegistry;
  pluginHookRunner?: PluginHookRunner;
  /** Skills discovered for the active drive() (empty outside a run). */
  getSkills(): DiscoveredSkill[];
  /** Pauses the run and waits for the user's answer (QuestionCoordinator). */
  requestUserAnswer(runId: string, questions: UserQuestion[]): Promise<QuestionAnswerInput>;
  /** Whether the run is still active (not cancelled). */
  isActive(runId: string): boolean;
  /** Flips the run back to the generating state (after a paused question). */
  setGenerating(runId: string): Promise<void>;
}

/**
 * One orchestrator-native tool: a tool the orchestrator executes itself (no
 * filesystem/network I/O, no permission gating). Bundles its advertised schema,
 * a per-mode availability rule, and its execution — so adding a tool is a single
 * new module registered in tools/index.ts, instead of edits scattered across
 * drive() and runToolCall().
 */
export interface OrchestratorTool {
  /** The schema advertised to the model (from workspaceTools.ts). */
  schema: ToolDef;
  /** Whether this tool is offered to the main agent under the given mode. */
  isAvailable(strategy: ModeStrategy): boolean;
  /** Executes the call, returning the JSON tool-result string. Never throws. */
  execute(ctx: OrchestratorToolContext, runId: string, run: Run, toolCall: any): Promise<string>;
}
