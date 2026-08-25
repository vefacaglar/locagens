import type { ModeStrategy, ToolDef } from "../prompts/index.js";
import type { OrchestratorTool } from "./types.js";
import { updatePlanTool } from "./updatePlan.js";
import { setChatTitleTool } from "./setChatTitle.js";
import { askUserQuestionTool } from "./askUserQuestion.js";
import { rememberTool } from "./remember.js";
import { loadSkillTool } from "./loadSkill.js";

export type { OrchestratorTool, OrchestratorToolContext, QuestionAnswerInput } from "./types.js";

/**
 * The orchestrator-native tools, in advertised order. update_plan leads (it used
 * to ride with plan mode's base tools), followed by the always-available tools.
 * Adding a tool = one new module appended here.
 */
export const ORCHESTRATOR_TOOLS: OrchestratorTool[] = [
  updatePlanTool,
  setChatTitleTool,
  askUserQuestionTool,
  rememberTool,
  loadSkillTool
];

const BY_NAME = new Map<string, OrchestratorTool>(
  ORCHESTRATOR_TOOLS.map(t => [t.schema.function.name, t])
);

/** The handler for a tool name, or undefined if it is not an orchestrator tool. */
export function getOrchestratorTool(name?: string): OrchestratorTool | undefined {
  return name ? BY_NAME.get(name) : undefined;
}

/** The schemas of the orchestrator tools offered to the main agent in this mode. */
export function availableSchemas(strategy: ModeStrategy): ToolDef[] {
  return ORCHESTRATOR_TOOLS.filter(t => t.isAvailable(strategy)).map(t => t.schema);
}
