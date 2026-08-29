import fs from "fs";
import path from "path";
import { defaultProviderConfigPath, type ProviderMetadata, type AgentPreset, type ModelReasoningSettings, type ReasoningEffort, type ReasoningOption, type ReasoningStyle, type ResolvedReasoningConfig, type ModelPricing, type PriceTier } from "@locagens/shared";
import type { ModelProvider } from "./ModelProvider.js";
import { ProviderFactory } from "./ProviderFactory.js";
import { MacOSKeychainProviderSecretStore, type ProviderSecretStore } from "./ProviderSecretStore.js";

interface ProviderConfigBlock {
  type: "openai-compatible" | "anthropic";
  displayName: string;
  baseUrl: string;
  apiKey: string;
  models: string[];
  modelSettings?: Record<string, ModelSettingsBlock>;
}

interface ModelSettingsBlock {
  reasoning?: ModelReasoningSettings;
  reasoningEfforts?: ReasoningEffort[];
  contextLimit?: number;
  pricing?: ModelPricing;
  temperature?: number;
}

interface PersistedProviderConfigBlock extends Omit<ProviderConfigBlock, "apiKey"> {
  apiKey?: string;
  apiKeyRef?: string;
}

export const PRESERVE_API_KEY_VALUE = "__LOCAGENS_PRESERVE_API_KEY__";
const REASONING_EFFORTS = new Set<ReasoningEffort>(["default", "none", "minimal", "low", "medium", "high", "xhigh", "max"]);
const REASONING_STYLES = new Set<ReasoningStyle>(["openai-chat", "anthropic-budget"]);

export interface EditableProviderConfigBlock extends Omit<ProviderConfigBlock, "apiKey"> {
  apiKey: typeof PRESERVE_API_KEY_VALUE | "";
  hasApiKey: boolean;
}

interface AgentPresetBlock {
  displayName: string;
  architect: { providerId: string; model: string; reasoningEffort?: ReasoningEffort };
  coder: { providerId: string; model: string; reasoningEffort?: ReasoningEffort };
  maxSubAgents?: number;
  utility?: { providerId: string; model: string; reasoningEffort?: ReasoningEffort };
  fallback?: boolean;
}

interface ConfigSchema {
  providers: Record<string, PersistedProviderConfigBlock>;
  agentPresets?: Record<string, AgentPresetBlock>;
  // User-overlay only: ids of predefined entries the user removed (tombstones).
  removedProviders?: string[];
  removedPresets?: string[];
}

export class ProviderRegistry {
  private configs: Record<string, ProviderConfigBlock> = {};
  private agentPresets: Record<string, AgentPresetBlock> = {};
  private providers: Map<string, ModelProvider> = new Map();
  private configPathOverride?: string;
  private persistPath = "";
  private secretStore: ProviderSecretStore;
  // Live config lives in the OS app-support directory (providers.json).
  // LOCAGENS_PROVIDER_USER_CONFIG_PATH still enables a read-only base + writable
  // overlay (used by tests); production uses a single file.
  private userConfigPath?: string;
  private baseConfigs: Record<string, ProviderConfigBlock> = {};
  private baseAgentPresets: Record<string, AgentPresetBlock> = {};
  private baseKeyRefIds: Set<string> = new Set();

  constructor(configPathOverride?: string, secretStore: ProviderSecretStore = new MacOSKeychainProviderSecretStore()) {
    this.configPathOverride = configPathOverride;
    this.secretStore = secretStore;
    this.loadConfiguration(configPathOverride);
  }

  private findWorkspaceRoot(): string {
    let currentDir = process.cwd();
    // Prevent infinite loop on root
    const rootDir = path.parse(currentDir).root;
    while (currentDir !== rootDir) {
      if (fs.existsSync(path.join(currentDir, "pnpm-workspace.yaml"))) {
        return currentDir;
      }
      currentDir = path.dirname(currentDir);
    }
    return process.cwd(); // Fallback to current working directory
  }

  private defaultConfigPath(): string {
    return defaultProviderConfigPath();
  }

