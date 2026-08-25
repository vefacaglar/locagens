import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import assert from "node:assert/strict";
import { PluginRegistry, BUILTIN_TEMPLATES } from "./PluginRegistry.js";
import { PluginHookRunner } from "./PluginHookRunner.js";

test("PluginRegistry returns builtin templates including context-mode", () => {
  const registry = new PluginRegistry();
  const templates = registry.getTemplates();
  assert.ok(templates.length >= 1);
  const contextMode = templates.find(t => t.id === "context-mode");
  assert.ok(contextMode);
  assert.equal(contextMode!.author, "mksglu");
  assert.ok(contextMode!.manifest.mcpServers?.["context-mode"]);
});

test("PluginRegistry discovers user and project plugins with project override", async () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-plugins-"));
  const userRoot = path.join(tmp, "user-plugins");
  const projectPath = path.join(tmp, "project");

  fs.mkdirSync(userRoot, { recursive: true });
  fs.mkdirSync(projectPath, { recursive: true });

  const registry = new PluginRegistry(userRoot);

  // Install template into user scope
  const installedUser = await registry.install({
    source: "template",
    uri: "context-mode",
    scope: "user"
  });
  assert.equal(installedUser.id, "context-mode");
  assert.equal(installedUser.scope, "user");

  // Discovered in user scope
  let discovered = registry.discover();
  assert.equal(discovered.length, 1);
  assert.equal(discovered[0].id, "context-mode");

  // Install custom plugin into project scope
  const installedProj = await registry.install({
    source: "local",
    uri: "custom-linter",
    scope: "project",
    projectPath,
    customManifest: {
      id: "custom-linter",
      name: "Custom Linter",
      description: "Project-level custom linter",
      enabled: true
    }
  });
  assert.equal(installedProj.id, "custom-linter");

  // Discover with projectPath
  discovered = registry.discover(projectPath);
  assert.equal(discovered.length, 2);

  // Toggle plugin
  const toggled = registry.toggle("context-mode", false, "user");
  assert.ok(toggled);
  assert.equal(toggled!.enabled, false);

  const active = registry.getActivePlugins(projectPath);
  assert.equal(active.length, 1);
  assert.equal(active[0].id, "custom-linter");

  // Delete project plugin
  const deleted = registry.delete("custom-linter", "project", projectPath);
  assert.equal(deleted, true);

  discovered = registry.discover(projectPath);
  assert.equal(discovered.length, 1);
});

test("PluginHookRunner processes context-mode output sandboxing", async () => {
  const runner = new PluginHookRunner();

  // Create a massive output (> 300 lines)
  const lines = Array.from({ length: 400 }, (_, i) => `Line ${i + 1}: log entry details`);
  const massiveOutput = lines.join("\n");

  const contextModePlugin = {
    id: "context-mode",
    name: "Context Mode",
    version: "1.0.0",
    description: "Context optimization",
    scope: "user" as const,
    enabled: true,
    hooks: {
      postToolUse: "internal:context-mode-sandbox"
    }
  };

  const result = await runner.runPostToolUse({
    runId: "run-test",
    toolName: "run_command",
    args: { command: "test" },
    rawResult: massiveOutput
  }, [contextModePlugin]);

  assert.ok(result.result.includes("Context-Mode: Sandboxed 200 middle lines"));
  assert.ok(result.result.startsWith("Line 1: log entry details"));
  assert.ok(result.result.endsWith("Line 400: log entry details"));
});

test("PluginHookRunner handles session start system prompt injection", async () => {
  const runner = new PluginHookRunner();

  const plugin = {
    id: "test-plugin",
    name: "Test Plugin",
    version: "1.0.0",
    description: "Test",
    scope: "user" as const,
    enabled: true,
    systemPrompt: "Extra rules for test plugin."
  };

  const result = await runner.runSessionStart({
    runId: "run-test",
    systemPrompt: "Base prompt."
  }, [plugin]);

  assert.ok(result.systemPromptSupplement);
  assert.match(result.systemPromptSupplement!, /\[Plugin: Test Plugin\]/);
  assert.match(result.systemPromptSupplement!, /Extra rules for test plugin\./);
});
