import type {
  ProviderMetadata,
  Run,
  RunMessage,
  Project,
  PermissionRule,
  Plan,
  AgentPreset,
  Memory,
  MemoryCategory,
  MemoryScope,
  AppSettings,
  PaginatedUsageLogs,
  RunUsageSummary,
  SecurityStatus,
  SkillsListResponse,
  McpServerConfig,
  McpServerInfo,
  McpServersResponse,
  ProcessInfo,
  ProcessesResponse,
  ProcessLogsResponse,
  SymbolsResponse,
  PluginManifest,
  PluginTemplate,
  PluginsResponse,
  InstallPluginPayload,
  PluginScope
} from '@locagens/shared';

interface DesktopApiResponse {
  status: number;
  contentType: string;
  body: string;
}

interface DesktopRunEvent {
  subscriptionId: string;
  type: 'message' | 'error' | 'closed';
  data?: string;
  error?: string;
}

interface DesktopBridge {
  apiRequest(input: { path: string; method: string; body?: unknown }): Promise<DesktopApiResponse>;
  subscribeRunEvents(input: { subscriptionId: string; runId: string }): Promise<unknown>;
  unsubscribeRunEvents(subscriptionId: string): Promise<unknown>;
  onRunEvent(listener: (event: DesktopRunEvent) => void): () => void;
  selectDirectory?: () => Promise<{ path: string; name: string } | null>;
  openPath?: (targetPath: string) => Promise<{ ok: true }>;
  restartBackend?: () => Promise<unknown>;
  toggleMaximize?: () => Promise<unknown>;
}

export interface RunEventStream {
  onmessage: ((event: MessageEvent<string>) => void) | null;
  onerror: ((event: Event) => void) | null;
  close(): void;
}

/**
 * Resolves the backend base URL. The config file (settings.json) is the single
 * source of truth for the port; this picks it up in two ways:
 *  1. Electron: the main process reads settings.json, starts the backend, and
 *     injects `window.__LOCAGENS_API_BASE__` for the renderer.
 *  2. Dev (Vite): vite.config reads the same file and injects VITE_API_BASE.
 * The hardcoded localhost default is only a last resort.
 */
function resolveApiBase(): string {
  const injected = (globalThis as any).__LOCAGENS_API_BASE__;
  if (typeof injected === 'string' && injected) return injected;
  const fromEnv = import.meta.env.VITE_API_BASE;
  if (typeof fromEnv === 'string' && fromEnv) return fromEnv;
  if (import.meta.env.DEV) return '';
  return 'http://localhost:4321';
}

export const API_BASE = resolveApiBase();

function desktopBridge(): DesktopBridge | undefined {
  return (globalThis as any).__LOCAGENS_DESKTOP__ as DesktopBridge | undefined;
}

export type PermissionDecision = 'allow_once' | 'allow_project' | 'allow_always' | 'allow_run' | 'deny';

// Optional dual-model fields: when an agent preset is active the architect uses
// providerId/model and delegates code-writing to the coder model below.
export interface AgentRunFields {
  coderProviderId?: string;
  coderModel?: string;
  coderReasoningEffort?: string;
  utilityProviderId?: string;
  utilityModel?: string;
  utilityReasoningEffort?: string;
  agentPreset?: string;
}

export interface CreateRunPayload extends AgentRunFields {
  task: string;
  projectPath?: string;
  projectName?: string;
  providerId: string;
  model: string;
  reasoningEffort?: string;
  mode: string;
  bypassPermissions: boolean;
}

export interface ContinueRunPayload extends AgentRunFields {
  task: string;
  providerId: string;
  model: string;
  reasoningEffort?: string;
  mode: string;
  bypassPermissions: boolean;
}

/** Reads `{ error }` from a failed response, falling back to a default message. */
async function errorMessage(response: Response, fallback: string): Promise<string> {
  const body = await response.json().catch(() => ({}));
  return body?.error || fallback;
}

