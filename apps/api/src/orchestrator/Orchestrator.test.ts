import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import assert from "node:assert";
import { db } from "../database/db.js";

// Set test environment so db.ts uses :memory: database
process.env.NODE_ENV = "test";

import { ProviderRegistry } from "../providers/ProviderRegistry.js";
import { RunRepository, MessageRepository, PlanRepository, MemoryRepository, UsageLogRepository } from "../database/repositories.js";
import { Orchestrator } from "./Orchestrator.js";
import { eventBus } from "./eventBus.js";
import { buildCoderSystemPrompt, buildSystemPrompt, buildUtilitySystemPrompt } from "./systemPrompt.js";
import { calculateCost } from "./pricing.js";
import {
  ASK_QUESTION_TOOL,
  DELEGATE_TASKS_TOOL,
  DELEGATE_UTILITY_TOOL,
  commandScansOutsideWorkspace,
  executeWorkspaceToolAsync,
  REMEMBER_TOOL,
  LOAD_SKILL_TOOL,
  SET_TITLE_TOOL,
  UPDATE_PLAN_TOOL,
  WORKSPACE_TOOLS
} from "./workspaceTools.js";
import type { Run, RunStatus } from "@locagens/shared";

// Mock config content
const mockConfig = {
  providers: {
    "test-provider": {
      type: "openai-compatible",
      displayName: "Test Provider",
      baseUrl: "http://localhost:9999/model",
      apiKey: "mock-key",
      models: ["model-1"]
    }
  }
};

