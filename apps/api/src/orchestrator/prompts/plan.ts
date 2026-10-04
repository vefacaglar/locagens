import type { ModeStrategy } from "./types.js";
import { PLAN_TOOLS } from "./shared.js";

/**
 * Plan mode is planning-only: read-only inspection plus the stable plan-panel
 * tool (update_plan), but no file mutation or commands — even with approval. It
 * is the only mode that offers/accepts update_plan.
 */
export const planStrategy: ModeStrategy = {
  mode: "plan",
  lightweight: false,
  allowsMutation: false,
  allowsDelegation: false,
  subAgentTypes: ["explore"],
  allowsPlanTool: true,
  bypassDangerousGating: false,
  gatesEveryTool: false,
  selectBaseTools: () => [...PLAN_TOOLS],
  promptSection(): string {
    return `\n\nCURRENT OPERATIONAL MODE: PLAN MODE
- Plan only: discuss architecture, decisions, and steps.
- Do NOT implement. Do NOT write production code, patches, diffs, full file contents, or copy-pastable fenced code blocks.
- DO NOT call any file-mutating tools ('write_file', 'edit_file', 'delete_file', 'create_directory', 'move_file') or 'run_command'. Read-only inspection is fine.
- If asked to implement, ask the user to switch to Build mode.
- If a plan was rejected, do not treat repeated or pasted plan text as approval.
- Use update_plan once for the stable plan panel; revise it only when the user asks. Do NOT output a <plan> block in plan mode. Keep the chat brief, using a <task_list> only if the task is complex.
- You MAY use ask_user_question when you are genuinely blocked on a planning decision only the user can make (e.g. choosing between two approaches, or a missing requirement). Asking real questions is encouraged when it makes the plan better.
- But do NOT ask execution / permission yes-no questions like "should I run this command?" or "should I edit this file?" — plan mode never executes or edits anything, so there is nothing to approve.
- The plan panel already gives the user the approve/revise/reject controls (a "Start building" button). After calling update_plan, do NOT ask for plan approval or whether to start building / switch to Build mode — end your turn and let the user approve via the panel.`;
  }
};
