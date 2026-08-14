<script setup lang="ts">
import { ref, computed, watch } from 'vue';
import type { ProviderModelSettings, ReasoningEffort, ReasoningOption, ReasoningStyle, PriceTier } from '@locagens/shared';
import { api } from '../../api/client';
import { useCustomDialog } from '../../composables/useCustomDialog';
import ThemedSelect from '../ui/ThemedSelect.vue';
import ThemedButton from '../ui/ThemedButton.vue';
import Spinner from '../ui/Spinner.vue';

const props = defineProps<{
  providersConfig: Record<string, any>;
  isLoading: boolean;
}>();

const emit = defineEmits<{
  (e: 'saved'): void;
}>();

const { showAlert, showConfirm } = useCustomDialog();

const PRESERVE_API_KEY_VALUE = '__LOCAGENS_PRESERVE_API_KEY__';

const configs = ref<Record<string, any>>({});
watch(() => props.providersConfig, (newVal) => {
  configs.value = { ...newVal };
}, { immediate: true });

const isEditing = ref(false);
const editingProviderId = ref<string | null>(null);

// Form fields
const formId = ref('');
const formType = ref<'openai-compatible' | 'anthropic'>('openai-compatible');
const formDisplayName = ref('');
const formBaseUrl = ref('');
const formApiKey = ref('');
const formModelRows = ref<ModelRow[]>([]);
const providerTypeOptions = [
  { value: 'openai-compatible', label: 'OpenAI Compatible' },
  { value: 'anthropic', label: 'Anthropic' }
];

const reasoningEffortChoices: { id: ReasoningEffort; label: string }[] = [
  { id: 'none', label: 'None' },
  { id: 'minimal', label: 'Minimal' },
  { id: 'low', label: 'Low' },
  { id: 'medium', label: 'Medium' },
  { id: 'high', label: 'High' },
  { id: 'xhigh', label: 'XHigh' },
  { id: 'max', label: 'Max' }
];

const reasoningStyleChoices: { id: ReasoningStyle; label: string }[] = [
  { id: 'openai-chat', label: 'Reasoning Effort' },
  { id: 'anthropic-budget', label: 'Thinking Budget' }
];

type PriceTierRow = {
  upToInputTokens: number | null;
  inputRate: number | null;
  outputRate: number | null;
  cacheReadRate: number | null;
  cacheWriteRate: number | null;
};

type ModelRow = {
  name: string;
  reasoningStyle: ReasoningStyle;
  reasoningOptions: ReasoningOption[];
  contextLimit: number | null;
  temperature: number | null;
  pricingTiers: PriceTierRow[];
};

function defaultReasoningStyle(): ReasoningStyle {
  return formType.value === 'anthropic' ? 'anthropic-budget' : 'openai-chat';
}

function emptyModelRow(name = ''): ModelRow {
  return { name, reasoningStyle: defaultReasoningStyle(), reasoningOptions: [], contextLimit: null, temperature: null, pricingTiers: [] };
}

function emptyPriceTierRow(): PriceTierRow {
  return { upToInputTokens: null, inputRate: null, outputRate: null, cacheReadRate: null, cacheWriteRate: null };
}

function addPricingTier(row: ModelRow) {
  row.pricingTiers.push(emptyPriceTierRow());
}

function removePricingTier(row: ModelRow, index: number) {
  row.pricingTiers.splice(index, 1);
}



function handleAddProvider() {
  editingProviderId.value = null;
  formId.value = '';
  formType.value = 'openai-compatible';
  formDisplayName.value = '';
  formBaseUrl.value = '';
  formApiKey.value = '';
  formModelRows.value = [emptyModelRow()];
  isEditing.value = true;
}

