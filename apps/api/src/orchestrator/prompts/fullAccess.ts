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
  subAgentTypes: ["explore", "general"],
  allowsPlanTool: false,
  bypassDangerousGating: true,
  gatesEveryTool: false,
  selectBaseTools: (delegating) => buildModeBaseTools(delegating),
  promptSection(): string {
    return `\n\nCURRENT OPERATIONAL MODE: FULL ACCESS MODE
- You are implementing autonomously. Directly create/edit/delete files with workspace tools.
- run_command (including downloads that declare network_domains), search_web, and fetch_url run WITHOUT user approval in this mode. Do not ask the user for permission to download or run them — just do it.
- Commands still run inside the OS sandbox: a command can only reach the exact hosts it lists in network_domains, so declare every host a download needs.
- Do NOT call update_plan here — write any plan as plain text in your reply (or a <task_list> for complex work), then implement.
- All file access is confined to the project workspace; you cannot operate outside the project folder.
- Stay within the user's intent: do not take destructive or irreversible actions beyond the requested task.`;
  }
};
