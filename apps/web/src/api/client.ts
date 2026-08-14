import type { ProviderMetadata, Run, RunMessage, Project, PermissionRule, Plan, AgentPreset, Memory, MemoryCategory, MemoryScope, AppSettings, PaginatedUsageLogs, RunUsageSummary } from '@locagens/shared';

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
  return 'http://localhost:4321';
}

export const API_BASE = resolveApiBase();

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
  const response = await fetch(`${API_BASE}${path}`, {
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
  const response = await fetch(`${API_BASE}${path}`);
  if (!response.ok) return null;
  return response.json() as Promise<T>;
}

/** DELETEs whose failures are intentionally ignored (existing behavior). */
async function deleteQuiet(path: string): Promise<void> {
  await fetch(`${API_BASE}${path}`, { method: 'DELETE' });
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

  eventsUrl: (runId: string) => `${API_BASE}/api/runs/${runId}/events`,

  revokePermission: (id: number) => deleteQuiet(`/api/permissions/${id}`),
  clearPermissions: () => deleteQuiet('/api/permissions'),

  getMemories: () => getJson<Memory[]>('/api/memories'),
  createMemory: (payload: { scope: MemoryScope; category: MemoryCategory; content: string; projectPath?: string }) =>
    requestJson<Memory>('/api/memories', { method: 'POST', body: payload, errorFallback: 'Failed to save memory.' }),
  updateMemory: (id: number, content: string) =>
    requestJson<Memory>(`/api/memories/${id}`, { method: 'PUT', body: { content }, errorFallback: 'Failed to update memory.' }),
  deleteMemory: (id: number) => deleteQuiet(`/api/memories/${id}`),
  clearMemories: () => deleteQuiet('/api/memories'),

  createRun: (payload: CreateRunPayload) =>
    requestJson<Run>('/api/runs', { method: 'POST', body: payload, errorFallback: 'Failed to start chat.' }),
  continueRun: (runId: string, payload: ContinueRunPayload) =>
    requestVoid(`/api/runs/${runId}/continue`, { method: 'POST', body: payload, errorFallback: 'Failed to send message.' }),

  // TODO: should return a typed result instead of leaking the raw Response.
  async cancelRun(runId: string): Promise<Response> {
    return fetch(`${API_BASE}/api/runs/${runId}/cancel`, { method: 'POST' });
  },

  sendPermissionDecision: (runId: string, decision: PermissionDecision) =>
    requestVoid(`/api/runs/${runId}/permission`, { method: 'POST', body: { decision }, errorFallback: 'Permission decision could not be processed.' }),
  answerQuestion: (runId: string, selections: string[][], notes: string[]) =>
    requestVoid(`/api/runs/${runId}/answer`, { method: 'POST', body: { selections, notes }, errorFallback: 'Answer could not be submitted.' }),

  createProject: (path: string, name: string) =>
    requestJson<Project>('/api/projects', { method: 'POST', body: { path, name }, errorFallback: 'Failed to add project.' }),
  deleteProject: (path: string) => deleteQuiet(`/api/projects?path=${encodeURIComponent(path)}`),
  browseFolder: () =>
    requestJson<{ path: string; name: string }>('/api/projects/select-dir', { method: 'POST', errorFallback: 'Failed to select folder.' }),

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
    const response = await fetch(`${API_BASE}/api/providers/fetch-models`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload)
    });
    if (!response.ok) {
      return { success: false, error: await errorMessage(response, 'Failed to fetch models.') };
    }
    return response.json() as Promise<{ success: boolean; models?: string[]; error?: string }>;
  }
};
