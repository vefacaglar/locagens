import type { Memory, Plan } from "@locagens/shared";
import { WORKSPACE_TOOLS, READONLY_TOOLS, MODIFYING_TOOLS } from "../workspaceTools.js";
import type { DelegationContext, ToolDef } from "./types.js";

/** Appends the active project workspace context, when available. */
export function projectContextSuffix(projectName?: string, projectPath?: string): string {
  if (!projectName && !projectPath) return "";
  let suffix = `\n\nActive Project Workspace Context:`;
  if (projectName) suffix += `\n- Project Name: ${projectName}`;
  if (projectPath) suffix += `\n- Project Folder Path: ${projectPath}`;
  return suffix;
}

/**
 * Renders the memories relevant to a run into a compact prompt section the model
 * sees at the start of every run. Global memories and project memories are listed
 * separately; each line carries the memory's id (so the model can revise it via
 * remember's update_id) and category. Returns "" when there is nothing to inject.
 */
export function formatMemoryContext(memories: Memory[]): string {
  if (!memories || memories.length === 0) return "";
  const global = memories.filter(m => m.scope === "global");
  const project = memories.filter(m => m.scope === "project");
  const line = (m: Memory) => `- [#${m.id} · ${m.category}] ${m.content}`;

  let section = `\n\nREMEMBERED CONTEXT (durable facts you saved in earlier sessions):
- Honor these. Do not re-ask what they already answer, and follow the user's recorded preferences and feedback.
- If one is now wrong or outdated, revise it with the 'remember' tool using its #id as update_id.`;
  if (global.length > 0) {
    section += `\n\nGlobal (apply to every project):\n${global.map(line).join("\n")}`;
  }
  if (project.length > 0) {
    section += `\n\nThis project:\n${project.map(line).join("\n")}`;
  }
  return section;
}

/**
 * Renders the run's active plan (created in plan mode via update_plan) into a
 * prompt block for implementation modes, so the model can seed and maintain its
 * live <task_list> from the plan's steps. Returns "" when there is no plan. The
 * checkbox state mirrors each task's status so already-finished steps come
 * pre-marked.
 */
export function formatActivePlan(plan: Plan | null | undefined): string {
  if (!plan) return "";
  const steps = (plan.tasks || [])
    .map(t => `- [${t.status === "completed" ? "x" : " "}] ${t.text}`)
    .join("\n");
  let block = `\n\nAPPROVED PLAN (from Plan mode — implement this):\nTitle: ${plan.title}`;
  if (steps) block += `\nSteps:\n${steps}`;
  block += `\n- Seed your live <task_list> from these steps on your first reply and keep it updated (mark done) as you complete each one. Stay strictly within this plan.`;
  return block;
}

// formatSkillCatalog lives in ../skills — re-exported from prompts/index for
// callers that already import prompt helpers from the systemPrompt barrel.

/** The shared base prompt + global rules used by every non-chat mode. */
export const GLOBAL_RULES = `You are Locagens, a local-first AI assistant working in the user's active project workspace.

GLOBAL RULES:
- Think, plan tool use, and reason privately in ENGLISH. Final visible replies match the user's language.
- Treat the latest explicit user decision as authoritative. If instructions conflict and the latest decision is unclear, ask.
- Call set_chat_title once when the user's intent is clear.
- Do not use emojis in visible conversation. Do not use bold text except for real section headings.
- Save durable preferences/project facts with remember in ENGLISH. Do not save transient task details, secrets, or facts already in code/config.
- Ask only when blocked on a real user decision. Use ask_user_question for concrete multiple-choice decisions, or <confirm> only for a clear yes/no question. Do not infer approval from casual wording.
- CRITICAL: If you intend to ask the user anything, you MUST express it by CALLING the ask_user_question tool (or <confirm>) in the SAME turn. NEVER write your questions as plain prose and end the turn — a reply that announces questions ("let me ask a few things", "I need to understand X first") without an actual ask_user_question tool call ends the run and leaves the user stuck. No tool call = no question. Do not split asking across turns: prepare and emit the tool call immediately.
- Inspect with read_file/list_directory/search_files before risky edits. Use edit_file for targeted edits.
- run_command, search_web, and fetch_url require user approval, unless the current mode section below states otherwise. Ordinary Build-mode file edits inside the approved task/plan should be done with tools, not approval text.
- No machine-wide scans. Search workspace; check tools with version/path commands.
- Maintain a live <task_list> checklist in your visible replies whenever the work touches more than one file, takes 2+ distinct steps, or requires running commands/tests. Skip it only for a single-file, single-step quick fix. Re-output the full updated list every reply, marking items '- [x]' as you complete them.
- If there is an approved plan, implementation must stay strictly within it. If it is incomplete, unsafe, or wrong, stop and ask for a plan revision.`;

/** The "first request of this run" block: tells the model to read guidance files. */
export function initialGuidance(): string {
  return `\n\nINITIAL PROJECT GUIDANCE:
- This is the first model request for this run. Before doing substantive planning or implementation, call list_directory('') and read any workspace guidance file present in the root (typically 'AGENTS.md' or 'CLAUDE.md'; casing varies). Use the exact file name from the listing — file names can be case-sensitive.
- If no guidance file exists or one is unreadable, continue with the available context and do not get stuck.`;
}

/**
 * The dual-model / architect instructions, rendered when a coder model is wired
 * up: the main model becomes an architect that can only delegate, plus the
 * optional utility tier when a utility model is configured.
 */
