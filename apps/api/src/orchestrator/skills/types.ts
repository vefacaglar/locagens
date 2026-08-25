import type { SkillSource } from "@locagens/shared";

/** A skill discovered on disk (full body kept for load_skill). */
export interface DiscoveredSkill {
  name: string;
  description: string;
  source: SkillSource;
  /** Absolute directory containing SKILL.md. */
  dir: string;
  /** Full markdown body after the frontmatter. */
  body: string;
}