test("Orchestrator Integration Tests", async (t) => {
  const testConfigPath = path.join(process.cwd(), "providers.orch-test.json");
  fs.writeFileSync(testConfigPath, JSON.stringify(mockConfig, null, 2));

  t.after(() => {
    if (fs.existsSync(testConfigPath)) {
      fs.unlinkSync(testConfigPath);
    }
  });

  const originalFetch = globalThis.fetch;
  t.afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  await t.test("Orchestrator - runs single model completion", async () => {
    const registry = new ProviderRegistry(testConfigPath);
    const runRepo = new RunRepository(db);
    const messageRepo = new MessageRepository(db);
    const orchestrator = new Orchestrator(runRepo, messageRepo, registry, new PlanRepository(db), new MemoryRepository(db), new UsageLogRepository(db));

    const runId = "run-test-123";
    const runData: Run = {
      id: runId,
      title: "Test Task",
      task: "Hello model",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    };

    await runRepo.create(runData);

    // Track status transitions via eventBus
    const statusesEmitted: RunStatus[] = [];
    const eventListener = (event: any) => {
      if (event.type === "status_changed") {
        statusesEmitted.push(event.status);
      }
    };
    eventBus.on(`run:${runId}`, eventListener);
    t.after(() => {
      eventBus.off(`run:${runId}`, eventListener);
    });

    // Mock fetch to simulate model response
    globalThis.fetch = async (url: any, options: any) => {
      return {
        ok: true,
        json: async () => ({
          choices: [{ message: { role: "assistant", content: "Hello back!" } }]
        })
      } as any;
    };

    // Trigger run execution
    await orchestrator.run(runId);

    // Verify database record has done status
    const finishedRun = runRepo.getById(runId);
    assert.ok(finishedRun);
    assert.strictEqual(finishedRun.status, "done");

    // Verify message saved
    const savedMsgs = messageRepo.listByRunId(runId);
    assert.strictEqual(savedMsgs.length, 1);
    assert.strictEqual(savedMsgs[0].content, "Hello back!");
    assert.strictEqual(savedMsgs[0].role, "assistant");

    // Assert correct state transitions sequence
    assert.deepStrictEqual(statusesEmitted, [
      "generating",
      "done"
    ]);
  });

  await t.test("Orchestrator - runs tool call write_file and saves results", async () => {
    const registry = new ProviderRegistry(testConfigPath);
    const runRepo = new RunRepository(db);
    const messageRepo = new MessageRepository(db);
    const orchestrator = new Orchestrator(runRepo, messageRepo, registry, new PlanRepository(db), new MemoryRepository(db), new UsageLogRepository(db));

    const runId = "run-test-tool-call";
    const runData: Run = {
      id: runId,
      title: "Test Tool Call",
      task: "Create selam.json with {}",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      projectPath: process.cwd(),
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    };

    await runRepo.create(runData);

    const createdFilePath = path.join(process.cwd(), "test_output_file.json");
    t.after(() => {
      if (fs.existsSync(createdFilePath)) {
        fs.unlinkSync(createdFilePath);
      }
    });

    let callCount = 0;
    globalThis.fetch = async (url: any, options: any) => {
      callCount++;
      if (callCount === 1) {
        // Return tool call
        return {
          ok: true,
          json: async () => ({
            choices: [{
              message: {
                role: "assistant",
                content: "Sure, let me create the file.",
                tool_calls: [{
                  id: "call_test_1",
                  type: "function",
                  function: {
                    name: "write_file",
                    arguments: JSON.stringify({ path: "test_output_file.json", content: "{}" })
                  }
                }]
              }
            }]
          })
        } as any;
      } else {
        // Return text response after tool call is processed
        return {
          ok: true,
          json: async () => ({
            choices: [{
              message: {
                role: "assistant",
                content: "I have successfully created the file."
              }
            }]
          })
        } as any;
      }
    };

    await orchestrator.run(runId);

    // Verify file is actually created
    assert.ok(fs.existsSync(createdFilePath));
    assert.strictEqual(fs.readFileSync(createdFilePath, "utf-8"), "{}");

    // Verify messages saved (1 assistant tools, 1 tool output, 1 final assistant text response)
    const savedMsgs = messageRepo.listByRunId(runId);
    assert.strictEqual(savedMsgs.length, 3);
    assert.strictEqual(savedMsgs[0].role, "assistant");
    assert.ok(savedMsgs[0].rawResponse?.includes("write_file"));
    assert.strictEqual(savedMsgs[1].role, "tool");
    assert.ok(savedMsgs[1].content.includes("success"));
    assert.strictEqual(savedMsgs[2].role, "assistant");
    assert.strictEqual(savedMsgs[2].content, "I have successfully created the file.");
  });

  await t.test("Orchestrator - injects mode instructions into system prompt", async () => {
    const registry = new ProviderRegistry(testConfigPath);
    const runRepo = new RunRepository(db);
    const messageRepo = new MessageRepository(db);
    const orchestrator = new Orchestrator(runRepo, messageRepo, registry, new PlanRepository(db), new MemoryRepository(db), new UsageLogRepository(db));

    const runId = "run-test-mode-prompt";
    const runData: Run = {
      id: runId,
      title: "Test Mode task",
      task: "Tell me a plan",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      mode: "plan",
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    };

    await runRepo.create(runData);

    let systemPromptUsed = "";
    globalThis.fetch = async (url: any, options: any) => {
      const parsed = JSON.parse(options.body);
      const systemMessage = parsed.messages.find((m: any) => m.role === "system");
      if (systemMessage) {
        systemPromptUsed = systemMessage.content;
      }
      return {
        ok: true,
        json: async () => ({
          choices: [{ message: { role: "assistant", content: "I suggest plan A." } }]
        })
      } as any;
    };

    await orchestrator.run(runId);

    // Verify mode system prompt addition was present
    assert.ok(systemPromptUsed.includes("CURRENT OPERATIONAL MODE: PLAN MODE"));
    assert.ok(systemPromptUsed.includes("DO NOT call any file-mutating tools"));
    assert.ok(systemPromptUsed.includes("Do NOT implement"));
    assert.ok(systemPromptUsed.includes("Do NOT write production code"));
    assert.ok(systemPromptUsed.includes("do not treat repeated or pasted plan text as approval"));
    assert.ok(systemPromptUsed.includes("INITIAL PROJECT GUIDANCE"));
    assert.ok(systemPromptUsed.includes("AGENTS.md"));
    assert.ok(systemPromptUsed.includes("CLAUDE.md"));
  });

  await t.test("System prompt - does not inject project guidance in chat mode", () => {
    const prompt = buildSystemPrompt({ projectName: "Test Project", projectPath: "/tmp/test-project", mode: "chat", shouldReadProjectGuidance: true });

    assert.ok(prompt.includes("CURRENT OPERATIONAL MODE: CHAT MODE"));
    assert.ok(prompt.includes("Do not claim you can run commands"));
    assert.ok(prompt.includes("sub-agents"));
    assert.ok(prompt.includes("Do not use emojis"));
    assert.ok(prompt.includes("Do not use bold text except for real section headings"));
    assert.ok(!prompt.includes("INITIAL PROJECT GUIDANCE"));
    assert.ok(!prompt.includes("Before doing substantive planning or implementation"));
  });

  await t.test("System prompt - stays compact while preserving guardrails", () => {
    const projectName = "Test Project";
    const projectPath = "/tmp/test-project";
    const delegation = { coderModel: "coder-model", maxSubAgents: 3, utilityModel: "utility-model" };

    const chat = buildSystemPrompt({ projectName, projectPath, mode: "chat", shouldReadProjectGuidance: true });
    const plan = buildSystemPrompt({ projectName, projectPath, mode: "plan", shouldReadProjectGuidance: true });
    const build = buildSystemPrompt({ projectName, projectPath, mode: "accept_edits", shouldReadProjectGuidance: true });
    const architect = buildSystemPrompt({ projectName, projectPath, mode: "accept_edits", shouldReadProjectGuidance: true, delegation });
    const coder = buildCoderSystemPrompt(projectName, projectPath, "Implement the requested change");
    const utility = buildUtilitySystemPrompt(projectName, projectPath, "Find the relevant file");

    assert.ok(chat.length <= 1500, `chat prompt too large: ${chat.length}`);
    assert.ok(plan.length <= 4500, `plan prompt too large: ${plan.length}`);
    assert.ok(build.length <= 4000, `build prompt too large: ${build.length}`);
    assert.ok(architect.length <= 7500, `architect prompt too large: ${architect.length}`);
    assert.ok(coder.length <= 1500, `coder prompt too large: ${coder.length}`);
    assert.ok(utility.length <= 1000, `utility prompt too large: ${utility.length}`);

    assert.ok(plan.includes("Do NOT implement"));
    assert.ok(plan.includes("Do NOT write production code"));
    assert.ok(plan.includes("do not treat repeated or pasted plan text as approval"));
    assert.ok(plan.includes("DO NOT call any file-mutating tools"));
    assert.ok(plan.includes("Do not use emojis in visible conversation"));
    assert.ok(plan.includes("Do not use bold text except for real section headings"));
    assert.ok(build.includes("implementation must stay strictly within it"));
    assert.ok(architect.includes("go to a CODER via delegate_tasks"));
    assert.ok(architect.includes("NEVER refuse a task"));
    assert.ok(architect.includes("Never delegate shell/delete/write work to utility"));
    assert.ok(utility.includes("You cannot run commands, delete files, edit files"));
  });

  await t.test("Tool schemas - stay compact", () => {
    const tools = [
      ...WORKSPACE_TOOLS,
      UPDATE_PLAN_TOOL,
      SET_TITLE_TOOL,
      ASK_QUESTION_TOOL,
      REMEMBER_TOOL,
      LOAD_SKILL_TOOL,
      DELEGATE_TASKS_TOOL,
      DELEGATE_UTILITY_TOOL
    ];

    const schemaText = JSON.stringify(tools);
    assert.ok(schemaText.length <= 12000, `tool schema too large: ${schemaText.length}`);
    for (const tool of tools) {
      assert.ok(tool.function.description.length <= 180, `${tool.function.name} description too large`);
    }
  });

  await t.test("Workspace tools - reject whole-machine find scans without running shell", async () => {
    assert.strictEqual(commandScansOutsideWorkspace('find / -name "some-tool" -type f 2>/dev/null | head -5'), true);
    assert.strictEqual(commandScansOutsideWorkspace("find . -name package.json"), false);

    const result = JSON.parse(await executeWorkspaceToolAsync({
      id: "run-test-find-block",
      title: "Find block",
      task: "Block bad find",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      projectPath: process.cwd(),
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    }, {
      id: "tool-find-block",
      type: "function",
      function: {
        name: "run_command",
        arguments: JSON.stringify({ command: 'find / -name "some-tool" -type f 2>/dev/null | head -5' })
      }
    }));

    assert.strictEqual(result.success, false);
    assert.match(result.error, /Refusing to scan outside the workspace/);
  });

  await t.test("Orchestrator - pauses for permissions in ask_permissions mode and resumes on approval", async () => {
    const registry = new ProviderRegistry(testConfigPath);
    const runRepo = new RunRepository(db);
    const messageRepo = new MessageRepository(db);
    const orchestrator = new Orchestrator(runRepo, messageRepo, registry, new PlanRepository(db), new MemoryRepository(db), new UsageLogRepository(db));

    const runId = "run-test-perm-ask";
    const runData: Run = {
      id: runId,
      title: "Test Perm Task",
      task: "Create a file",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      mode: "ask_permissions",
      projectPath: process.cwd(),
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    };

    await runRepo.create(runData);

    const createdFilePath = path.join(process.cwd(), "test_perm_file.json");
    t.after(() => {
      if (fs.existsSync(createdFilePath)) {
        fs.unlinkSync(createdFilePath);
      }
      db.exec("DELETE FROM permissions");
    });

    let callCount = 0;
    globalThis.fetch = async (url: any, options: any) => {
      callCount++;
      if (callCount === 1) {
        return {
          ok: true,
          json: async () => ({
            choices: [{
              message: {
                role: "assistant",
                content: "Proposing a tool call",
                tool_calls: [{
                  id: "call_perm_1",
                  type: "function",
                  function: {
                    name: "write_file",
                    arguments: JSON.stringify({ path: "test_perm_file.json", content: "{}" })
                  }
                }]
              }
            }]
          })
        } as any;
      } else {
        return {
          ok: true,
          json: async () => ({
            choices: [{ message: { role: "assistant", content: "File created successfully." } }]
          })
        } as any;
      }
    };

    const runPromise = orchestrator.run(runId);

    // Wait for the async task loop to process and hit the permission request wait point
    await new Promise(r => setTimeout(r, 100));

    const pendingRun = runRepo.getById(runId);
    assert.strictEqual(pendingRun?.status, "awaiting_permission");

    // Resolve decision as allow_once
    const resolved = orchestrator.resolvePermission(runId, "allow_once");
    assert.ok(resolved);

    await runPromise;

    const finishedRun = runRepo.getById(runId);
    assert.strictEqual(finishedRun?.status, "done");
    assert.ok(fs.existsSync(createdFilePath));
  });

  await t.test("Orchestrator - allow_run reuses only the same permission identity", async () => {
    const registry = new ProviderRegistry(testConfigPath);
    const runRepo = new RunRepository(db);
    const messageRepo = new MessageRepository(db);
    const orchestrator = new Orchestrator(runRepo, messageRepo, registry, new PlanRepository(db), new MemoryRepository(db), new UsageLogRepository(db));

    const runId = "run-test-allow-run";
    const runData: Run = {
      id: runId,
      title: "Test allow_run",
      task: "Create two files",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      // ask_permissions gates EVERY tool call, so without allow_run both writes
      // would prompt.
      mode: "ask_permissions",
      projectPath: process.cwd(),
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    };
    await runRepo.create(runData);

    const fileA = path.join(process.cwd(), "allow_run_a.json");
    const fileB = path.join(process.cwd(), "allow_run_b.json");
    t.after(() => {
      for (const f of [fileA, fileB]) if (fs.existsSync(f)) fs.unlinkSync(f);
      db.exec("DELETE FROM permissions");
    });

    let callCount = 0;
    const write = (id: string, name: string) => ({
      ok: true,
      json: async () => ({
        choices: [{
          message: {
            role: "assistant",
            content: "writing",
            tool_calls: [{ id, type: "function", function: { name: "write_file", arguments: JSON.stringify({ path: name, content: "{}" }) } }]
          }
        }]
      })
    } as any);
    globalThis.fetch = async () => {
      callCount++;
      if (callCount === 1) return write("call_a", "allow_run_a.json"); // first write -> prompts
      if (callCount === 2) return write("call_b", "allow_run_b.json"); // second write -> must NOT prompt
      return { ok: true, json: async () => ({ choices: [{ message: { role: "assistant", content: "done" } }] }) } as any;
    };

    const runPromise = orchestrator.run(runId);

    // Wait for the first permission prompt, then grant this tool identity for the run.
    await new Promise(r => setTimeout(r, 100));
    assert.strictEqual(runRepo.getById(runId)?.status, "awaiting_permission");
    assert.ok(orchestrator.resolvePermission(runId, "allow_run"));

    // Both calls share the write_file permission identity. A different tool (or
    // a different command/domain key) would still create another prompt.
    await runPromise;

    assert.strictEqual(runRepo.getById(runId)?.status, "done");
    assert.ok(fs.existsSync(fileA), "first file written");
    assert.ok(fs.existsSync(fileB), "second file written without a second prompt");
  });

  await t.test("Orchestrator - runs tool call read_file and list_directory successfully", async () => {
    const registry = new ProviderRegistry(testConfigPath);
    const runRepo = new RunRepository(db);
    const messageRepo = new MessageRepository(db);
    const orchestrator = new Orchestrator(runRepo, messageRepo, registry, new PlanRepository(db), new MemoryRepository(db), new UsageLogRepository(db));

    const runId = "run-test-read-list";
    const tempDir = path.join(process.cwd(), "temp_test_dir");
    if (!fs.existsSync(tempDir)) {
      fs.mkdirSync(tempDir);
    }
    const tempJsonPath = path.join(tempDir, "test_read_temp.json");
    fs.writeFileSync(tempJsonPath, '{"status": "ok"}', "utf-8");

    const runData: Run = {
      id: runId,
      title: "Test Read List Task",
      task: "Read index.json and list current dir",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      projectPath: tempDir,
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    };

    await runRepo.create(runData);

    t.after(() => {
      if (fs.existsSync(tempJsonPath)) {
        fs.unlinkSync(tempJsonPath);
      }
      if (fs.existsSync(tempDir)) {
        fs.rmdirSync(tempDir);
      }
    });

    let callCount = 0;
    globalThis.fetch = async (url: any, options: any) => {
      callCount++;
      if (callCount === 1) {
        return {
          ok: true,
          json: async () => ({
            choices: [{
              message: {
                role: "assistant",
                content: "Reading file and listing directory...",
                tool_calls: [
                  {
                    id: "call_read_1",
                    type: "function",
                    function: {
                      name: "read_file",
                      arguments: JSON.stringify({ path: "test_read_temp.json" })
                    }
                  },
                  {
                    id: "call_list_1",
                    type: "function",
                    function: {
                      name: "list_directory",
                      arguments: JSON.stringify({ path: "" })
                    }
                  }
                ]
              }
            }]
          })
        } as any;
      } else {
        return {
          ok: true,
          json: async () => ({
            choices: [{ message: { role: "assistant", content: "Completed read and list." } }]
          })
        } as any;
      }
    };

    await orchestrator.run(runId);

    // Verify database record has done status
    const finishedRun = runRepo.getById(runId);
    assert.strictEqual(finishedRun?.status, "done");

    // Verify messages saved (1 assistant tools, 2 tool outputs, 1 final assistant text response)
    const savedMsgs = messageRepo.listByRunId(runId);
    assert.strictEqual(savedMsgs.length, 4);
    assert.strictEqual(savedMsgs[0].role, "assistant");
    
    // Parse read_file response
    const readResponse = JSON.parse(savedMsgs[1].content);
    assert.strictEqual(readResponse.success, true);
    assert.strictEqual(readResponse.content, '{"status": "ok"}');

    // Parse list_directory response
    const listResponse = JSON.parse(savedMsgs[2].content);
    assert.strictEqual(listResponse.success, true);
    assert.ok(listResponse.files.some((f: any) => f.name === 'test_read_temp.json'));
    
    assert.strictEqual(savedMsgs[3].role, "assistant");
  });

  await t.test("Orchestrator - delegate_tasks runs coder sub-agents sequentially", async () => {
    const registry = new ProviderRegistry(testConfigPath);
    const runRepo = new RunRepository(db);
    const messageRepo = new MessageRepository(db);
    const orchestrator = new Orchestrator(runRepo, messageRepo, registry, new PlanRepository(db), new MemoryRepository(db), new UsageLogRepository(db));

    const runId = "run-test-delegate";
    const runData: Run = {
      id: runId,
      title: "Test Delegate",
      task: "Build two things",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      // Dual-model: architect + coder on the same mock provider.
      coderProviderId: "test-provider",
      coderModel: "model-1",
      projectPath: process.cwd(),
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    };

    await runRepo.create(runData);

    let callCount = 0;
    globalThis.fetch = async (_url: any, _options: any) => {
      callCount++;
      if (callCount === 1) {
        // Architect delegates two sub-tasks sequentially.
        return {
          ok: true,
          json: async () => ({
            choices: [{
              message: {
                role: "assistant",
                content: "Delegating the work.",
                tool_calls: [{
                  id: "call_delegate_1",
                  type: "function",
                  function: {
                    name: "delegate_tasks",
                    arguments: JSON.stringify({
                      parallel: false,
                      tasks: [
                        { title: "Task A", instructions: "Do A" },
                        { title: "Task B", instructions: "Do B" }
                      ]
                    })
                  }
                }]
              }
            }]
          })
        } as any;
      }
      // Coder sub-agents (calls 2 and 3) and the architect's final turn (call 4)
      // all return a plain text answer with no further tool calls.
      const text = callCount === 2 ? "Coder finished A"
        : callCount === 3 ? "Coder finished B"
        : "All subtasks complete.";
      return {
        ok: true,
        json: async () => ({
          choices: [{ message: { role: "assistant", content: text } }]
        })
      } as any;
    };

    await orchestrator.run(runId);

    const finishedRun = runRepo.getById(runId);
    assert.strictEqual(finishedRun?.status, "done");

    const savedMsgs = messageRepo.listByRunId(runId);
    // architect tool call, coder A, coder B, delegate tool result, architect
    // final, then one post-delegation verification nudge retry (the mock
    // architect delegates but never verifies, so the loop nudges it once more).
    assert.strictEqual(savedMsgs.length, 6);
    assert.strictEqual(savedMsgs[0].agentRole, "planner");
    assert.ok(savedMsgs[0].rawResponse?.includes("delegate_tasks"));
    assert.strictEqual(savedMsgs[1].agentRole, "coder");
    assert.strictEqual(savedMsgs[1].content, "Coder finished A");
    assert.strictEqual(savedMsgs[2].agentRole, "coder");
    assert.strictEqual(savedMsgs[2].content, "Coder finished B");
    assert.strictEqual(savedMsgs[3].role, "tool");

    const delegateResult = JSON.parse(savedMsgs[3].content);
    assert.strictEqual(delegateResult.success, true);
    assert.strictEqual(delegateResult.parallel, false);
    assert.strictEqual(delegateResult.results.length, 2);
    assert.strictEqual(delegateResult.results[0].summary, "Coder finished A");

    assert.strictEqual(savedMsgs[4].content, "All subtasks complete.");
    assert.strictEqual(savedMsgs[5].content, "All subtasks complete.");
  });

  await t.test("Orchestrator - architect sees mutating tools as traps and is redirected to delegate_tasks", async () => {
    const registry = new ProviderRegistry(testConfigPath);
    const runRepo = new RunRepository(db);
    const messageRepo = new MessageRepository(db);
    const orchestrator = new Orchestrator(runRepo, messageRepo, registry, new PlanRepository(db), new MemoryRepository(db), new UsageLogRepository(db));

    const runId = "run-test-trap";
    const runData: Run = {
      id: runId,
      title: "Test Trap",
      task: "Delete a file",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      // Dual-model so the main agent is the architect.
      coderProviderId: "test-provider",
      coderModel: "model-1",
      mode: "accept_edits",
      projectPath: process.cwd(),
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    };
    await runRepo.create(runData);

    let callCount = 0;
    let advertisedTools: any[] = [];
    globalThis.fetch = async (_url: any, options: any) => {
      callCount++;
      if (callCount === 1) {
        // Capture the tools advertised to the architect, then have it attempt a
        // direct delete (which must be trapped and redirected).
        advertisedTools = JSON.parse(options.body).tools || [];
        return {
          ok: true,
          json: async () => ({
            choices: [{
              message: {
                role: "assistant",
                content: "Deleting the file.",
                tool_calls: [{
                  id: "call_delete_1",
                  type: "function",
                  function: { name: "delete_file", arguments: JSON.stringify({ path: "trap-test.txt" }) }
                }]
              }
            }]
          })
        } as any;
      }
      return {
        ok: true,
        json: async () => ({ choices: [{ message: { role: "assistant", content: "Delegated instead." } }] })
      } as any;
    };

    await orchestrator.run(runId);

    // #1a: mutating tools are advertised to the architect (as traps), alongside delegate_tasks.
    const toolNames = advertisedTools.map((tool: any) => tool.function?.name);
    assert.ok(toolNames.includes("delete_file"), "architect should be advertised delete_file as a trap");
    assert.ok(toolNames.includes("write_file"), "architect should be advertised write_file as a trap");
    assert.ok(toolNames.includes("delegate_tasks"), "architect should have delegate_tasks");

    // #1b: calling one is rejected with a redirect to delegate_tasks, not executed.
    const savedMsgs = messageRepo.listByRunId(runId);
    const toolMsg = savedMsgs.find(m => m.role === "tool");
    assert.ok(toolMsg, "expected a tool result message");
    const result = JSON.parse(toolMsg!.content);
    assert.strictEqual(result.success, false);
    assert.ok(result.error.includes("ARCHITECT"));
    assert.ok(result.error.includes("delegate_tasks"));
  });

  await t.test("Orchestrator - idle architect with an approved plan gets one nudge before finishing", async () => {
    const registry = new ProviderRegistry(testConfigPath);
    const runRepo = new RunRepository(db);
    const messageRepo = new MessageRepository(db);
    const planRepo = new PlanRepository(db);
    const orchestrator = new Orchestrator(runRepo, messageRepo, registry, planRepo, new MemoryRepository(db), new UsageLogRepository(db));

    const runId = "run-test-idle-nudge";
    const runData: Run = {
      id: runId,
      title: "Test Idle Nudge",
      task: "Implement the approved plan",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      coderProviderId: "test-provider",
      coderModel: "model-1",
      mode: "accept_edits",
      projectPath: process.cwd(),
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    };
    await runRepo.create(runData);
    // An approved plan exists -> implementation is expected, so the idle nudge is armed.
    await planRepo.upsert(runId, {
      title: "Build it",
      tasks: [{ text: "Scaffold", status: "pending" }]
    });

    let callCount = 0;
    const userMessagesSeen: string[][] = [];
    globalThis.fetch = async (_url: any, options: any) => {
      callCount++;
      // Record the user-role messages the model received this turn so we can prove
      // the nudge was injected on the second call.
      userMessagesSeen.push((JSON.parse(options.body).messages || [])
        .filter((m: any) => m.role === "user")
        .map((m: any) => m.content));
      // The architect always answers with plain text and NO tool calls — the
      // premature-stop failure mode we are guarding against.
      return {
        ok: true,
        json: async () => ({ choices: [{ message: { role: "assistant", content: "I'll begin now." } }] })
      } as any;
    };

    await orchestrator.run(runId);

    // First call finishes with no tool call -> one nudge -> second call -> done.
    assert.strictEqual(callCount, 2, "expected exactly one idle nudge (2 model calls)");
    assert.ok(
      userMessagesSeen[1].some(c => c.includes("without calling any tool")),
      "the second model call should include the injected idle nudge"
    );
    assert.strictEqual(runRepo.getById(runId)?.status, "done");
  });

  await t.test("Orchestrator - architect with utility tier exhausts its direct-inspection budget", async () => {
    const registry = new ProviderRegistry(testConfigPath);
    const runRepo = new RunRepository(db);
    const messageRepo = new MessageRepository(db);
    const orchestrator = new Orchestrator(runRepo, messageRepo, registry, new PlanRepository(db), new MemoryRepository(db), new UsageLogRepository(db));

    const runId = "run-test-inspect-budget";
    await runRepo.create({
      id: runId,
      title: "Test inspect budget",
      task: "Explore the project",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      coderProviderId: "test-provider",
      coderModel: "model-1",
      utilityProviderId: "test-provider",
      utilityModel: "model-1",
      projectPath: process.cwd(),
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    } as Run);

    let callCount = 0;
    globalThis.fetch = async (_url: any, _options: any) => {
      callCount++;
      if (callCount === 1) {
        // Architect fires 5 read-only calls in one turn — one over the budget of 4.
        return {
          ok: true,
          json: async () => ({
            choices: [{
              message: {
                role: "assistant",
                content: "Exploring.",
                tool_calls: [1, 2, 3, 4, 5].map(i => ({
                  id: `call_read_${i}`,
                  type: "function",
                  function: { name: "list_directory", arguments: JSON.stringify({ path: "" }) }
                }))
              }
            }]
          })
        } as any;
      }
      return {
        ok: true,
        json: async () => ({ choices: [{ message: { role: "assistant", content: "Done." } }] })
      } as any;
    };

    await orchestrator.run(runId);
    assert.strictEqual(runRepo.getById(runId)?.status, "done");

    const toolResults = messageRepo.listByRunId(runId).filter(m => m.role === "tool").map(m => JSON.parse(m.content));
    assert.strictEqual(toolResults.length, 5);
    // First 4 inspections succeed, the 5th is redirected to the utility tier.
    for (const r of toolResults.slice(0, 4)) assert.strictEqual(r.success, true);
    assert.strictEqual(toolResults[4].success, false);
    assert.ok(toolResults[4].error.includes("delegate_to_utility"));
  });

  await t.test("Orchestrator - delegate_to_utility runs a utility sub-agent", async () => {
    const registry = new ProviderRegistry(testConfigPath);
    const runRepo = new RunRepository(db);
    const messageRepo = new MessageRepository(db);
    const orchestrator = new Orchestrator(runRepo, messageRepo, registry, new PlanRepository(db), new MemoryRepository(db), new UsageLogRepository(db));

    const runId = "run-test-utility";
    const runData: Run = {
      id: runId,
      title: "Test Utility",
      task: "Find a file then build",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      // Dual-model + utility tier, all on the same mock provider.
      coderProviderId: "test-provider",
      coderModel: "model-1",
      utilityProviderId: "test-provider",
      utilityModel: "model-1",
      projectPath: process.cwd(),
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    };

    await runRepo.create(runData);

    let callCount = 0;
    globalThis.fetch = async (_url: any, _options: any) => {
      callCount++;
      if (callCount === 1) {
        // Architect delegates one tiny lookup to the utility tier.
        return {
          ok: true,
          json: async () => ({
            choices: [{
              message: {
                role: "assistant",
                content: "Let me locate it.",
                tool_calls: [{
                  id: "call_util_1",
                  type: "function",
                  function: {
                    name: "delegate_to_utility",
                    arguments: JSON.stringify({
                      tasks: [{ title: "Locate config", instructions: "Where is config defined?" }]
                    })
                  }
                }]
              }
            }]
          })
        } as any;
      }
      // Utility sub-agent (call 2) returns a short answer; architect final (call 3).
      const text = callCount === 2 ? "It's at src/config.ts:10" : "Done.";
      return {
        ok: true,
        json: async () => ({
          choices: [{ message: { role: "assistant", content: text } }]
        })
      } as any;
    };

    await orchestrator.run(runId);

    const finishedRun = runRepo.getById(runId);
    assert.strictEqual(finishedRun?.status, "done");

    const savedMsgs = messageRepo.listByRunId(runId);
    // architect tool call, utility sub-agent, utility tool result, architect final
    assert.strictEqual(savedMsgs.length, 4);
    assert.ok(savedMsgs[0].rawResponse?.includes("delegate_to_utility"));
    assert.strictEqual(savedMsgs[1].agentRole, "utility");
    assert.strictEqual(savedMsgs[1].agentName, "Locate config");
    assert.strictEqual(savedMsgs[1].content, "It's at src/config.ts:10");
    assert.strictEqual(savedMsgs[2].role, "tool");

    const utilResult = JSON.parse(savedMsgs[2].content);
    assert.strictEqual(utilResult.success, true);
    assert.strictEqual(utilResult.results.length, 1);
    assert.strictEqual(utilResult.results[0].summary, "It's at src/config.ts:10");

    assert.strictEqual(savedMsgs[3].content, "Done.");
  });

  await t.test("Orchestrator - plan mode blocks mutating/command tools even if the model calls them", async () => {
    const registry = new ProviderRegistry(testConfigPath);
    const runRepo = new RunRepository(db);
    const messageRepo = new MessageRepository(db);
    const orchestrator = new Orchestrator(runRepo, messageRepo, registry, new PlanRepository(db), new MemoryRepository(db), new UsageLogRepository(db));

    const runId = "run-test-plan-block";
    const blockedFilePath = path.join(process.cwd(), "test_plan_blocked.json");
    t.after(() => {
      if (fs.existsSync(blockedFilePath)) fs.unlinkSync(blockedFilePath);
    });

    await runRepo.create({
      id: runId,
      title: "Plan block",
      task: "Plan only",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      mode: "plan",
      projectPath: process.cwd(),
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    });

    const statusesEmitted: RunStatus[] = [];
    const listener = (event: any) => {
      if (event.type === "status_changed") statusesEmitted.push(event.status);
    };
    eventBus.on(`run:${runId}`, listener);
    t.after(() => eventBus.off(`run:${runId}`, listener));

    let callCount = 0;
    globalThis.fetch = async () => {
      callCount++;
      if (callCount === 1) {
        // Model (mis)behaves: tries to write a file while in plan mode.
        return {
          ok: true,
          json: async () => ({
            choices: [{
              message: {
                role: "assistant",
                content: "Writing file",
                tool_calls: [{
                  id: "call_plan_write",
                  type: "function",
                  function: { name: "write_file", arguments: JSON.stringify({ path: "test_plan_blocked.json", content: "{}" }) }
                }]
              }
            }]
          })
        } as any;
      }
      return {
        ok: true,
        json: async () => ({ choices: [{ message: { role: "assistant", content: "Understood, staying in plan mode." } }] })
      } as any;
    };

    await orchestrator.run(runId);

    // The file must NOT have been created, and no permission prompt should occur.
    assert.ok(!fs.existsSync(blockedFilePath), "plan mode must not create files");
    assert.ok(!statusesEmitted.includes("awaiting_permission"), "plan mode must not even prompt for permission");

    const savedMsgs = messageRepo.listByRunId(runId);
    const toolMsg = savedMsgs.find(m => m.role === "tool");
    assert.ok(toolMsg);
    const result = JSON.parse(toolMsg!.content);
    assert.strictEqual(result.success, false);
    assert.match(result.error, /Plan mode/);
  });

  await t.test("Orchestrator - delegate_tasks clamps to at most 3 sub-agents", async () => {
    const registry = new ProviderRegistry(testConfigPath);
    const runRepo = new RunRepository(db);
    const messageRepo = new MessageRepository(db);
    const orchestrator = new Orchestrator(runRepo, messageRepo, registry, new PlanRepository(db), new MemoryRepository(db), new UsageLogRepository(db));

    const runId = "run-test-delegate-clamp";
    await runRepo.create({
      id: runId,
      title: "Clamp",
      task: "Too many",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      coderProviderId: "test-provider",
      coderModel: "model-1",
      projectPath: process.cwd(),
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    });

    let callCount = 0;
    globalThis.fetch = async () => {
      callCount++;
      if (callCount === 1) {
        return {
          ok: true,
          json: async () => ({
            choices: [{
              message: {
                role: "assistant",
                content: "Delegating five tasks.",
                tool_calls: [{
                  id: "call_delegate_clamp",
                  type: "function",
                  function: {
                    name: "delegate_tasks",
                    arguments: JSON.stringify({
                      parallel: false,
                      tasks: [1, 2, 3, 4, 5].map(n => ({ title: `T${n}`, instructions: `Do ${n}` }))
                    })
                  }
                }]
              }
            }]
          })
        } as any;
      }
      return {
        ok: true,
        json: async () => ({ choices: [{ message: { role: "assistant", content: "ok" } }] })
      } as any;
    };

    await orchestrator.run(runId);

    const savedMsgs = messageRepo.listByRunId(runId);
    const toolMsg = savedMsgs.find(m => m.role === "tool");
    assert.ok(toolMsg);
    const result = JSON.parse(toolMsg!.content);
    assert.strictEqual(result.results.length, 3, "should clamp 5 requested tasks to 3");
  });

  await t.test("Orchestrator - delegate_tasks supports 3 parallel sub-agents writing the same run", async () => {
    const registry = new ProviderRegistry(testConfigPath);
    const runRepo = new RunRepository(db);
    const messageRepo = new MessageRepository(db);
    const orchestrator = new Orchestrator(runRepo, messageRepo, registry, new PlanRepository(db), new MemoryRepository(db), new UsageLogRepository(db));

    const runId = "run-test-delegate-parallel";
    await runRepo.create({
      id: runId,
      title: "Parallel",
      task: "Parallel work",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      coderProviderId: "test-provider",
      coderModel: "model-1",
      projectPath: process.cwd(),
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    });

    let callCount = 0;
    let activeCoderCalls = 0;
    let maxActiveCoderCalls = 0;
    globalThis.fetch = async () => {
      callCount++;
      if (callCount === 1) {
        return {
          ok: true,
          json: async () => ({
            choices: [{
              message: {
                role: "assistant",
                content: "Delegating in parallel.",
                tool_calls: [{
                  id: "call_delegate_parallel",
                  type: "function",
                  function: {
                    name: "delegate_tasks",
                    arguments: JSON.stringify({
                      parallel: true,
                      tasks: [
                        { title: "Task A", instructions: "Do A" },
                        { title: "Task B", instructions: "Do B" },
                        { title: "Task C", instructions: "Do C" }
                      ]
                    })
                  }
                }]
              }
            }]
          })
        } as any;
      }

      if (callCount >= 2 && callCount <= 4) {
        activeCoderCalls++;
        maxActiveCoderCalls = Math.max(maxActiveCoderCalls, activeCoderCalls);
        await new Promise(resolve => setTimeout(resolve, 5));
        activeCoderCalls--;
      }

      const text = callCount === 5 ? "All subtasks complete." : `Coder finished ${callCount - 1}`;
      return {
        ok: true,
        json: async () => ({ choices: [{ message: { role: "assistant", content: text } }] })
      } as any;
    };

    await orchestrator.run(runId);

    assert.ok(maxActiveCoderCalls > 1, "coder sub-agents should overlap when parallel=true");

    const finishedRun = runRepo.getById(runId);
    assert.strictEqual(finishedRun?.status, "done");

    const savedMsgs = messageRepo.listByRunId(runId);
    const toolMsg = savedMsgs.find(m => m.role === "tool");
    assert.ok(toolMsg);
    const result = JSON.parse(toolMsg!.content);
    assert.strictEqual(result.parallel, true);
    assert.strictEqual(result.results.length, 3);
  });

  await t.test("Orchestrator - remember saves a memory and later runs inject it", async () => {
    const registry = new ProviderRegistry(testConfigPath);
    const runRepo = new RunRepository(db);
    const messageRepo = new MessageRepository(db);
    const memoryRepo = new MemoryRepository(db);
    const orchestrator = new Orchestrator(runRepo, messageRepo, registry, new PlanRepository(db), memoryRepo, new UsageLogRepository(db));

    const projectPath = process.cwd();
    const runId = "run-test-remember";
    await runRepo.create({
      id: runId,
      title: "Remember",
      task: "I always want 2-space indentation",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      mode: "accept_edits",
      projectPath,
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    });

    let callCount = 0;
    globalThis.fetch = async () => {
      callCount++;
      if (callCount === 1) {
        return {
          ok: true,
          json: async () => ({
            choices: [{
              message: {
                role: "assistant",
                content: "Noting your preference.",
                tool_calls: [{
                  id: "call_remember_1",
                  type: "function",
                  function: {
                    name: "remember",
                    arguments: JSON.stringify({
                      scope: "global",
                      category: "user",
                      content: "Prefers 2-space indentation."
                    })
                  }
                }]
              }
            }]
          })
        } as any;
      }
      return {
        ok: true,
        json: async () => ({ choices: [{ message: { role: "assistant", content: "Done." } }] })
      } as any;
    };

    await orchestrator.run(runId);

    // The memory was persisted (silently, no permission prompt).
    const all = memoryRepo.list();
    const saved = all.find(m => m.content === "Prefers 2-space indentation.");
    assert.ok(saved, "memory should be saved");
    assert.strictEqual(saved!.scope, "global");
    assert.strictEqual(saved!.category, "user");

    // The remember tool result reported success.
    const toolMsg = messageRepo.listByRunId(runId).find(m => m.role === "tool");
    assert.ok(toolMsg);
    assert.strictEqual(JSON.parse(toolMsg!.content).success, true);

    // A subsequent run injects the memory into the system prompt.
    const run2Id = "run-test-remember-2";
    await runRepo.create({
      id: run2Id,
      title: "Next session",
      task: "Write some code",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      mode: "accept_edits",
      projectPath,
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    });

    let systemPromptUsed = "";
    globalThis.fetch = async (_url: any, options: any) => {
      const parsed = JSON.parse(options.body);
      const systemMessage = parsed.messages.find((m: any) => m.role === "system");
      if (systemMessage) systemPromptUsed = systemMessage.content;
      return {
        ok: true,
        json: async () => ({ choices: [{ message: { role: "assistant", content: "ok" } }] })
      } as any;
    };

    await orchestrator.run(run2Id);

    assert.ok(systemPromptUsed.includes("REMEMBERED CONTEXT"));
    assert.ok(systemPromptUsed.includes("Prefers 2-space indentation."));
  });

  await t.test("Orchestrator - load_skill returns body and catalog appears in prompt", async () => {
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "locagens-orch-skill-"));
    const userSkills = path.join(tmp, "skills");
    const skillDir = path.join(userSkills, "demo-skill");
    fs.mkdirSync(skillDir, { recursive: true });
    fs.writeFileSync(
      path.join(skillDir, "SKILL.md"),
      `---\nname: demo-skill\ndescription: "Demo skill for tests. USE FOR: demo."\n---\n# Demo\nFollow these steps.\n`,
      "utf-8"
    );

    const { SkillRegistry } = await import("./skills/index.js");
    const registry = new ProviderRegistry(testConfigPath);
    const runRepo = new RunRepository(db);
    const messageRepo = new MessageRepository(db);
    const orchestrator = new Orchestrator(
      runRepo,
      messageRepo,
      registry,
      new PlanRepository(db),
      new MemoryRepository(db),
      new UsageLogRepository(db),
      new SkillRegistry(userSkills)
    );

    const runId = "run-test-load-skill";
    await runRepo.create({
      id: runId,
      title: "Load skill",
      task: "Use the demo skill",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      mode: "accept_edits",
      projectPath: process.cwd(),
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    });

    let systemPromptUsed = "";
    let callCount = 0;
    globalThis.fetch = async (_url: any, options: any) => {
      callCount++;
      const parsed = JSON.parse(options.body);
      const systemMessage = parsed.messages.find((m: any) => m.role === "system");
      if (systemMessage) systemPromptUsed = systemMessage.content;
      if (callCount === 1) {
        return {
          ok: true,
          json: async () => ({
            choices: [{
              message: {
                role: "assistant",
                content: "Loading skill.",
                tool_calls: [{
                  id: "call_load_skill_1",
                  type: "function",
                  function: {
                    name: "load_skill",
                    arguments: JSON.stringify({ name: "demo-skill" })
                  }
                }]
              }
            }]
          })
        } as any;
      }
      return {
        ok: true,
        json: async () => ({ choices: [{ message: { role: "assistant", content: "Done." } }] })
      } as any;
    };

    await orchestrator.run(runId);

    assert.ok(systemPromptUsed.includes("AVAILABLE SKILLS"));
    assert.ok(systemPromptUsed.includes("demo-skill"));

    const toolMsg = messageRepo.listByRunId(runId).find(m => m.role === "tool");
    assert.ok(toolMsg);
    const result = JSON.parse(toolMsg!.content);
    assert.strictEqual(result.success, true);
    assert.strictEqual(result.name, "demo-skill");
    assert.match(result.content, /Follow these steps/);

    fs.rmSync(tmp, { recursive: true, force: true });
  });

  await t.test("Orchestrator - architect is blocked from executing delete_file directly when preset is active", async () => {
    const registry = new ProviderRegistry(testConfigPath);
    const runRepo = new RunRepository(db);
    const messageRepo = new MessageRepository(db);
    const orchestrator = new Orchestrator(runRepo, messageRepo, registry, new PlanRepository(db), new MemoryRepository(db), new UsageLogRepository(db));

    const runId = "run-test-architect-blocked";
    await runRepo.create({
      id: runId,
      title: "Blocked Command",
      task: "Delete some files",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      coderProviderId: "test-provider",
      coderModel: "model-1",
      projectPath: process.cwd(),
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    });

    let callCount = 0;
    globalThis.fetch = async () => {
      callCount++;
      if (callCount === 1) {
        return {
          ok: true,
          json: async () => ({
            choices: [{
              message: {
                role: "assistant",
                content: "I will delete the file myself.",
                tool_calls: [{
                  id: "call_blocked_delete",
                  type: "function",
                  function: {
                    name: "delete_file",
                    arguments: JSON.stringify({ path: "some_file.json" })
                  }
                }]
              }
            }]
          })
        } as any;
      }
      return {
        ok: true,
        json: async () => ({
          choices: [{
            message: {
              role: "assistant",
              content: "Understood, I cannot do that directly."
            }
          }]
        })
      } as any;
    };

    await orchestrator.run(runId);

    const savedMsgs = messageRepo.listByRunId(runId);
    const toolMsg = savedMsgs.find(m => m.role === "tool");
    assert.ok(toolMsg);
    const result = JSON.parse(toolMsg!.content);
    assert.strictEqual(result.success, false);
    assert.match(result.error, /You are the ARCHITECT and cannot run/);
    assert.match(result.error, /delegate_tasks/);
  });

  await t.test("Pricing - cost is zero without user-entered pricing (no built-in table)", () => {
    // There is no hardcoded pricing sheet and no name-based guessing: any model
    // without configured pricing costs 0, regardless of how it is named.
    assert.strictEqual(calculateCost(undefined, 10000, 2000, 5000, 1000), 0.0);
    assert.strictEqual(calculateCost(null, 10000, 2000), 0.0);
    assert.strictEqual(calculateCost({ tiers: [] }, 10000, 2000), 0.0);
  });

  await t.test("Pricing - flat pricing computes from a single tier", () => {
    const pricing = { tiers: [{ inputRate: 0.30, outputRate: 1.20, cacheReadRate: 0.06 }] };
    const cost = calculateCost(pricing, 10_000, 2_000, 5_000, 0);
    assert.strictEqual(cost, (10_000 / 1e6 * 0.30) + (2_000 / 1e6 * 1.20) + (5_000 / 1e6 * 0.06));
  });

  await t.test("Pricing - tiered pricing selects the tier by prompt size", () => {
    // <=250k prompt tokens cheaper, above it pricier. Tier is chosen by the
    // request's total prompt (input + cache) tokens.
    const pricing = {
      tiers: [
        { upToInputTokens: 250_000, inputRate: 1.0, outputRate: 4.0 },
        { inputRate: 2.0, outputRate: 8.0 }
      ]
    };

    // 100k prompt -> low tier
    const low = calculateCost(pricing, 100_000, 1_000, 0, 0);
    assert.strictEqual(low, (100_000 / 1e6 * 1.0) + (1_000 / 1e6 * 4.0));

    // 300k prompt -> high tier (tier selection counts input + cache tokens)
    const high = calculateCost(pricing, 200_000, 1_000, 100_000, 0);
    assert.strictEqual(high, (200_000 / 1e6 * 2.0) + (1_000 / 1e6 * 8.0) + (100_000 / 1e6 * 0));
  });

  await t.test("UsageLogRepository - creates and retrieves usage logs", async () => {
    const usageLogRepo = new UsageLogRepository(db);
    const runId = "run-test-logging-db";

    // Setup a dummy run first to satisfy foreign key constraint
    const runRepo = new RunRepository(db);
    await runRepo.create({
      id: runId,
      title: "Test Logging Run",
      task: "Test logging",
      status: "created",
      providerId: "test-provider",
      providerDisplayName: "Test Provider",
      model: "model-1",
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    });

    const logEntry = {
      runId,
      agentRole: "coder",
      providerId: "anthropic",
      model: "claude-3-5-sonnet",
      inputTokens: 1000,
      outputTokens: 500,
      cacheReadTokens: 200,
      cacheWriteTokens: 100,
      cacheHitRate: 15,
      cost: 0.012,
      createdAt: new Date().toISOString()
    };

    await usageLogRepo.create(logEntry);

    const logs = usageLogRepo.listByRunId(runId);
    assert.strictEqual(logs.length, 1);
    assert.strictEqual(logs[0].runId, runId);
    assert.strictEqual(logs[0].agentRole, "coder");
    assert.strictEqual(logs[0].providerId, "anthropic");
    assert.strictEqual(logs[0].model, "claude-3-5-sonnet");
    assert.strictEqual(logs[0].inputTokens, 1000);
    assert.strictEqual(logs[0].outputTokens, 500);
    assert.strictEqual(logs[0].cacheReadTokens, 200);
    assert.strictEqual(logs[0].cacheWriteTokens, 100);
    assert.strictEqual(logs[0].cacheHitRate, 15);
    assert.strictEqual(logs[0].cost, 0.012);
  });
});
