import type { FastifyInstance } from "fastify";
import type { AppContext } from "../context.js";
import { requireRegisteredProject } from "../security/projectPaths.js";
import type { SymbolKind } from "../orchestrator/symbols/types.js";

export function registerSymbolRoutes(server: FastifyInstance, ctx: AppContext) {
  server.get("/api/projects/symbols", async (request, reply) => {
    const { path: rawPath, query, kind } = request.query as {
      path?: string;
      query?: string;
      kind?: string;
    };

    if (!rawPath) {
      reply.status(400);
      return { error: "Missing required query parameter: path" };
    }

    try {
      const canonicalPath = requireRegisteredProject(ctx.projectRepo, rawPath);
      const symbols = await ctx.symbolIndexer.search(
        canonicalPath,
        query || "",
        (kind as SymbolKind) || undefined
      );

      return { symbols };
    } catch (err: any) {
      reply.status(400);
      return { error: err?.message || "Failed to search symbols" };
    }
  });
}
