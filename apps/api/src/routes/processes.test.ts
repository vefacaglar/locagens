import test from "node:test";
import assert from "node:assert/strict";
import fastify from "fastify";
import { registerProcessRoutes } from "./processes.js";
import { ProcessManager } from "../orchestrator/processes/ProcessManager.js";

test("Process routes spawn, logs, kill cycle", async () => {
  const server = fastify();
  const processManager = new ProcessManager();
  const mockRepo: any = {
    list: () => [{ path: process.cwd(), name: "Workspace" }],
    get: (p: string) => ({ path: p, name: "Workspace" }),
    create: () => {},
    delete: () => true
  };

  const mockCtx: any = {
    processManager,
    projectRepo: mockRepo,
    defaultProjectPath: process.cwd()
  };

  registerProcessRoutes(server, mockCtx);

  // 1. Spawn process
  const spawnRes = await server.inject({
    method: "POST",
    url: "/api/processes/spawn",
    payload: {
      command: 'node -e "console.log(\'route test running\'); setTimeout(() => {}, 5000)"',
      projectPath: process.cwd()
    }
  });

  assert.equal(spawnRes.statusCode, 200);
  const spawnBody = JSON.parse(spawnRes.body);
  assert.ok(spawnBody.process);
  const procId = spawnBody.process.id;

  await new Promise((r) => setTimeout(r, 300));

  // 2. Get list
  const listRes = await server.inject({
    method: "GET",
    url: "/api/processes"
  });
  assert.equal(listRes.statusCode, 200);
  const listBody = JSON.parse(listRes.body);
  assert.ok(listBody.processes.some((p: any) => p.id === procId));

  // 3. Get logs
  const logsRes = await server.inject({
    method: "GET",
    url: `/api/processes/${procId}/logs`
  });
  assert.equal(logsRes.statusCode, 200);
  const logsBody = JSON.parse(logsRes.body);
  assert.ok(logsBody.logs.some((l: any) => l.text.includes("route test running")));

  // 4. Kill process
  const killRes = await server.inject({
    method: "POST",
    url: `/api/processes/${procId}/kill`
  });
  assert.equal(killRes.statusCode, 200);
  const killBody = JSON.parse(killRes.body);
  assert.equal(killBody.success, true);
});
