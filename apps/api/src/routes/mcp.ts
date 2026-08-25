import type { FastifyInstance } from "fastify";
import type { McpServerConfig, McpServersResponse } from "@locagens/shared";
import type { AppContext } from "../context.js";
import { requireRegisteredProject } from "../security/projectPaths.js";

/**
 * Settings → MCP: manage and inspect Model Context Protocol (MCP) servers.
 */
export function registerMcpRoutes(server: FastifyInstance, ctx: AppContext) {
  server.get("/api/mcp/servers", async (request) => {
    const query = request.query as { projectPath?: string };
    let projectPath: string | undefined;
    if (typeof query.projectPath === "string" && query.projectPath.trim()) {
      try {
        projectPath = requireRegisteredProject(ctx.projectRepo, query.projectPath);
      } catch {
        projectPath = undefined;
      }
    }

    const servers = await ctx.mcpManager.getStatusList(projectPath);
    const response: McpServersResponse = { servers };
    return response;
  });

  server.post("/api/mcp/servers", async (request, reply) => {
    const body = request.body as Partial<McpServerConfig>;
    const name = String(body.name || "").trim().toLowerCase();
    if (!name || !/^[a-z0-9][a-z0-9_-]*$/.test(name)) {
      reply.status(400);
      return { error: "Server name is required (lowercase alphanumeric, dash, underscore only)." };
    }

    const scope = body.scope === "project" ? "project" : "user";
    let projectPath: string | undefined;
    if (scope === "project") {
      const raw = typeof body.projectPath === "string" ? body.projectPath.trim() : "";
      if (!raw) {
        reply.status(400);
        return { error: "projectPath is required for project MCP servers." };
      }
      try {
        projectPath = requireRegisteredProject(ctx.projectRepo, raw);
      } catch (err: any) {
        reply.status(400);
        return { error: err?.message ?? "Invalid project path" };
      }
    }

    const transport = body.transport === "sse" ? "sse" : "stdio";
    if (transport === "stdio" && !body.command?.trim()) {
      reply.status(400);
      return { error: "command is required for stdio transport." };
    }
    if (transport === "sse" && !body.url?.trim()) {
      reply.status(400);
      return { error: "url is required for sse transport." };
    }

    const config: McpServerConfig = {
      name,
      scope,
      transport,
      command: body.command?.trim(),
      args: Array.isArray(body.args) ? body.args : [],
      env: body.env && typeof body.env === "object" ? body.env : undefined,
      url: body.url?.trim(),
      enabled: body.enabled !== false,
      projectPath
    };

    try {
      ctx.mcpManager.getConfigStore().saveConfig(config, projectPath);
      const serverInfo = await ctx.mcpManager.restartServer(name, projectPath);
      return { success: true as const, server: serverInfo };
    } catch (err: any) {
      reply.status(400);
      return { error: err?.message ?? "Failed to save MCP server." };
    }
  });

  server.delete("/api/mcp/servers/:name", async (request, reply) => {
    const params = request.params as { name?: string };
    const query = (request.query || {}) as { projectPath?: string };
    const body = (request.body || {}) as { projectPath?: string };

    const name = String(params.name || "").trim().toLowerCase();
    if (!name) {
      reply.status(400);
      return { error: "Server name is required." };
    }

    let projectPath: string | undefined;
    const rawProject = typeof (body.projectPath ?? query.projectPath) === "string" ? (body.projectPath ?? query.projectPath)!.trim() : "";
    if (rawProject) {
      try {
        projectPath = requireRegisteredProject(ctx.projectRepo, rawProject);
      } catch {
        projectPath = undefined;
      }
    }

    try {
      const deleted = await ctx.mcpManager.deleteServer(name, projectPath);
      return { success: true as const, deleted };
    } catch (err: any) {
      reply.status(400);
      return { error: err?.message ?? "Failed to delete MCP server." };
    }
  });

  server.post("/api/mcp/servers/:name/restart", async (request, reply) => {
    const params = request.params as { name?: string };
    const body = (request.body || {}) as { projectPath?: string };
    const name = String(params.name || "").trim().toLowerCase();
    if (!name) {
      reply.status(400);
      return { error: "Server name is required." };
    }

    let projectPath: string | undefined;
    if (body.projectPath?.trim()) {
      try {
        projectPath = requireRegisteredProject(ctx.projectRepo, body.projectPath.trim());
      } catch {
        projectPath = undefined;
      }
    }

    try {
      const serverInfo = await ctx.mcpManager.restartServer(name, projectPath);
      return { success: true as const, server: serverInfo };
    } catch (err: any) {
      reply.status(400);
      return { error: err?.message ?? "Failed to restart MCP server." };
    }
  });

  server.post("/api/mcp/servers/:name/toggle", async (request, reply) => {
    const params = request.params as { name?: string };
    const body = (request.body || {}) as { enabled?: boolean; projectPath?: string };
    const name = String(params.name || "").trim().toLowerCase();
    if (!name) {
      reply.status(400);
      return { error: "Server name is required." };
    }

    const enabled = body.enabled !== false;
    let projectPath: string | undefined;
    if (body.projectPath?.trim()) {
      try {
        projectPath = requireRegisteredProject(ctx.projectRepo, body.projectPath.trim());
      } catch {
        projectPath = undefined;
      }
    }

    try {
      const serverInfo = await ctx.mcpManager.toggleServer(name, enabled, projectPath);
      return { success: true as const, server: serverInfo };
    } catch (err: any) {
      reply.status(400);
      return { error: err?.message ?? "Failed to toggle MCP server." };
    }
  });
}
