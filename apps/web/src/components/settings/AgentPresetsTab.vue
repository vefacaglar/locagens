<script setup lang="ts">
import { ref, computed, watch } from 'vue';
import type { AgentPreset, ProviderMetadata, ReasoningEffort, ReasoningOption } from '@locagens/shared';
import { api } from '../../api/client';
import { splitCombined } from '../../lib/format';
import { useCustomDialog } from '../../composables/useCustomDialog';
import ThemedSelect from '../ui/ThemedSelect.vue';
import ThemedButton from '../ui/ThemedButton.vue';

const props = defineProps<{
  providers: ProviderMetadata[];
  presets: AgentPreset[];
}>();

const emit = defineEmits<{
  (e: 'saved'): void;
}>();

const { showAlert, showConfirm } = useCustomDialog();

const presets = ref<AgentPreset[]>([]);
watch(() => props.presets, (newVal) => {
  presets.value = [...newVal];
}, { immediate: true });
const isEditing = ref(false);
const editingOriginalId = ref<string | null>(null);

// Form fields. Architect/coder are stored as a combined "providerId:model" value.
const formId = ref('');
const formDisplayName = ref('');
const formArchitect = ref('');
const formArchitectEffort = ref('');
const formCoder = ref('');
const formCoderEffort = ref('');
// Empty string => no utility tier (the "None" option).
const formUtility = ref('');
const formUtilityEffort = ref('');
const formMaxSubAgents = ref(3);
const formFallback = ref(false);

// Flattened provider/model options for the architect + coder dropdowns.
const modelOptions = computed(() => {
  const out: { value: string; label: string }[] = [];
  for (const p of props.providers) {
    for (const m of p.models) {
      out.push({ value: `${p.id}:${m}`, label: `${p.displayName} / ${m}` });
    }
  }
  return out;
});
const utilityModelOptions = computed(() => [
  { value: '', label: 'None' },
  ...modelOptions.value
]);
const maxSubAgentOptions = [
  { value: 1, label: '1' },
  { value: 2, label: '2' },
  { value: 3, label: '3' }
];

function combined(providerId: string, model: string): string {
  return `${providerId}:${model}`;
}

function normalizedReasoningOptions(value: string): ReasoningOption[] {
  if (!value) return [];
  const { providerId, model } = splitCombined(value);
  const provider = props.providers.find(p => p.id === providerId);
  const settings = provider?.modelSettings?.[model];
  const reasoning = settings?.reasoning ?? settings;
  const rawOptions = reasoning?.options?.length
    ? reasoning.options
    : reasoning?.reasoningEfforts?.map(id => ({ id }));
  return (rawOptions ?? []).filter((option): option is ReasoningOption => !!option?.id && option.id !== 'default');
}

function reasoningOptionsFor(value: string): { id: string; label: string }[] {
  const options = normalizedReasoningOptions(value);
  if (options.length === 0) return [];
  return [
    { id: '', label: 'Default' },
    ...options.map(option => ({ id: option.id, label: option.label ?? effortLabel(option.id) }))
  ];
}

function effortLabel(effort: ReasoningEffort | string | undefined): string {
  switch (effort) {
    case 'none': return 'None';
    case 'minimal': return 'Minimal';
    case 'low': return 'Low';
    case 'medium': return 'Medium';
    case 'high': return 'High';
    case 'xhigh': return 'XHigh';
    case 'max': return 'Max';
    default: return 'Default';
  }
}

function safeEffort(value: string, combinedValue: string): ReasoningEffort | undefined {
  if (!value) return undefined;
  return normalizedReasoningOptions(combinedValue).some(option => option.id === value) ? value as ReasoningEffort : undefined;
}

const architectReasoningOptions = computed(() => reasoningOptionsFor(formArchitect.value));
const coderReasoningOptions = computed(() => reasoningOptionsFor(formCoder.value));
const utilityReasoningOptions = computed(() => reasoningOptionsFor(formUtility.value));