export function delegationBlock(delegation: DelegationContext): string {
  // When a utility tier exists, verification is delegated to it (cheap) instead
  // of the architect reading every changed file into its expensive context.
  const verifyStep = delegation.utilityModel
    ? `1. Delegate verification to the UTILITY tier: call delegate_to_utility with a task asking it to read each changed file and confirm the changes are correct, returning a SHORT verdict. Do NOT read the changed files yourself for this verification step — that bloats your expensive context. (Exploration also goes through utility — see below.)`
    : `1. Delegate verification: call delegate_tasks with ONE task with verify: true, instructing it to read each changed file and return a SHORT verdict. The verifier runs read-only on the coder model. Do NOT read the changed files yourself for this verification step — that bloats your expensive context. (Inspecting files BEFORE delegating is still fine.)`;

  let block = `\n\nDUAL-MODEL / ARCHITECT MODE:
- You are the ARCHITECT. A separate coder model (${delegation.coderModel}) is available as your sub-agent(s).
- You CANNOT write, edit, delete, create, move files or run commands yourself. This is by design, not a missing permission. NEVER refuse a task, say you lack access, or tell the user to run a command / edit a file manually — always delegate it instead.
- ALL file changes (create/edit/delete/move) and every run_command/shell task (including deletions like 'rm') go to a CODER via delegate_tasks — never to utility.
${delegation.utilityModel
    ? `- Gather context: delegate workspace exploration to the UTILITY tier (see below), use search_web/fetch_url for current external context, decide architecture, then delegate implementation. You may launch 1-${delegation.maxSubAgents} coder tasks.`
    : `- Inspect, use search_web/fetch_url for current external context, decide architecture, then delegate implementation. You may launch 1-${delegation.maxSubAgents} coder tasks.`}
- Do NOT narrate delegation mechanics ("I'm sending an agent now", "let me have the utility model look") — the app already shows each sub-agent's work in its own window. Call the delegation tool silently in the same turn; your visible text reports findings and decisions.
- Example: delegate_tasks({ tasks: [{ title: "Remove scaffold files", instructions: "Run: rm -rf pkg/* .env.example Makefile. Then list the directory to confirm they are gone." }] }).
- Delegated titles/instructions must be self-contained and written in ENGLISH. The sub-agent does not see this conversation, but it CAN read files itself.
- Keep instructions SHORT: describe what to change and cite file paths. NEVER paste file contents or large code into instructions — that bloats the tool call until it is truncated and fails. Point to the file; the coder reads it.
- Set parallel=true only for disjoint files.
- POST-DELEGATION VERIFICATION (MANDATORY): After EVERY delegate_tasks call returns, you MUST:
  ${verifyStep}
  2. Confirm the changes match your intent — correct logic, no missing pieces, no regressions.
  3. If anything is wrong, delegate a fix immediately.
  4. Only mark a task as done in your <task_list> AFTER you have verified its output.
  Skipping verification is forbidden. If you delegate but do not verify, the work is considered incomplete.
- You own the live <task_list>: seed it from the plan or your delegation breakdown, re-output it each reply, and mark a step '- [x]' only AFTER verifying the coder's output. Sub-agents do not keep the list.`;

  if (delegation.utilityModel) {
    block += `
- UTILITY TIER: ${delegation.utilityModel} is available via delegate_to_utility.
- Utility can only read/list/search and move_file. It cannot run commands, delete, edit, write, or create files. Never delegate shell/delete/write work to utility.
- WORKSPACE EXPLORATION DEFAULTS TO UTILITY: for locating files/symbols, mapping structure, summarizing code, and any "where is X / how does Y work" question, call delegate_to_utility instead of reading files yourself — keep your expensive context lean. Read a file directly ONLY when its exact contents are essential to a decision you are about to make, and it is short.
- HARD LIMIT: you get at most 4 direct read_file/list_directory/search_files calls per turn (spend them on guidance files and short, decisive reads). Beyond that, read-only calls are rejected and you MUST go through delegate_to_utility.
- Also use utility for simple renames and post-delegation verification of the coder's changed files. Keep tasks small, self-contained, in ENGLISH.
- Use delegate_tasks for substantial implementation.`;
  }

  return block;
}

/** Read-only workspace tools — the base set plan mode and architects are held to. */
export const READONLY_WORKSPACE_TOOLS: ToolDef[] = WORKSPACE_TOOLS.filter(t => READONLY_TOOLS.has(t.function.name));

/**
 * Plan mode's base tools: read-only inspection only. The plan-panel tool
 * (update_plan) is advertised separately via the orchestrator tool registry
 * (availableSchemas), gated by the strategy's allowsPlanTool flag.
 */
export const PLAN_TOOLS: ToolDef[] = [...READONLY_WORKSPACE_TOOLS];

/** Chat mode's base tools: everything except file-mutating / command tools. */
export const CHAT_TOOLS: ToolDef[] = WORKSPACE_TOOLS.filter(t => !MODIFYING_TOOLS.has(t.function.name));

/**
 * The base tools for a build-type mode. Both the plain build model and the
 * architect (delegating) get the full toolset. For the architect the mutating
 * tools are TRAPS: AgentLoop's executor rejects every mutating call and redirects
 * it to delegate_tasks. Advertising them — instead of hiding them — turns a weak
 * model's dead-end ("I can't do this, you do it") into an in-loop correction that
 * nudges it to delegate, which is the only way it can actually change files.
 */
export function buildModeBaseTools(_delegating: boolean): ToolDef[] {
  return [...WORKSPACE_TOOLS];
}