function handleEditProvider(id: string) {
  const block = configs.value[id];
  if (!block) return;
  
  editingProviderId.value = id;
  formId.value = id;
  formType.value = block.type;
  formDisplayName.value = block.displayName;
  formBaseUrl.value = block.baseUrl;
  formApiKey.value = block.apiKey === PRESERVE_API_KEY_VALUE ? '' : block.apiKey || '';
  formModelRows.value = block.models && block.models.length > 0
    ? block.models.map((model: string) => modelRowFromSettings(model, block.modelSettings?.[model]))
    : [emptyModelRow()];
  isEditing.value = true;
}

function addModelInput() {
  formModelRows.value.push(emptyModelRow());
}

function removeModelInput(index: number) {
  formModelRows.value.splice(index, 1);
}

function toggleReasoningEffort(index: number, effort: ReasoningEffort) {
  const row = formModelRows.value[index];
  if (!row) return;
  row.reasoningOptions = row.reasoningOptions.some(x => x.id === effort)
    ? row.reasoningOptions.filter(x => x.id !== effort)
    : [...row.reasoningOptions, defaultReasoningOption(row.reasoningStyle, effort)];
}

function setReasoningStyle(row: ModelRow, style: ReasoningStyle) {
  row.reasoningStyle = style;
  row.reasoningOptions = row.reasoningOptions.map(option => ({
    ...option,
    ...(style === 'anthropic-budget'
      ? { budgetTokens: option.budgetTokens || defaultBudgetTokens(option.id) }
      : { budgetTokens: undefined })
  }));
}

function isReasoningEffortSelected(row: ModelRow, effort: ReasoningEffort): boolean {
  return row.reasoningOptions.some(option => option.id === effort);
}

function modelRowFromSettings(name: string, settings?: ProviderModelSettings): ModelRow {
  const reasoning = settings?.reasoning || settings;
  const style = reasoning?.style || defaultReasoningStyle();
  const options = reasoning?.options?.length
    ? reasoning.options.map(option => ({ ...option }))
    : (reasoning?.reasoningEfforts || []).map(id => defaultReasoningOption(style, id));
  const pricingTiers = (settings?.pricing?.tiers || []).map(tier => ({
    upToInputTokens: tier.upToInputTokens ?? null,
    inputRate: tier.inputRate ?? null,
    outputRate: tier.outputRate ?? null,
    cacheReadRate: tier.cacheReadRate ?? null,
    cacheWriteRate: tier.cacheWriteRate ?? null
  }));
  return { name, reasoningStyle: style, reasoningOptions: options, contextLimit: settings?.contextLimit ?? null, temperature: settings?.temperature ?? null, pricingTiers };
}

function defaultReasoningOption(style: ReasoningStyle, id: ReasoningEffort): ReasoningOption {
  return {
    id,
    label: reasoningEffortChoices.find(effort => effort.id === id)?.label || id,
    value: id,
    ...(style === 'anthropic-budget' ? { budgetTokens: defaultBudgetTokens(id) } : {})
  };
}

function defaultBudgetTokens(id: ReasoningEffort): number {
  if (id === 'low' || id === 'minimal') return 1024;
  if (id === 'medium') return 4096;
  if (id === 'high') return 10000;
  if (id === 'xhigh') return 32000;
  if (id === 'max') return 64000;
  return 1024;
}

async function handleDeleteProvider() {
  if (!editingProviderId.value) return;
  if (!(await showConfirm(`Are you sure you want to delete the provider "${editingProviderId.value}"?`))) return;

  const newConfigs = { ...configs.value };
  delete newConfigs[editingProviderId.value];

  try {
    await api.saveProvidersConfig(newConfigs);
    configs.value = newConfigs;
    isEditing.value = false;
    emit('saved');
  } catch (err: any) {
    await showAlert(err.message || 'An error occurred.');
  }
}

