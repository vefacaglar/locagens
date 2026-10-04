import type { ModeStrategy } from "./types.js";
import { buildModeBaseTools } from "./shared.js";

/**
 * Build mode (the `accept_edits` backend mode shown in the UI): the model
 * implements directly with workspace tools; ordinary approved edits do not ask,
 * but dangerous tools (run_command/search_web/fetch_url) still gate. This is also the
 * default for unknown/legacy modes.
 */
export const buildStrategy: ModeStrategy = {
  mode: "accept_edits",
  lightweight: false,
  allowsMutation: true,
  allowsDelegation: true,
  subAgentTypes: ["explore", "general"],
  allowsPlanTool: false,
  bypassDangerousGating: false,
  gatesEveryTool: false,
  selectBaseTools: (delegating) => buildModeBaseTools(delegating),
  promptSection(): string {
    return `\n\nCURRENT OPERATIONAL MODE: BUILD MODE
- You are implementing, not just planning. Directly create/edit/delete files with workspace tools; do not ask before ordinary approved edits.
- Do not just announce that you will start ("I'll begin now") and then end your turn — make the actual tool call (e.g. delegate_tasks, or a workspace tool) in the SAME message. End a turn with no tool call only when the work is genuinely finished.
- Do NOT call update_plan here — the stable plan panel belongs to Plan mode only.
- For any multi-step or multi-file work, keep a live <task_list> and re-output it each reply, marking steps done. If an APPROVED PLAN is provided above, seed the task_list from its steps. Skip the list only for a trivial one-step fix.
- Example task_list format:
<task_list>
- [x] Inspect the existing config
- [ ] Add the new endpoint
- [ ] Run the tests
</task_list>`;
  }
};