interface RequestInitJson {
  method?: 'GET' | 'POST' | 'PUT' | 'DELETE';
  /** JSON.stringify'd into the body; sets the Content-Type header. */
  body?: unknown;
  errorFallback?: string;
}

/** Core fetch wrapper: throws Error(`{ error }` body or fallback) on !ok. */
async function request(path: string, init: RequestInitJson = {}): Promise<Response> {
  const { method = 'GET', body, errorFallback = 'Request failed.' } = init;
  const desktop = desktopBridge();
  const response = desktop?.apiRequest
      ? await desktop.apiRequest({ path, method, ...(body !== undefined ? { body } : {}) })
      .then(result => new Response(result.body, {
        status: result.status,
        headers: result.contentType ? { 'content-type': result.contentType } : undefined
      }))
    : await fetch(`${API_BASE}${path}`, {
      method,
      ...(body !== undefined
        ? { headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }
        : {})
    });
  if (!response.ok) throw new Error(await errorMessage(response, errorFallback));
  return response;
}

async function requestJson<T>(path: string, init: RequestInitJson = {}): Promise<T> {
  return (await request(path, init)).json() as Promise<T>;
}

async function requestVoid(path: string, init: RequestInitJson = {}): Promise<void> {
  await request(path, init);
}

/** GETs that report failure as null so callers can `if (data)` instead of try/catch. */
async function getJson<T>(path: string): Promise<T | null> {
  try {
    return (await request(path)).json() as Promise<T>;
  } catch {
    return null;
  }
}

/** DELETEs whose failures are intentionally ignored (existing behavior). */
async function deleteQuiet(path: string): Promise<void> {
  await request(path, { method: 'DELETE' }).catch(() => undefined);
}

