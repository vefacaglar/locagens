import test from "node:test";
import assert from "node:assert";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { execute, handleLine, initDB } from "./core.js";

function openTestDB(t: test.TestContext): DatabaseSync {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "db-writer-test-"));
  const db = new DatabaseSync(path.join(dir, "locagens.db"));
  initDB(db);
  t.after(() => {
    db.close();
    fs.rmSync(dir, { recursive: true, force: true });
  });
  return db;
}

test("execute run.create and message.create updates run last_active_at", (t) => {
  const db = openTestDB(t);

  execute(db, {
    id: "1",
    op: "run.create",
    args: {
      run: {
        id: "run-1",
        title: "Title",
        task: "Task",
        projectPath: "/tmp/project",
        projectName: "project",
        status: "created",
        providerId: "provider",
        providerDisplayName: "Provider",
        model: "model",
        createdAt: "2026-01-01T00:00:00Z",
        updatedAt: "2026-01-01T00:00:00Z"
      }
    }
  });

  execute(db, {
    id: "2",
    op: "message.create",
    args: {
      message: {
        id: "msg-1",
        runId: "run-1",
        role: "assistant",
        content: "hello",
        createdAt: "2026-01-01T00:00:01Z"
      }
    }
  });

  const row = db.prepare("SELECT status, last_active_at FROM runs WHERE id = ?").get("run-1") as { status: string; last_active_at: string };
  assert.strictEqual(row.status, "created");
  assert.strictEqual(row.last_active_at, "2026-01-01T00:00:01Z");
});

test("execute memory.create returns lastInsertRowid and memory.update applies", (t) => {
  const db = openTestDB(t);

  const result = execute(db, {
    id: "1",
    op: "memory.create",
    args: {
      scope: "project",
      projectPath: "/tmp/project",
      category: "project",
      content: "Remember this",
      now: "2026-01-01T00:00:00Z"
    }
  }) as { lastInsertRowid: number };
  assert.ok(Number.isInteger(result.lastInsertRowid) && result.lastInsertRowid > 0);

  execute(db, {
    id: "2",
    op: "memory.update",
    args: {
      id: result.lastInsertRowid,
      content: "Updated",
      now: "2026-01-01T00:00:01Z"
    }
  });

  const row = db.prepare("SELECT content FROM memory WHERE id = ?").get(result.lastInsertRowid) as { content: string };
  assert.strictEqual(row.content, "Updated");
});

test("handleLine returns error response for invalid json", (t) => {
  const db = openTestDB(t);
  const res = handleLine(db, "{not json");
  assert.ok(res);
  assert.strictEqual(res.id, "");
  assert.strictEqual(res.ok, false);
  assert.match(res.error ?? "", /^invalid json: /);
});

test("handleLine returns error response for unknown op", (t) => {
  const db = openTestDB(t);
  const res = handleLine(db, JSON.stringify({ id: "9", op: "nope.nope", args: {} }));
  assert.ok(res);
  assert.strictEqual(res.id, "9");
  assert.strictEqual(res.ok, false);
  assert.match(res.error ?? "", /unknown op "nope\.nope"/);
});

test("handleLine skips empty lines and returns ok with id for success", (t) => {
  const db = openTestDB(t);
  assert.strictEqual(handleLine(db, "   "), null);

  const res = handleLine(db, JSON.stringify({
    id: "3",
    op: "project.create",
    args: { project: { path: "/tmp/p", name: "p", createdAt: "2026-01-01T00:00:00Z" } }
  }));
  assert.ok(res);
  assert.strictEqual(res.id, "3");
  assert.strictEqual(res.ok, true);
  assert.ok(res.result && typeof res.result === "object" && "changes" in res.result);
});

test("execute UNIQUE constraint violation surfaces sqlite message", (t) => {
  const db = openTestDB(t);
  const req = {
    id: "1",
    op: "run.create",
    args: {
      run: {
        id: "run-dup",
        title: "T",
        task: "T",
        status: "created",
        providerId: "p",
        providerDisplayName: "P",
        model: "m",
        createdAt: "2026-01-01T00:00:00Z",
        updatedAt: "2026-01-01T00:00:00Z"
      }
    }
  };
  execute(db, req);
  assert.throws(() => execute(db, req), /UNIQUE constraint failed/);
});
