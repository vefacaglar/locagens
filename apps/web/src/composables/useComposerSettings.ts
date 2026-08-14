import { computed, ref, watch, nextTick, type Ref } from 'vue';
import type { ProviderMetadata, AgentPreset, ReasoningOption, ProviderModelSettings, Run } from '@locagens/shared';
import { splitCombined } from '../lib/format';

// Three modes are exposed: Build (applies edits directly; the backend's
// 'accept_edits' mode), Plan (planning via the plan panel) and Full Access
// (autonomous — runs every tool, including commands, with NO approval prompts;
// still confined to the project folder). 'chat' is a retired backend mode kept
// in the type so older persisted runs still load/render — it is no longer
// offered in the picker.
export type ChatMode = 'chat' | 'accept_edits' | 'plan' | 'full_access';

export const MODES_LIST = [
  { id: 'accept_edits', label: 'Build', shortcut: '1' },
  { id: 'plan', label: 'Plan', shortcut: '2' },
  { id: 'full_access', label: 'Full Access', shortcut: '3' }
] as const;

export interface ModelOption {
  value: string;
  label: string;
  providerId: string;
  model: string;
  reasoningOptions: ReasoningOption[];
  contextLimit?: number;
}

export const REASONING_EFFORTS = [
  { id: 'default', label: 'Default' },
  { id: 'none', label: 'None' },
  { id: 'minimal', label: 'Minimal' },
  { id: 'low', label: 'Low' },
  { id: 'medium', label: 'Medium' },
  { id: 'high', label: 'High' },
  { id: 'xhigh', label: 'XHigh' },
  { id: 'max', label: 'Max' }
] as const;

const REASONING_EFFORT_IDS = new Set(REASONING_EFFORTS.map(x => x.id));

// --- Persisted composer-settings storage -----------------------------------
// The bm_run_* / bm_draft_* / bm_last_used_* localStorage namespace is owned by
// this module. Other composables (useChatSession) persist through these helpers
// instead of writing the raw keys, so the key scheme has a single owner.

const COMPOSER_SETTING_SUFFIXES = [
  'selected_model',
  'reasoning_effort',
  'selected_preset',
  'current_mode',
  'bypass_permissions'
] as const;

export interface PersistedComposerSettings {
  selectedModel: string;
  reasoningEffort: string;
  selectedPreset: string;
  mode: string;
  bypassPermissions: boolean;
}

function settingValues(s: PersistedComposerSettings): Record<(typeof COMPOSER_SETTING_SUFFIXES)[number], string> {
  return {
    selected_model: s.selectedModel,
    reasoning_effort: s.reasoningEffort,
    selected_preset: s.selectedPreset,
    current_mode: s.mode,
    bypass_permissions: String(s.bypassPermissions)
  };
}

/** Persists the "last sent with" settings used as defaults for fresh sessions. */
export function saveLastUsedComposerSettings(s: PersistedComposerSettings) {
  const values = settingValues(s);
  for (const suffix of COMPOSER_SETTING_SUFFIXES) {
    localStorage.setItem(`bm_last_used_${suffix}`, values[suffix]);
  }
}

/** Persists the settings snapshot tied to a specific run. */
export function saveRunComposerSettings(runId: string, s: PersistedComposerSettings) {
  const values = settingValues(s);
  for (const suffix of COMPOSER_SETTING_SUFFIXES) {
    localStorage.setItem(`bm_run_${runId}_${suffix}`, values[suffix]);
  }
}

/** Clears the draft (no-active-run) settings, e.g. once a draft becomes a run. */
export function clearDraftComposerSettings() {
  for (const suffix of COMPOSER_SETTING_SUFFIXES) {
    localStorage.removeItem(`bm_draft_${suffix}`);
  }
}

/**
 * Owns the composer's persisted settings: selected model, operational mode
 * and the bypass-permissions toggle, plus derived model option lists.
 */
