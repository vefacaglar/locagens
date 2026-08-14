import type { FastifyInstance } from "fastify";

const ALLOWED_ORIGINS = new Set([
  "null",
  "http://localhost:5173",
  "http://127.0.0.1:5173"
]);

export function isAllowedControlPlaneOrigin(origin: string | undefined): boolean {
  return !origin || ALLOWED_ORIGINS.has(origin);
}

/** CORS headers alone do not stop cross-origin form submissions, so reject them server-side. */
export function registerOriginPolicy(server: FastifyInstance): void {
  server.addHook("onRequest", async (request, reply) => {
    if (!isAllowedControlPlaneOrigin(request.headers.origin)) {
      return reply.status(403).send({ error: "Origin not allowed" });
    }
  });
}
