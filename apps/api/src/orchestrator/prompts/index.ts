import type { DelegationContext, ModeStrategy } from "./types.js";
import {
  GLOBAL_RULES,
  initialGuidance,
  delegationBlock,
  projectContextSuffix,
  formatMemoryContext
} from "./shared.js";
import { chatStrategy } from "./chat.js";
import { planStrategy } from "./plan.js";
import { buildStrategy } from "./build.js";
import { autoStrategy } from "./auto.js";
import { askPermissionsStrategy } from "./askPermissions.js";
import { fullAccessStrategy } from "./fullAccess.js";

export type { ModeStrategy, DelegationContext, PromptContext, ToolDef } from "./types.js";
export { formatMemoryContext, formatActivePlan } from "./shared.js";
export { formatSkillCatalog } from "../skills/index.js";
export { buildCoderSystemPrompt, buildUtilitySystemPrompt, buildVerifierSystemPrompt, formatCoderMemoryContext } from "./subAgents.js";

const STRATEGIES: Record<string, ModeStrategy> = {
  [chatStrategy.mode]: chatStrategy,
  [planStrategy.mode]: planStrategy,
  [buildStrategy.mode]: buildStrategy,
  [autoStrategy.mode]: autoStrategy,
  [askPermissionsStrategy.mode]: askPermissionsStrategy,
  [fullAccessStrategy.mode]: fullAccessStrategy
};

/**
 * Resolves the strategy for a run's mode. Unknown / legacy modes fall back to
 * the build (accept_edits) strategy — the same default the previous if/else
 * chain used (its final `else` branch was accept_edits).
 */
export function getModeStrategy(mode?: string): ModeStrategy {
  return STRATEGIES[mode ?? ""] ?? buildStrategy;
}

/** Everything buildSystemPrompt composes a prompt from. All fields optional. */
export interface SystemPromptOptions {
  projectName?: string;
  projectPath?: string;
  mode?: string;
  /** First model request of this run: tells the model to read guidance files. */
  shouldReadProjectGuidance?: boolean;
  /** Set when a coder model is configured — renders the architect instructions. */
  delegation?: DelegationContext;
  /** Pre-rendered REMEMBERED CONTEXT section (formatMemoryContext). */
  memoryContext?: string;
  /** Pre-rendered APPROVED PLAN section (formatActivePlan). */
  planContext?: string;
  /** Pre-rendered AVAILABLE SKILLS section (formatSkillCatalog). */
  skillCatalog?: string;
}

/**
 * Builds the system prompt for a single-model workspace chat session. Picks the
 * mode strategy, then composes the shared sections around its prompt block:
 * global rules -> initial guidance -> mode section -> delegation -> plan ->
 * memory -> skills -> project context. A lightweight strategy (chat) returns its
 * entire prompt standalone, so no wrapping is applied (skills still append).
 */
export function buildSystemPrompt(options: SystemPromptOptions = {}): string {
  const {
    projectName,
    projectPath,
    mode,
    shouldReadProjectGuidance = false,
    delegation,
    memoryContext = "",
    planContext = "",
    skillCatalog = ""
  } = options;
  const strategy = getModeStrategy(mode);
  const ctx = { projectName, projectPath, shouldReadProjectGuidance, delegation, memoryContext };

  if (strategy.lightweight) {
    return strategy.promptSection(ctx) + skillCatalog;
  }

  let prompt = GLOBAL_RULES;
  if (shouldReadProjectGuidance) prompt += initialGuidance();
  prompt += strategy.promptSection(ctx);
  // Architect (dual-model) instructions: when a coder model is wired up, the
  // main model acts as an architect and delegates the heavy code-writing.
  if (delegation) prompt += delegationBlock(delegation);
  prompt += planContext;
  prompt += memoryContext;
  prompt += skillCatalog;
  prompt += projectContextSuffix(projectName, projectPath);

  return prompt;
}