watch(formArchitect, () => {
  if (!safeEffort(formArchitectEffort.value, formArchitect.value)) formArchitectEffort.value = '';
});

watch(formCoder, () => {
  if (!safeEffort(formCoderEffort.value, formCoder.value)) formCoderEffort.value = '';
});

watch(formUtility, () => {
  if (!safeEffort(formUtilityEffort.value, formUtility.value)) formUtilityEffort.value = '';
});



function handleAdd() {
  editingOriginalId.value = null;
  formId.value = '';
  formDisplayName.value = '';
  formArchitect.value = modelOptions.value[0]?.value ?? '';
  formArchitectEffort.value = '';
  formCoder.value = modelOptions.value[0]?.value ?? '';
  formCoderEffort.value = '';
  formUtility.value = '';
  formUtilityEffort.value = '';
  formMaxSubAgents.value = 3;
  formFallback.value = false;
  isEditing.value = true;
}

function handleEdit(preset: AgentPreset) {
  editingOriginalId.value = preset.id;
  formId.value = preset.id;
  formDisplayName.value = preset.displayName;
  formArchitect.value = combined(preset.architect.providerId, preset.architect.model);
  formArchitectEffort.value = preset.architect.reasoningEffort ?? '';
  formCoder.value = combined(preset.coder.providerId, preset.coder.model);
  formCoderEffort.value = preset.coder.reasoningEffort ?? '';
  formUtility.value = preset.utility ? combined(preset.utility.providerId, preset.utility.model) : '';
  formUtilityEffort.value = preset.utility?.reasoningEffort ?? '';
  formMaxSubAgents.value = preset.maxSubAgents;
  formFallback.value = !!preset.fallback;
  isEditing.value = true;
}

function cancelEdit() {
  isEditing.value = false;
  editingOriginalId.value = null;
}

/** Serializes the current preset list (plus the edited one) into the save shape. */
function toBlocks(list: AgentPreset[]): Record<string, any> {
  const blocks: Record<string, any> = {};
  for (const p of list) {
    const architect = {
      providerId: p.architect.providerId,
      model: p.architect.model,
      ...(p.architect.reasoningEffort ? { reasoningEffort: p.architect.reasoningEffort } : {})
    };
    const coder = {
      providerId: p.coder.providerId,
      model: p.coder.model,
      ...(p.coder.reasoningEffort ? { reasoningEffort: p.coder.reasoningEffort } : {})
    };
    const utility = p.utility
      ? {
          providerId: p.utility.providerId,
          model: p.utility.model,
          ...(p.utility.reasoningEffort ? { reasoningEffort: p.utility.reasoningEffort } : {})
        }
      : undefined;
    blocks[p.id] = {
      displayName: p.displayName,
      architect,
      coder,
      maxSubAgents: p.maxSubAgents,
      ...(utility ? { utility } : {}),
      ...(p.fallback ? { fallback: true } : {})
    };
  }
  return blocks;
}

async function persist(list: AgentPreset[]) {
  await api.saveAgentPresets(toBlocks(list));
  emit('saved');
}

