import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import assert from "node:assert/strict";
import Fastify from "fastify";
import { SkillRegistry } from "../orchestrator/skills/index.js";
import { registerSkillRoutes } from "./skills.js";

function makeCtx(userRoot: string, projectRepo?: { get: (p: string) => unknown; list: () => any[] }) {
  return {
    skillRegistry: new SkillRegistry(userRoot),
    projectRepo: projectRepo ?? {
      get: () => null,
      list: () => []
    }
  } as any;
}

test("POST /api/skills/install writes SKILL.md for user target", async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-skills-route-"));
  const userRoot = path.join(tmp, "skills");
  const server = Fastify({ bodyLimit: 256 * 1024 });
  registerSkillRoutes(server, makeCtx(userRoot));
  await server.ready();

  const content = `---
name: browser-skill
description: "Installed from browser file picker"
---
# Steps
1. Do the thing.
`;

  const res = await server.inject({
    method: "POST",
    url: "/api/skills/install",
    payload: { target: "user", content }
  });
  assert.equal(res.statusCode, 200);
  const body = res.json();
  assert.equal(body.success, true);
  assert.equal(body.skill.name, "browser-skill");

  const written = fs.readFileSync(path.join(userRoot, "browser-skill", "SKILL.md"), "utf-8");
  assert.match(written, /Do the thing/);

  const list = await server.inject({ method: "GET", url: "/api/skills" });
  assert.equal(list.statusCode, 200);
  const listed = list.json();
  assert.equal(listed.skills.length, 1);
  assert.equal(listed.skills[0].name, "browser-skill");
  assert.equal(listed.roots.user, path.resolve(userRoot));

  await server.close();
  fs.rmSync(tmp, { recursive: true, force: true });
});

test("POST /api/skills/install rejects invalid SKILL.md", async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-skills-route-bad-"));
  const server = Fastify();
  registerSkillRoutes(server, makeCtx(path.join(tmp, "skills")));
  await server.ready();

  const res = await server.inject({
    method: "POST",
    url: "/api/skills/install",
    payload: { target: "user", content: "not valid" }
  });
  assert.equal(res.statusCode, 400);
  assert.match(res.json().error, /Invalid SKILL/);

  await server.close();
  fs.rmSync(tmp, { recursive: true, force: true });
});

test("DELETE /api/skills/:name deletes skill", async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-skills-route-del-"));
  const userRoot = path.join(tmp, "skills");
  const server = Fastify();
  registerSkillRoutes(server, makeCtx(userRoot));
  await server.ready();

  // Install first
  const content = `---
name: del-test
description: "To be deleted"
---
# Body
`;
  await server.inject({
    method: "POST",
    url: "/api/skills/install",
    payload: { target: "user", content }
  });

  const listBefore = await server.inject({ method: "GET", url: "/api/skills" });
  assert.equal(listBefore.json().skills.length, 1);

  const delRes = await server.inject({
    method: "DELETE",
    url: "/api/skills/del-test",
    payload: { target: "user" }
  });
  assert.equal(delRes.statusCode, 200);
  assert.equal(delRes.json().success, true);
  assert.equal(delRes.json().deleted, true);

  const listAfter = await server.inject({ method: "GET", url: "/api/skills" });
  assert.equal(listAfter.json().skills.length, 0);

  await server.close();
  fs.rmSync(tmp, { recursive: true, force: true });
});

test("POST /api/skills/open-folder returns allowlisted folder path", async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-skills-route-folder-"));
  const userRoot = path.join(tmp, "skills");
  const server = Fastify();
  registerSkillRoutes(server, makeCtx(userRoot));
  await server.ready();

  const res = await server.inject({
    method: "POST",
    url: "/api/skills/open-folder",
    payload: { target: "user" }
  });
  assert.equal(res.statusCode, 200);
  assert.equal(res.json().target, "user");
  assert.ok(fs.existsSync(res.json().path));

  await server.close();
  fs.rmSync(tmp, { recursive: true, force: true });
});