  private materializeIfMissing(dest: string) {
    if (fs.existsSync(dest)) return;
    const seed = process.env.LOCAGENS_PROVIDER_SEED_PATH || path.join(this.findWorkspaceRoot(), "config", "providers.json");
    const legacyOverlay = path.join(path.dirname(dest), "providers.user.json");
    const hasSeed = seed !== dest && fs.existsSync(seed);
    const hasOverlay = fs.existsSync(legacyOverlay);
    if (!hasSeed && !hasOverlay) return;

    let payload: ConfigSchema;
    if (hasOverlay && hasSeed) {
      payload = this.mergeConfigSchemas(this.readConfigFile(seed), this.readConfigFile(legacyOverlay));
    } else if (hasOverlay) {
      payload = this.readConfigFile(legacyOverlay);
      delete payload.removedProviders;
      delete payload.removedPresets;
    } else {
      payload = this.readConfigFile(seed);
    }
    this.writeConfigFile(dest, payload);
    console.log(`[ProviderRegistry] Seeded provider config at ${dest}`);
  }

  private mergeConfigSchemas(base: ConfigSchema, overlay: ConfigSchema): ConfigSchema {
    const removedProviders = new Set(overlay.removedProviders || []);
    const removedPresets = new Set(overlay.removedPresets || []);
    const providers: Record<string, PersistedProviderConfigBlock> = {};
    for (const [id, block] of Object.entries(base.providers || {})) {
      if (!removedProviders.has(id)) providers[id] = block;
    }
    Object.assign(providers, overlay.providers || {});
    const agentPresets: Record<string, AgentPresetBlock> = {};
    for (const [id, preset] of Object.entries(base.agentPresets || {})) {
      if (!removedPresets.has(id)) agentPresets[id] = preset;
    }
    Object.assign(agentPresets, overlay.agentPresets || {});
    return { providers, agentPresets };
  }

  private loadConfiguration(configPathOverride?: string) {
    const basePath = configPathOverride || this.defaultConfigPath();
    if (!configPathOverride && !process.env.LOCAGENS_PROVIDER_USER_CONFIG_PATH) {
      this.materializeIfMissing(basePath);
    }
    this.userConfigPath = process.env.LOCAGENS_PROVIDER_USER_CONFIG_PATH || undefined;
    // Saves go to the user overlay when present, else straight to the base file.
    this.persistPath = this.userConfigPath || basePath;

    // 1. Base layer (OS app-support file, or a read-only catalog when overlaying).
    const base = this.readConfigFile(basePath);
    this.baseConfigs = this.hydrateConfigs(base.providers || {});
    this.baseAgentPresets = base.agentPresets || {};
    this.baseKeyRefIds = new Set(
      Object.entries(base.providers || {}).filter(([, b]) => !!b.apiKeyRef).map(([id]) => id)
    );
    if (Object.keys(this.baseConfigs).length) {
      console.log(`[ProviderRegistry] Loaded predefined catalog from: ${basePath}`);
    } else {
      console.warn(`[ProviderRegistry] No predefined catalog at ${basePath}.`);
    }

    // 2. Writable user overlay layer (custom providers + edits + tombstones).
    const removedProviders = new Set<string>();
    const removedPresets = new Set<string>();
    let overlayProviders: Record<string, ProviderConfigBlock> = {};
    let overlayPresets: Record<string, AgentPresetBlock> = {};
    if (this.userConfigPath) {
      const user = this.readConfigFile(this.userConfigPath);
      overlayProviders = this.hydrateConfigs(user.providers || {});
      overlayPresets = user.agentPresets || {};
      (user.removedProviders || []).forEach(id => removedProviders.add(id));
      (user.removedPresets || []).forEach(id => removedPresets.add(id));
      console.log(`[ProviderRegistry] Loaded user overlay from: ${this.userConfigPath}`);
    }

    // 3. Merge: base minus tombstones, then user overrides/additions win by id.
    this.configs = {};
    for (const [id, block] of Object.entries(this.baseConfigs)) {
      if (!removedProviders.has(id)) this.configs[id] = block;
    }
    Object.assign(this.configs, overlayProviders);

    this.agentPresets = {};
    for (const [id, preset] of Object.entries(this.baseAgentPresets)) {
      if (!removedPresets.has(id)) this.agentPresets[id] = preset;
    }
    Object.assign(this.agentPresets, overlayPresets);

    // Single-file mode only: migrate any legacy inline keys into the keychain.
    if (!this.userConfigPath && this.shouldMigrateInlineApiKeys(base.providers || {})) {
      this.persist();
    }
  }