async function handleSave() {
  const id = formId.value.trim();
  if (!id) return showAlert('Please enter an id for the preset (e.g. "opusplan").');
  if (!/^[a-zA-Z0-9_-]+$/.test(id)) return showAlert('Preset id may only contain letters, numbers, "-" and "_".');
  if (!formArchitect.value || !formCoder.value) return showAlert('Please choose both an architect and a coder model.');

  // Prevent duplicate ids (unless editing the same one).
  if (id !== editingOriginalId.value && presets.value.some(p => p.id === id)) {
    return showAlert(`A preset with id "${id}" already exists.`);
  }

  const architect = splitCombined(formArchitect.value);
  const architectReasoningEffort = safeEffort(formArchitectEffort.value, formArchitect.value);
  const coder = splitCombined(formCoder.value);
  const coderReasoningEffort = safeEffort(formCoderEffort.value, formCoder.value);
  const utility = formUtility.value ? splitCombined(formUtility.value) : undefined;
  const utilityReasoningEffort = safeEffort(formUtilityEffort.value, formUtility.value);
  const next: AgentPreset = {
    id,
    displayName: formDisplayName.value.trim() || id,
    architect: { ...architect, ...(architectReasoningEffort ? { reasoningEffort: architectReasoningEffort } : {}) },
    coder: { ...coder, ...(coderReasoningEffort ? { reasoningEffort: coderReasoningEffort } : {}) },
    maxSubAgents: Math.min(3, Math.max(1, Number(formMaxSubAgents.value) || 1)),
    utility: utility ? { ...utility, ...(utilityReasoningEffort ? { reasoningEffort: utilityReasoningEffort } : {}) } : undefined,
    fallback: formFallback.value
  };

  // Replace the edited preset (by original id) or append a new one.
  const list = presets.value.filter(p => p.id !== editingOriginalId.value && p.id !== id);
  list.push(next);

  try {
    await persist(list);
    isEditing.value = false;
    editingOriginalId.value = null;
  } catch (err: any) {
    showAlert(err.message || 'Failed to save preset.');
  }
}

async function handleDelete(preset: AgentPreset) {
  const ok = await showConfirm(`Delete the "${preset.displayName}" agent preset?`);
  if (!ok) return;
  try {
    await persist(presets.value.filter(p => p.id !== preset.id));
  } catch (err: any) {
    showAlert(err.message || 'Failed to delete preset.');
  }
}
</script>

