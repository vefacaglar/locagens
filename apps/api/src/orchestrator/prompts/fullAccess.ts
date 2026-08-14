import type { ModeStrategy } from "./types.js";
import { buildModeBaseTools } from "./shared.js";

/**
 * Full access mode: fully autonomous inside the fixed security boundary. Most
 * dangerous-tool prompts are skipped, but fresh command egress still gates.
 * A valid backend mode (not offered in the UI mode picker).
 */
export const fullAccessStrategy: ModeStrategy = {
  mode: "full_access",
  lightweight: false,
  allowsMutation: true,
  allowsDelegation: true,
  allowsPlanTool: false,
  bypassDangerousGating: true,
  gatesEveryTool: false,
  selectBaseTools: (delegating) => buildModeBaseTools(delegating),
  promptSection(): string {
    return `\n\nCURRENT OPERATIONAL MODE: FULL ACCESS MODE
- You are implementing autonomously. Directly create/edit/delete files with workspace tools. Commands still run inside the OS sandbox, and any run_command network_domains that lack an exact standing grant require approval.
- Network-free run_command calls, search_web, and fetch_url do not require user approval in this mode. A new run_command domain set still does.
- Do NOT call update_plan here — write any plan as plain text in your reply (or a <task_list> for complex work), then implement.
- All file access is confined to the project workspace; you cannot operate outside the project folder.
- Stay within the user's intent: do not take destructive or irreversible actions beyond the requested task.`;
  }
};
