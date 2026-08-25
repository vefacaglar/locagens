/** Local application settings (not provider config). Held in settings.json. */
export interface AppSettings {
  /** TCP port the backend HTTP server listens on. */
  port: number;
}

export type RunStatus =
  | "created"
  | "generating"
  | "awaiting_permission"
  | "awaiting_input"
  | "done"
  | "failed"
  | "cancelled";

export interface ToolCall {
  id: string;
  type: "function";
  function: {
    name: string;
    arguments: string;
  };
}

export interface ChatMessage {
  role: "system" | "user" | "assistant" | "tool";
  content: string;
  tool_call_id?: string;
  name?: string;
  toolCalls?: ToolCall[];
}

export type ReasoningEffort = "default" | "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max";
export type ReasoningStyle = "openai-chat" | "anthropic-budget";

export interface ReasoningOption {
  id: ReasoningEffort;
  label?: string;
  value?: string;
  budgetTokens?: number;
}

export interface ModelReasoningSettings {
  style?: ReasoningStyle;
  options?: ReasoningOption[];
  // Backward-compatible shorthand from older configs.
  reasoningEfforts?: ReasoningEffort[];
}

// One pricing band for a model. Rates are USD per 1,000,000 tokens. Models with
// flat pricing use a single tier (no `upToInputTokens`). Tiered models (e.g.
// Gemini-style "<=200k prompt one rate, >200k another") list several tiers; the
// tier is chosen by the request's total prompt (input) token count.
export interface PriceTier {
  upToInputTokens?: number; // inclusive upper bound on prompt tokens; omit on the last/only tier (= no ceiling)
  inputRate: number;        // USD per 1M input tokens
  outputRate: number;       // USD per 1M output tokens
  cacheReadRate?: number;   // USD per 1M cached-read tokens
  cacheWriteRate?: number;  // USD per 1M cache-write tokens
}

export interface ModelPricing {
  tiers: PriceTier[];
}

export interface ProviderModelSettings extends ModelReasoningSettings {
  reasoning?: ModelReasoningSettings;
  contextLimit?: number;    // max context window in tokens (display/metadata)
  pricing?: ModelPricing;   // user-entered cost rates; falls back to built-in sheet when absent
  temperature?: number;     // per-model temperature override (0-2); omit for provider default
}

export interface ResolvedReasoningConfig {
  style: ReasoningStyle;
  value?: string;
  budgetTokens?: number;
}

export interface CompletionRequest {
  model: string;
  systemPrompt?: string;
  messages: ChatMessage[];
  temperature?: number;
  maxTokens?: number;
  reasoningEffort?: ReasoningEffort;
  reasoning?: ResolvedReasoningConfig;
  tools?: any[];
}

export interface CompletionResponse {
  content: string;
  reasoningContent?: string;
  toolCalls?: ToolCall[];
  raw?: unknown;
  usage?: {
    // Fresh (uncached) input tokens billed at the full input rate.
    inputTokens?: number;
    outputTokens?: number;
    totalTokens?: number;
    // Prompt-cache accounting: tokens read back from cache (cheap) and, for
    // Anthropic, tokens written to the cache on this call (a one-time premium).
    cacheReadInputTokens?: number;
    cacheWriteInputTokens?: number;
  };
}

export interface RunModelSnapshot {
  providerId: string;
  providerDisplayName: string;
  model: string;
  reasoningEffort?: ReasoningEffort;
}

export interface RunMessage {
  id: string;
  runId: string;
  role: "system" | "user" | "assistant" | "tool";
  agentRole?: "planner" | "coder" | "reviewer" | "user" | "utility";
  /**
   * For delegated coder sub-agents: the sub-task title that identifies WHICH
   * sub-agent produced this message. Lets the UI render each coder in its own
   * window instead of merging all coders into one box. Undefined for the
   * architect/main agent.
   */
  agentName?: string;
  providerId?: string;
  providerDisplayName?: string;
  model?: string;
  content: string;
  reasoningContent?: string;
  rawResponse?: string;
  createdAt: string;
}

