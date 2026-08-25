import type { FastifyInstance } from "fastify";
import type { PluginsResponse, InstallPluginPayload, PluginScope } from "@locagens/shared";
import type { AppContext } from "../context.js";
import { requireRegisteredProject } from "../security/projectPaths.js";

/**
 * Settings → Plugins routes:
 * Discover, install, toggle, and delete plugins (and list built-in templates like context-mode).
 */
export function registerPluginRoutes(server: FastifyInstance, ctx: AppContext) {
  server.get("/api/plugins", async (request) => {
    const query = request.query as { projectPath?: string };
    let projectPath: string | undefined;
    if (typeof query.projectPath === "string" && query.projectPath.trim()) {
      try {
        projectPath = requireRegisteredProject(ctx.projectRepo, query.projectPath);
      } catch {
        projectPath = undefined;
      }
    }

    const plugins = ctx.pluginRegistry.discover(projectPath);
    const templates = ctx.pluginRegistry.getTemplates();
    const userPluginsDir = ctx.pluginRegistry.userRoot();
    const projectPluginsDir = ctx.pluginRegistry.projectRoot(projectPath);

    const response: PluginsResponse = {
      plugins,
      templates,
      userPluginsDir,
      projectPluginsDir
    };
    return response;
  });

  server.get("/api/plugins/templates", async () => {
    return {
      templates: ctx.pluginRegistry.getTemplates()
    };
  });

  server.post("/api/plugins/install", async (request, reply) => {
    const body = request.body as InstallPluginPayload;
    if (!body || !body.uri || !body.source) {
      reply.status(400);
      return { error: "source and uri are required to install a plugin." };
    }

    let projectPath: string | undefined;
    if (body.scope === "project") {
      if (!body.projectPath?.trim()) {
        reply.status(400);
        return { error: "projectPath is required for project-scoped plugins." };
      }
      try {
        projectPath = requireRegisteredProject(ctx.projectRepo, body.projectPath);
      } catch (err: any) {
        reply.status(400);
        return { error: err?.message || "Invalid project path." };
      }
    }

    try {
      const installed = await ctx.pluginRegistry.install({
        ...body,
        projectPath
      });

      // If plugin has MCP servers configured, sync them
      if (installed.mcpServers && Object.keys(installed.mcpServers).length > 0) {
        for (const [serverName, serverConf] of Object.entries(installed.mcpServers)) {
          if (serverConf.enabled !== false) {
            try {
              ctx.mcpManager.saveConfig({
                name: `${installed.id}__${serverName}`,
                scope: installed.scope === "project" ? "project" : "user",
                transport: serverConf.transport || "stdio",
                command: serverConf.command,
                args: serverConf.args,
                env: serverConf.env,
                url: serverConf.url,
                enabled: true,
                projectPath
              });
            } catch (err) {
              console.warn(`[Plugins] Auto-registering MCP server for ${installed.id} warning:`, err);
            }
          }
        }
      }

      return { success: true, plugin: installed };
    } catch (err: any) {
      reply.status(500);
      return { error: err?.message || "Failed to install plugin." };
    }
  });

  server.post("/api/plugins/:id/toggle", async (request, reply) => {
    const { id } = request.params as { id: string };
    const body = request.body as { enabled?: boolean; scope?: PluginScope; projectPath?: string };

    const enabled = body.enabled !== false;
    const scope: PluginScope = body.scope === "project" ? "project" : "user";
    let projectPath: string | undefined;
    if (scope === "project" && body.projectPath) {
      try {
        projectPath = requireRegisteredProject(ctx.projectRepo, body.projectPath);
      } catch {
        projectPath = undefined;
      }
    }

    const updated = ctx.pluginRegistry.toggle(id, enabled, scope, projectPath);
    if (!updated) {
      reply.status(404);
      return { error: `Plugin "${id}" not found.` };
    }

    return { success: true, plugin: updated };
  });

  server.delete("/api/plugins/:id", async (request, reply) => {
    const { id } = request.params as { id: string };
    const query = request.query as { scope?: PluginScope; projectPath?: string };

    const scope: PluginScope = query.scope === "project" ? "project" : "user";
    let projectPath: string | undefined;
    if (scope === "project" && query.projectPath) {
      try {
        projectPath = requireRegisteredProject(ctx.projectRepo, query.projectPath);
      } catch {
        projectPath = undefined;
      }
    }

    const deleted = ctx.pluginRegistry.delete(id, scope, projectPath);
    if (!deleted) {
      reply.status(404);
      return { error: `Plugin "${id}" not found or could not be deleted.` };
    }

    return { success: true, deleted: true };
  });
}