<template>
  <div class="settings-tab-panel">
    <div class="settings-section-head">
      <div>
        <h2 class="settings-section-title">Agent presets</h2>
        <p class="settings-section-desc">
          Dual-model pairings (like <code>opusplan</code>): an <strong>architect</strong> model
          plans and delegates code-writing to a <strong>coder</strong> model running as 1–3
          sub-agents, with an optional cheap <strong>utility</strong> model for tiny lookups
          and renames. Selectable next to the model picker in the composer.
        </p>
      </div>
      <ThemedButton v-if="!isEditing" variant="primary" size="sm" @click="handleAdd">Add preset</ThemedButton>
    </div>

    <!-- Editor -->
    <div v-if="isEditing" class="preset-editor">
      <div class="form-row">
        <label>Id</label>
        <input v-model="formId" class="text-input" placeholder="opusplan" :disabled="!!editingOriginalId" />
      </div>
      <div class="form-row">
        <label>Display name</label>
        <input v-model="formDisplayName" class="text-input" placeholder="Opus Plan" />
      </div>
      <div class="form-row">
        <label>Architect model</label>
        <ThemedSelect v-model="formArchitect" :options="modelOptions" />
        <div v-if="architectReasoningOptions.length > 0" class="effort-row">
          <span class="effort-label">Reasoning</span>
          <ThemedButton
            v-for="option in architectReasoningOptions"
            :key="`architect-${option.id}`"
            variant="chip"
            size="sm"
            :active="formArchitectEffort === option.id"
            @click="formArchitectEffort = option.id"
          >
            {{ option.label }}
          </ThemedButton>
        </div>
      </div>
      <div class="form-row">
        <label>Coder model</label>
        <ThemedSelect v-model="formCoder" :options="modelOptions" />
        <div v-if="coderReasoningOptions.length > 0" class="effort-row">
          <span class="effort-label">Reasoning</span>
          <ThemedButton
            v-for="option in coderReasoningOptions"
            :key="`coder-${option.id}`"
            variant="chip"
            size="sm"
            :active="formCoderEffort === option.id"
            @click="formCoderEffort = option.id"
          >
            {{ option.label }}
          </ThemedButton>
        </div>
      </div>
      <div class="form-row">
        <label>Utility model <span class="form-hint">(optional — cheap lookups & renames)</span></label>
        <ThemedSelect v-model="formUtility" :options="utilityModelOptions" />
        <div v-if="utilityReasoningOptions.length > 0" class="effort-row">
          <span class="effort-label">Reasoning</span>
          <ThemedButton
            v-for="option in utilityReasoningOptions"
            :key="`utility-${option.id}`"
            variant="chip"
            size="sm"
            :active="formUtilityEffort === option.id"
            @click="formUtilityEffort = option.id"
          >
            {{ option.label }}
          </ThemedButton>
        </div>
      </div>
      <div class="form-row">
        <label>Max sub-agents</label>
        <ThemedSelect v-model="formMaxSubAgents" :options="maxSubAgentOptions" />
      </div>
      <label class="form-toggle">
        <input type="checkbox" v-model="formFallback" />
        <span>
          Fallback to architect
          <span class="form-hint">
            — if a delegated task fails on the utility/coder model, escalate
            (utility → coder → architect) and let the architect finish it directly.
          </span>
        </span>
      </label>
      <div class="editor-actions">
        <ThemedButton variant="secondary" @click="cancelEdit">Cancel</ThemedButton>
        <ThemedButton variant="primary" @click="handleSave">Save preset</ThemedButton>
      </div>
    </div>

    <!-- List -->
    <div v-else>
      <p v-if="presets.length === 0" class="settings-empty">No agent presets yet.</p>
      <div v-for="preset in presets" :key="preset.id" class="preset-row">
        <div class="preset-info">
          <span class="preset-name">{{ preset.displayName }}</span>
          <span class="preset-flow">
            {{ preset.architect.model }}<template v-if="preset.architect.reasoningEffort"> ({{ effortLabel(preset.architect.reasoningEffort) }})</template> → {{ preset.coder.model }}<template v-if="preset.coder.reasoningEffort"> ({{ effortLabel(preset.coder.reasoningEffort) }})</template><template v-if="preset.utility"> · utility: {{ preset.utility.model }}<template v-if="preset.utility.reasoningEffort"> ({{ effortLabel(preset.utility.reasoningEffort) }})</template></template> · up to {{ preset.maxSubAgents }} sub-agent{{ preset.maxSubAgents === 1 ? '' : 's' }}<template v-if="preset.fallback"> · fallback on</template>
          </span>
        </div>
        <div class="preset-actions">
          <ThemedButton variant="secondary" size="sm" @click="handleEdit(preset)">Edit</ThemedButton>
          <ThemedButton variant="danger" size="sm" @click="handleDelete(preset)">Delete</ThemedButton>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.preset-editor {
  display: flex;
  flex-direction: column;
  gap: 14px;
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 18px;
  background: var(--surface);
}

.form-row {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.form-row label {
  font-size: 0.78rem;
  color: var(--muted);
}

.form-hint {
  color: var(--faint);
  font-weight: 400;
}

.form-toggle {
  display: flex;
  align-items: flex-start;
  gap: 9px;
  font-size: 0.82rem;
  color: var(--text);
  cursor: pointer;
}

.form-toggle input {
  margin-top: 2px;
  flex: 0 0 auto;
  accent-color: var(--control-accent);
}

.form-row input.text-input {
  padding: 9px 11px;
  font-size: 0.85rem;
}

.effort-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  margin-top: 2px;
}

.effort-label {
  margin-right: 2px;
  font-size: 0.74rem;
  color: var(--faint);
}

.editor-actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  margin-top: 4px;
}

.preset-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 14px 16px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 12px;
  margin-bottom: 10px;
}

.preset-info {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.preset-name {
  font-size: 0.92rem;
  font-weight: 600;
  color: var(--text);
}

.preset-flow {
  font-size: 0.76rem;
  color: var(--faint);
  font-family: monospace;
}

.preset-actions {
  display: flex;
  gap: 8px;
  flex: 0 0 auto;
}
</style>