function openDesktopEventStream(runId: string, desktop: DesktopBridge): RunEventStream {
  const subscriptionId = `run-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  let closed = false;
  const stream: RunEventStream = {
    onmessage: null,
    onerror: null,
    close() {
      if (closed) return;
      closed = true;
      removeListener();
      void desktop.unsubscribeRunEvents(subscriptionId);
    }
  };
  const removeListener = desktop.onRunEvent((event) => {
    if (closed || event.subscriptionId !== subscriptionId) return;
    if (event.type === 'message' && event.data !== undefined) {
      stream.onmessage?.(new MessageEvent('message', { data: event.data }));
    } else {
      stream.onerror?.(new Event(event.type));
      if (event.type === 'closed') stream.close();
    }
  });
  void desktop.subscribeRunEvents({ subscriptionId, runId }).catch(() => stream.onerror?.(new Event('error')));
  return stream;
}

export const api = {
  getSettings: () => getJson<AppSettings>('/api/settings'),
  saveSettings: (settings: AppSettings) =>
    requestJson<AppSettings & { restartRequired?: boolean }>('/api/settings', { method: 'PUT', body: settings, errorFallback: 'Failed to save settings.' }),
  getProviders: () => getJson<ProviderMetadata[]>('/api/providers'),
  getAgentPresets: () => getJson<AgentPreset[]>('/api/agent-presets'),
  saveAgentPresets: (presets: Record<string, any>) =>
    requestVoid('/api/agent-presets', { method: 'PUT', body: presets, errorFallback: 'Failed to save agent presets.' }),
  getProvidersConfig: () => getJson<Record<string, any>>('/api/providers/config'),
  saveProvidersConfig: (configs: Record<string, any>) =>
    requestVoid('/api/providers/config', { method: 'POST', body: configs, errorFallback: 'Failed to save provider settings.' }),
  getRuns: () => getJson<Run[]>('/api/runs'),
  getMessages: (runId: string) => getJson<RunMessage[]>(`/api/runs/${runId}/messages`),
  getRunPlan: (runId: string) => getJson<Plan | null>(`/api/runs/${runId}/plan`),
  getRunUsage: (runId: string) => getJson<RunUsageSummary>(`/api/runs/${runId}/usage`),
  getRunPending: (runId: string) => getJson<{ permissionRequest: any | null; questionRequest: any | null }>(`/api/runs/${runId}/pending`),
  getProjects: () => getJson<Project[]>('/api/projects'),
  getProjectFiles: (path: string, query?: string) => {
    const q = query ? `&query=${encodeURIComponent(query)}` : '';
    return getJson<{ files: string[] }>(`/api/projects/files?path=${encodeURIComponent(path)}${q}`);
  },
  getProjectSymbols: (path: string, query?: string, kind?: string) => {
    const q = query ? `&query=${encodeURIComponent(query)}` : '';
    const k = kind ? `&kind=${encodeURIComponent(kind)}` : '';
    return getJson<SymbolsResponse>(`/api/projects/symbols?path=${encodeURIComponent(path)}${q}${k}`);
  },
  getPermissions: () => getJson<PermissionRule[]>('/api/permissions'),
  getUsageLogs: (params?: {
    limit?: number;
    offset?: number;
    search?: string;
    providerId?: string;
    agentRole?: string;
  }) => {
    const searchParams = new URLSearchParams();
    if (params) {
      if (params.limit !== undefined) searchParams.set('limit', String(params.limit));
      if (params.offset !== undefined) searchParams.set('offset', String(params.offset));
      if (params.search) searchParams.set('search', params.search);
      if (params.providerId) searchParams.set('providerId', params.providerId);
      if (params.agentRole) searchParams.set('agentRole', params.agentRole);
    }
    const queryStr = searchParams.toString();
    return getJson<PaginatedUsageLogs>(`/api/usage-logs${queryStr ? '?' + queryStr : ''}`);
  },

  openEvents(runId: string): RunEventStream {
    const desktop = desktopBridge();
    return desktop?.subscribeRunEvents
      ? openDesktopEventStream(runId, desktop)
      : new EventSource(`${API_BASE}/api/runs/${encodeURIComponent(runId)}/events`);
  },

  revokePermission: (id: number) => deleteQuiet(`/api/permissions/${id}`),
  clearPermissions: () => deleteQuiet('/api/permissions'),

  getMemories: () => getJson<Memory[]>('/api/memories'),
  createMemory: (payload: { scope: MemoryScope; category: MemoryCategory; content: string; projectPath?: string }) =>
    requestJson<Memory>('/api/memories', { method: 'POST', body: payload, errorFallback: 'Failed to save memory.' }),
  updateMemory: (id: number, content: string) =>
    requestJson<Memory>(`/api/memories/${id}`, { method: 'PUT', body: { content }, errorFallback: 'Failed to update memory.' }),
  deleteMemory: (id: number) => deleteQuiet(`/api/memories/${id}`),
  clearMemories: () => deleteQuiet('/api/memories'),

  getSkills: (projectPath?: string) => {
    const q = projectPath ? `?projectPath=${encodeURIComponent(projectPath)}` : '';
    return getJson<SkillsListResponse>(`/api/skills${q}`);
  },
  /** Install a SKILL.md (full file text) into user or project skills. */
  installSkill: (payload: { target: 'user' | 'project'; content: string; projectPath?: string }) =>
    requestJson<{ success: true; skill: import('@locagens/shared').SkillSummary }>('/api/skills/install', {
      method: 'POST',
      body: payload,
      errorFallback: 'Failed to install skill.'
    }),
  deleteSkill: (payload: { target: 'user' | 'project'; name: string; projectPath?: string }) =>
    requestJson<{ success: true; deleted: boolean }>(`/api/skills/${encodeURIComponent(payload.name)}`, {
      method: 'DELETE',
      body: { target: payload.target, projectPath: payload.projectPath },
      errorFallback: 'Failed to delete skill.'
    }),
  openSkillsFolder: async (target: 'user' | 'project', projectPath?: string) => {
    const result = await requestJson<{ path: string; target: 'user' | 'project' }>(
      '/api/skills/open-folder',
      {
        method: 'POST',
        body: { target, projectPath },
        errorFallback: 'Failed to resolve skills folder.'
      }
    );
    const desktop = desktopBridge();
    if (desktop?.openPath) {
      await desktop.openPath(result.path);
      return { ...result, opened: true as const };
    }
    try {
      if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(result.path);
        return { ...result, opened: false as const, copied: true as const };
      }
    } catch {
      /* ignore clipboard failures */
    }
    return { ...result, opened: false as const, copied: false as const };
  },

  getMcpServers: (projectPath?: string) => {
    const q = projectPath ? `?projectPath=${encodeURIComponent(projectPath)}` : '';
    return getJson<McpServersResponse>(`/api/mcp/servers${q}`);
  },
  saveMcpServer: (config: McpServerConfig) =>
    requestJson<{ success: true; server: McpServerInfo }>('/api/mcp/servers', {
      method: 'POST',
      body: config,
      errorFallback: 'Failed to save MCP server.'
    }),
  deleteMcpServer: (name: string, projectPath?: string) =>
    requestJson<{ success: true; deleted: boolean }>(`/api/mcp/servers/${encodeURIComponent(name)}`, {
      method: 'DELETE',
      body: { projectPath },
      errorFallback: 'Failed to delete MCP server.'
    }),
  restartMcpServer: (name: string, projectPath?: string) =>
    requestJson<{ success: true; server: McpServerInfo }>(`/api/mcp/servers/${encodeURIComponent(name)}/restart`, {
      method: 'POST',
      body: { projectPath },
      errorFallback: 'Failed to restart MCP server.'
    }),
  toggleMcpServer: (name: string, enabled: boolean, projectPath?: string) =>
    requestJson<{ success: true; server: McpServerInfo }>(`/api/mcp/servers/${encodeURIComponent(name)}/toggle`, {
      method: 'POST',
      body: { enabled, projectPath },
      errorFallback: 'Failed to toggle MCP server.'
    }),

  getPlugins: (projectPath?: string) => {
    const q = projectPath ? `?projectPath=${encodeURIComponent(projectPath)}` : '';
    return getJson<PluginsResponse>(`/api/plugins${q}`);
  },
  getPluginTemplates: () =>
    getJson<{ templates: PluginTemplate[] }>('/api/plugins/templates'),
  installPlugin: (payload: InstallPluginPayload) =>
    requestJson<{ success: true; plugin: PluginManifest }>('/api/plugins/install', {
      method: 'POST',
      body: payload,
      errorFallback: 'Failed to install plugin.'
    }),
  togglePlugin: (id: string, enabled: boolean, scope?: PluginScope, projectPath?: string) =>
    requestJson<{ success: true; plugin: PluginManifest }>(`/api/plugins/${encodeURIComponent(id)}/toggle`, {
      method: 'POST',
      body: { enabled, scope, projectPath },
      errorFallback: 'Failed to toggle plugin.'
    }),
  deletePlugin: (id: string, scope?: PluginScope, projectPath?: string) => {
    const q = `?scope=${scope || 'user'}${projectPath ? `&projectPath=${encodeURIComponent(projectPath)}` : ''}`;
    return requestJson<{ success: true; deleted: boolean }>(`/api/plugins/${encodeURIComponent(id)}${q}`, {
      method: 'DELETE',
      errorFallback: 'Failed to delete plugin.'
    });
  },

  createRun: (payload: CreateRunPayload) =>
    requestJson<Run>('/api/runs', { method: 'POST', body: payload, errorFallback: 'Failed to start chat.' }),
  continueRun: (runId: string, payload: ContinueRunPayload) =>
    requestVoid(`/api/runs/${runId}/continue`, { method: 'POST', body: payload, errorFallback: 'Failed to send message.' }),

  // TODO: should return a typed result instead of leaking the raw Response.
  async cancelRun(runId: string): Promise<Response> {
    return request(`/api/runs/${runId}/cancel`, { method: 'POST' });
  },

  sendPermissionDecision: (runId: string, decision: PermissionDecision) =>
    requestVoid(`/api/runs/${runId}/permission`, { method: 'POST', body: { decision }, errorFallback: 'Permission decision could not be processed.' }),
  answerQuestion: (runId: string, selections: string[][], notes: string[]) =>
    requestVoid(`/api/runs/${runId}/answer`, { method: 'POST', body: { selections, notes }, errorFallback: 'Answer could not be submitted.' }),

  createProject: (path: string, name: string) =>
    requestJson<Project>('/api/projects', { method: 'POST', body: { path, name }, errorFallback: 'Failed to add project.' }),
  deleteProject: (path: string) => deleteQuiet(`/api/projects?path=${encodeURIComponent(path)}`),
  browseFolder: async () => {
    const desktop = desktopBridge();
    if (desktop) {
      if (desktop.selectDirectory) return desktop.selectDirectory();
      throw new Error('The desktop shell is out of date. Fully quit and reopen Locagens.');
    }
    return requestJson<{ path: string; name: string }>('/api/projects/select-dir', { method: 'POST', errorFallback: 'Failed to select folder.' });
  },

  getGitStatus: (path: string) =>
    requestJson<{ isGit: boolean; branch?: string; hasChanges?: boolean }>(
      `/api/projects/git/status?path=${encodeURIComponent(path)}`,
      { errorFallback: 'Failed to fetch status' }
    ),
  getGitDiffDetails: (path: string) =>
    requestJson<{ files?: Array<{ path: string; kind: string; oldText: string; newText: string }> }>(
      `/api/projects/git/diff-details?path=${encodeURIComponent(path)}`,
      { errorFallback: 'Failed to fetch diff details' }
    ),
  generateCommitMessage: (runId: string) =>
    requestJson<{ message: string }>('/api/projects/git/generate-message', { method: 'POST', body: { runId }, errorFallback: 'Failed to generate message' }),
  gitCommit: (path: string, message: string, action: string) =>
    requestVoid('/api/projects/git/commit', { method: 'POST', body: { path, message, action }, errorFallback: 'Failed to execute git action' }),

  async fetchModels(payload: { type: string; baseUrl: string; apiKey?: string; providerId?: string }): Promise<{ success: boolean; models?: string[]; error?: string }> {
    // HTTP failures come back as { success: false } (callers branch on it);
    // network errors still throw, matching the callers' try/catch path.
    const response = await request('/api/providers/fetch-models', { method: 'POST', body: payload, errorFallback: 'Failed to fetch models.' });
    if (!response.ok) {
      return { success: false, error: await errorMessage(response, 'Failed to fetch models.') };
    }
    return response.json() as Promise<{ success: boolean; models?: string[]; error?: string }>;
  },
  getSecurityStatus: () => getJson<SecurityStatus>('/api/security/status'),
  installWindowsSandbox: () => requestJson<SecurityStatus>('/api/security/sandbox/setup', { method: 'POST', body: { confirmed: true }, errorFallback: 'Sandbox setup failed.' }),

  // Background Processes & Terminal
  getProcesses: (projectPath?: string) => {
    const q = projectPath ? `?projectPath=${encodeURIComponent(projectPath)}` : '';
    return getJson<ProcessesResponse>(`/api/processes${q}`);
  },
  spawnProcess: (command: string, projectPath?: string, env?: Record<string, string>) =>
    requestJson<{ process: ProcessInfo }>('/api/processes/spawn', {
      method: 'POST',
      body: { command, projectPath, env },
      errorFallback: 'Failed to start background process.'
    }),
  killProcess: (id: string) =>
    requestJson<{ success: boolean }>(`/api/processes/${encodeURIComponent(id)}/kill`, {
      method: 'POST',
      errorFallback: 'Failed to stop process.'
    }),
  restartProcess: (id: string) =>
    requestJson<{ process: ProcessInfo }>(`/api/processes/${encodeURIComponent(id)}/restart`, {
      method: 'POST',
      errorFallback: 'Failed to restart process.'
    }),
  getProcessLogs: (id: string) =>
    getJson<ProcessLogsResponse>(`/api/processes/${encodeURIComponent(id)}/logs`)
};
