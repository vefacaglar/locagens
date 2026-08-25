/**
 * Backward-compatible barrel. The system-prompt building and per-mode behavior
 * now live in ./prompts/ as ModeStrategy modules selected by getModeStrategy.
 * This file re-exports the public surface so existing imports keep working.
 */
export type { DelegationContext, ModeStrategy, PromptContext, ToolDef, SystemPromptOptions } from "./prompts/index.js";
export {
  buildSystemPrompt,
  buildCoderSystemPrompt,
  buildUtilitySystemPrompt,
  buildVerifierSystemPrompt,
  formatCoderMemoryContext,
  formatMemoryContext,
  formatActivePlan,
  formatSkillCatalog,
  getModeStrategy
} from "./prompts/index.js";