  /** Reads + parses a config file, returning an empty schema if missing/invalid. */
  private readConfigFile(filePath: string): ConfigSchema {
    try {
      if (!fs.existsSync(filePath)) return { providers: {} };
      return JSON.parse(fs.readFileSync(filePath, "utf-8")) as ConfigSchema;
    } catch (err: any) {
      console.error(`[ProviderRegistry] Failed to parse configuration file at ${filePath}:`, err.message);
      return { providers: {} };
    }
  }

  getSafeMetadata(): ProviderMetadata[] {
    return Object.entries(this.configs).map(([id, block]) => ({
      id,
      displayName: block.displayName,
      type: block.type,
      models: block.models,
      modelSettings: this.safeModelSettings(block)
    }));
  }

  getSupportedReasoningEfforts(providerId: string, model: string): ReasoningEffort[] {
    return this.normalizedReasoningSettings(providerId, model)?.options?.map(option => option.id) ?? [];
  }

  resolveReasoning(providerId: string, model: string, effort: ReasoningEffort | undefined): ResolvedReasoningConfig | undefined {
    if (!effort || effort === "default") return undefined;
    const reasoning = this.normalizedReasoningSettings(providerId, model);
    const option = reasoning?.options?.find(o => o.id === effort);
    if (!reasoning?.style || !option) return undefined;
    if (reasoning.style === "anthropic-budget" && !option.budgetTokens) return undefined;
    return {
      style: reasoning.style,
      value: option.value || option.id,
      budgetTokens: option.budgetTokens
    };
  }

  /**
   * Safe metadata for the configured dual-model agent presets. Presets only
   * reference provider ids + model names (already public), so no secrets leak.
   * maxSubAgents is clamped to 1..3.
   */
  getAgentPresets(): AgentPreset[] {
    return Object.entries(this.agentPresets).map(([id, block]) => ({
      id,
      displayName: block.displayName || id,
      architect: {
        providerId: block.architect.providerId,
        model: block.architect.model,
        reasoningEffort: this.safePresetReasoningEffort(block.architect.providerId, block.architect.model, block.architect.reasoningEffort)
      },
      coder: {
        providerId: block.coder.providerId,
        model: block.coder.model,
        reasoningEffort: this.safePresetReasoningEffort(block.coder.providerId, block.coder.model, block.coder.reasoningEffort)
      },
      maxSubAgents: Math.min(3, Math.max(1, block.maxSubAgents ?? 3)),
      utility: block.utility
        ? {
            providerId: block.utility.providerId,
            model: block.utility.model,
            reasoningEffort: this.safePresetReasoningEffort(block.utility.providerId, block.utility.model, block.utility.reasoningEffort)
          }
        : undefined,
      fallback: !!block.fallback
    }));
  }

  private safePresetReasoningEffort(providerId: string, model: string, effort: ReasoningEffort | undefined): ReasoningEffort | undefined {
    if (!effort || effort === "default") return undefined;
    return this.getSupportedReasoningEfforts(providerId, model).includes(effort) ? effort : undefined;
  }

  getAgentPreset(id: string): AgentPreset | undefined {
    return this.getAgentPresets().find(p => p.id === id);
  }

  /** Persists the full agent-preset set (keeps providers intact). */
  saveAgentPresets(presets: Record<string, AgentPresetBlock>) {
    this.agentPresets = presets;
    this.persist();
  }

