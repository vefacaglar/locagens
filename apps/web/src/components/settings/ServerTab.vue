<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { api, API_BASE } from '../../api/client';
import ThemedButton from '../ui/ThemedButton.vue';

const port = ref<number | null>(null);
const initialPort = ref<number | null>(null);
const isLoading = ref(true);
const isSaving = ref(false);
const error = ref('');
const savedRestartRequired = ref(false);

async function load() {
  isLoading.value = true;
  error.value = '';
  const settings = await api.getSettings();
  if (settings) {
    port.value = settings.port;
    initialPort.value = settings.port;
  } else {
    error.value = 'Could not load server settings.';
  }
  isLoading.value = false;
}

function isValidPort(value: number | null): value is number {
  return value != null && Number.isInteger(value) && value >= 1 && value <= 65535;
}

async function save() {
  if (!isValidPort(port.value)) {
    error.value = 'Enter a port between 1 and 65535.';
    return;
  }
  isSaving.value = true;
  error.value = '';
  savedRestartRequired.value = false;
  try {
    const result = await api.saveSettings({ port: port.value });
    const changed = result.port !== initialPort.value;
    port.value = result.port;
    initialPort.value = result.port;
    // In the desktop app the main process can restart the backend on the new
    // port and reload the window; in a plain browser the user restarts manually.
    const desktop = (window as any).__LOCAGENS_DESKTOP__;
    if (changed && desktop?.restartBackend) {
      await desktop.restartBackend();
      return; // window reloads
    }
    savedRestartRequired.value = changed;
  } catch (err: any) {
    error.value = err?.message || 'Failed to save settings.';
  } finally {
    isSaving.value = false;
  }
}

onMounted(load);
</script>

<template>
  <div class="settings-tab-panel">
    <header class="settings-section-head">
      <div>
        <h3 class="settings-section-title">Server</h3>
        <p class="settings-section-desc">
          The port the local backend listens on. Stored in
          <code>settings.json</code> and read on startup. Changing it takes
          effect after the app restarts.
        </p>
      </div>
    </header>

    <div v-if="isLoading" class="settings-empty">Loading…</div>

    <template v-else>
      <div class="server-field">
        <label class="server-label" for="server-port">Backend port</label>
        <input
          id="server-port"
          v-model.number="port"
          class="server-input"
          type="number"
          min="1"
          max="65535"
          inputmode="numeric"
          @keydown.enter="save"
        />
        <p class="server-hint">Currently connected to <code>{{ API_BASE }}</code></p>
      </div>

      <p v-if="error" class="server-msg server-msg-error">{{ error }}</p>
      <p v-else-if="savedRestartRequired" class="server-msg server-msg-ok">
        Saved. Restart the app for the new port to take effect.
      </p>

      <div class="server-actions">
        <ThemedButton variant="primary" size="md" :disabled="isSaving" @click="save">
          {{ isSaving ? 'Saving…' : 'Save' }}
        </ThemedButton>
      </div>
    </template>
  </div>
</template>

<style scoped>
.server-field {
  display: flex;
  flex-direction: column;
  gap: 8px;
  max-width: 280px;
}

.server-label {
  font-size: 0.82rem;
  font-weight: 500;
  color: var(--muted);
}

.server-input {
  width: 100%;
  padding: 10px 12px;
  font-size: 0.9rem;
  color: var(--text);
  background: var(--surface-strong);
  border: 1px solid var(--settings-card-border);
  border-radius: 8px;
  outline: none;
  transition: border-color 0.15s ease;
}

.server-input:focus {
  border-color: var(--muted);
}

.server-hint {
  margin: 0;
  font-size: 0.78rem;
  color: var(--faint);
}

.server-hint code,
.settings-section-desc code {
  font-family: monospace;
}

.server-msg {
  margin: 16px 0 0;
  font-size: 0.82rem;
}

.server-msg-error {
  color: var(--danger, #e5534b);
}

.server-msg-ok {
  color: var(--muted);
}

.server-actions {
  margin-top: 20px;
}
</style>
