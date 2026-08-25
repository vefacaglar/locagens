import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import assert from "node:assert/strict";
import Fastify from "fastify";
import { PluginRegistry } from "../orchestrator/plugins/index.js";
import { registerPluginRoutes } from "./plugins.js";

function makeCtx(userRoot: string, projectRepo?: { get: (p: string) => unknown; list: () => any[] }) {
  return {
    pluginRegistry: new PluginRegistry(userRoot),
    mcpManager: {
      saveConfig: () => {}
    },
    projectRepo: projectRepo ?? {
      get: () => null,
      list: () => []
    }
  } as any;
}

test("GET /api/plugins returns empty list and templates initially", async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-plugins-route-"));
  const userRoot = path.join(tmp, "plugins");
  const server = Fastify();
  registerPluginRoutes(server, makeCtx(userRoot));
  await server.ready();

  const res = await server.inject({ method: "GET", url: "/api/plugins" });
  assert.equal(res.statusCode, 200);
  const body = res.json();
  assert.equal(body.plugins.length, 0);
  assert.ok(body.templates.length >= 1);
  assert.ok(body.templates.some((t: any) => t.id === "context-mode"));

  await server.close();
  fs.rmSync(tmp, { recursive: true, force: true });
});

test("POST /api/plugins/install installs template and toggles it", async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-plugins-route-"));
  const userRoot = path.join(tmp, "plugins");
  const server = Fastify();
  registerPluginRoutes(server, makeCtx(userRoot));
  await server.ready();

  const installRes = await server.inject({
    method: "POST",
    url: "/api/plugins/install",
    payload: {
      source: "template",
      uri: "context-mode",
      scope: "user"
    }
  });

  assert.equal(installRes.statusCode, 200);
  const installBody = installRes.json();
  assert.equal(installBody.success, true);
  assert.equal(installBody.plugin.id, "context-mode");
  assert.equal(installBody.plugin.enabled, true);

  // Toggle
  const toggleRes = await server.inject({
    method: "POST",
    url: "/api/plugins/context-mode/toggle",
    payload: {
      enabled: false,
      scope: "user"
    }
  });
  assert.equal(toggleRes.statusCode, 200);
  const toggleBody = toggleRes.json();
  assert.equal(toggleBody.plugin.enabled, false);

  // Delete
  const deleteRes = await server.inject({
    method: "DELETE",
    url: "/api/plugins/context-mode?scope=user"
  });
  assert.equal(deleteRes.statusCode, 200);
  assert.equal(deleteRes.json().deleted, true);

  await server.close();
  fs.rmSync(tmp, { recursive: true, force: true });
});
