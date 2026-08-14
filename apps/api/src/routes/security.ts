import type { FastifyInstance } from "fastify";
import type { SecurityStatus } from "@locagens/shared";
import { commandSandbox } from "../security/CommandSandbox.js";

async function securityStatus(): Promise<SecurityStatus> {
  return {
    api: { bindAddress: "127.0.0.1", authenticated: true },
    sandbox: await commandSandbox.status(),
    policy: { commandNetwork: "deny_by_default", workspaceWritesOnly: true }
  };
}

export function registerSecurityRoutes(server: FastifyInstance): void {
  server.get("/api/security/status", async () => securityStatus());

  server.post("/api/security/sandbox/setup", async (request, reply) => {
    const { confirmed } = (request.body || {}) as { confirmed?: boolean };
    if (confirmed !== true) {
      reply.status(400);
      return { error: "Explicit user confirmation is required." };
    }
    if (process.platform !== "win32") {
      reply.status(400);
      return { error: "Sandbox setup is only required on Windows." };
    }
    try {
      await commandSandbox.installWindows();
      return securityStatus();
    } catch (error: any) {
      reply.status(500);
      return { error: error?.message || "Windows sandbox setup failed." };
    }
  });
}
