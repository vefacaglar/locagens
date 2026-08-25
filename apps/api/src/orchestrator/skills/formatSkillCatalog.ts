import type { SkillSummary } from "@locagens/shared";

/**
 * Renders the AVAILABLE SKILLS block for the main-agent system prompt.
 * Only name + description (progressive disclosure); full body comes from load_skill.
 */
export function formatSkillCatalog(skills: SkillSummary[]): string {
  if (!skills || skills.length === 0) return "";

  const lines = skills.map(s => {
    const src = s.source === "project" ? "project" : "user";
    return `- ${s.name} (${src}): ${s.description}`;
  });

  return `\n\nAVAILABLE SKILLS:
- Specialized instruction packs. When the user task matches a skill's description, call load_skill with that name BEFORE improvising the workflow.
- load_skill returns the full skill body; follow it. Do not invent conflicting steps.
- If no skill matches, ignore this list and continue normally.

${lines.join("\n")}`;
}