export interface Run {
  id: string;
  title: string;
  task: string;
  projectPath?: string;
  projectName?: string;
  status: RunStatus;
  providerId: string;
  providerDisplayName: string;
  model: string;
  reasoningEffort?: ReasoningEffort;
  mode?: string;
  // Dual-model ("architect + coder") runs: the architect uses providerId/model
  // above and delegates code-writing to the coder model below via delegate_tasks.
  // All three are empty for ordinary single-model runs.
  coderProviderId?: string;
  coderModel?: string;
  coderReasoningEffort?: ReasoningEffort;
  // Optional lightweight "utility" tier (set from the preset's utility endpoint).
  utilityProviderId?: string;
  utilityModel?: string;
  utilityReasoningEffort?: ReasoningEffort;
  agentPreset?: string;
  errorMessage?: string;
  createdAt: string;
  updatedAt: string;
  lastActiveAt?: string;
}

// One end of a dual-model agent preset: which provider + model to use.
export interface AgentPresetEndpoint {
  providerId: string;
  model: string;
  reasoningEffort?: ReasoningEffort;
}

// A saved "architect + coder" pairing (e.g. "opusplan"). Stored server-side in
// providers.local.json and selectable, optionally, next to the model picker.
// The architect plans/coordinates and delegates code-writing to 1..maxSubAgents
// instances of the coder model via the delegate_tasks tool.
export interface AgentPreset {
  id: string;
  displayName: string;
  architect: AgentPresetEndpoint;
  coder: AgentPresetEndpoint;
  maxSubAgents: number;
  // Optional lightweight tier: a cheap model the architect can delegate tiny
  // mechanical tasks to (locating files/symbols, summarizing, simple renames)
  // via delegate_to_utility, keeping the architect's context lean. Absent =>
  // the run has no utility tier.
  utility?: AgentPresetEndpoint;
  // When true, a delegated task that fails on its sub-agent (utility/coder) is
  // retried down a resilience chain — utility -> coder -> architect — and as a
  // last resort the architect executes it directly with the full toolset. When
  // false (default), a failed delegation is not escalated to the architect.
  fallback?: boolean;
}

export type RunEvent =
  | {
      type: "run_started";
      runId: string;
    }
  | {
      type: "status_changed";
      status: RunStatus;
    }
  | {
      type: "round_started";
      round: number;
    }
  | {
      type: "message_created";
      message: RunMessage;
    }
  | {
      type: "model_snapshot_locked";
      snapshot: RunModelSnapshot;
    }
  | {
      type: "run_completed";
      finalOutput: string;
    }
  | {
      type: "run_failed";
      errorMessage: string;
    }
  | {
      type: "permission_requested";
      runId: string;
      toolCall: any;
      preview?: PermissionPreview | null;
    }
  | {
      type: "plan_updated";
      plan: Plan;
    }
  | {
      type: "question_requested";
      runId: string;
      questions: UserQuestion[];
    }
  | {
      type: "run_title_changed";
      runId: string;
      title: string;
    };

// One selectable answer in a user question. `description` is optional helper text.
export interface UserQuestionOption {
  label: string;
  description?: string;
}

// A multiple-choice question the assistant asks the user via the ask_user_question
// tool. The run pauses (status "awaiting_input") until the user answers, mirroring
// the permission flow. `header` is a very short chip label; `multiSelect` allows
// picking more than one option.
export interface UserQuestion {
  question: string;
  header: string;
  multiSelect: boolean;
  options: UserQuestionOption[];
}

// A single step the assistant tracks inside a plan.
export interface PlanTask {
  text: string;
  status: "pending" | "in_progress" | "completed";
}

// A chat-specific plan the assistant maintains via the update_plan tool.
// Stored in SQLite, surfaced in the right-hand plan panel.
export interface Plan {
  id: string;
  runId: string;
  title: string;
  body?: string;          // optional markdown context / rationale
  tasks: PlanTask[];
  status: "active" | "completed";
  version: number;        // 1-based; increments each time a new plan supersedes the old
  createdAt: string;
  updatedAt: string;
}

// Context attached to a permission request so the UI can render a preview
// (e.g. a red/green diff for file edits) instead of just raw arguments.
export interface PermissionPreview {
  tool: string;
  action: "create" | "edit" | "delete" | "read" | "list" | "mkdir" | "move" | "search" | "command" | "fetch";
  path: string;
  absolutePath: string;
  oldContent: string | null;
  newContent: string | null;
  // Extra display context for tools that are not plain file edits.
  command?: string;   // run_command: the shell command to execute
  networkDomains?: string[]; // run_command: explicitly requested outbound hosts
  destPath?: string;  // move_file: the destination path
  query?: string;     // search_files/search_web: the search query
  url?: string;       // fetch_url: the URL to request
}