  getProvider(id: string): ModelProvider {
    // Check cache first
    let provider = this.providers.get(id);
    if (provider) return provider;

    const block = this.configs[id];
    if (!block) {
      throw new Error(`Provider config with id "${id}" not found`);
    }

    provider = ProviderFactory.create(block.type, block.baseUrl, block.apiKey);
    this.providers.set(id, provider);
    return provider;
  }

  getFullConfigs(): Record<string, ProviderConfigBlock> {
    return this.configs;
  }

  getEditableConfigs(): Record<string, EditableProviderConfigBlock> {
    return Object.fromEntries(
      Object.entries(this.configs).map(([id, block]) => [
        id,
        {
          type: block.type,
          displayName: block.displayName,
          baseUrl: block.baseUrl,
          apiKey: block.apiKey ? PRESERVE_API_KEY_VALUE : "",
          hasApiKey: !!block.apiKey,
          models: block.models,
          modelSettings: this.safeModelSettings(block)
        }
      ])
    );
  }

  saveConfigs(configs: Record<string, ProviderConfigBlock>) {
    this.configs = this.mergePreservedApiKeys(configs);
    this.providers.clear();
    this.persist();
  }

  private hydrateConfigs(configs: Record<string, PersistedProviderConfigBlock>): Record<string, ProviderConfigBlock> {
    return Object.fromEntries(
      Object.entries(configs).map(([id, block]) => [
        id,
        {
          type: block.type,
          displayName: block.displayName,
          baseUrl: block.baseUrl,
          apiKey: block.apiKeyRef ? this.secretStore.get(block.apiKeyRef) : block.apiKey || "",
          models: block.models,
          modelSettings: this.safeModelSettings(block)
        }
      ])
    );
  }

  private shouldMigrateInlineApiKeys(configs: Record<string, PersistedProviderConfigBlock>): boolean {
    return !this.configPathOverride && this.secretStore.supportsSecurePersistence && Object.values(configs).some(block => !!block.apiKey);
  }

  private mergePreservedApiKeys(configs: Record<string, ProviderConfigBlock>): Record<string, ProviderConfigBlock> {
    return Object.fromEntries(
      Object.entries(configs).map(([id, block]) => {
        const current = this.configs[id];
        const nextApiKey = block.apiKey === PRESERVE_API_KEY_VALUE ? current?.apiKey || "" : block.apiKey || "";
        return [
          id,
          {
            type: block.type,
            displayName: block.displayName,
            baseUrl: block.baseUrl,
            apiKey: nextApiKey,
            models: block.models,
            modelSettings: this.safeModelSettings(block)
          }
        ];
      })
    );
  }

  private normalizedReasoningSettings(providerId: string, model: string): ModelReasoningSettings | undefined {
    const block = this.configs[providerId];
    if (!block) return undefined;
    const settings = this.safeModelSettings(block)[model]?.reasoning;
    return settings?.options?.length ? settings : undefined;
  }

  private safeModelSettings(block: Pick<ProviderConfigBlock, "type" | "models" | "modelSettings">): Record<string, ModelSettingsBlock> {
    const modelSet = new Set(block.models);
    const entries = Object.entries(block.modelSettings || {})
      .filter(([model]) => modelSet.has(model))
      .map(([model, settings]) => {
        const reasoning = this.safeReasoningSettings(block.type, settings);
        const pricing = this.safePricing(settings.pricing);
        const contextLimit = this.safeContextLimit(settings.contextLimit);
        const clean: ModelSettingsBlock = {};
        if (reasoning) {
          clean.reasoning = reasoning;
          clean.reasoningEfforts = reasoning.options?.map(option => option.id);
        }
        if (pricing) clean.pricing = pricing;
        if (contextLimit !== undefined) clean.contextLimit = contextLimit;
        if (typeof settings.temperature === 'number' && settings.temperature >= 0 && settings.temperature <= 2) {
          clean.temperature = settings.temperature;
        }
        return [model, clean] as const;
      })
      // Keep a model entry only if it carries at least one usable setting.
      .filter(([, settings]) => (settings.reasoning?.options?.length ?? 0) > 0 || !!settings.pricing || settings.contextLimit !== undefined || settings.temperature !== undefined);
    return Object.fromEntries(entries);
  }

