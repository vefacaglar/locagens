import type { ModeStrategy } from "./types.js";
import { buildModeBaseTools } from "./shared.js";

/**
 * Ask-permissions mode: every tool call is gated behind an explicit approval
 * (not just the inherently dangerous ones). A valid backend mode (older
 * persisted runs and the permission flow) though no longer offered in the UI.
 */
export const askPermissionsStrategy: ModeStrategy = {
  mode: "ask_permissions",
  lightweight: false,
  allowsMutation: true,
  allowsDelegation: true,
  subAgentTypes: ["explore", "general"],
  allowsPlanTool: false,
  bypassDangerousGating: false,
  gatesEveryTool: true,
  selectBaseTools: (delegating) => buildModeBaseTools(delegating),
  promptSection(): string {
    return `\n\nCURRENT OPERATIONAL MODE: ASK PERMISSIONS MODE
- Call the matching tool for inspection or changes. Do not ask for permission in plain text; the app intercepts tool calls and shows approvals.`;
  }
};
