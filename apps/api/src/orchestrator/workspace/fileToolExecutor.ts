import fs from "node:fs";
import path from "node:path";
import type { Run, ToolCall } from "@locagens/shared";
import { WORKSPACE_TOOLS } from "./toolSchemas.js";
import { resolveInsideForMutation, resolveInsideForRead, requirePath } from "./pathGuards.js";

/**
 * Synchronous execution of the filesystem workspace tools. Network/shell tools
 * (run_command / fetch_url / search_web) live in networkTools.ts and must go
 * through executeWorkspaceToolAsync.
 */

/** Applies an exact-substring edit, validating uniqueness unless replace_all. */
export function applyEdit(content: string, oldString: string, newString: string, replaceAll: boolean): string {
  if (oldString === "") {
    throw new Error("Missing parameter: old_string must not be empty.");
  }
  const occurrences = content.split(oldString).length - 1;
  if (occurrences === 0) {
    throw new Error("old_string was not found in the file. read_file it first and copy the exact text to replace, including whitespace and indentation.");
  }
  if (occurrences > 1 && !replaceAll) {
    throw new Error(`old_string is not unique (found ${occurrences} times). Provide more surrounding context or set replace_all to true.`);
  }
  return replaceAll ? content.split(oldString).join(newString) : content.replace(oldString, newString);
}

/** Recursively collects files matching the query by name or content. */
function searchWorkspace(rootDir: string, baseDir: string, query: string): Array<{ path: string; line?: number; preview?: string }> {
  const results: Array<{ path: string; line?: number; preview?: string }> = [];
  const lowerQuery = query.toLowerCase();
  const SKIP_DIRS = new Set(["node_modules", ".git", "dist", "build", ".next"]);
  const MAX_RESULTS = 100;

  const walk = (dir: string) => {
    if (results.length >= MAX_RESULTS) return;
    let entries: fs.Dirent[];
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      if (results.length >= MAX_RESULTS) return;
      const absChild = path.join(dir, entry.name);
      const relChild = path.relative(baseDir, absChild);
      if (entry.isDirectory()) {
        if (SKIP_DIRS.has(entry.name)) continue;
        walk(absChild);
      } else if (entry.isFile()) {
        if (entry.name.toLowerCase().includes(lowerQuery)) {
          results.push({ path: relChild });
          continue;
        }
        let content: string;
        try {
          const stat = fs.statSync(absChild);
          if (stat.size > 2_000_000) continue; // skip very large/binary files
          content = fs.readFileSync(absChild, "utf-8");
        } catch {
          continue;
        }
        const lines = content.split("\n");
        for (let i = 0; i < lines.length; i++) {
          if (lines[i].toLowerCase().includes(lowerQuery)) {
            results.push({ path: relChild, line: i + 1, preview: lines[i].trim().slice(0, 200) });
            break; // one hit per file is enough for an overview
          }
        }
      }
    }
  };

  walk(rootDir);
  return results;
}

// read_file paging caps. Without them a single read of a lockfile or a minified
// bundle blows up the model's context window in one call.
const READ_FILE_MAX_LINES = 2_000;
const READ_FILE_MAX_CHARS = 60_000;

function writeTextNoFollow(filePath: string, content: string): void {
  const flags = fs.constants.O_WRONLY | fs.constants.O_CREAT | fs.constants.O_TRUNC |
    (fs.constants.O_NOFOLLOW ?? 0);
  const fd = fs.openSync(filePath, flags, 0o644);
  try {
    fs.writeFileSync(fd, content, "utf-8");
  } finally {
    fs.closeSync(fd);
  }
}

/**
 * Slices a file's content to the requested offset/limit window, capped at
 * READ_FILE_MAX_LINES / READ_FILE_MAX_CHARS. When the window does not cover the
 * whole file, the result carries total_lines, the returned range, and a notice
 * telling the model how to page for the rest.
 */
function readFileWindow(full: string, args: any): Record<string, unknown> {
  const lines = full.split("\n");
  const offset = Math.max(1, Math.floor(Number(args.offset) || 1));
  const requested = Math.floor(Number(args.limit) || READ_FILE_MAX_LINES);
  const limit = Math.min(READ_FILE_MAX_LINES, Math.max(1, requested));

  const window = lines.slice(offset - 1, offset - 1 + limit);
  let content = window.join("\n");
  let charClipped = false;
  if (content.length > READ_FILE_MAX_CHARS) {
    content = content.slice(0, READ_FILE_MAX_CHARS);
    charClipped = true;
  }

  const endLine = charClipped
    ? offset - 1 + content.split("\n").length
    : offset - 1 + window.length;
  const coversWholeFile = offset === 1 && !charClipped && endLine >= lines.length;
  if (coversWholeFile) {
    return { success: true, content };
  }

  return {
    success: true,
    content,
    total_lines: lines.length,
    returned_lines: `${offset}-${endLine}`,
    notice:
      `File has ${lines.length} lines; this result covers lines ${offset}-${endLine}` +
      (charClipped ? ` (clipped at ${READ_FILE_MAX_CHARS} characters)` : "") +
      (endLine < lines.length
        ? `. Call read_file again with offset=${endLine + 1} to continue reading. Prefer search_files to locate the relevant section instead of paging through the whole file.`
        : ".")
  };
}

/**
 * Executes a single workspace tool call against the run's project directory.
 * All file access is constrained to the workspace; any failure is returned as
 * a JSON error string rather than thrown, so the model can react to it.
 */
