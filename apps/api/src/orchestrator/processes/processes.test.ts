import test from "node:test";
import assert from "node:assert/strict";
import { ProcessManager } from "./ProcessManager.js";

test("ProcessManager spawns, lists, captures logs and kills a process", async () => {
  const pm = new ProcessManager();

  // Spawn an echo/sleep process
  const proc = pm.spawnProcess({
    command: 'node -e "console.log(\'hello from background\'); setTimeout(() => {}, 5000)"',
    projectPath: process.cwd()
  });

  assert.ok(proc.id.startsWith("proc-"));
  assert.equal(proc.status, "running");

  // Wait a bit for stdout
  await new Promise((r) => setTimeout(r, 300));

  const list = pm.list();
  assert.ok(list.some((p) => p.id === proc.id));

  const logs = pm.getLogs(proc.id);
  assert.ok(logs.some((l) => l.text.includes("hello from background")));

  // Kill
  const killed = await pm.kill(proc.id);
  assert.equal(killed, true);

  const updated = pm.get(proc.id);
  assert.equal(updated?.status, "stopped");
});
