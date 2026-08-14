import type { FastifyInstance } from "fastify";
import type { AppContext } from "../context.js";
import { registerProviderRoutes } from "./providers.js";
import { registerProjectRoutes } from "./projects.js";
import { registerRunRoutes } from "./runs.js";
import { registerPermissionRoutes } from "./permissions.js";
import { registerMemoryRoutes } from "./memory.js";
import { registerSettingsRoutes } from "./settings.js";
import { registerSecurityRoutes } from "./security.js";

export function registerRoutes(server: FastifyInstance, ctx: AppContext) {
  server.get("/ping", async () => ({ status: "ok", timestamp: new Date().toISOString() }));

  registerProviderRoutes(server, ctx);
  registerProjectRoutes(server, ctx);
  registerRunRoutes(server, ctx);
  registerPermissionRoutes(server, ctx);
  registerMemoryRoutes(server, ctx);
  registerSettingsRoutes(server, ctx);
  registerSecurityRoutes(server);
}
