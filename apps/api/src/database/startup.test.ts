import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { DatabaseSync } from "node:sqlite";
import { normalizeStoredProjectPaths } from "./startup.js";

test("normalizes legacy project paths and their project-scoped references", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-project-path-"));
  const canonicalPath = fs.realpathSync.native(root);
  const legacyPath = `${canonicalPath}${path.sep}`;
  const db = new DatabaseSync(":memory:");

  try {
    db.exec(`
      CREATE TABLE projects (path TEXT PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL);
      CREATE TABLE runs (id TEXT PRIMARY KEY, project_path TEXT NOT NULL);
      CREATE TABLE memory (id INTEGER PRIMARY KEY, project_path TEXT NOT NULL);
      CREATE TABLE permissions (
        id INTEGER PRIMARY KEY,
        project_path TEXT NOT NULL,
        tool TEXT NOT NULL,
        command TEXT NOT NULL,
        network_domains TEXT NOT NULL,
        UNIQUE(project_path, tool, command, network_domains)
      );
    `);
    db.prepare("INSERT INTO projects VALUES (?, 'Legacy', '2026-01-01')").run(legacyPath);
    db.prepare("INSERT INTO runs VALUES ('run-1', ?)").run(legacyPath);
    db.prepare("INSERT INTO memory VALUES (1, ?)").run(legacyPath);
    db.prepare("INSERT INTO permissions VALUES (1, ?, 'run_command', 'pwd', '[]')").run(legacyPath);

    normalizeStoredProjectPaths(db);

    assert.equal((db.prepare("SELECT path FROM projects").get() as any).path, canonicalPath);
    assert.equal((db.prepare("SELECT project_path FROM runs").get() as any).project_path, canonicalPath);
    assert.equal((db.prepare("SELECT project_path FROM memory").get() as any).project_path, canonicalPath);
    assert.equal((db.prepare("SELECT project_path FROM permissions").get() as any).project_path, canonicalPath);
  } finally {
    db.close();
    fs.rmSync(root, { recursive: true, force: true });
  }
});