async function handleSave() {
  const idSlug = formId.value.trim().toLowerCase().replace(/[^a-z0-9-_]/g, '');
  if (!idSlug) {
    await showAlert('Provider ID is required (only lowercase letters, numbers, hyphens, and underscores).');
    return;
  }
  if (!formDisplayName.value.trim()) {
    await showAlert('Display Name is required.');
    return;
  }
  if (!formBaseUrl.value.trim()) {
    await showAlert('Base URL is required.');
    return;
  }

  const modelRows = formModelRows.value
    .map(row => ({
      name: row.name.trim(),
      reasoningStyle: row.reasoningStyle,
      reasoningOptions: row.reasoningOptions,
      contextLimit: row.contextLimit,
      temperature: row.temperature,
      pricingTiers: row.pricingTiers
    }))
    .filter(row => row.name);
  const modelsArray = modelRows
    .map(row => row.name)
    .filter((model, index, arr) => arr.indexOf(model) === index);
  const modelSettings = Object.fromEntries(modelRows
    .map(row => {
      const settings: Record<string, unknown> = {};
      if (row.reasoningOptions.length > 0) {
        settings.reasoning = { style: row.reasoningStyle, options: row.reasoningOptions };
      }
      if (row.contextLimit && row.contextLimit > 0) {
        settings.contextLimit = row.contextLimit;
      }
      if (row.temperature !== null && row.temperature >= 0 && row.temperature <= 2) {
        settings.temperature = row.temperature;
      }
      const tiers = row.pricingTiers
        .filter(tier => tier.inputRate != null && tier.outputRate != null)
        .map(tier => {
          const clean: PriceTier = { inputRate: Number(tier.inputRate), outputRate: Number(tier.outputRate) };
          if (tier.upToInputTokens != null && tier.upToInputTokens > 0) clean.upToInputTokens = Number(tier.upToInputTokens);
          if (tier.cacheReadRate != null) clean.cacheReadRate = Number(tier.cacheReadRate);
          if (tier.cacheWriteRate != null) clean.cacheWriteRate = Number(tier.cacheWriteRate);
          return clean;
        });
      if (tiers.length > 0) settings.pricing = { tiers };
      return [row.name, settings] as const;
    })
    .filter(([, settings]) => Object.keys(settings).length > 0));

  const currentBlock = editingProviderId.value ? configs.value[editingProviderId.value] : null;
  const trimmedApiKey = formApiKey.value.trim();
  const updatedBlock = {
    type: formType.value,
    displayName: formDisplayName.value.trim(),
    baseUrl: formBaseUrl.value.trim(),
    apiKey: trimmedApiKey || (currentBlock?.hasApiKey ? PRESERVE_API_KEY_VALUE : ''),
    models: modelsArray,
    modelSettings
  };

  const newConfigs = { ...configs.value };
  
  if (editingProviderId.value && editingProviderId.value !== idSlug) {
    delete newConfigs[editingProviderId.value];
  }
  
  newConfigs[idSlug] = updatedBlock;

  try {
    await api.saveProvidersConfig(newConfigs);
    configs.value = newConfigs;
    isEditing.value = false;
    emit('saved');
  } catch (err: any) {
    await showAlert(err.message || 'An error occurred.');
  }
}

const canFetchModels = computed(() => {
  if (formType.value === 'anthropic') return false;
  const idLower = formId.value.trim().toLowerCase();
  if (idLower === 'openai') return false;
  const urlLower = formBaseUrl.value.trim().toLowerCase();
  if (urlLower.includes('api.openai.com')) return false;
  return true;
});

const isFetchingModels = ref(false);

async function handleFetchModels() {
  if (!formBaseUrl.value.trim()) {
    await showAlert('Please enter a Base URL first to fetch models.');
    return;
  }

  isFetchingModels.value = true;
  try {
    const res = await api.fetchModels({
      type: formType.value,
      baseUrl: formBaseUrl.value.trim(),
      apiKey: formApiKey.value.trim(),
      providerId: editingProviderId.value || undefined
    });

    if (res.success && res.models && res.models.length > 0) {
      formModelRows.value = res.models.map(model => emptyModelRow(model));
    } else if (res.error) {
      await showAlert(`Failed to fetch models: ${res.error}`);
    } else {
      await showAlert('No models returned from provider.');
    }
  } catch (err: any) {
    await showAlert(`Error fetching models: ${err.message || err}`);
  } finally {
    isFetchingModels.value = false;
  }
}
</script>

