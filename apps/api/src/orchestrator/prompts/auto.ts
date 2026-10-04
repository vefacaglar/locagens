import type { ModeStrategy } from "./types.js";
import { buildModeBaseTools } from "./shared.js";

/**
 * Auto/Build mode: autonomous build; the model performs needed edits directly.
 * Dangerous tools still follow permission gating. A valid backend mode (older
 * persisted runs) though no longer offered in the UI mode picker.
 */
export const autoStrategy: ModeStrategy = {
  mode: "auto",
  lightweight: false,
  allowsMutation: true,
  allowsDelegation: true,
  subAgentTypes: ["explore", "general"],
  allowsPlanTool: false,
  bypassDangerousGating: false,
  gatesEveryTool: false,
  selectBaseTools: (delegating) => buildModeBaseTools(delegating),
  promptSection(): string {
    return `\n\nCURRENT OPERATIONAL MODE: AUTO/BUILD MODE
- Directly perform needed workspace edits with tools. Dangerous tools still follow permission gating.`;
  }
};
