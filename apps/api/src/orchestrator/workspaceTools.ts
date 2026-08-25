/**
 * Barrel for the workspace tool layer, split into focused modules under
 * ./workspace/:
 *
 *   toolSchemas.ts        tool definitions advertised to the model + gating sets
 *   pathGuards.ts         workspace confinement + shared arg/output guards
 *   fileToolExecutor.ts   synchronous filesystem tool execution
 *   networkTools.ts       run_command / fetch_url / search_web + async entry point
 *   permissionPreview.ts  grant scoping, escape heuristics, permission-card preview
 *
 * Import from here — the public surface is unchanged from when this was one file.
 */
export {
  WORKSPACE_TOOLS,
  UPDATE_PLAN_TOOL,
  SET_TITLE_TOOL,
  ASK_QUESTION_TOOL,
  REMEMBER_TOOL,
  LOAD_SKILL_TOOL,
  DELEGATE_TASKS_TOOL,
  DELEGATE_UTILITY_TOOL,
  DANGEROUS_TOOLS,
  READONLY_TOOLS,
  MODIFYING_TOOLS,
  UTILITY_TOOL_NAMES,
  UTILITY_TOOLS
} from "./workspace/toolSchemas.js";
export {
  permissionKey,
  commandEscapesWorkspace,
  commandScansOutsideWorkspace,
  buildPermissionPreview
} from "./workspace/permissionPreview.js";
export { executeWorkspaceTool } from "./workspace/fileToolExecutor.js";
export { executeWorkspaceToolAsync } from "./workspace/networkTools.js";
