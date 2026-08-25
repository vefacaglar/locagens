import fs from "node:fs";
import path from "node:path";
import type { FastifyInstance } from "fastify";
import type { SkillsListResponse } from "@locagens/shared";
import type { AppContext } from "../context.js";
import { requireRegisteredProject } from "../security/projectPaths.js";

/**
 * Settings → Skills: list discovered skills and resolve allowlisted folder paths
 * for "Open folder". Skill bodies are not edited through the API in v1.
 */
export function registerSkillRoutes(server: FastifyInstance, ctx: AppContext) {
  server.get("/api/skills", async (request) => {
    const query = request.query as { projectPath?: string };
    let projectPath: string | undefined;
    if (typeof query.projectPath === "string" && query.projectPath.trim()) {
      try {
        projectPath = requireRegisteredProject(ctx.projectRepo, query.projectPath);
      } catch {
        projectPath = undefined;
      }
    }

    const skills = ctx.skillRegistry.listSummaries(projectPath);
    const response: SkillsListResponse = {
      skills,
      roots: {
        user: ctx.skillRegistry.userRoot(),
        project: ctx.skillRegistry.projectRoot(projectPath)
      }
    };
    return response;
  });

  /**
   * Resolves an allowlisted skills directory path for the UI / desktop shell.
   * Creates the directory when missing so Open folder always has a target.
   * Does not open the folder itself (desktop uses shell.openPath on the path).
   */
  server.post("/api/skills/open-folder", async (request, reply) => {
    const body = request.body as { target?: string; projectPath?: string };
    const target = body.target === "project" ? "project" : body.target === "user" ? "user" : null;
    if (!target) {
      reply.status(400);
      return { error: 'target must be "user" or "project"' };
    }

    if (target === "user") {
      const dir = ctx.skillRegistry.ensureUserRoot();
      return { path: dir, target: "user" as const };
    }

    const rawProject = typeof body.projectPath === "string" ? body.projectPath.trim() : "";
    if (!rawProject) {
      reply.status(400);
      return { error: "projectPath is required for project skills folder" };
    }

    let projectPath: string;
    try {
      projectPath = requireRegisteredProject(ctx.projectRepo, rawProject);
    } catch (err: any) {
      reply.status(400);
      return { error: err?.message ?? "Invalid project path" };
    }

    const dir = ctx.skillRegistry.ensureProjectRoot(projectPath);
    if (!dir) {
      reply.status(400);
      return { error: "Could not create project skills folder" };
    }

    // Defense in depth: resolved path must still sit under the project.
    const canonicalProject = fs.realpathSync.native(projectPath);
    if (dir !== canonicalProject && !dir.startsWith(canonicalProject + path.sep)) {
      reply.status(400);
      return { error: "Skills folder resolved outside the project" };
    }

    return { path: dir, target: "project" as const };
  });
}