// Safe Provider Metadata structure returned by GET /api/providers
export interface ProviderMetadata {
  id: string;
  displayName: string;
  type: "openai-compatible" | "anthropic";
  models: string[];
  modelSettings?: Record<string, ProviderModelSettings>;
}

export interface Project {
  path: string;
  name: string;
  createdAt: string;
}

// A persisted standing approval for workspace tool calls.
export interface PermissionRule {
  id: number;
  scope: "global" | "project";
  projectPath: string;
  tool: string;
  command: string;
  networkDomains: string[];
  status: string;
}

export interface SecurityStatus {
  api: {
    bindAddress: string;
    authenticated: boolean;
  };
  sandbox: {
    platform: string;
    status: "ready" | "setup_required" | "unavailable";
    errors: string[];
    warnings: string[];
    canInstall: boolean;
  };
  policy: {
    commandNetwork: "deny_by_default";
    workspaceWritesOnly: true;
  };
}

// Skills are specialized instruction packs (SKILL.md) the main agent can load
// on demand via load_skill. Discovered from user + project skill directories.
export type SkillSource = "project" | "user";

export interface SkillSummary {
  name: string;
  description: string;
  source: SkillSource;
  body?: string;
}

/** Settings → Skills list payload (catalog + folder roots for Open folder). */
export interface SkillsListResponse {
  skills: SkillSummary[];
  roots: {
    user: string;
    project: string | null;
  };
}

// "global" memories apply to every project; "project" memories only to one path.
export type MemoryScope = "global" | "project";

// What kind of fact a memory holds, so the UI and the model can reason about it:
//   user      — who the user is / their general working style & preferences
//   feedback  — guidance the user gave on HOW the assistant should work
//   project   — a durable fact about this specific codebase
//   reference — a pointer to an external resource (URL, ticket, dashboard)
export type MemoryCategory = "user" | "feedback" | "project" | "reference";

// A durable fact the assistant has chosen to remember across sessions via the
// `remember` tool. Global memories are injected into every run; project memories
// only into runs for the matching project path. Managed by the user in
// Settings → Memory.
export interface Memory {
  id: number;
  scope: MemoryScope;
  projectPath: string;   // "" for global memories
  category: MemoryCategory;
  content: string;
  createdAt: string;
  updatedAt: string;
}

export interface UsageLog {
  id?: number;
  runId: string;
  agentRole?: string;
  providerId: string;
  model: string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  cacheHitRate: number;
  cost: number;
  createdAt: string;
  durationMs?: number;
}

export interface PaginatedUsageLogs {
  logs: UsageLog[];
  total: number;
  uniqueProviders: string[];
  uniqueRoles: string[];
  metrics: {
    totalCost: number;
    totalCalls: number;
    totalTokens: number;
    avgCacheHitRate: number;
  };
}

// Aggregated cost/token totals for one run, with a per-agent-role breakdown so
// the real saving of an architect/coder/utility preset is measurable.
export interface RunUsageSummary {
  runId: string;
  totalCost: number;
  totalCalls: number;
  totalInputTokens: number;
  totalOutputTokens: number;
  totalCacheReadTokens: number;
  avgCacheHitRate: number;
  byRole: Array<{
    agentRole: string;
    cost: number;
    calls: number;
    inputTokens: number;
    outputTokens: number;
  }>;
}

// ============================================================================
// Model Context Protocol (MCP) Types
// ============================================================================

export type McpTransportType = "stdio" | "sse";
export type McpServerScope = "user" | "project";
export type McpConnectionStatus = "connected" | "connecting" | "disconnected" | "error" | "disabled";

export interface McpToolParameter {
  type: string;
  description?: string;
  [key: string]: any;
}

export interface McpToolDefinition {
  name: string;
  originalName: string;
  serverName: string;
  description?: string;
  inputSchema?: {
    type?: string;
    properties?: Record<string, McpToolParameter>;
    required?: string[];
    [key: string]: any;
  };
}

export interface McpServerConfig {
  name: string;
  scope: McpServerScope;
  transport: McpTransportType;
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  url?: string;
  enabled: boolean;
  projectPath?: string;
}

export interface McpServerInfo {
  config: McpServerConfig;
  status: McpConnectionStatus;
  error?: string | null;
  tools: McpToolDefinition[];
}

export interface McpServersResponse {
  servers: McpServerInfo[];
}
