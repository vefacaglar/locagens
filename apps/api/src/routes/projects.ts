import path from "node:path";
import fs from "node:fs";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import type { FastifyInstance } from "fastify";
import type { AppContext } from "../context.js";
import { canonicalProjectPath, requireRegisteredProject } from "../security/projectPaths.js";
import { resolveInsideForRead } from "../orchestrator/workspace/pathGuards.js";

const execFileAsync = promisify(execFile);

async function git(projectPath: string, args: string[]): Promise<string> {
  const { stdout } = await execFileAsync("git", args, {
    cwd: projectPath,
    encoding: "utf-8",
    maxBuffer: 8 * 1024 * 1024
  });
  return stdout;
}

interface GitStatusEntry {
  xy: string;
  path: string;
  oldPath?: string;
}

export function parsePorcelainZ(output: string): GitStatusEntry[] {
  const records = output.split("\0");
  const entries: GitStatusEntry[] = [];
  for (let index = 0; index < records.length; index++) {
    const record = records[index];
    if (!record || record.length < 4) continue;
    const xy = record.slice(0, 2);
    const filePath = record.slice(3);
    if (xy.includes("R") || xy.includes("C")) {
      const oldPath = records[++index] || "";
      entries.push({ xy, path: filePath, oldPath });
    } else {
      entries.push({ xy, path: filePath });
    }
  }
  return entries;
}

