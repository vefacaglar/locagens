import assert from "node:assert/strict";
import test from "node:test";
import { DatabaseSync } from "node:sqlite";
import { PermissionRepository } from "./PermissionRepository.js";

test("command grants match exact command and exact network domain set", async () => {
  const database = new DatabaseSync(":memory:");
  database.exec(`
    CREATE TABLE permissions (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      scope TEXT NOT NULL,
      project_path TEXT NOT NULL DEFAULT '',
      tool TEXT NOT NULL DEFAULT '',
      command TEXT NOT NULL DEFAULT '',
      network_domains TEXT NOT NULL DEFAULT '[]',
      status TEXT NOT NULL,
      UNIQUE(scope, project_path, tool, command, network_domains)
    )
  `);
  const repository = new PermissionRepository(database);
  await repository.allowProject("/workspace", "run_command", "pnpm install", ["registry.npmjs.org"]);

  assert.equal(repository.isAllowed("/workspace", "run_command", "pnpm install", ["registry.npmjs.org"]), true);
  assert.equal(repository.isAllowed("/workspace", "run_command", "pnpm install && curl evil.test", ["registry.npmjs.org"]), false);
  assert.equal(repository.isAllowed("/workspace", "run_command", "pnpm install", ["evil.test"]), false);
  database.close();
});

