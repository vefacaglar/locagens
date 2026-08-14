import { timingSafeEqual } from "node:crypto";
import type { FastifyInstance, FastifyRequest } from "fastify";

const API_PREFIX = "/api/";

function tokenMatches(actual: string, expected: string): boolean {
  const actualBytes = Buffer.from(actual);
  const expectedBytes = Buffer.from(expected);
  return actualBytes.length === expectedBytes.length && timingSafeEqual(actualBytes, expectedBytes);
}

function bearerToken(request: FastifyRequest): string {
  const authorization = request.headers.authorization;
  if (!authorization?.startsWith("Bearer ")) return "";
  return authorization.slice("Bearer ".length).trim();
}

export function requireApiToken(): string {
  const token = process.env.LOCAGENS_API_TOKEN?.trim();
  if (!token || token.length < 32) {
    throw new Error("LOCAGENS_API_TOKEN must be set to a random value of at least 32 characters.");
  }
  return token;
}

/** Protects every control-plane route while keeping the minimal health probe public. */
export function registerApiAuthentication(server: FastifyInstance, expectedToken: string): void {
  server.addHook("onRequest", async (request, reply) => {
    const pathname = request.url.split("?", 1)[0];
    if (!pathname.startsWith(API_PREFIX)) return;
    const actual = bearerToken(request);
    if (!actual || !tokenMatches(actual, expectedToken)) {
      reply.header("WWW-Authenticate", "Bearer");
      return reply.status(401).send({ error: "Unauthorized" });
    }
  });
}

