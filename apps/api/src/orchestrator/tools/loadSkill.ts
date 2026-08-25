import { LOAD_SKILL_TOOL } from "../workspaceTools.js";
import { findSkill } from "../skills/index.js";
import type { OrchestratorTool } from "./types.js";

/**
 * load_skill: returns the full SKILL.md body for a discovered skill. Silent —
 * local allowlisted disk read only, no permission prompt. Available to the main
 * agent in every mode. Never throws.
 */
export const loadSkillTool: OrchestratorTool = {
  schema: LOAD_SKILL_TOOL,
  isAvailable: () => true,
  async execute(ctx, _runId, _run, toolCall) {
    try {
      const args = JSON.parse(toolCall.function.arguments || "{}");
      const name = typeof args.name === "string" ? args.name.trim() : "";
      if (!name) {
        return JSON.stringify({ success: false, error: "Missing skill name." });
      }

      const skills = ctx.getSkills();
      const skill = findSkill(skills, name);
      if (!skill) {
        const available = skills.map(s => s.name).sort();
        return JSON.stringify({
          success: false,
          error: `Unknown skill "${name}".`,
          available
        });
      }

      return JSON.stringify({
        success: true,
        name: skill.name,
        description: skill.description,
        source: skill.source,
        content: skill.body
      });
    } catch (err: any) {
      return JSON.stringify({ success: false, error: err?.message ?? "Failed to load skill." });
    }
  }
};
