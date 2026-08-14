import type { FastifyInstance } from "fastify";
import type { ReasoningEffort, Run, RunStatus } from "@locagens/shared";
import { type AppContext, normalizeProject } from "../context.js";
import { eventBus } from "../orchestrator/eventBus.js";
import { permissionKey, commandEscapesWorkspace, buildPermissionPreview } from "../orchestrator/workspaceTools.js";

const PERMISSION_DECISIONS = ["allow_once", "allow_project", "allow_always", "allow_run", "deny"] as const;
type PermissionDecision = (typeof PERMISSION_DECISIONS)[number];
const REASONING_EFFORTS = new Set<ReasoningEffort>(["default", "none", "minimal", "low", "medium", "high", "xhigh", "max"]);

function normalizeReasoningEffort(ctx: AppContext, providerId: string | undefined, model: string | undefined, value: unknown): ReasoningEffort | undefined {
  if (!providerId || !model) return undefined;
  const supported = ctx.registry.getSupportedReasoningEfforts(providerId, model);
  return typeof value === "string" && REASONING_EFFORTS.has(value as ReasoningEffort) && value !== "default" && supported.includes(value as ReasoningEffort)
    ? value as ReasoningEffort
    : undefined;
}

export function registerRunRoutes(server: FastifyInstance, ctx: AppContext) {
  // Create and trigger a new orchestration run.
  server.post("/api/runs", async (request, reply) => {
    const { task, projectPath, projectName, providerId, model, reasoningEffort, mode, coderProviderId, coderModel, coderReasoningEffort, utilityProviderId, utilityModel, utilityReasoningEffort, agentPreset } = request.body as {
      task: string;
      projectPath?: string;
      projectName?: string;
      providerId: string;
      model: string;
      reasoningEffort?: string;
      mode?: string;
      coderProviderId?: string;
      coderModel?: string;
      coderReasoningEffort?: string;
      utilityProviderId?: string;
      utilityModel?: string;
      utilityReasoningEffort?: string;
      agentPreset?: string;
    };

    if (!task || !providerId || !model) {
      reply.status(400);
      return { error: "Missing required fields: task, providerId, model" };
    }

    const providerMeta = ctx.registry.getSafeMetadata().find(p => p.id === providerId);
    const providerDisplayName = providerMeta ? providerMeta.displayName : providerId;

    const runId = `run-${Date.now()}`;
    // Sessions start named with the user's first message; the model can rename them later.
    let title = "New session…";
    if (task) {
      const firstLine = task.trim().split('\n')[0].trim();
      if (firstLine) {
        title = firstLine.length > 40 ? firstLine.substring(0, 40).trim() + "…" : firstLine;
      }
    }
    let project: ReturnType<typeof normalizeProject>;
    try {
      project = normalizeProject(ctx, projectPath, projectName);
    } catch (error: any) {
      reply.status(400);
      return { error: error.message };
    }

    const run = {
      id: runId,
      title,
      task,
      ...project,
      status: "created" as RunStatus,
      providerId,
      providerDisplayName,
      model,
      reasoningEffort: normalizeReasoningEffort(ctx, providerId, model, reasoningEffort),
      mode: mode || "accept_edits",
      coderProviderId: coderProviderId || undefined,
      coderModel: coderModel || undefined,
      coderReasoningEffort: normalizeReasoningEffort(ctx, coderProviderId, coderModel, coderReasoningEffort),
      utilityProviderId: utilityProviderId || undefined,
      utilityModel: utilityModel || undefined,
      utilityReasoningEffort: normalizeReasoningEffort(ctx, utilityProviderId, utilityModel, utilityReasoningEffort),
      agentPreset: agentPreset || undefined,
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString()
    };

    await ctx.runRepo.create(run);

    // Run orchestration asynchronously in the background.
    ctx.orchestrator.run(runId).catch(err => {
      console.error(`[Orchestrator] Error running job ${runId}:`, err.message);
    });

    return run;
  });

  // Cancel a running orchestration job.
  server.post("/api/runs/:id/cancel", async (request, reply) => {
    const { id } = request.params as { id: string };
    const success = await ctx.orchestrator.cancel(id);
    if (!success) {
      const run = ctx.runRepo.getById(id);
      if (run) {
        return { success: true, message: "Run was not actively running." };
      }
      reply.status(404);
      return { error: `Run with id "${id}" not found` };
    }
    return { success: true };
  });

  // Continue a run with follow-up instructions in the same thread.
  server.post("/api/runs/:id/continue", async (request, reply) => {
    const { id } = request.params as { id: string };
    const { task, providerId, model, reasoningEffort, mode, coderProviderId, coderModel, coderReasoningEffort, utilityProviderId, utilityModel, utilityReasoningEffort, agentPreset } = request.body as {
      task: string;
      providerId?: string;
      model?: string;
      reasoningEffort?: string;
      mode?: string;
      coderProviderId?: string;
      coderModel?: string;
      coderReasoningEffort?: string;
      utilityProviderId?: string;
      utilityModel?: string;
      utilityReasoningEffort?: string;
      agentPreset?: string;
    };

    if (!task || !task.trim()) {
      reply.status(400);
      return { error: "Missing required field: task" };
    }

    const run = ctx.runRepo.getById(id);
    if (!run) {
      reply.status(404);
      return { error: `Run with id "${id}" not found` };
    }

    let canonicalProject: ReturnType<typeof normalizeProject>;
    try {
      canonicalProject = normalizeProject(ctx, run.projectPath, run.projectName);
    } catch (error: any) {
      reply.status(400);
      return { error: error.message };
    }

    // Update model or mode if changed mid-conversation.
    const updates: Partial<Run> = canonicalProject.projectPath === run.projectPath
      ? {}
      : canonicalProject;
    if (providerId && model) {
      const providerMeta = ctx.registry.getSafeMetadata().find(p => p.id === providerId);
      updates.providerId = providerId;
      updates.providerDisplayName = providerMeta ? providerMeta.displayName : providerId;
      updates.model = model;
    }
    if ("reasoningEffort" in (request.body as Record<string, unknown>)) {
      updates.reasoningEffort = normalizeReasoningEffort(ctx, providerId || run.providerId, model || run.model, reasoningEffort);
    }
    if (mode) {
      updates.mode = mode;
    }
    // Allow switching the agent preset (or back to single model) mid-conversation.
    // The web client always sends the current selection, so reflect it verbatim.
    const body = request.body as Record<string, unknown>;
    if (
      "agentPreset" in body ||
      "coderModel" in body ||
      "coderProviderId" in body ||
      "coderReasoningEffort" in body ||
      "utilityModel" in body ||
      "utilityProviderId" in body ||
      "utilityReasoningEffort" in body
    ) {
      updates.agentPreset = agentPreset || undefined;
      updates.coderProviderId = coderProviderId || undefined;
      updates.coderModel = coderModel || undefined;
      updates.coderReasoningEffort = normalizeReasoningEffort(ctx, coderProviderId, coderModel, coderReasoningEffort);
      updates.utilityProviderId = utilityProviderId || undefined;
      updates.utilityModel = utilityModel || undefined;
      updates.utilityReasoningEffort = normalizeReasoningEffort(ctx, utilityProviderId, utilityModel, utilityReasoningEffort);
    }
    if (Object.keys(updates).length > 0) {
      await ctx.runRepo.update(id, updates);
    }

    // Persist the user message and surface it immediately.
    const userMsg = {
      id: `msg-user-${Date.now()}`,
      runId: id,
      role: "user" as const,
      content: task,
      createdAt: new Date().toISOString()
    };
    await ctx.messageRepo.create(userMsg);
    eventBus.emit(`run:${id}`, { type: "message_created", message: userMsg });

    await ctx.runRepo.update(id, { status: "generating" });
    eventBus.emit(`run:${id}`, { type: "status_changed", status: "generating" });

    ctx.orchestrator.continueRun(id, task).catch(err => {
      console.error(`[Orchestrator] Error continuing job ${id}:`, err.message);
    });

    return { success: true };
  });

  // Resolve a pending permission request for a running job.
  server.post("/api/runs/:id/permission", async (request, reply) => {
    const { id } = request.params as { id: string };
    const { decision } = request.body as { decision: PermissionDecision };

    if (!PERMISSION_DECISIONS.includes(decision)) {
      reply.status(400);
      return { error: `Invalid decision value. Must be one of: ${PERMISSION_DECISIONS.join(", ")}` };
    }

    const run = ctx.runRepo.getById(id);
    if (!run) {
      reply.status(404);
      return { error: `Run with id "${id}" not found` };
    }

    // Persist only an exact tool/command/domain key, so an approved command
    // cannot be extended with a suffix or broader network access later.
    const pending = ctx.orchestrator.getPendingPermission(id);
    try {
      if (pending?.toolCall && decision !== "deny") {
        const { tool, command, networkDomains } = permissionKey(pending.toolCall);
        // Never remember a grant for a run_command that escapes the workspace
        // (cd .., absolute/home paths) — those keep prompting on every call.
        // search_web/fetch_url ARE persistable, scoped by query/host (command holds that scope).
        const neverPersist = tool === "run_command" && commandEscapesWorkspace(command);

        if (neverPersist) {
          // no-op: keep asking every time
        } else if (decision === "allow_always") {
          await ctx.permissionRepo.allowGlobal(tool, command, networkDomains);
        } else if (decision === "allow_project" && run.projectPath) {
          await ctx.permissionRepo.allowProject(run.projectPath, tool, command, networkDomains);
        }
      }
    } catch (err: any) {
      console.error("[Server] Error saving permission:", err.message);
    }

    const resolved = ctx.orchestrator.resolvePermission(id, decision);
    if (!resolved) {
      reply.status(400);
      return { error: "No pending permission request found for this run" };
    }

    return { success: true };
  });

  // Resolve a pending ask_user_question request with the user's answer.
  // Body: { selections: string[][], notes?: string[] } — chosen labels and a
  // free-text note per question, both aligned to the questions order.
  server.post("/api/runs/:id/answer", async (request, reply) => {
    const { id } = request.params as { id: string };
    const { selections, notes } = request.body as { selections: unknown; notes?: unknown };

    if (!Array.isArray(selections) || !selections.every(s => Array.isArray(s) && s.every(v => typeof v === "string"))) {
      reply.status(400);
      return { error: "Invalid selections: expected an array of string arrays." };
    }
    if (notes !== undefined && (!Array.isArray(notes) || !notes.every(n => typeof n === "string"))) {
      reply.status(400);
      return { error: "Invalid notes: expected an array of strings." };
    }

    const resolved = ctx.orchestrator.resolveQuestion(id, {
      selections: selections as string[][],
      notes: (notes as string[]) ?? []
    });
    if (!resolved) {
      reply.status(400);
      return { error: "No pending question found for this run" };
    }

    return { success: true };
  });

  // Run history list.
  server.get("/api/runs", async () => {
    return ctx.runRepo.list();
  });

  // Single run details.
  server.get("/api/runs/:id", async (request, reply) => {
    const { id } = request.params as { id: string };
    const run = ctx.runRepo.getById(id);
    if (!run) {
      reply.status(404);
      return { error: `Run with id "${id}" not found` };
    }
    return run;
  });

  // Messages for a single run.
  server.get("/api/runs/:id/messages", async (request, reply) => {
    const { id } = request.params as { id: string };
    const run = ctx.runRepo.getById(id);
    if (!run) {
      reply.status(404);
      return { error: `Run with id "${id}" not found` };
    }
    return ctx.messageRepo.listByRunId(id);
  });

  // Aggregated token/cost totals for a single run, broken down by agent role
  // (main/coder/utility) so preset savings are measurable.
  server.get("/api/runs/:id/usage", async (request, reply) => {
    const { id } = request.params as { id: string };
    const run = ctx.runRepo.getById(id);
    if (!run) {
      reply.status(404);
      return { error: `Run with id "${id}" not found` };
    }
    return ctx.usageLogRepo.getRunSummary(id);
  });

  // Active plan for a single run (drives the plan side panel). Returns null
  // when the run has no plan yet.
  server.get("/api/runs/:id/plan", async (request, reply) => {
    const { id } = request.params as { id: string };
    const run = ctx.runRepo.getById(id);
    if (!run) {
      reply.status(404);
      return { error: `Run with id "${id}" not found` };
    }
    return ctx.planRepo.getActive(id);
  });

  // Pending user-facing request for a run. Permission/question prompts are kept
  // in memory while the orchestrator is paused; this lets the UI restore the card
  // when the user navigates away and later returns to the run.
  server.get("/api/runs/:id/pending", async (request, reply) => {
    const { id } = request.params as { id: string };
    const run = ctx.runRepo.getById(id);
    if (!run) {
      reply.status(404);
      return { error: `Run with id "${id}" not found` };
    }

    const pendingPermission = ctx.orchestrator.getPendingPermission(id);
    const pendingQuestion = ctx.orchestrator.getPendingQuestion(id);

    return {
      permissionRequest: pendingPermission
        ? {
            type: "permission_requested",
            runId: id,
            toolCall: pendingPermission.toolCall,
            preview: buildPermissionPreview(run, pendingPermission.toolCall)
          }
        : null,
      questionRequest: pendingQuestion
        ? {
            type: "question_requested",
            runId: id,
            questions: pendingQuestion.questions
          }
        : null
    };
  });

  // SSE event stream for a specific run.
  server.get("/api/runs/:id/events", async (request, reply) => {
    const { id } = request.params as { id: string };

    reply.raw.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      "Connection": "keep-alive"
    });

    reply.raw.write(`data: ${JSON.stringify({ type: "connected" })}\n\n`);

    const run = ctx.runRepo.getById(id);
    if (!run) {
      reply.raw.write(`data: ${JSON.stringify({ type: "run_failed", errorMessage: "Run not found." })}\n\n`);
      reply.raw.end();
      return;
    }

    // If the run is already in a final state, immediately notify client and close the stream.
    if (run.status === "done" || run.status === "failed" || run.status === "cancelled") {
      if (run.status === "failed") {
        reply.raw.write(`data: ${JSON.stringify({ type: "run_failed", errorMessage: run.errorMessage })}\n\n`);
      } else if (run.status === "done") {
        reply.raw.write(`data: ${JSON.stringify({ type: "run_completed", finalOutput: "" })}\n\n`);
      } else {
        reply.raw.write(`data: ${JSON.stringify({ type: "status_changed", status: run.status })}\n\n`);
      }
      reply.raw.end();
      return;
    }

    const pendingPermission = ctx.orchestrator.getPendingPermission(id);
    if (pendingPermission) {
      reply.raw.write(`data: ${JSON.stringify({
        type: "permission_requested",
        runId: id,
        toolCall: pendingPermission.toolCall,
        preview: buildPermissionPreview(run, pendingPermission.toolCall)
      })}\n\n`);
    }

    const pendingQuestion = ctx.orchestrator.getPendingQuestion(id);
    if (pendingQuestion) {
      reply.raw.write(`data: ${JSON.stringify({
        type: "question_requested",
        runId: id,
        questions: pendingQuestion.questions
      })}\n\n`);
    }

    const listener = (event: any) => {
      reply.raw.write(`data: ${JSON.stringify(event)}\n\n`);

      const finished =
        event.type === "run_completed" ||
        event.type === "run_failed" ||
        (event.type === "status_changed" &&
          (event.status === "done" || event.status === "failed" || event.status === "cancelled"));

      if (finished) {
        eventBus.off(`run:${id}`, listener);
        reply.raw.end();
      }
    };

    eventBus.on(`run:${id}`, listener);

    request.raw.on("close", () => {
      eventBus.off(`run:${id}`, listener);
    });
  });

  // Get paginated/filtered usage logs.
  server.get("/api/usage-logs", async (request) => {
    const query = request.query as any;
    const limit = parseInt(query.limit) || 50;
    const offset = parseInt(query.offset) || 0;
    const search = query.search || undefined;
    const providerId = query.providerId || undefined;
    const agentRole = query.agentRole || undefined;

    return ctx.usageLogRepo.getPaginated({
      limit,
      offset,
      search,
      providerId,
      agentRole
    });
  });
}
