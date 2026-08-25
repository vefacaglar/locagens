import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import assert from "node:assert/strict";
import Fastify from "fastify";
import { McpClientManager, McpConfigStore } from "../orchestrator/mcp/index.js";
import { registerMcpRoutes } from "./mcp.js";

function makeCtx(userConfigPath: string, projectRepo?: { get: (p: string) => unknown; list: () => any[] }) {
  const store = new McpConfigStore(userConfigPath);
  const manager = new McpClientManager(store);
  return {
    mcpManager: manager,
    projectRepo: projectRepo ?? {
      get: () => null,
      list: () => []
    }
  } as any;
}

test("MCP routes CRUD cycle", async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-mcp-route-"));
  const userConfig = path.join(tmp, "mcp_servers.json");
  const server = Fastify();
  const ctx = makeCtx(userConfig);
  registerMcpRoutes(server, ctx);
  await server.ready();

  // 1. GET empty
  const getRes1 = await server.inject({ method: "GET", url: "/api/mcp/servers" });
  assert.equal(getRes1.statusCode, 200);
  assert.deepEqual(getRes1.json().servers, []);

  // 2. POST create server (disabled by default so it doesn't try to spawn a real process during test)
  const postRes = await server.inject({
    method: "POST",
    url: "/api/mcp/servers",
    payload: {
      name: "test-server",
      scope: "user",
      transport: "stdio",
      command: "echo",
      args: ["hello"],
      enabled: false
    }
  });
  assert.equal(postRes.statusCode, 200);
  assert.equal(postRes.json().success, true);
  assert.equal(postRes.json().server.config.name, "test-server");

  // 3. GET list
  const getRes2 = await server.inject({ method: "GET", url: "/api/mcp/servers" });
  assert.equal(getRes2.statusCode, 200);
  assert.equal(getRes2.json().servers.length, 1);
  assert.equal(getRes2.json().servers[0].config.name, "test-server");
  assert.equal(getRes2.json().servers[0].status, "disabled");

  // 4. Toggle
  const toggleRes = await server.inject({
    method: "POST",
    url: "/api/mcp/servers/test-server/toggle",
    payload: { enabled: false }
  });
  assert.equal(toggleRes.statusCode, 200);

  // 5. DELETE
  const delRes = await server.inject({
    method: "DELETE",
    url: "/api/mcp/servers/test-server"
  });
  assert.equal(delRes.statusCode, 200);
  assert.equal(delRes.json().success, true);
  assert.equal(delRes.json().deleted, true);

  // 6. GET empty again
  const getRes3 = await server.inject({ method: "GET", url: "/api/mcp/servers" });
  assert.equal(getRes3.json().servers.length, 0);

  await server.close();
  fs.rmSync(tmp, { recursive: true, force: true });
});