<template>
  <div class="settings-tab-panel">
    <header class="settings-section-head">
      <div class="header-info">
        <h3 class="settings-section-title">Providers</h3>
        <p class="settings-section-desc">
          Configure model providers and API keys. Secrets are stored in macOS Keychain when available.
        </p>
      </div>
      <ThemedButton 
        v-if="!isEditing" 
        variant="primary" 
        size="sm"
        @click="handleAddProvider"
      >
        Add Provider
      </ThemedButton>
    </header>

    <div v-if="isLoading" class="settings-empty">
      <Spinner :size="24" style="color: var(--btn-primary-bg)" />
      <span>Loading configurations...</span>
    </div>

    <div v-else-if="isEditing" class="edit-form-card">
      <h4 class="form-title">
        {{ editingProviderId ? 'Edit Provider: ' + formDisplayName : 'Add New Provider' }}
      </h4>

      <div class="form-grid">
        <div class="form-group">
          <label>Provider ID <span class="label-desc">(slug, e.g. custom-ollama)</span></label>
          <input 
            v-model="formId" 
            type="text" 
            placeholder="e.g. custom-ollama" 
            :disabled="!!editingProviderId"
          />
        </div>

        <div class="form-group">
          <label>Display Name</label>
          <input v-model="formDisplayName" type="text" placeholder="e.g. Local Ollama" />
        </div>

        <div class="form-group">
          <label>Provider Type</label>
          <ThemedSelect v-model="formType" :options="providerTypeOptions" />
        </div>

        <div class="form-group url-group">
          <label>Base URL</label>
          <input v-model="formBaseUrl" type="text" placeholder="e.g. http://localhost:11434/v1" />
        </div>

        <div class="form-group key-group">
          <label>API Key</label>
          <input v-model="formApiKey" type="password" :placeholder="editingProviderId && configs[editingProviderId]?.hasApiKey ? 'Leave blank to keep existing key' : 'API Key or secret token'" />
        </div>

        <div class="form-group full-width models-section">
          <div class="models-header-row">
            <label class="section-label">Available Models</label>
            <span class="models-count-badge">{{ formModelRows.length }} {{ formModelRows.length === 1 ? 'model' : 'models' }}</span>
          </div>
          
          <div class="models-edit-list">
            <div v-for="(row, index) in formModelRows" :key="index" class="model-card">
              
              <!-- Model Card Header -->
              <div class="model-card-header">
                <div class="model-card-title-input">
                  <input 
                    v-model="row.name" 
                    type="text" 
                    placeholder="e.g. gpt-4o or deepseek/deepseek-chat" 
                    class="model-name-field"
                  />
                </div>
                <button
                  type="button" 
                  class="delete-model-btn-modern" 
                  @click="removeModelInput(index)"
                  title="Remove model"
                >
                  <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="3 6 5 6 21 6"></polyline>
                    <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path>
                  </svg>
                </button>
              </div>

              <!-- Model Card Body -->
              <div class="model-card-body">
                
                <!-- Parameters Subgrid -->
                <div class="model-settings-grid">
                  <div class="setting-group">
                    <label>Context limit (tokens)</label>
                    <input v-model.number="row.contextLimit" type="number" min="0" step="1000" placeholder="e.g. 128000" />
                  </div>
                  <div class="setting-group">
                    <label>Temperature (0-2)</label>
                    <input v-model.number="row.temperature" type="number" min="0" max="2" step="0.1" placeholder="provider default" />
                  </div>
                </div>

                <!-- Reasoning Settings -->
                <div class="reasoning-settings-block">
                  <div class="reasoning-block-header">Reasoning Configuration</div>
                  
                  <div class="reasoning-row-field">
                    <span class="field-sub-label">Style:</span>
                    <div class="reasoning-style-toggle">
                      <ThemedButton
                        v-for="style in reasoningStyleChoices"
                        :key="style.id"
                        variant="chip"
                        size="sm"
                        :active="row.reasoningStyle === style.id"
                        @click="setReasoningStyle(row, style.id)"
                      >
                        {{ style.label }}
                      </ThemedButton>
                    </div>
                  </div>

                  <div class="reasoning-row-field">
                    <span class="field-sub-label">Efforts:</span>
                    <div class="reasoning-effort-checks">
                      <ThemedButton
                        v-for="effort in reasoningEffortChoices"
                        :key="effort.id"
                        variant="chip"
                        size="sm"
                        :active="isReasoningEffortSelected(row, effort.id)"
                        @click="toggleReasoningEffort(index, effort.id)"
                      >
                        {{ effort.label }}
                      </ThemedButton>
                    </div>
                  </div>

                  <!-- Budgets (if anthropic-budget) -->
                  <div v-if="row.reasoningStyle === 'anthropic-budget' && row.reasoningOptions.length" class="budget-inputs-block">
                    <div class="budget-title">Thinking Budgets (tokens)</div>
                    <div class="budget-grid">
                      <label v-for="option in row.reasoningOptions" :key="option.id" class="budget-input-item">
                        <span class="budget-label">{{ option.label || option.id }}</span>
                        <input v-model.number="option.budgetTokens" type="number" min="1024" step="1024" />
                      </label>
                    </div>
                  </div>
                </div>

                <!-- Pricing Section -->
                <div class="pricing-editor-block">
                  <div class="pricing-block-header">
                    <span class="pricing-label">Pricing Tiers (USD per 1M tokens)</span>
                    <span class="pricing-hint">Add tiers based on token size threshold. Leave the last threshold's value empty.</span>
                  </div>

                  <div class="pricing-tiers-wrapper">
                    <!-- Column Headers -->
                    <div v-if="row.pricingTiers.length" class="pricing-tier-row headers">
                      <span class="tier-col">Prompt Threshold (≤)</span>
                      <span class="tier-col">Input Rate</span>
                      <span class="tier-col">Output Rate</span>
                      <span class="tier-col">Cache Read</span>
                      <span class="tier-col">Cache Write</span>
                      <span class="tier-col actions"></span>
                    </div>

                    <!-- Tier Rows -->
                    <div v-for="(tier, tIndex) in row.pricingTiers" :key="tIndex" class="pricing-tier-row inputs">
                      <div class="tier-col">
                        <input v-model.number="tier.upToInputTokens" type="number" min="0" step="1000" placeholder="∞ (last)" />
                      </div>
                      <div class="tier-col">
                        <input v-model.number="tier.inputRate" type="number" min="0" step="0.01" placeholder="0.00" />
                      </div>
                      <div class="tier-col">
                        <input v-model.number="tier.outputRate" type="number" min="0" step="0.01" placeholder="0.00" />
                      </div>
                      <div class="tier-col">
                        <input v-model.number="tier.cacheReadRate" type="number" min="0" step="0.01" placeholder="—" />
                      </div>
                      <div class="tier-col">
                        <input v-model.number="tier.cacheWriteRate" type="number" min="0" step="0.01" placeholder="—" />
                      </div>
                      <div class="tier-col actions">
                        <button type="button" class="delete-tier-btn" @click="removePricingTier(row, tIndex)" title="Remove tier">
                          <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <line x1="18" y1="6" x2="6" y2="18"></line>
                            <line x1="6" y1="6" x2="18" y2="18"></line>
                          </svg>
                        </button>
                      </div>
                    </div>

                    <div v-if="!row.pricingTiers.length" class="no-pricing-notice">
                      No pricing tiers defined. Standard rates will not be calculated.
                    </div>
                  </div>

                  <div class="pricing-actions">
                    <ThemedButton variant="secondary" size="sm" @click="addPricingTier(row)">
                      {{ row.pricingTiers.length ? 'Add Pricing Tier' : 'Add Pricing' }}
                    </ThemedButton>
                  </div>
                </div>

              </div>
            </div>

            <!-- Add/Fetch actions -->
            <div class="models-actions-row">
              <ThemedButton 
                variant="secondary"
                size="sm"
                @click="addModelInput"
              >
                Add Model
              </ThemedButton>
              <ThemedButton 
                v-if="canFetchModels"
                variant="secondary"
                size="sm"
                :disabled="isFetchingModels"
                @click="handleFetchModels"
                class="fetch-models-btn"
              >
                {{ isFetchingModels ? 'Fetching...' : 'Fetch Models from API' }}
              </ThemedButton>
            </div>
          </div>
        </div>
      </div>

      <div class="form-actions">
        <ThemedButton v-if="editingProviderId" variant="danger" class="delete-btn" @click="handleDeleteProvider">
          Delete Provider
        </ThemedButton>
        <ThemedButton variant="secondary" @click="isEditing = false">Cancel</ThemedButton>
        <ThemedButton variant="primary" @click="handleSave">Save Configuration</ThemedButton>
      </div>
    </div>

    <div v-else-if="Object.keys(configs).length === 0" class="settings-empty">
      No providers configured. Click "Add Provider" to create one.
    </div>

    <div v-else class="provider-plain-list">
      <div v-for="(provider, id) in configs" :key="id" class="provider-plain-row">
        <span class="provider-plain-text">
          {{ provider.displayName }} <span class="provider-plain-meta">({{ provider.type }}, ID: {{ id }})</span>
        </span>
        <ThemedButton 
          variant="secondary"
          size="sm"
          @click="handleEditProvider(id)"
        >
          Edit
        </ThemedButton>
      </div>
    </div>
  </div>