export function useComposerSettings(
  providers: Ref<ProviderMetadata[]>,
  agentPresets: Ref<AgentPreset[]>,
  activeRunId: Ref<string | null>,
  activeRun: Ref<Run | null>
) {
  const selectedModelCombined = ref('');
  const selectedReasoningEffort = ref('default');
  const selectedPresetId = ref('');
  const currentMode = ref<ChatMode>('accept_edits');
  const bypassPermissions = ref(false);

  let isLoadingSettings = false;

  function saveSetting(keySuffix: string, value: string) {
    if (isLoadingSettings) return;
    const runId = activeRunId.value;
    if (runId) {
      localStorage.setItem(`bm_run_${runId}_${keySuffix}`, value);
    } else {
      localStorage.setItem(`bm_draft_${keySuffix}`, value);
    }
  }

  watch(selectedModelCombined, (v) => { if (v) saveSetting('selected_model', v); });
  watch(selectedReasoningEffort, (v) => saveSetting('reasoning_effort', v || 'default'));
  watch(selectedPresetId, (v) => saveSetting('selected_preset', v));
  watch(currentMode, (v) => saveSetting('current_mode', v));
  watch(bypassPermissions, (v) => saveSetting('bypass_permissions', String(v)));

  function sanitizeReasoning(val: string | null): string {
    return val && REASONING_EFFORT_IDS.has(val as any) ? val : 'default';
  }

  // Both the activeRunId and activeRun watchers call this, and every status
  // update replaces the activeRun object — dedupe so a run switch loads once
  // instead of twice and status churn doesn't re-read localStorage.
  let lastSettingsLoadKey: string | null = null;

  function settingsLoadKey(runId: string | null): string {
    const run = runId && activeRun.value?.id === runId ? activeRun.value : null;
    return [
      runId ?? '',
      run?.providerId ?? '',
      run?.model ?? '',
      run?.reasoningEffort ?? '',
      run?.agentPreset ?? '',
      run?.mode ?? ''
    ].join('|');
  }

  function loadSettingsForRun(runId: string | null) {
    const loadKey = settingsLoadKey(runId);
    if (loadKey === lastSettingsLoadKey) return;
    lastSettingsLoadKey = loadKey;

    isLoadingSettings = true;
    try {
      if (runId) {
        const storedModel = localStorage.getItem(`bm_run_${runId}_selected_model`);
        const storedReasoning = localStorage.getItem(`bm_run_${runId}_reasoning_effort`);
        const storedPreset = localStorage.getItem(`bm_run_${runId}_selected_preset`);
        const storedMode = localStorage.getItem(`bm_run_${runId}_current_mode`);
        const storedBypass = localStorage.getItem(`bm_run_${runId}_bypass_permissions`);

        const runObj = activeRun.value;

        let modelVal = '';
        if (storedModel !== null) {
          modelVal = storedModel;
        } else if (runObj && runObj.providerId && runObj.model) {
          modelVal = `${runObj.providerId}:${runObj.model}`;
        } else {
          modelVal = localStorage.getItem('bm_last_used_selected_model') || localStorage.getItem('bm_selected_model') || '';
        }

        let reasoningVal = 'default';
        if (storedReasoning !== null) {
          reasoningVal = sanitizeReasoning(storedReasoning);
        } else if (runObj && runObj.reasoningEffort) {
          reasoningVal = sanitizeReasoning(runObj.reasoningEffort);
        } else {
          reasoningVal = sanitizeReasoning(localStorage.getItem('bm_last_used_reasoning_effort'));
        }

        let presetVal = '';
        if (storedPreset !== null) {
          presetVal = storedPreset;
        } else if (runObj && runObj.agentPreset) {
          presetVal = runObj.agentPreset;
        } else {
          presetVal = localStorage.getItem('bm_last_used_selected_preset') || '';
        }

        let modeVal: ChatMode = 'accept_edits';
        if (storedMode !== null) {
          modeVal = storedMode as ChatMode;
        } else if (runObj && runObj.mode) {
          modeVal = runObj.mode as ChatMode;
        } else {
          // Fresh start (no message ever sent): default to Build. Otherwise reuse
          // the mode the user last SENT a message with (set in handleSendTask).
          modeVal = (localStorage.getItem('bm_last_used_current_mode') || 'accept_edits') as ChatMode;
        }

        let bypassVal = false;
        if (storedBypass !== null) {
          bypassVal = storedBypass === 'true';
        } else {
          bypassVal = localStorage.getItem('bm_last_used_bypass_permissions') === 'true';
        }

        selectedModelCombined.value = modelVal;
        selectedReasoningEffort.value = reasoningVal;
        selectedPresetId.value = presetVal;
        currentMode.value = modeVal;
        bypassPermissions.value = bypassVal;
      } else {
        const storedModel = localStorage.getItem('bm_draft_selected_model');
        const storedReasoning = localStorage.getItem('bm_draft_reasoning_effort');
        const storedPreset = localStorage.getItem('bm_draft_selected_preset');
        const storedMode = localStorage.getItem('bm_draft_current_mode');
        const storedBypass = localStorage.getItem('bm_draft_bypass_permissions');

        const lastUsedModel = localStorage.getItem('bm_last_used_selected_model') || localStorage.getItem('bm_selected_model') || '';
        const lastUsedReasoning = localStorage.getItem('bm_last_used_reasoning_effort') || 'default';
        const lastUsedPreset = localStorage.getItem('bm_last_used_selected_preset') || '';
        const lastUsedMode = localStorage.getItem('bm_last_used_current_mode') || 'accept_edits';
        const lastUsedBypass = localStorage.getItem('bm_last_used_bypass_permissions') === 'true';

        selectedModelCombined.value = storedModel !== null ? storedModel : lastUsedModel;
        selectedReasoningEffort.value = sanitizeReasoning(storedReasoning !== null ? storedReasoning : lastUsedReasoning);
        selectedPresetId.value = storedPreset !== null ? storedPreset : lastUsedPreset;
        currentMode.value = (storedMode !== null ? storedMode : lastUsedMode) as ChatMode;
        bypassPermissions.value = storedBypass !== null ? (storedBypass === 'true') : lastUsedBypass;
      }
    } finally {
      nextTick(() => {
        isLoadingSettings = false;
      });
    }
  }

  watch(activeRunId, (newId) => {
    loadSettingsForRun(newId);
  }, { immediate: true });

  watch(activeRun, (newRun) => {
    if (newRun && activeRunId.value === newRun.id) {
      loadSettingsForRun(newRun.id);
    }
  });

  const modelOptions = computed<ModelOption[]>(() => {
    const options: ModelOption[] = [];
    for (const provider of providers.value) {
      for (const model of provider.models) {
        options.push({
          value: `${provider.id}:${model}`,
          label: `${provider.displayName} / ${model}`,
          providerId: provider.id,
          model,
          reasoningOptions: normalizedReasoningOptions(provider.modelSettings?.[model]),
          contextLimit: provider.modelSettings?.[model]?.contextLimit
        });
      }
    }
    return options;
  });

  const activeModelDisplayName = computed(() => {
    const opt = modelOptions.value.find(o => o.value === selectedModelCombined.value);
    if (!opt) return 'Select Model';
    return opt.label.split(' / ').pop() || opt.label;
  });

  // The currently selected dual-model preset, or null for single-model mode.
  const activePreset = computed(() =>
    agentPresets.value.find(p => p.id === selectedPresetId.value) ?? null
  );

  watch(agentPresets, () => {
    if (selectedPresetId.value && !activePreset.value) {
      selectedPresetId.value = '';
    }
  });

  // The main/architect model a run should use: the preset's architect when a
  // preset is active, otherwise the plain single-model selection.
  const effectiveModel = computed<{ providerId: string; model: string }>(() => {
    if (activePreset.value) {
      return { ...activePreset.value.architect };
    }
    return splitCombined(selectedModelCombined.value);
  });

  const activeReasoningOptions = computed<ReasoningOption[]>(() => {
    const effective = effectiveModel.value;
    const provider = providers.value.find(p => p.id === effective.providerId);
    return normalizedReasoningOptions(provider?.modelSettings?.[effective.model]);
  });

  const reasoningEffortOptions = computed(() => [
    REASONING_EFFORTS[0],
    ...activeReasoningOptions.value.map(option => ({
      id: option.id,
      label: option.label || REASONING_EFFORTS.find(x => x.id === option.id)?.label || option.id
    }))
  ]);

  watch(activeReasoningOptions, (options) => {
    if (selectedReasoningEffort.value !== 'default' && !options.some(option => option.id === selectedReasoningEffort.value)) {
      selectedReasoningEffort.value = 'default';
    }
  });

  // Dual-model fields to attach to a run; empty object in single-model mode.
  const agentRunFields = computed<{ coderProviderId?: string; coderModel?: string; coderReasoningEffort?: string; utilityProviderId?: string; utilityModel?: string; utilityReasoningEffort?: string; agentPreset?: string }>(() => {
    if (!activePreset.value) return { agentPreset: '' };
    return {
      agentPreset: activePreset.value.id,
      coderProviderId: activePreset.value.coder.providerId,
      coderModel: activePreset.value.coder.model,
      coderReasoningEffort: activePreset.value.coder.reasoningEffort,
      utilityProviderId: activePreset.value.utility?.providerId,
      utilityModel: activePreset.value.utility?.model,
      utilityReasoningEffort: activePreset.value.utility?.reasoningEffort
    };
  });

  const effectiveReasoningEffort = computed(() =>
    activePreset.value?.architect.reasoningEffort || selectedReasoningEffort.value
  );

  function getModeLabel(modeId: string): string {
    return MODES_LIST.find(m => m.id === modeId)?.label ?? 'Chat';
  }

  /** Picks a sensible default model (preferring Anthropic) once providers load. */
  function ensureDefaultModel() {
    if (modelOptions.value.length === 0) return;
    if (modelOptions.value.some(o => o.value === selectedModelCombined.value)) return;
    selectedModelCombined.value =
      modelOptions.value.find(o => o.providerId === 'anthropic')?.value || modelOptions.value[0].value;
  }

  watch(modelOptions, () => ensureDefaultModel());

  return {
    selectedModelCombined,
    selectedReasoningEffort,
    reasoningEffortOptions,
    selectedPresetId,
    activePreset,
    effectiveModel,
    effectiveReasoningEffort,
    agentRunFields,
    currentMode,
    bypassPermissions,
    modelOptions,
    activeModelDisplayName,
    getModeLabel,
    ensureDefaultModel
  };
}

function normalizedReasoningOptions(settings: ProviderModelSettings | undefined): ReasoningOption[] {
  if (!settings) return [];
  if (settings.reasoning?.options?.length) return settings.reasoning.options;
  if (settings.options?.length) return settings.options;
  return (settings.reasoningEfforts || []).map(id => ({ id }));
}