  /** Sanitizes user-entered pricing: drops invalid tiers and sorts by ceiling. */
  private safePricing(pricing?: ModelPricing): ModelPricing | undefined {
    if (!pricing || !Array.isArray(pricing.tiers)) return undefined;
    const tiers = pricing.tiers
      .map(tier => this.safePriceTier(tier))
      .filter((tier): tier is PriceTier => !!tier)
      .sort((a, b) => (a.upToInputTokens ?? Infinity) - (b.upToInputTokens ?? Infinity));
    return tiers.length > 0 ? { tiers } : undefined;
  }

  private safePriceTier(tier: PriceTier): PriceTier | undefined {
    const inputRate = Number(tier?.inputRate);
    const outputRate = Number(tier?.outputRate);
    if (!Number.isFinite(inputRate) || inputRate < 0 || !Number.isFinite(outputRate) || outputRate < 0) return undefined;
    const clean: PriceTier = { inputRate, outputRate };
    const ceiling = Number(tier.upToInputTokens);
    if (Number.isFinite(ceiling) && ceiling > 0) clean.upToInputTokens = Math.floor(ceiling);
    const cacheReadRate = Number(tier.cacheReadRate);
    if (Number.isFinite(cacheReadRate) && cacheReadRate >= 0) clean.cacheReadRate = cacheReadRate;
    const cacheWriteRate = Number(tier.cacheWriteRate);
    if (Number.isFinite(cacheWriteRate) && cacheWriteRate >= 0) clean.cacheWriteRate = cacheWriteRate;
    return clean;
  }

  private safeContextLimit(value?: number): number | undefined {
    const n = Number(value);
    return Number.isFinite(n) && n > 0 ? Math.floor(n) : undefined;
  }

  /** The user-entered pricing for a model, if any (else undefined => built-in sheet). */
  resolvePricing(providerId: string, model: string): ModelPricing | undefined {
    const block = this.configs[providerId];
    if (!block) return undefined;
    return this.safeModelSettings(block)[model]?.pricing;
  }

  /** The user-entered context window (tokens) for a model; undefined = no compaction. */
  resolveContextLimit(providerId: string, model: string): number | undefined {
    const block = this.configs[providerId];
    if (!block) return undefined;
    return this.safeContextLimit(this.safeModelSettings(block)[model]?.contextLimit);
  }

  /** The user-entered temperature override for a model; undefined = provider default. */
  resolveTemperature(providerId: string, model: string): number | undefined {
    const block = this.configs[providerId];
    if (!block) return undefined;
    const temp = this.safeModelSettings(block)[model]?.temperature;
    return temp !== undefined ? temp : undefined;
  }

  private safeReasoningSettings(providerType: ProviderConfigBlock["type"], settings: ModelSettingsBlock): ModelReasoningSettings | undefined {
    const styleCandidate = settings.reasoning?.style;
    const style = REASONING_STYLES.has(styleCandidate as ReasoningStyle)
      ? styleCandidate as ReasoningStyle
      : providerType === "anthropic" ? "anthropic-budget" : "openai-chat";
    const rawOptions: ReasoningOption[] = settings.reasoning?.options?.length
      ? settings.reasoning.options
      : (settings.reasoning?.reasoningEfforts || settings.reasoningEfforts || []).map(id => ({ id }));
    const options = rawOptions
      .map(option => this.safeReasoningOption(style, option))
      .filter((option): option is ReasoningOption => !!option);
    return options.length > 0 ? { style, options } : undefined;
  }

  private safeReasoningOption(style: ReasoningStyle, option: ReasoningOption): ReasoningOption | undefined {
    if (!REASONING_EFFORTS.has(option.id) || option.id === "default") return undefined;
    const clean: ReasoningOption = {
      id: option.id,
      label: option.label,
      value: option.value || option.id
    };
    if (style === "anthropic-budget") {
      const budgetTokens = Number(option.budgetTokens);
      if (!Number.isFinite(budgetTokens) || budgetTokens < 1024) return undefined;
      clean.budgetTokens = Math.floor(budgetTokens);
    }
    return clean;
  }