</template>

<style scoped>
.settings-section-head {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  margin-bottom: 24px;
  gap: 16px;
}

.header-info {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.settings-section-title {
  font-size: 1.25rem;
  font-weight: 600;
  color: var(--text);
  margin: 0;
}

.settings-section-desc {
  font-size: 0.85rem;
  color: var(--muted);
  margin: 0;
  line-height: 1.4;
}

.plus-icon {
  font-size: 1.1rem;
  margin-right: 2px;
  font-weight: bold;
}

.settings-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 40px 20px;
  background: var(--surface);
  border: 1px dashed var(--border);
  border-radius: 12px;
  color: var(--muted);
  gap: 12px;
  font-size: 0.9rem;
}

/* Provider List View Mode */
.provider-plain-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
  border-top: 1px solid var(--border-soft);
  padding-top: 12px;
}

.provider-plain-row {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 8px 0;
  border-bottom: 1px solid var(--border-soft);
}

.provider-plain-text {
  font-size: 0.88rem;
  color: var(--muted);
}

.provider-plain-meta {
  font-size: 0.78rem;
  color: var(--faint);
  margin-left: 4px;
}






/* Edit form styling */
.edit-form-card {
  background: var(--card-bg);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 24px;
  margin-bottom: 24px;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.3);
  display: flex;
  flex-direction: column;
  gap: 20px;
}

