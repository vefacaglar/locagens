import type {
  Run,
  RunMessage,
  Project,
  PermissionRule,
  Plan,
  PlanTask,
  Memory,
  MemoryScope,
  MemoryCategory,
  UsageLog,
  PaginatedUsageLogs,
  RunUsageSummary
} from "@locagens/shared";

export interface IRunRepository {
  create(run: Run): Promise<void>;
  getById(id: string): Run | null;
  list(): Run[];
  update(id: string, updates: Partial<Run>): Promise<void>;
}

export interface IMessageRepository {
  create(message: RunMessage): Promise<void>;
  listByRunId(runId: string): RunMessage[];
  update(id: string, updates: Partial<RunMessage>): Promise<void>;
}

export interface PlanInput {
  title?: string;
  body?: string;
  tasks: PlanTask[];
  startNew?: boolean;
}

export interface IPlanRepository {
  getActive(runId: string): Plan | null;
  listByRunId(runId: string): Plan[];
  upsert(runId: string, input: PlanInput): Promise<Plan>;
}

export interface IProjectRepository {
  create(project: Project): Promise<void>;
  list(): Project[];
  delete(path: string): Promise<void>;
  get(path: string): Project | null;
}

export interface IPermissionRepository {
  list(): PermissionRule[];
  isAllowed(projectPath: string | undefined, tool: string, command: string, networkDomains?: string[]): boolean;
  allowProject(projectPath: string, tool: string, command: string, networkDomains?: string[]): Promise<void>;
  allowGlobal(tool: string, command: string, networkDomains?: string[]): Promise<void>;
  deleteById(id: number): Promise<boolean>;
  clear(): Promise<void>;
}

export interface MemoryInput {
  scope: MemoryScope;
  projectPath: string;
  category: MemoryCategory;
  content: string;
}

export interface IMemoryRepository {
  list(): Memory[];
  listForContext(projectPath: string | undefined): Memory[];
  create(input: MemoryInput): Promise<Memory>;
  update(id: number, content: string): Promise<Memory | null>;
  getById(id: number): Memory | null;
  deleteById(id: number): Promise<boolean>;
  clear(): Promise<void>;
}

export interface IUsageLogRepository {
  create(log: UsageLog): Promise<void>;
  listByRunId(runId: string): UsageLog[];
  listAll(): UsageLog[];
  getPaginated(params: {
    limit: number;
    offset: number;
    search?: string;
    providerId?: string;
    agentRole?: string;
  }): PaginatedUsageLogs;
  getRunSummary(runId: string): RunUsageSummary;
}
