import path from "node:path";
import { ProviderRegistry } from "./providers/ProviderRegistry.js";
import { db } from "./database/db.js";
import {
  RunRepository,
  MessageRepository,
  ProjectRepository,
  PermissionRepository,
  PlanRepository,
  MemoryRepository,
  UsageLogRepository,
  IRunRepository,
  IMessageRepository,
  IProjectRepository,
  IPermissionRepository,
  IPlanRepository,
  IMemoryRepository,
  IUsageLogRepository
} from "./database/repositories.js";
import { Orchestrator } from "./orchestrator/Orchestrator.js";
import { SkillRegistry } from "./orchestrator/skills/index.js";
import { McpClientManager, McpConfigStore } from "./orchestrator/mcp/index.js";
import { PluginRegistry, PluginHookRunner } from "./orchestrator/plugins/index.js";
import { ProcessManager } from "./orchestrator/processes/ProcessManager.js";
import { SymbolIndexer } from "./orchestrator/symbols/index.js";
import { AppSettingsStore } from "./config/AppSettingsStore.js";
import { requireRegisteredProject } from "./security/projectPaths.js";

/**
 * Shared application dependencies, instantiated once and passed to every
 * route module. Keeps wiring in one place instead of module-level globals.
 */
export interface AppContext {
  registry: ProviderRegistry;
  runRepo: IRunRepository;
  messageRepo: IMessageRepository;
  projectRepo: IProjectRepository;
  permissionRepo: IPermissionRepository;
  planRepo: IPlanRepository;
  memoryRepo: IMemoryRepository;
  usageLogRepo: IUsageLogRepository;
  orchestrator: Orchestrator;
  skillRegistry: SkillRegistry;
  mcpManager: McpClientManager;
  pluginRegistry: PluginRegistry;
  pluginHookRunner: PluginHookRunner;
  processManager: ProcessManager;
  symbolIndexer: SymbolIndexer;
  settingsStore: AppSettingsStore;
  defaultProjectPath: string;
}

export function createAppContext(): AppContext {
  const registry = new ProviderRegistry();
  const runRepo = new RunRepository(db);
  const messageRepo = new MessageRepository(db);
  const projectRepo = new ProjectRepository(db);
  const permissionRepo = new PermissionRepository(db);
  const planRepo = new PlanRepository(db);
  const memoryRepo = new MemoryRepository(db);
  const usageLogRepo = new UsageLogRepository(db);
  const skillRegistry = new SkillRegistry();
  const mcpManager = new McpClientManager(new McpConfigStore());
  const pluginRegistry = new PluginRegistry();
  const pluginHookRunner = new PluginHookRunner();
  const processManager = new ProcessManager();
  const symbolIndexer = new SymbolIndexer();
  const orchestrator = new Orchestrator(
    runRepo,
    messageRepo,
    registry,
    planRepo,
    memoryRepo,
    usageLogRepo,
    skillRegistry,
    mcpManager,
    pluginRegistry,
    pluginHookRunner
  );
  const settingsStore = new AppSettingsStore();

  return {
    registry,
    runRepo,
    messageRepo,
    projectRepo,
    permissionRepo,
    planRepo,
    memoryRepo,
    usageLogRepo,
    orchestrator,
    skillRegistry,
    mcpManager,
    pluginRegistry,
    pluginHookRunner,
    processManager,
    symbolIndexer,
    settingsStore,
    defaultProjectPath: process.cwd()
  };
}

/** Normalizes an optional project path/name pair to concrete values. */
export function normalizeProject(ctx: AppContext, projectPath?: string, projectName?: string) {
  const normalizedPath = requireRegisteredProject(ctx.projectRepo, projectPath?.trim() || ctx.defaultProjectPath);
  return {
    projectPath: normalizedPath,
    projectName: projectName?.trim() || path.basename(normalizedPath) || "Workspace"
  };
}
