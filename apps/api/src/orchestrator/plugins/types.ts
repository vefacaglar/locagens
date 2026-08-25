import type {
  PluginManifest,
  PluginSummary,
  PluginTemplate,
  PluginScope,
  PluginToolDefinition,
  PluginHooksConfig,
  InstallPluginPayload,
  PluginsResponse
} from "@locagens/shared";

export type {
  PluginManifest,
  PluginSummary,
  PluginTemplate,
  PluginScope,
  PluginToolDefinition,
  PluginHooksConfig,
  InstallPluginPayload,
  PluginsResponse
};

export interface SessionStartHookContext {
  runId: string;
  projectPath?: string;
  projectName?: string;
  systemPrompt: string;
}

export interface SessionStartResult {
  systemPromptSupplement?: string;
}

export interface PreToolUseHookContext {
  runId: string;
  projectPath?: string;
  toolName: string;
  args: Record<string, any>;
}

export interface PreToolUseResult {
  proceed: boolean;
  modifiedArgs?: Record<string, any>;
  handledResult?: string;
  error?: string;
}

export interface PostToolUseHookContext {
  runId: string;
  projectPath?: string;
  toolName: string;
  args: Record<string, any>;
  rawResult: string;
}

export interface PostToolUseResult {
  result: string;
  metadata?: Record<string, any>;
}

export interface PreCompactHookContext {
  runId: string;
  projectPath?: string;
  messages: any[];
}

export interface PreCompactResult {
  summarySupplement?: string;
}
