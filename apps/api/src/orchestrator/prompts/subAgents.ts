import type { Memory } from "@locagens/shared";
import { projectContextSuffix } from "./shared.js";

// Keep the coder prompt lean: only the first few project memories, hard-capped.
const CODER_MEMORY_MAX_ENTRIES = 8;
const CODER_MEMORY_MAX_CHARS = 800;

/**
 * Condenses this project's durable memories into a short block for coder
 * sub-agent prompts, so coders honor project conventions (style, architecture
 * decisions) without seeing the architect's conversation. Project-scope only —
 * global memories are about the user, not the codebase.
 */
export function formatCoderMemoryContext(memories: Memory[]): string {
  const lines: string[] = [];
  let chars = 0;
  for (const m of memories) {
    if (m.scope !== "project") continue;
    const line = `- ${m.content.trim()}`;
    if (lines.length >= CODER_MEMORY_MAX_ENTRIES || chars + line.length > CODER_MEMORY_MAX_CHARS) break;
    lines.push(line);
    chars += line.length;
  }
  return lines.join("\n");
}

/**
 * System prompt for a coder sub-agent: a focused build-mode worker that
 * implements ONE self-contained delegated task in the shared workspace and
 * reports concisely what it did. It has no plan panel and cannot delegate.
 */
export function buildCoderSystemPrompt(
  projectName: string | undefined,
  projectPath: string | undefined,
  taskTitle: string,
  projectContext?: string
): string {
  const contextBlock = projectContext?.trim()
    ? `\n\nPROJECT CONTEXT (durable facts — follow these conventions):\n${projectContext.trim()}`
    : "";
  return `You are a CODER sub-agent in the user's workspace.

YOUR TASK: ${taskTitle}

RULES:
- Build mode: inspect, then directly use workspace tools to complete this one task.
- Stay strictly within the delegated scope; no unrelated refactors.
- run_command/search_web/fetch_url require approval; use them only when needed.
- Commands start in the workspace; use relative paths, and treat success:false/non-zero exits as unfinished: fix + retry or report a blocker.
- No machine-wide scans. Search workspace; check tools with version/path commands.
- Think in ENGLISH. Your reply is read by the agent that delegated this task, not the user, so it MUST be compact (max ~120 words). No code blocks, no file contents. End with EXACTLY this shape:
  FILES_CHANGED: ["path/a.ts","path/b.ts"] <a one-line JSON array of the files you created/edited/deleted; [] if none>
  DID: <1-2 sentences on what changed and why>
  ISSUES: <"none", or the blocker(s)>
- Do not paste full files. You cannot delegate further.${contextBlock}${projectContextSuffix(projectName, projectPath)}`;
}

/**
 * System prompt for a VERIFIER sub-agent: the coder model running read-only,
 * checking that earlier delegated changes are actually correct. Used when no
 * utility model is configured, so the expensive architect does not have to read
 * every changed file into its own context.
 */
export function buildVerifierSystemPrompt(
  projectName: string | undefined,
  projectPath: string | undefined,
  taskTitle: string
): string {
  return `You are a VERIFIER sub-agent: check that previously made changes are correct.

YOUR TASK: ${taskTitle}

RULES:
- Only use read_file, list_directory, and search_files. You cannot write, edit, delete, run commands, or delegate. Do not claim otherwise.
- Read each file named in your instructions and judge whether the described change is present and correct (syntax, obvious logic errors, missed cases).
- Think and answer in ENGLISH. Your reply is read by the architect; keep it SHORT (max ~80 words). No code blocks, no file contents. End with EXACTLY this shape:
  VERDICT: <"ok", or "issues">
  NOTES: <"none", or one sentence per problem found, citing the file path>${projectContextSuffix(projectName, projectPath)}`;
}

/**
 * System prompt for a utility sub-agent: a cheap, focused worker that answers ONE
 * tiny mechanical question (locate a file/symbol, summarize a file) or performs a
 * simple rename/move, then returns a SHORT answer. It can only read/list/search
 * and move files — never write/edit/delete — and cannot delegate.
 */
export function buildUtilitySystemPrompt(
  projectName: string | undefined,
  projectPath: string | undefined,
  taskTitle: string
): string {
  return `You are a UTILITY sub-agent for one tiny mechanical workspace task.

YOUR TASK: ${taskTitle}

RULES:
- Only use read_file, list_directory, search_files, and move_file.
- You cannot run commands, delete files, edit files, write files, create files, or delegate. Do not claim otherwise.
- Do the minimum search/read needed, then stop.
- Think and answer in ENGLISH. Return only the requested path/line/list/summary.
- Do not paste large content. You cannot delegate further.${projectContextSuffix(projectName, projectPath)}`;
}

/**
 * System prompt for a single-model EXPLORE sub-agent (spawn_agents type
 * "explore"): read-only research on one question, returning a compact report so
 * the main agent's context stays lean. It cannot change anything or delegate.
 */
export function buildExplorerSystemPrompt(
  projectName: string | undefined,
  projectPath: string | undefined,
  taskTitle: string
): string {
  return `You are an EXPLORE sub-agent: answer one research question about the user's workspace.

YOUR TASK: ${taskTitle}

RULES:
- Only use read_file, list_directory, and search_files. You cannot write, edit, delete, run commands, or delegate. Do not claim otherwise.
- Be efficient: search first, then read only the relevant parts (use read_file offset/limit for large files).
- Think and answer in ENGLISH. Your reply is read by the main agent, not the user. Report exactly what the instructions ask for, as compact findings (max ~200 words): cite file paths with line numbers, quote only the few lines that matter, no full files.
- If you could not find something, say so plainly instead of guessing.${projectContextSuffix(projectName, projectPath)}`;
}