.form-title {
  margin: 0;
  font-size: 1.1rem;
  font-weight: 600;
  color: var(--text);
  border-bottom: 1px solid var(--border-soft);
  padding-bottom: 12px;
}

.form-grid {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 16px;
}

.form-group {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.form-group.url-group {
  grid-column: span 2;
}

.form-group.key-group {
  grid-column: span 2;
}

.form-group.full-width {
  grid-column: span 2;
}

.form-group label {
  font-size: 0.78rem;
  font-weight: 600;
  color: var(--muted);
  display: flex;
  align-items: center;
  gap: 4px;
}

.label-desc {
  font-weight: normal;
  color: var(--faint);
}

.form-group input {
  background: var(--control-bg);
  border: 1px solid var(--control-border);
  border-radius: 8px;
  padding: 10px 14px;
  color: var(--text);
  outline: none;
  font-size: 0.88rem;
  transition: all 0.2s ease;
}

.form-group input:focus {
  border-color: var(--control-border-focus);
  background: var(--control-bg-hover);
  box-shadow: 0 0 0 2px rgba(255, 255, 255, 0.03);
}

.form-group input:disabled {
  background: rgba(255, 255, 255, 0.02);
  color: var(--faint);
  border-color: var(--border-soft);
  cursor: not-allowed;
}

.form-actions {
  display: flex;
  justify-content: flex-end;
  gap: 12px;
  border-top: 1px solid var(--border-soft);
  padding-top: 20px;
  margin-top: 10px;
}

.form-actions .delete-btn {
  margin-right: auto;
}

/* Models Edit List Section */
.models-section {
  margin-top: 8px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.models-header-row {
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.section-label {
  font-size: 0.85rem;
  font-weight: 600;
  color: var(--text);
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.models-count-badge {
  font-size: 0.72rem;
  font-weight: 600;
  color: var(--muted);
  background: rgba(255, 255, 255, 0.05);
  padding: 2px 8px;
  border-radius: 10px;
}

.models-edit-list {
  display: flex;
  flex-direction: column;
  gap: 16px;
  background: rgba(0, 0, 0, 0.12);
  border: 1px solid var(--border-soft);
  border-radius: 10px;
  padding: 16px;
}

.models-actions-row {
  display: flex;
  gap: 10px;
  margin-top: 4px;
}

.fetch-models-btn {
  color: var(--success);
  border-color: var(--success-border) !important;
}

.fetch-models-btn:hover:not(:disabled) {
  background: var(--success-soft) !important;
}

/* Model Card Editor */
.model-card {
  background: var(--surface-elevated);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 16px;
  display: flex;
  flex-direction: column;
  gap: 16px;
  position: relative;
  transition: all 0.2s cubic-bezier(0.4, 0, 0.2, 1);
}

.model-card:hover {
  border-color: var(--control-border-focus);
}

.model-card-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  border-bottom: 1px solid var(--border-soft);
  padding-bottom: 12px;
}

.model-card-title-input {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 1;
}

.model-icon {
  font-size: 1.1rem;
  opacity: 0.8;
}

.model-name-field {
  background: transparent !important;
  border: none !important;
  border-bottom: 1.5px solid var(--border) !important;
  border-radius: 0 !important;
  padding: 4px 0 !important;
  font-family: monospace;
  font-size: 0.95rem;
  font-weight: 600;
  color: var(--text);
  outline: none !important;
  width: 100%;
  max-width: 320px;
  transition: border-color 0.2s ease !important;
}

.model-name-field:focus {
  border-color: var(--btn-primary-bg) !important;
  box-shadow: none !important;
}

.delete-model-btn-modern {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border-radius: 8px;
  background: transparent;
  border: 1px solid var(--border-soft);
  color: var(--muted);
  cursor: pointer;
  transition: all 0.2s ease;
}

.delete-model-btn-modern:hover {
  color: var(--danger);
  background: var(--danger-soft);
  border-color: var(--danger-border);
  transform: scale(1.05);
}

.model-card-body {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.model-settings-grid {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 16px;
}

.setting-group {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.setting-group label {
  font-size: 0.72rem;
  font-weight: 600;
  color: var(--muted);
  text-transform: uppercase;
  letter-spacing: 0.3px;
}

.setting-group input {
  background: var(--control-bg);
  border: 1px solid var(--control-border);
  border-radius: 8px;
  padding: 8px 12px;
  color: var(--text);
  outline: none;
  font-size: 0.82rem;
}

.setting-group input:focus {
  border-color: var(--control-border-focus);
}

/* Reasoning configuration card section */
.reasoning-settings-block {
  background: rgba(0, 0, 0, 0.15);
  border: 1px solid var(--border-soft);
  border-radius: 8px;
  padding: 12px 14px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.reasoning-block-header {
  font-size: 0.75rem;
  font-weight: 600;
  color: var(--muted);
  text-transform: uppercase;
  letter-spacing: 0.3px;
  border-bottom: 1px solid var(--border-soft);
  padding-bottom: 6px;
  margin-bottom: 2px;
}

.reasoning-row-field {
  display: flex;
  align-items: center;
  gap: 10px;
}

.field-sub-label {
  font-size: 0.75rem;
  color: var(--faint);
  width: 50px;
  flex-shrink: 0;
}

.reasoning-style-toggle {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}

.reasoning-effort-checks {
  display: flex;
  gap: 4px;
  flex-wrap: wrap;
}

.budget-inputs-block {
  border-top: 1px dashed var(--border-soft);
  padding-top: 10px;
  margin-top: 4px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.budget-title {
  font-size: 0.72rem;
  font-weight: 600;
  color: var(--faint);
}

.budget-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
  gap: 8px;
}

.budget-input-item {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.budget-label {
  font-size: 0.68rem;
  color: var(--muted);
  text-transform: capitalize;
}

.budget-input-item input {
  background: var(--control-bg);
  border: 1px solid var(--control-border);
  border-radius: 6px;
  padding: 6px 10px;
  color: var(--text);
  outline: none;
  font-size: 0.8rem;
  font-family: monospace;
}

.budget-input-item input:focus {
  border-color: var(--control-border-focus);
}

/* Pricing Grid Editor */
.pricing-editor-block {
  border-top: 1px solid var(--border-soft);
  padding-top: 14px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.pricing-block-header {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.pricing-label {
  font-size: 0.75rem;
  font-weight: 600;
  color: var(--muted);
  text-transform: uppercase;
  letter-spacing: 0.3px;
}

.pricing-hint {
  font-size: 0.68rem;
  color: var(--faint);
  line-height: 1.3;
}

.pricing-tiers-wrapper {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.pricing-tier-row {
  display: grid;
  grid-template-columns: repeat(5, 1fr) 32px;
  gap: 8px;
  align-items: center;
}

.pricing-tier-row.headers {
  font-size: 0.68rem;
  font-weight: 600;
  color: var(--faint);
  padding-bottom: 2px;
  border-bottom: 1px solid var(--border-soft);
  text-align: center;
}

.pricing-tier-row.headers .tier-col {
  text-align: left;
}

.pricing-tier-row.headers .tier-col:not(:first-child) {
  text-align: center;
}

.tier-col input {
  width: 100%;
  background: var(--control-bg);
  border: 1px solid var(--control-border);
  border-radius: 6px;
  padding: 6px 10px;
  color: var(--text);
  outline: none;
  font-size: 0.8rem;
  font-family: monospace;
  text-align: center;
  transition: all 0.2s ease;
}

.tier-col input:first-child {
  text-align: left;
}

.tier-col input:focus {
  border-color: var(--control-border-focus);
  background: var(--control-bg-hover);
}

.pricing-tier-row.inputs .tier-col.actions {
  display: flex;
  justify-content: center;
}

.delete-tier-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border-radius: 6px;
  border: 1px solid var(--border-soft);
  background: transparent;
  color: var(--muted);
  cursor: pointer;
  transition: all 0.2s ease;
}

.delete-tier-btn:hover {
  color: var(--danger);
  background: var(--danger-soft);
  border-color: var(--danger-border);
}

.no-pricing-notice {
  padding: 16px;
  text-align: center;
  font-size: 0.78rem;
  color: var(--faint);
  border: 1px dashed var(--border-soft);
  border-radius: 8px;
  background: rgba(0, 0, 0, 0.05);
}

.pricing-actions {
  display: flex;
  justify-content: flex-start;
  margin-top: 4px;
}
</style>
