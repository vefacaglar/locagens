import type { SubAgentType } from "../workspaceTools.js";

/**
 * One advertised tool schema. Kept structurally loose (the various tools have
 * heterogeneous `parameters` shapes) — the orchestrator forwards these to the
 * provider as an opaque list, matching the previously untyped tool arrays.
 */
export interface ToolDef {
  type: "function";
  function: {
    name: string;
    description: string;
    parameters: unknown;
  };
}

/**
 * Dual-model context: when a coder model is wired up the main model becomes an
 * architect that delegates implementation. Carried into the prompt so the
 * architect instructions (and the optional utility tier) can be rendered.
 */
export interface DelegationContext {
  coderModel: string;
  maxSubAgents: number;
  // When set, an optional cheap "utility" tier is available via delegate_to_utility.
  utilityModel?: string;
}

/** Everything a mode strategy needs to render its prompt section. */
export interface PromptContext {
  projectName?: string;
  projectPath?: string;
  shouldReadProjectGuidance: boolean;
  delegation?: DelegationContext;
  memoryContext: string;
}

/**
 * A single mode's complete behavior: the prompt text it contributes, the base
 * tool set it exposes, and the gating policy the orchestrator enforces for it.
 * Selecting a mode = picking one of these (see getModeStrategy). All per-mode
 * `run.mode === ...` branching across the orchestrator reads these instead.
 */
export interface ModeStrategy {
  mode: string;
  /**
   * True for the lightweight chat shell: a short standalone prompt with no global
   * rules / tool catalog / planning scaffolding. When set, promptSection returns
   * the ENTIRE prompt and buildSystemPrompt does not wrap it.
   */
  lightweight: boolean;
  /** The mode-specific section appended after the global rules (or, for a
   * lightweight strategy, the entire standalone prompt). */
  promptSection(ctx: PromptContext): string;
  /** Whether this mode permits file mutation / command execution (build-type). */
  allowsMutation: boolean;
  /** Whether delegate_tasks / delegate_to_utility may be offered in this mode. */
  allowsDelegation: boolean;
  /**
   * spawn_agents sub-agent types a single-model run may launch in this mode:
   * build-type modes get explore + general, plan mode explore only (read-only),
   * chat none. Preset (architect) runs use delegate_tasks instead.
   */
  subAgentTypes: SubAgentType[];
  /** Whether update_plan is offered and accepted in this mode (plan only). */
  allowsPlanTool: boolean;
  /** Whether dangerous tools (run_command/search_web/fetch_url) skip gating (full access). */
  bypassDangerousGating: boolean;
  /** Whether every tool call must be gated behind approval (ask_permissions). */
  gatesEveryTool: boolean;
  /**
   * The base workspace tools for this mode, before delegation / shared tools are
   * appended. `delegating` = a coder model is configured for the run (the
   * architect is then held to read-only tools so it can only delegate).
   */
  selectBaseTools(delegating: boolean): ToolDef[];
}
