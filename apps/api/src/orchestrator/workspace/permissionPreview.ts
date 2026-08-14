import path from "node:path";
import type { Run, ToolCall, PermissionPreview } from "@locagens/shared";
import { resolveInside, readExistingFile } from "./pathGuards.js";
import { applyEdit } from "./fileToolExecutor.js";
import { normalizeNetworkDomains } from "../../security/CommandSandbox.js";

function safeNetworkDomains(value: unknown): string[] {
  try {
    return normalizeNetworkDomains(value);
  } catch {
    return [];
  }
}

/**
 * The permission-flow side of the workspace tools: grant scoping, the
 * escape-the-workspace command heuristics, and the preview the UI renders
 * before a gated tool runs.
 */

/**
 * The key a standing permission grant is scoped to. For run_command the grant
 * is per exact command string, so approving "dotnet build" does not approve
 * "dotnet build -f". Other tools are scoped per tool name (command is empty).
 */
export function permissionKey(toolCall: ToolCall): { tool: string; command: string; networkDomains: string[] } {
  const tool = toolCall.function.name;
  if (tool === "run_command") {
    let command = "";
    try {
      command = String(JSON.parse(toolCall.function.arguments || "{}").command ?? "").trim();
    } catch {
      command = "";
    }
    let networkDomains: string[] = [];
    try {
      networkDomains = safeNetworkDomains(JSON.parse(toolCall.function.arguments || "{}").network_domains);
    } catch {
      networkDomains = [];
    }
    return { tool, command, networkDomains };
  }
  if (tool === "search_web") {
    let query = "";
    try {
      query = String(JSON.parse(toolCall.function.arguments || "{}").query ?? "").trim();
    } catch {
      query = "";
    }
    return { tool, command: query, networkDomains: [] };
  }
  if (tool === "fetch_url") {
    // Scope the grant to the host, so approving one site does not approve the
    // whole web. The host is stashed in the `command` slot of the key.
    let host = "";
    try {
      const raw = String(JSON.parse(toolCall.function.arguments || "{}").url ?? "").trim();
      host = new URL(raw).host;
    } catch {
      host = "";
    }
    return { tool, command: host.toLowerCase(), networkDomains: [] };
  }
  return { tool, command: "", networkDomains: [] };
}

/**
 * Heuristic: does this shell command try to leave the project workspace? Such
 * commands (changing into a parent/absolute/home directory, or traversing with
 * "..") ALWAYS require an approval prompt — even in Full Access, and even when a
 * standing grant would otherwise cover the command family. Returning true forces
 * a prompt and prevents the command from being silently granted/auto-granted.
 *
 * Care: Go-style "./pkg/..." wildcards are NOT parent traversal — the dots are
 * bounded so "..." does not trip the "../" check.
 */
export function commandEscapesWorkspace(command: string): boolean {
  if (!command) return false;
  // Parent-directory traversal as a real path segment: "..", "../x", "/.." — but
  // not "..." (the next char after ".." must be a separator/quote or end).
  if (/(^|[\s/'"=:(])\.\.([/\s'"]|$)/.test(command)) return true;
  // Home-directory references: "~", "~/...".
  if (/(^|[\s'"=:(])~($|[/\s'"])/.test(command)) return true;
  // Changing directory to an absolute, home, or parent location.
  if (/\b(cd|pushd|chdir)\s+['"]?(\/|~|\.\.)/.test(command)) return true;
  return false;
}

export function commandScansOutsideWorkspace(command: string): boolean {
  if (!command) return false;
  return /(^|[;&|]\s*)find\s+['"]?(\/|~|\.\.)([\s/'"]|$)/.test(command);
}

/**
 * Builds the preview shown in the permission card before a tool runs.
 * For file writes/edits it computes the resulting content so the UI can render
 * a diff; for create/read/list it carries the path; run_command carries the
 * command; move_file carries source + destination.
 */
export function buildPermissionPreview(run: Run, toolCall: ToolCall): PermissionPreview | null {
  let args: any = {};
  try {
    args = JSON.parse(toolCall.function.arguments || "{}");
  } catch {
    args = {};
  }

  const baseDir = path.resolve(run.projectPath || process.cwd());
  const safeResolve = (rel: string): string => {
    try {
      return resolveInside(baseDir, rel);
    } catch {
      return path.resolve(baseDir, rel);
    }
  };

  const relativePath = typeof args.path === "string" ? args.path : "";
  const absolutePath = safeResolve(relativePath);

  switch (toolCall.function.name) {
    case "write_file": {
      const oldContent = readExistingFile(absolutePath);
      return {
        tool: "write_file",
        action: oldContent === null ? "create" : "edit",
        path: relativePath,
        absolutePath,
        oldContent,
        newContent: typeof args.content === "string" ? args.content : ""
      };
    }
    case "edit_file": {
      const oldContent = readExistingFile(absolutePath);
      const newContent = oldContent === null
        ? null
        : applyEdit(oldContent, args.old_string ?? "", args.new_string ?? "", !!args.replace_all);
      return {
        tool: "edit_file",
        action: "edit",
        path: relativePath,
        absolutePath,
        oldContent,
        newContent
      };
    }
    case "delete_file":
      return { tool: "delete_file", action: "delete", path: relativePath, absolutePath, oldContent: readExistingFile(absolutePath), newContent: null };
    case "read_file":
      return { tool: "read_file", action: "read", path: relativePath, absolutePath, oldContent: null, newContent: null };
    case "list_directory":
      return { tool: "list_directory", action: "list", path: relativePath, absolutePath, oldContent: null, newContent: null };
    case "create_directory":
      return { tool: "create_directory", action: "mkdir", path: relativePath, absolutePath, oldContent: null, newContent: null };
    case "move_file": {
      const source = typeof args.source_path === "string" ? args.source_path : "";
      const destination = typeof args.destination_path === "string" ? args.destination_path : "";
      return {
        tool: "move_file",
        action: "move",
        path: source,
        absolutePath: safeResolve(source),
        oldContent: null,
        newContent: null,
        destPath: destination
      };
    }
    case "search_files":
      return {
        tool: "search_files",
        action: "search",
        path: typeof args.path === "string" ? args.path : "",
        absolutePath,
        oldContent: null,
        newContent: null,
        query: typeof args.query === "string" ? args.query : ""
      };
    case "run_command":
      return {
        tool: "run_command",
        action: "command",
        path: "",
        absolutePath: baseDir,
        oldContent: null,
        newContent: null,
        command: typeof args.command === "string" ? args.command : "",
        networkDomains: safeNetworkDomains(args.network_domains)
      };
    case "search_web":
      return {
        tool: "search_web",
        action: "search",
        path: "",
        absolutePath: baseDir,
        oldContent: null,
        newContent: null,
        query: typeof args.query === "string" ? args.query : ""
      };
    case "fetch_url":
      return {
        tool: "fetch_url",
        action: "fetch",
        path: "",
        absolutePath: baseDir,
        oldContent: null,
        newContent: null,
        url: typeof args.url === "string" ? args.url : ""
      };
    default:
      return null;
  }
}
