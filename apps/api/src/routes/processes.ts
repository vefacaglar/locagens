import type { FastifyInstance } from "fastify";
import type { AppContext } from "../context.js";
import { requireRegisteredProject } from "../security/projectPaths.js";

export function registerProcessRoutes(server: FastifyInstance, ctx: AppContext) {
  // List processes (optionally filtered by project path)
  server.get("/api/processes", async (request) => {
    const { projectPath } = request.query as { projectPath?: string };
    const resolvedPath = projectPath ? requireRegisteredProject(ctx.projectRepo, projectPath) : undefined;
    const processes = ctx.processManager.list(resolvedPath);
    return { processes };
  });

  // Spawn a background process
  server.post("/api/processes/spawn", async (request, reply) => {
    const { command, projectPath, env } = request.body as {
      command?: string;
      projectPath?: string;
      env?: Record<string, string>;
    };

    if (!command || !command.trim()) {
      reply.status(400);
      return { error: "Missing required parameter: command" };
    }

    try {
      const canonicalPath = requireRegisteredProject(ctx.projectRepo, projectPath || ctx.defaultProjectPath);
      const processInfo = ctx.processManager.spawnProcess({
        command: command.trim(),
        projectPath: canonicalPath,
        env
      });

      return { process: processInfo };
    } catch (err: any) {
      reply.status(400);
      return { error: err?.message || "Failed to spawn process" };
    }
  });

  // Kill a background process
  server.post("/api/processes/:id/kill", async (request, reply) => {
    const { id } = request.params as { id: string };
    const success = await ctx.processManager.kill(id);
    return { success };
  });

  // Restart a background process
  server.post("/api/processes/:id/restart", async (request, reply) => {
    const { id } = request.params as { id: string };
    const processInfo = await ctx.processManager.restart(id);
    if (!processInfo) {
      reply.status(404);
      return { error: "Process not found" };
    }
    return { process: processInfo };
  });

  // Get process logs
  server.get("/api/processes/:id/logs", async (request, reply) => {
    const { id } = request.params as { id: string };
    const processInfo = ctx.processManager.get(id);
    if (!processInfo) {
      reply.status(404);
      return { error: "Process not found" };
    }
    const logs = ctx.processManager.getLogs(id);
    return { process: processInfo, logs };
  });
}