  /** Persists config: a user overlay when active, else the single base file. */
  private persist() {
    if (this.userConfigPath) {
      this.persistUserOverlay();
      return;
    }
    const payload: ConfigSchema = {
      providers: this.toPersistedConfigs(this.configs),
      // Persist an explicit empty object so deleting all presets does not fall
      // back to the template presets on the next reload.
      agentPresets: this.agentPresets
    };
    this.writeConfigFile(this.persistPath, payload);
  }

  /**
   * Writes ONLY the user layer: custom providers, edits to predefined ones, and
   * tombstones for removed predefined entries. The predefined base file is never
   * touched, so app updates can refresh it while user data survives. Keys for
   * untouched predefined providers are kept in the keychain (the base file's
   * apiKeyRef still points there).
   */
  private persistUserOverlay() {
    const overlay: Record<string, ProviderConfigBlock> = {};
    for (const [id, block] of Object.entries(this.configs)) {
      if (this.isUnchangedPredefined(id, block)) {
        // Not forked into the overlay; just keep its key current in the keychain.
        if (block.apiKey && this.secretStore.supportsSecurePersistence) {
          this.secretStore.set(id, block.apiKey);
        }
      } else {
        overlay[id] = block;
      }
    }
    const overlayPresets: Record<string, AgentPresetBlock> = {};
    for (const [id, preset] of Object.entries(this.agentPresets)) {
      const base = this.baseAgentPresets[id];
      if (!base || JSON.stringify(base) !== JSON.stringify(preset)) overlayPresets[id] = preset;
    }
    const payload: ConfigSchema = {
      providers: this.toPersistedConfigs(overlay),
      agentPresets: overlayPresets,
      removedProviders: Object.keys(this.baseConfigs).filter(id => !(id in this.configs)),
      removedPresets: Object.keys(this.baseAgentPresets).filter(id => !(id in this.agentPresets))
    };
    this.writeConfigFile(this.persistPath, payload);
  }

  /** True if a provider is a predefined entry the user has not modified. */
  private isUnchangedPredefined(id: string, block: ProviderConfigBlock): boolean {
    const base = this.baseConfigs[id];
    if (!base) return false; // user-added custom provider
    // A key added where the base referenced none must be tracked in the overlay.
    if (block.apiKey && !this.baseKeyRefIds.has(id)) return false;
    const shape = (b: ProviderConfigBlock) => JSON.stringify({
      type: b.type, displayName: b.displayName, baseUrl: b.baseUrl,
      models: b.models, modelSettings: this.safeModelSettings(b)
    });
    return shape(block) === shape(base);
  }

  private writeConfigFile(filePath: string, payload: ConfigSchema) {
    fs.mkdirSync(path.dirname(filePath), { recursive: true });
    fs.writeFileSync(filePath, JSON.stringify(payload, null, 2), "utf-8");
  }

  private toPersistedConfigs(configs: Record<string, ProviderConfigBlock>): Record<string, PersistedProviderConfigBlock> {
    return Object.fromEntries(
      Object.entries(configs).map(([id, block]) => {
        const persisted: PersistedProviderConfigBlock = {
          type: block.type,
          displayName: block.displayName,
          baseUrl: block.baseUrl,
          models: block.models,
          modelSettings: this.safeModelSettings(block)
        };
        // API keys NEVER touch the JSON. When a key exists and secure storage is
        // available it goes to the OS keychain and only a (non-secret) reference
        // pointer is persisted. Without secure storage the key is simply not
        // persisted (e.g. Windows support is handled separately, later).
        if (block.apiKey && this.secretStore.supportsSecurePersistence) {
          persisted.apiKeyRef = this.secretStore.set(id, block.apiKey);
        }
        return [id, persisted];
      })
    );
  }

  // Reload config for testing purposes
  reload() {
    this.providers.clear();
    this.loadConfiguration(this.configPathOverride);
  }
}
