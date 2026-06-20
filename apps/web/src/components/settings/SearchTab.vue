<script setup lang="ts">
import { ref, watch } from 'vue';
import type { AppSettings, SearchEngine } from '@locagens/shared';
import { api } from '../../api/client';
import { useCustomDialog } from '../../composables/useCustomDialog';
import ThemedSelect from './ThemedSelect.vue';
import ThemedButton from '../ThemedButton.vue';

const props = defineProps<{
  settings: AppSettings | null;
  isLoading: boolean;
}>();

const emit = defineEmits<{
  (e: 'saved'): void;
}>();

const { showAlert, showConfirm } = useCustomDialog();

const PRESERVE_API_KEY_VALUE = '__LOCAGENS_PRESERVE_API_KEY__';

const ENGINE_OPTIONS: { value: SearchEngine; label: string }[] = [
  { value: 'duckduckgo', label: 'DuckDuckGo (free, no key)' },
  { value: 'brave', label: 'Brave Search API' },
  { value: 'google', label: 'Google Custom Search' },
  { value: 'disabled', label: 'Disabled' }
] as const;

const engine = ref<SearchEngine>('duckduckgo');
const braveApiKey = ref('');
const hasBraveApiKey = ref(false);
const googleApiKey = ref('');
const hasGoogleApiKey = ref(false);
const googleSearchEngineId = ref('');
const isSaving = ref(false);

watch(() => props.settings, (next) => {
  if (!next) return;
  engine.value = next.search?.engine ?? 'duckduckgo';
  braveApiKey.value = '';
  googleApiKey.value = '';
  hasBraveApiKey.value = next.search?.hasBraveApiKey ?? false;
  hasGoogleApiKey.value = next.search?.hasGoogleApiKey ?? false;
  googleSearchEngineId.value = next.search?.googleSearchEngineId ?? '';
}, { immediate: true });

function keyPlaceholder(hasKey: boolean): string {
  return hasKey ? 'Leave blank to keep existing key' : 'Paste API key here';
}

function apiKeyPayload(inputValue: string, hasStoredKey: boolean): string {
  const trimmed = inputValue.trim();
  if (trimmed) return trimmed;
  return hasStoredKey ? PRESERVE_API_KEY_VALUE : '';
}

async function handleSave() {
  isSaving.value = true;
  try {
    const payload: AppSettings = {
      port: props.settings?.port ?? 4321,
      search: {
        engine: engine.value,
        braveApiKey: apiKeyPayload(braveApiKey.value, hasBraveApiKey.value),
        googleApiKey: apiKeyPayload(googleApiKey.value, hasGoogleApiKey.value),
        googleSearchEngineId: googleSearchEngineId.value.trim() || undefined
      }
    };
    await api.saveSettings(payload);
    emit('saved');
  } catch (err: any) {
    await showAlert(err.message || 'Failed to save search settings.');
  } finally {
    isSaving.value = false;
  }
}

async function clearStoredKey(clear: () => void) {
  if (!(await showConfirm('Clear the stored API key?'))) return;
  clear();
  await handleSave();
}

function clearBraveKey() {
  clearStoredKey(() => {
    braveApiKey.value = '';
    hasBraveApiKey.value = false;
  });
}

function clearGoogleKey() {
  clearStoredKey(() => {
    googleApiKey.value = '';
    hasGoogleApiKey.value = false;
  });
}
</script>

<template>
  <div class="settings-tab-panel">
    <header class="settings-section-head">
      <div class="header-info">
        <h3 class="settings-section-title">Search</h3>
        <p class="settings-section-desc">
          Configure the web search engine used by the <code>search_web</code> tool when a model needs current information from the internet.
        </p>
      </div>
      <ThemedButton variant="primary" size="sm" :disabled="isSaving" @click="handleSave">
        {{ isSaving ? 'Saving...' : 'Save' }}
      </ThemedButton>
    </header>

    <div v-if="isLoading" class="settings-empty">
      <div class="spinner-spinner"></div>
      <span>Loading settings...</span>
    </div>

    <div v-else class="edit-form-card">
      <div class="form-grid">
        <div class="form-group full-width">
          <label>Search Engine</label>
          <ThemedSelect v-model="engine" :options="ENGINE_OPTIONS" />
          <p class="field-hint">
            DuckDuckGo works without an API key. Brave and Google require free API keys.
          </p>
        </div>

        <template v-if="engine === 'brave'">
          <div class="form-group full-width">
            <label>Brave Search API Key</label>
            <div class="key-input-wrap">
              <input
                v-model="braveApiKey"
                type="password"
                :placeholder="keyPlaceholder(hasBraveApiKey)"
              />
              <ThemedButton
                v-if="hasBraveApiKey"
                variant="danger"
                size="sm"
                @click="clearBraveKey"
              >
                Clear
              </ThemedButton>
            </div>
            <p class="field-hint">
              Get a free key at <a href="https://api.search.brave.com" target="_blank" rel="noopener">api.search.brave.com</a>.
              The key is stored in macOS Keychain and never saved in project files.
            </p>
          </div>
        </template>

        <template v-if="engine === 'google'">
          <div class="form-group full-width">
            <label>Google API Key</label>
            <div class="key-input-wrap">
              <input
                v-model="googleApiKey"
                type="password"
                :placeholder="keyPlaceholder(hasGoogleApiKey)"
              />
              <ThemedButton
                v-if="hasGoogleApiKey"
                variant="danger"
                size="sm"
                @click="clearGoogleKey"
              >
                Clear
              </ThemedButton>
            </div>
            <p class="field-hint">
              Create a key in the <a href="https://console.cloud.google.com/apis/credentials" target="_blank" rel="noopener">Google Cloud Console</a> and enable Custom Search API.
            </p>
          </div>

          <div class="form-group full-width">
            <label>Google Search Engine ID</label>
            <input
              v-model="googleSearchEngineId"
              type="text"
              placeholder="e.g. 012345678901234567890:abc123def"
            />
            <p class="field-hint">
              Create a Programmable Search Engine at <a href="https://programmablesearchengine.google.com" target="_blank" rel="noopener">programmablesearchengine.google.com</a>.
            </p>
          </div>
        </template>
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

.settings-section-desc code {
  font-family: monospace;
  background: var(--surface-strong);
  padding: 1px 5px;
  border-radius: 4px;
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

.spinner-spinner {
  width: 24px;
  height: 24px;
  border: 2px solid rgba(255, 255, 255, 0.1);
  border-top-color: var(--btn-primary-bg);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}

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

.form-group.full-width {
  grid-column: span 2;
}

.form-group label {
  font-size: 0.78rem;
  font-weight: 600;
  color: var(--muted);
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
  width: 100%;
}

.form-group input:focus {
  border-color: var(--control-border-focus);
  background: var(--control-bg-hover);
  box-shadow: 0 0 0 2px rgba(255, 255, 255, 0.03);
}

.key-input-wrap {
  display: flex;
  gap: 10px;
  align-items: center;
}

.key-input-wrap input {
  flex: 1;
}

.field-hint {
  font-size: 0.75rem;
  color: var(--faint);
  margin: 4px 0 0;
  line-height: 1.4;
}

.field-hint a {
  color: var(--btn-primary-bg);
  text-decoration: none;
}

.field-hint a:hover {
  text-decoration: underline;
}

@media (max-width: 600px) {
  .form-grid {
    grid-template-columns: 1fr;
  }

  .form-group.full-width {
    grid-column: span 1;
  }
}
</style>
