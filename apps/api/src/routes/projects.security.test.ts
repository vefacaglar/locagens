import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import test from "node:test";
import Fastify from "fastify";
import type { Project } from "@locagens/shared";
import { registerProjectRoutes } from "./projects.js";

function git(cwd: string, args: string[]): string {
  return execFileSync("git", args, { cwd, encoding: "utf-8" });
}

function projectRepo(project: Project) {
  return {
    list: () => [project],
    get: (candidate: string) => candidate === project.path ? project : null,
    create: async () => undefined,
    delete: async () => undefined
  };
}

test("git diff endpoint leaves the index untouched and commit messages remain literal", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-git-test-"));
  const project: Project = { path: fs.realpathSync.native(root), name: "test", createdAt: new Date().toISOString() };
  try {
    git(root, ["init"]);
    git(root, ["config", "user.email", "test@example.com"]);
    git(root, ["config", "user.name", "Locagens Test"]);
    fs.writeFileSync(path.join(root, "tracked.txt"), "initial\n");
    git(root, ["add", "tracked.txt"]);
    git(root, ["commit", "-m", "initial"]);
    fs.writeFileSync(path.join(root, "untracked.txt"), "new\n");
    const indexBefore = git(root, ["write-tree"]).trim();

    const server = Fastify();
    registerProjectRoutes(server, { projectRepo: projectRepo(project) } as any);
    const diffResponse = await server.inject({ method: "GET", url: `/api/projects/git/diff-details?path=${encodeURIComponent(project.path)}` });
    assert.equal(diffResponse.statusCode, 200, diffResponse.body);
    assert.equal(git(root, ["write-tree"]).trim(), indexBefore);
    assert.ok(diffResponse.json().files.some((file: any) => file.path === "untracked.txt"));

    const marker = path.join(root, "injected.txt");
    const malicious = `security test \" $(touch ${marker}) \`touch ${marker}\`\nsecond line`;
    const commitResponse = await server.inject({
      method: "POST",
      url: "/api/projects/git/commit",
      payload: { path: project.path, message: malicious, action: "commit" }
    });
    assert.equal(commitResponse.statusCode, 200, commitResponse.body);
    assert.equal(fs.existsSync(marker), false);
    assert.equal(git(root, ["log", "-1", "--pretty=%B"]).trim(), malicious.trim());
    await server.close();
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

