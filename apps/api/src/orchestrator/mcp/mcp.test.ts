import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import assert from "node:assert/strict";
import { McpConfigStore } from "./McpConfigStore.js";
import { adaptMcpToolsToSchemas, formatMcpToolResult } from "./mcpToolAdapter.js";
import type { McpToolDefinition } from "./types.js";

test("McpConfigStore saves, lists, toggles, and deletes server configs", () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-mcp-test-"));
  const userConfig = path.join(tmp, "mcp_servers.json");
  const store = new McpConfigStore(userConfig);

  // 1. Save user config
  store.saveConfig({
    name: "github",
    scope: "user",
    transport: "stdio",
    command: "npx",
    args: ["-y", "@modelcontextprotocol/server-github"],
    env: { GITHUB_TOKEN: "tok_123" },
    enabled: true
  });

  const list1 = store.listConfigs();
  assert.equal(list1.length, 1);
  assert.equal(list1[0].name, "github");
  assert.equal(list1[0].enabled, true);
  assert.equal(list1[0].env?.GITHUB_TOKEN, "tok_123");

  // 2. Toggle config
  store.toggleConfig("github", false, "user");
  const list2 = store.listConfigs();
  assert.equal(list2[0].enabled, false);

  // 3. Delete config
  const deleted = store.deleteConfig("github", "user");
  assert.equal(deleted, true);
  assert.equal(store.listConfigs().length, 0);

  fs.rmSync(tmp, { recursive: true, force: true });
});

test("McpConfigStore project config overrides user config with same name", () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-mcp-proj-"));
  const userConfig = path.join(tmp, "user", "mcp_servers.json");
  const projectPath = path.join(tmp, "project");
  fs.mkdirSync(projectPath, { recursive: true });

  const store = new McpConfigStore(userConfig);

  // Save user github
  store.saveConfig({
    name: "github",
    scope: "user",
    transport: "stdio",
    command: "npx-user",
    enabled: true
  });

  // Save project github
  store.saveConfig(
    {
      name: "github",
      scope: "project",
      transport: "stdio",
      command: "npx-project",
      enabled: true
    },
    projectPath
  );

  // Save project-only postgres
  store.saveConfig(
    {
      name: "postgres",
      scope: "project",
      transport: "stdio",
      command: "npx-pg",
      enabled: true
    },
    projectPath
  );

  const list = store.listConfigs(projectPath);
  assert.equal(list.length, 2);
  const gh = list.find((c) => c.name === "github");
  assert.ok(gh);
  assert.equal(gh!.command, "npx-project");
  assert.equal(gh!.scope, "project");

  fs.rmSync(tmp, { recursive: true, force: true });
});

test("adaptMcpToolsToSchemas converts MCP tools into OpenAI-compatible functions", () => {
  const mcpTools: McpToolDefinition[] = [
    {
      name: "mcp__github__create_issue",
      originalName: "create_issue",
      serverName: "github",
      description: "Create an issue in a GitHub repository",
      inputSchema: {
        type: "object",
        properties: {
          title: { type: "string", description: "Issue title" },
          body: { type: "string", description: "Issue body" }
        },
        required: ["title"]
      }
    }
  ];

  const schemas = adaptMcpToolsToSchemas(mcpTools);
  assert.equal(schemas.length, 1);
  assert.equal(schemas[0].type, "function");
  assert.equal(schemas[0].function.name, "mcp__github__create_issue");
  assert.equal(schemas[0].function.description, "Create an issue in a GitHub repository");
  assert.deepEqual(schemas[0].function.parameters.required, ["title"]);
});

test("formatMcpToolResult handles text and errors properly", () => {
  const okResult = formatMcpToolResult({
    content: [{ type: "text", text: "Created issue #42" }],
    isError: false
  });
  assert.equal(okResult, "Created issue #42");

  const errResult = formatMcpToolResult({
    content: [{ type: "text", text: "Repository not found" }],
    isError: true
  });
  assert.match(errResult, /"success":false/);
  assert.match(errResult, /Repository not found/);
});
