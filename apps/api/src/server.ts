import Fastify from "fastify";
import cors from "@fastify/cors";
import { createAppContext } from "./context.js";
import { registerRoutes } from "./routes/index.js";
import { registerApiAuthentication, requireApiToken } from "./security/apiAuth.js";
import { isAllowedControlPlaneOrigin, registerOriginPolicy } from "./security/originPolicy.js";

async function start() {
  const apiToken = requireApiToken();
  const server = Fastify({
    // SKILL.md install posts full file text as JSON (browser file picker).
    bodyLimit: 256 * 1024,
    logger: {
      redact: ["req.headers.authorization"]
    }
  });

  await server.register(cors, {
    origin(origin, callback) {
      callback(null, isAllowedControlPlaneOrigin(origin));
    },
    allowedHeaders: ["authorization", "content-type"],
    methods: ["GET", "POST", "PUT", "DELETE", "OPTIONS"]
  });

  registerOriginPolicy(server);
  registerApiAuthentication(server, apiToken);

  const ctx = createAppContext();
  registerRoutes(server, ctx);

  try {
    // The settings file (editable from the app's Settings screen) is the
    // source of truth for the port; PORT env is only a fallback default.
    const port = ctx.settingsStore.resolvePort();
    await server.listen({ port, host: "127.0.0.1" });
    (process as any).parentPort?.postMessage({ type: "locagens:backend-ready", port });
    console.log(`Server is running on http://127.0.0.1:${port}`);
  } catch (err) {
    server.log.error(err);
    process.exit(1);
  }
}

start();
