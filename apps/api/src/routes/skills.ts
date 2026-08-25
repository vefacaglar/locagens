import fs from "node:fs";
import path from "node:path";
import type { FastifyInstance } from "fastify";
import type { SkillSummary, SkillsListResponse } from "@locagens/shared";
import type { AppContext } from "../context.js";
import { requireRegisteredProject } from "../security/projectPaths.js";

/**
 * Settings → Skills: list discovered skills and install SKILL.md content into
 * the allowlisted user/project skills roots.
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
   * Installs a SKILL.md (JSON body with full file text) into user or project
   * skills. Used by Settings → Skills file picker — no multipart needed so the
   * desktop API bridge stays JSON-only.
   */
  server.post("/api/skills/install", async (request, reply) => {
    const body = request.body as {
      target?: string;
      content?: string;
      projectPath?: string;
    };

    const target = body.target === "project" ? "project" : body.target === "user" ? "user" : null;
    if (!target) {
      reply.status(400);
      return { error: 'target must be "user" or "project"' };
    }

    const content = typeof body.content === "string" ? body.content : "";
    if (!content.trim()) {
      reply.status(400);
      return { error: "content is required (full SKILL.md text)" };
    }

    let projectPath: string | undefined;
    if (target === "project") {
      const raw = typeof body.projectPath === "string" ? body.projectPath.trim() : "";
      if (!raw) {
        reply.status(400);
        return { error: "projectPath is required for project skills" };
      }
      try {
        projectPath = requireRegisteredProject(ctx.projectRepo, raw);
      } catch (err: any) {
        reply.status(400);
        return { error: err?.message ?? "Invalid project path" };
      }
    }

    try {
      const skill: SkillSummary = ctx.skillRegistry.installSkillMd(target, content, projectPath);
      return { success: true as const, skill };
    } catch (err: any) {
      reply.status(400);
      return { error: err?.message ?? "Failed to install skill" };
    }
  });

  /**
   * Resolves an allowlisted skills directory path (creates if missing).
   * Optional helper for advanced users; primary UX is file install.
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

    const canonicalProject = fs.realpathSync.native(projectPath);
    if (dir !== canonicalProject && !dir.startsWith(canonicalProject + path.sep)) {
      reply.status(400);
      return { error: "Skills folder resolved outside the project" };
    }

    return { path: dir, target: "project" as const };
  });
}