export function registerProjectRoutes(server: FastifyInstance, ctx: AppContext) {
  // List all projects.
  server.get("/api/projects", async () => {
    return ctx.projectRepo.list();
  });

  // List workspace files for @-mention autocompletion
  server.get("/api/projects/files", async (request, reply) => {
    const { path: rawPath, query } = request.query as { path?: string; query?: string };
    if (!rawPath) {
      reply.status(400);
      return { error: "Missing required query parameter: path" };
    }

    try {
      const canonicalPath = requireRegisteredProject(ctx.projectRepo, rawPath);
      let fileList: string[] = [];

      try {
        const out = await git(canonicalPath, ["ls-files", "-co", "--exclude-standard"]);
        fileList = out.split("\n").map((f) => f.trim()).filter(Boolean);
      } catch {
        const walk = (dir: string, base: string, depth = 0): string[] => {
          if (depth > 5 || fileList.length > 500) return [];
          const entries = fs.readdirSync(dir, { withFileTypes: true });
          const res: string[] = [];
          for (const ent of entries) {
            if (ent.name.startsWith(".") || ent.name === "node_modules" || ent.name === "dist" || ent.name === "build") continue;
            const rel = base ? `${base}/${ent.name}` : ent.name;
            if (ent.isDirectory()) {
              res.push(...walk(path.join(dir, ent.name), rel, depth + 1));
            } else if (ent.isFile()) {
              res.push(rel);
            }
          }
          return res;
        };
        fileList = walk(canonicalPath, "");
      }

      const q = (query || "").trim().toLowerCase();
      if (q) {
        fileList = fileList.filter((f) => f.toLowerCase().includes(q));
      }

      return { files: fileList.slice(0, 150) };
    } catch (err: any) {
      reply.status(400);
      return { error: err?.message || "Failed to list project files" };
    }
  });

  // Create/add a project manually.
  server.post("/api/projects", async (request, reply) => {
    const { path: projectPath, name: projectName } = request.body as {
      path?: string;
      name?: string;
    };

    if (!projectPath || !projectPath.trim()) {
      reply.status(400);
      return { error: "Missing required field: path" };
    }

    let resolvedPath: string;
    try {
      resolvedPath = canonicalProjectPath(projectPath);
    } catch (error: any) {
      reply.status(400);
      return { error: error.message };
    }
    const resolvedName = projectName?.trim() || path.basename(resolvedPath) || "Workspace";

    const project = {
      path: resolvedPath,
      name: resolvedName,
      createdAt: new Date().toISOString()
    };

    await ctx.projectRepo.create(project);
    return project;
  });

  // Remove a project from the list (chat history is preserved).
  server.delete("/api/projects", async (request, reply) => {
    const { path: projectPath } = request.query as { path?: string };

    if (!projectPath) {
      reply.status(400);
      return { error: "Missing required query parameter: path" };
    }

    await ctx.projectRepo.delete(projectPath);
    return { success: true };
  });

  // Trigger the native macOS folder picker.
  server.post("/api/projects/select-dir", async (request, reply) => {
    if (process.platform !== "darwin") {
      reply.status(400);
      return { error: "Automatic folder picking is only supported on macOS. Please input the path manually." };
    }

    try {
      const { stdout } = await execFileAsync("osascript", ["-e", "POSIX path of (choose folder with prompt \"Select a project folder:\")"]);
      const selectedPath = canonicalProjectPath(stdout.trim());
      if (!selectedPath) {
        reply.status(400);
        return { error: "No folder selected" };
      }
      const folderName = path.basename(selectedPath) || "Workspace";
      return { path: selectedPath, name: folderName };
    } catch (error: any) {
      if (error.message && (error.message.includes("User canceled") || error.message.includes("-128"))) {
        reply.status(400);
        return { error: "Selection cancelled by user." };
      }
      reply.status(500);
      return { error: `Failed to open folder picker: ${error.message}` };
    }
  });

  // Check git status of a project.
  server.get("/api/projects/git/status", async (request, reply) => {
    const { path: projectPath } = request.query as { path?: string };
    if (!projectPath) {
      reply.status(400);
      return { error: "Missing required query parameter: path" };
    }

    try {
      const canonicalPath = requireRegisteredProject(ctx.projectRepo, projectPath);
      // Check if git is initialized.
      await git(canonicalPath, ["rev-parse", "--is-inside-work-tree"]);
      const branch = await git(canonicalPath, ["branch", "--show-current"]);
      const status = await git(canonicalPath, ["status", "--porcelain"]);
      
      return {
        isGit: true,
        branch: branch.trim(),
        hasChanges: status.trim().length > 0
      };
    } catch (error: any) {
      console.error("[Git Status Route Error]:", error);
      return { isGit: false };
    }
  });

  // Get git diff details for all modified files.
  server.get("/api/projects/git/diff-details", async (request, reply) => {
    const { path: projectPath } = request.query as { path?: string };
    if (!projectPath) {
      reply.status(400);
      return { error: "Missing required query parameter: path" };
    }

    try {
      const canonicalPath = requireRegisteredProject(ctx.projectRepo, projectPath);
      const status = await git(canonicalPath, ["status", "--porcelain=v1", "-z", "--untracked-files=all"]);
      const entries = parsePorcelainZ(status);

      const results = [];
      for (const entry of entries) {
        const { xy } = entry;
        let fileSpec = entry.path;
        let kind: "created" | "edited" | "deleted" | "moved" = "edited";

        // Handle rename formatting: "R  old -> new"
        let oldPath = entry.oldPath || fileSpec;
        let newPath = fileSpec;
        if (xy.startsWith("R")) {
          kind = "moved";
          newPath = entry.path;
        } else if (xy.includes("A") || xy.includes("?")) {
          kind = "created";
        } else if (xy.includes("D")) {
          kind = "deleted";
        }

        let oldText = "";
        let newText = "";

        // Read old content from Git
        if (kind !== "created") {
          try {
            oldText = await git(canonicalPath, ["show", `HEAD:${oldPath}`]);
          } catch {
            // HEAD might not exist yet (e.g. initial commit)
            oldText = "";
          }
        }

        // Read new content from disk
        if (kind !== "deleted") {
          try {
            const fullPath = resolveInsideForRead(canonicalPath, newPath);
            newText = await fs.promises.readFile(fullPath, "utf-8");
          } catch {
            newText = "";
          }
        }

        results.push({
          path: fileSpec,
          kind,
          oldText,
          newText
        });
      }

      return { files: results };
    } catch (error: any) {
      reply.status(500);
      return { error: error.message || "Failed to fetch diff details" };
    }
  });

  // Generate a commit message using the LLM model.
  server.post("/api/projects/git/generate-message", async (request, reply) => {
    const { runId } = request.body as { runId?: string };
    if (!runId) {
      reply.status(400);
      return { error: "Missing required body parameter: runId" };
    }

    const run = ctx.runRepo.getById(runId);
    if (!run) {
      reply.status(404);
      return { error: "Run not found" };
    }

    let projectPath: string;
    try {
      projectPath = requireRegisteredProject(ctx.projectRepo, run.projectPath || ctx.defaultProjectPath);
    } catch (error: any) {
      reply.status(400);
      return { error: error.message };
    }

    try {
      // Get the git diff.
      const diff = await git(projectPath, ["diff", "HEAD"]);
      if (!diff.trim()) {
        // Fallback to checking if there are untracked files
        const status = await git(projectPath, ["status", "--porcelain"]);
        if (!status.trim()) {
          return { message: "chore: update workspace" };
        }
      }

      // Truncate the diff if it is too large (e.g. limit to 12000 chars to avoid model context blow up)
      const maxDiffLen = 12000;
      const truncatedDiff = diff.length > maxDiffLen ? diff.slice(0, maxDiffLen) + "\n... [diff truncated] ..." : diff;

      const providerId = run.coderProviderId || run.providerId;
      const model = run.coderModel || run.model;

      const provider = ctx.registry.getProvider(providerId);
      const prompt = `Write a concise, professional Git commit message based on the following diff.
Use the conventional commits format (e.g., feat: ..., fix: ..., chore: ..., docs: ..., refactor: ..., style: ...).
The message should be short (50-72 characters for the subject line) and summarize the key changes. Do not include markdown formatting or backticks around the message. Just output the message itself.

Diff:
${truncatedDiff}`;

      const result = await provider.complete({
        model,
        temperature: 0.2,
        messages: [{ role: "user", content: prompt }]
      });

      // Clean up the output message
      let message = result.content.trim();
      // Remove any surrounding quotes or markdown ticks if returned
      message = message.replace(/^["'`\s]+|["'`\s]+$/g, "");
      
      return { message };
    } catch (error: any) {
      reply.status(500);
      return { error: `Failed to generate commit message: ${error.message}` };
    }
  });

  // Execute git commit or push action.
  server.post("/api/projects/git/commit", async (request, reply) => {
    const { path: projectPath, message, action } = request.body as {
      path?: string;
      message?: string;
      action?: "commit" | "commit-push" | "push";
    };

    if (!projectPath) {
      reply.status(400);
      return { error: "Missing required body parameter: path" };
    }

    if (!action || !["commit", "commit-push", "push"].includes(action)) {
      reply.status(400);
      return { error: "Invalid action. Must be 'commit', 'commit-push', or 'push'" };
    }

    if ((action === "commit" || action === "commit-push") && (!message || !message.trim())) {
      reply.status(400);
      return { error: "Missing required parameter: message" };
    }

    try {
      const canonicalPath = requireRegisteredProject(ctx.projectRepo, projectPath);
      if (action === "commit" || action === "commit-push") {
        // Stage files
        await git(canonicalPath, ["add", "--all", "--", "."]);
        // Do not execute repository-controlled hooks from the desktop API.
        await git(canonicalPath, ["-c", "core.hooksPath=/dev/null", "commit", "-m", message!.trim()]);
      }

      if (action === "commit-push" || action === "push") {
        await git(canonicalPath, ["push"]);
      }

      return { success: true };
    } catch (error: any) {
      reply.status(500);
      return { error: error.message || "Failed to execute git action" };
    }
  });
}
