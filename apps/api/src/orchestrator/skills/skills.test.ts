import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import assert from "node:assert/strict";
import { parseSkillMd } from "./parseSkillMd.js";
import { SkillRegistry, findSkill } from "./SkillRegistry.js";
import { formatSkillCatalog } from "./formatSkillCatalog.js";

function writeSkill(dir: string, name: string, description: string, body = "# Body\n") {
  const skillDir = path.join(dir, name);
  fs.mkdirSync(skillDir, { recursive: true });
  fs.writeFileSync(
    path.join(skillDir, "SKILL.md"),
    `---\nname: ${name}\ndescription: "${description}"\n---\n${body}`,
    "utf-8"
  );
}

test("parseSkillMd accepts valid frontmatter", () => {
  const parsed = parseSkillMd(`---
name: deploy-model
description: "Deploy Azure OpenAI models. USE FOR: deploy gpt."
---
# Deploy
Do the thing.
`);
  assert.ok(parsed);
  assert.equal(parsed!.name, "deploy-model");
  assert.match(parsed!.description, /Deploy Azure/);
  assert.match(parsed!.body, /Do the thing/);
});

test("parseSkillMd rejects missing name or description", () => {
  assert.equal(parseSkillMd("---\ndescription: only\n---\nbody"), null);
  assert.equal(parseSkillMd("---\nname: x\n---\nbody"), null);
  assert.equal(parseSkillMd("no frontmatter"), null);
});

test("parseSkillMd rejects unsafe names", () => {
  assert.equal(parseSkillMd("---\nname: ../escape\ndescription: x\n---\n"), null);
  assert.equal(parseSkillMd("---\nname: Bad Name\ndescription: x\n---\n"), null);
});

test("SkillRegistry discovers user and project skills with project override", () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-skills-"));
  const userRoot = path.join(tmp, "user-skills");
  const projectPath = path.join(tmp, "project");
  const projectSkills = path.join(projectPath, ".locagens", "skills");
  fs.mkdirSync(projectPath, { recursive: true });

  writeSkill(userRoot, "shared", "User version of shared skill");
  writeSkill(userRoot, "user-only", "Only in user root");
  writeSkill(projectSkills, "shared", "Project version of shared skill");
  writeSkill(projectSkills, "project-only", "Only in project");

  // Invalid skill is skipped
  fs.mkdirSync(path.join(userRoot, "broken"), { recursive: true });
  fs.writeFileSync(path.join(userRoot, "broken", "SKILL.md"), "not valid", "utf-8");

  const registry = new SkillRegistry(userRoot);
  const skills = registry.discover(projectPath);
  const names = skills.map(s => s.name).sort();
  assert.deepEqual(names, ["project-only", "shared", "user-only"]);

  const shared = findSkill(skills, "shared");
  assert.ok(shared);
  assert.equal(shared!.source, "project");
  assert.match(shared!.description, /Project version/);

  const userOnly = findSkill(skills, "USER-ONLY");
  assert.ok(userOnly);
  assert.equal(userOnly!.source, "user");

  fs.rmSync(tmp, { recursive: true, force: true });
});

test("SkillRegistry ignores symlink escapes outside root", () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-skills-esc-"));
  const userRoot = path.join(tmp, "user-skills");
  const outside = path.join(tmp, "outside");
  fs.mkdirSync(userRoot, { recursive: true });
  fs.mkdirSync(outside, { recursive: true });
  fs.writeFileSync(
    path.join(outside, "SKILL.md"),
    `---\nname: evil\ndescription: "should not load"\n---\n# no\n`,
    "utf-8"
  );
  try {
    fs.symlinkSync(outside, path.join(userRoot, "evil"), "dir");
  } catch {
    // Some CI environments disallow symlinks; skip assertion in that case.
    fs.rmSync(tmp, { recursive: true, force: true });
    return;
  }

  writeSkill(userRoot, "safe", "Safe skill");
  const registry = new SkillRegistry(userRoot);
  const skills = registry.discover();
  assert.equal(skills.length, 1);
  assert.equal(skills[0].name, "safe");

  fs.rmSync(tmp, { recursive: true, force: true });
});

test("formatSkillCatalog is empty without skills and lists names when present", () => {
  assert.equal(formatSkillCatalog([]), "");
  const text = formatSkillCatalog([
    { name: "a", description: "Alpha skill", source: "user" },
    { name: "b", description: "Beta skill", source: "project" }
  ]);
  assert.match(text, /AVAILABLE SKILLS/);
  assert.match(text, /load_skill/);
  assert.match(text, /- a \(user\): Alpha skill/);
  assert.match(text, /- b \(project\): Beta skill/);
});