export function executeWorkspaceTool(run: Run, toolCall: ToolCall): string {
  try {
    const args = JSON.parse(toolCall.function.arguments);
    const baseDir = path.resolve(run.projectPath || process.cwd());

    switch (toolCall.function.name) {
      case "write_file": {
        const absolutePath = resolveInsideForMutation(baseDir, requirePath(args.path));
        fs.mkdirSync(path.dirname(absolutePath), { recursive: true });
        writeTextNoFollow(absolutePath, args.content || "");
        return JSON.stringify({ success: true, message: `File written successfully at ${args.path}` });
      }
      case "edit_file": {
        const absolutePath = resolveInsideForMutation(baseDir, requirePath(args.path));
        if (!fs.existsSync(absolutePath) || !fs.statSync(absolutePath).isFile()) {
          return JSON.stringify({ success: false, error: `File not found at ${args.path}` });
        }
        const current = fs.readFileSync(absolutePath, "utf-8");
        const updated = applyEdit(current, args.old_string ?? "", args.new_string ?? "", !!args.replace_all);
        writeTextNoFollow(absolutePath, updated);
        return JSON.stringify({ success: true, message: `File edited successfully at ${args.path}` });
      }
      case "delete_file": {
        const absolutePath = resolveInsideForMutation(baseDir, requirePath(args.path));
        if (fs.existsSync(absolutePath)) {
          fs.unlinkSync(absolutePath);
          return JSON.stringify({ success: true, message: `File deleted successfully at ${args.path}` });
        }
        return JSON.stringify({ success: false, message: `File not found at ${args.path}` });
      }
      case "read_file": {
        const absolutePath = resolveInsideForRead(baseDir, requirePath(args.path));
        if (!fs.existsSync(absolutePath)) {
          return JSON.stringify({ success: false, error: `File not found at ${args.path}` });
        }
        if (fs.statSync(absolutePath).isDirectory()) {
          throw new Error(`Path '${args.path}' is a directory, not a file. Use list_directory instead.`);
        }
        return JSON.stringify(readFileWindow(fs.readFileSync(absolutePath, "utf-8"), args));
      }
      case "list_directory": {
        const absolutePath = resolveInsideForRead(baseDir, requirePath(args.path));
        if (!fs.existsSync(absolutePath)) {
          return JSON.stringify({ success: false, error: `Directory not found at ${args.path}` });
        }
        if (!fs.statSync(absolutePath).isDirectory()) {
          throw new Error(`Path '${args.path}' is a file, not a directory. Use read_file instead.`);
        }
        const files = fs.readdirSync(absolutePath).map(file => {
          const fileStat = fs.lstatSync(path.join(absolutePath, file));
          return { name: file, isDirectory: fileStat.isDirectory(), isSymbolicLink: fileStat.isSymbolicLink(), size: fileStat.size };
        });
        return JSON.stringify({ success: true, files });
      }
      case "create_directory": {
        const absolutePath = resolveInsideForMutation(baseDir, requirePath(args.path));
        fs.mkdirSync(absolutePath, { recursive: true });
        return JSON.stringify({ success: true, message: `Directory created at ${args.path}` });
      }
      case "move_file": {
        const sourceAbs = resolveInsideForMutation(baseDir, requirePath(args.source_path, "source_path"));
        const destAbs = resolveInsideForMutation(baseDir, requirePath(args.destination_path, "destination_path"));
        if (!fs.existsSync(sourceAbs)) {
          return JSON.stringify({ success: false, error: `Source not found at ${args.source_path}` });
        }
        fs.mkdirSync(path.dirname(destAbs), { recursive: true });
        fs.renameSync(sourceAbs, destAbs);
        return JSON.stringify({ success: true, message: `Moved ${args.source_path} to ${args.destination_path}` });
      }
      case "search_files": {
        const query = typeof args.query === "string" ? args.query : "";
        if (query === "") {
          throw new Error("Missing parameter: query");
        }
        const rootAbs = resolveInsideForRead(baseDir, typeof args.path === "string" ? args.path : "");
        if (!fs.existsSync(rootAbs)) {
          return JSON.stringify({ success: false, error: `Search path not found at ${args.path}` });
        }
        const matches = searchWorkspace(rootAbs, baseDir, query);
        return JSON.stringify({ success: true, matchCount: matches.length, matches });
      }
      case "run_command": {
        const command = typeof args.command === "string" ? args.command.trim() : "";
        if (command === "") {
          throw new Error("Missing parameter: command");
        }
        return JSON.stringify({ success: false, error: "run_command must be executed via executeWorkspaceToolAsync." });
      }
      case "search_web":
        return JSON.stringify({ success: false, error: "search_web must be executed via executeWorkspaceToolAsync." });
      case "fetch_url":
        // Network access, so it is gated like run_command. Returns a promise
        // that the caller awaits via executeWorkspaceToolAsync.
        return JSON.stringify({ success: false, error: "fetch_url must be executed via executeWorkspaceToolAsync." });
      default: {
        // A self-correcting error: weak models hallucinate tool names, and a bare
        // "unknown function" dead-ends them. Listing the real names lets the model
        // fix its call on the next turn.
        const knownTools = WORKSPACE_TOOLS.map(t => t.function.name).join(", ");
        throw new Error(
          `Unknown tool "${toolCall.function.name}" — it does not exist. Use one of these exact tool names instead: ${knownTools}.`
        );
      }
    }
  } catch (err: any) {
    return JSON.stringify({ success: false, error: err.message });
  }
}
