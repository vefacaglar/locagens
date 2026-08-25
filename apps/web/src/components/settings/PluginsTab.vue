<script setup lang="ts">
import { computed, ref } from 'vue';
import type {
  PluginManifest,
  PluginTemplate,
  InstallPluginPayload,
  PluginScope
} from '@locagens/shared';
import ThemedButton from '../ui/ThemedButton.vue';
import ThemedSelect from '../ui/ThemedSelect.vue';

const props = defineProps<{
  plugins: PluginManifest[];
  templates: PluginTemplate[];
  userPluginsDir: string;
  projectPluginsDir: string | null;
  activeProjectPath: string;
  activeProjectName: string;
  isLoading: boolean;
  isInstalling: boolean;
  error?: string | null;
  statusMessage?: string | null;
}>();

const emit = defineEmits<{
  (e: 'refresh'): void;
  (e: 'install', payload: InstallPluginPayload): void;
  (e: 'toggle', payload: { id: string; enabled: boolean; scope: PluginScope }): void;
  (e: 'delete', payload: { id: string; scope: PluginScope }): void;
}>();

const canInstallProject = computed(() => !!props.activeProjectPath);
const userPlugins = computed(() => props.plugins.filter(p => p.scope === 'user'));
const projectPlugins = computed(() => props.plugins.filter(p => p.scope === 'project'));

const showInstallModal = ref(false);
const installSource = ref<'template' | 'github' | 'npm' | 'custom'>('template');
const selectedTemplateId = ref<string>('context-mode');
const installUri = ref('');
const installScope = ref<PluginScope>('user');
const customManifestText = ref('');
const expandedPlugins = ref<Set<string>>(new Set());

function toggleExpand(id: string) {
  if (expandedPlugins.value.has(id)) {
    expandedPlugins.value.delete(id);
  } else {
    expandedPlugins.value.add(id);
  }
}

function openInstall(templateId?: string) {
  if (templateId) {
    installSource.value = 'template';
    selectedTemplateId.value = templateId;
  }
  showInstallModal.value = true;
}

function closeInstall() {
  showInstallModal.value = false;
  installUri.value = '';
  customManifestText.value = '';
}

function submitInstall() {
  let uri = '';
  let customManifest: any = undefined;

  if (installSource.value === 'template') {
    uri = selectedTemplateId.value;
  } else if (installSource.value === 'github' || installSource.value === 'npm') {
    uri = installUri.value.trim();
    if (!uri) return;
  } else if (installSource.value === 'custom') {
    try {
      customManifest = JSON.parse(customManifestText.value);
      uri = customManifest.id || 'custom-plugin';
    } catch {
      alert('Invalid JSON manifest.');
      return;
    }
  }

  emit('install', {
    source: installSource.value === 'custom' ? 'local' : installSource.value,
    uri,
    scope: installScope.value,
    projectPath: installScope.value === 'project' ? props.activeProjectPath : undefined,
    customManifest
  });

  closeInstall();
}

function quickInstallTemplate(templateId: string) {
  emit('install', {
    source: 'template',
    uri: templateId,
    scope: 'user'
  });
}
</script>

<template>
  <div class="settings-tab-panel">
    <!-- Header -->
    <header class="settings-section-head">
      <div>
        <h3 class="settings-section-title">Plugins & Lifecycle Hooks</h3>
        <p class="settings-section-desc">
          Extend Locagens with tools, MCP servers, and lifecycle hooks (e.g. context-mode token optimization, pre/post tool interceptors, custom commands).
        </p>
      </div>
      <div class="plugin-head-actions">
        <ThemedButton
          variant="primary"
          size="sm"
          :disabled="isInstalling"
          @click="openInstall()"
        >
          + Install Plugin
        </ThemedButton>
        <ThemedButton
          variant="secondary"
          size="sm"
          :disabled="isLoading || isInstalling"
          @click="emit('refresh')"
        >
          Refresh
        </ThemedButton>
      </div>
    </header>

    <!-- Feedback Banners -->
    <p v-if="error" class="skills-banner skills-banner-error">{{ error }}</p>
    <p v-else-if="statusMessage" class="skills-banner">{{ statusMessage }}</p>

    <!-- Quick Templates Bar -->
    <div class="plugin-presets-bar">
      <span class="plugin-presets-label">Featured Plugins:</span>
      <div class="plugin-presets-list">
        <button
          v-for="t in templates"
          :key="t.id"
          type="button"
          class="plugin-preset-pill"
          :title="t.description"
          @click="quickInstallTemplate(t.id)"
        >
          <span class="preset-name">+ {{ t.name }}</span>
        </button>
      </div>
    </div>

    <!-- Install Modal / Dialog -->
    <div v-if="showInstallModal" class="plugin-modal-backdrop">
      <div class="plugin-modal">
        <div class="plugin-modal-header">
          <h4 class="plugin-modal-title">Install Plugin</h4>
          <button class="plugin-modal-close" @click="closeInstall">✕</button>
        </div>

        <div class="plugin-modal-body">
          <!-- Install Source Tabs -->
          <div class="plugin-source-tabs">
            <button
              type="button"
              class="plugin-source-tab"
              :class="{ active: installSource === 'template' }"
              @click="installSource = 'template'"
            >
              Template
            </button>
            <button
              type="button"
              class="plugin-source-tab"
              :class="{ active: installSource === 'github' }"
              @click="installSource = 'github'"
            >
              GitHub Repo
            </button>
            <button
              type="button"
              class="plugin-source-tab"
              :class="{ active: installSource === 'npm' }"
              @click="installSource = 'npm'"
            >
              NPM Package
            </button>
            <button
              type="button"
              class="plugin-source-tab"
              :class="{ active: installSource === 'custom' }"
              @click="installSource = 'custom'"
            >
              Custom JSON
            </button>
          </div>

          <!-- Template Selection -->
          <div v-if="installSource === 'template'" class="plugin-form-group">
            <label class="plugin-label">Choose Template</label>
            <ThemedSelect
              v-model="selectedTemplateId"
              :options="templates.map(t => ({ value: t.id, label: `${t.name} (${t.author || 'builtin'})` }))"
            />
            <p v-if="templates.find(t => t.id === selectedTemplateId)" class="plugin-help-text">
              {{ templates.find(t => t.id === selectedTemplateId)?.description }}
            </p>
          </div>

          <!-- GitHub Input -->
          <div v-else-if="installSource === 'github'" class="plugin-form-group">
            <label class="plugin-label">GitHub Repository</label>
            <input
              v-model="installUri"
              type="text"
              class="plugin-input"
              placeholder="e.g. mksglu/context-mode"
            />
            <p class="plugin-help-text">Enter owner/repo or full GitHub URL.</p>
          </div>

          <!-- NPM Input -->
          <div v-else-if="installSource === 'npm'" class="plugin-form-group">
            <label class="plugin-label">NPM Package Name</label>
            <input
              v-model="installUri"
              type="text"
              class="plugin-input"
              placeholder="e.g. context-mode"
            />
            <p class="plugin-help-text">Package will be executed via npx / stdio.</p>
          </div>

          <!-- Custom JSON Input -->
          <div v-else-if="installSource === 'custom'" class="plugin-form-group">
            <label class="plugin-label">Plugin Manifest (JSON)</label>
            <textarea
              v-model="customManifestText"
              class="plugin-textarea"
              rows="6"
              placeholder='{ "id": "my-plugin", "name": "My Plugin", "mcpServers": { ... }, "hooks": { ... } }'
            ></textarea>
          </div>

          <!-- Scope Selection -->
          <div class="plugin-form-group">
            <label class="plugin-label">Installation Scope</label>
            <div class="plugin-scope-options">
              <label class="plugin-radio-label">
                <input
                  v-model="installScope"
                  type="radio"
                  value="user"
                />
                <span>User (Global — all projects)</span>
              </label>
              <label class="plugin-radio-label" :class="{ disabled: !canInstallProject }">
                <input
                  v-model="installScope"
                  type="radio"
                  value="project"
                  :disabled="!canInstallProject"
                />
                <span>Project (Active Project only{{ !canInstallProject ? ' — Select project first' : '' }})</span>
              </label>
            </div>
          </div>
        </div>

        <div class="plugin-modal-footer">
          <ThemedButton variant="secondary" size="sm" @click="closeInstall">Cancel</ThemedButton>
          <ThemedButton variant="primary" size="sm" :disabled="isInstalling" @click="submitInstall">
            {{ isInstalling ? 'Installing...' : 'Install Plugin' }}
          </ThemedButton>
        </div>
      </div>
    </div>

    <!-- Plugins Content -->
    <div class="plugin-list-wrap">
      <!-- Empty State -->
      <div v-if="plugins.length === 0" class="plugin-empty-state">
        <p class="plugin-empty-title">No plugins installed yet</p>
        <p class="plugin-empty-desc">
          Install featured plugins like <strong>Context Mode</strong> to optimize token usage or add custom MCP tools.
        </p>
        <ThemedButton
          variant="primary"
          size="sm"
          class="plugin-empty-btn"
          @click="openInstall('context-mode')"
        >
          Install Context Mode
        </ThemedButton>
      </div>

      <!-- Plugins List -->
      <div v-else class="plugin-cards">
        <div
          v-for="p in plugins"
          :key="`${p.scope}-${p.id}`"
          class="plugin-card"
          :class="{ disabled: !p.enabled }"
        >
          <div class="plugin-card-header">
            <div class="plugin-card-title-group">
              <div class="plugin-title-row">
                <h4 class="plugin-card-name">{{ p.name }}</h4>
                <span class="plugin-version-badge">v{{ p.version }}</span>
                <span
                  class="plugin-scope-badge"
                  :class="p.scope === 'project' ? 'badge-project' : 'badge-user'"
                >
                  {{ p.scope === 'project' ? 'Project' : 'User' }}
                </span>
              </div>
              <p class="plugin-card-desc">{{ p.description }}</p>
            </div>

            <!-- Toggle Switch -->
            <div class="plugin-toggle-wrap">
              <label class="plugin-switch" :title="p.enabled ? 'Disable plugin' : 'Enable plugin'">
                <input
                  type="checkbox"
                  :checked="p.enabled"
                  @change="emit('toggle', { id: p.id, enabled: !p.enabled, scope: p.scope })"
                />
                <span class="plugin-slider"></span>
              </label>
            </div>
          </div>

          <!-- Feature Tags -->
          <div class="plugin-tags-row">
            <span v-if="p.mcpServers && Object.keys(p.mcpServers).length > 0" class="plugin-feature-tag tag-mcp">
              MCP Servers ({{ Object.keys(p.mcpServers).length }})
            </span>
            <span v-if="p.hooks && Object.keys(p.hooks).length > 0" class="plugin-feature-tag tag-hooks">
              Hooks ({{ Object.keys(p.hooks).length }})
            </span>
            <span v-if="p.tools && p.tools.length > 0" class="plugin-feature-tag tag-tools">
              Tools ({{ p.tools.length }})
            </span>
            <span v-if="p.systemPrompt" class="plugin-feature-tag tag-prompt">
              Prompt Instructions
            </span>
            <a
              v-if="p.homepage"
              :href="p.homepage"
              target="_blank"
              rel="noopener noreferrer"
              class="plugin-link"
            >
              GitHub ↗
            </a>
          </div>

          <!-- Details & Actions -->
          <div class="plugin-card-footer">
            <button
              type="button"
              class="plugin-details-toggle"
              @click="toggleExpand(`${p.scope}-${p.id}`)"
            >
              {{ expandedPlugins.has(`${p.scope}-${p.id}`) ? 'Hide details ▴' : 'Show details ▾' }}
            </button>

            <ThemedButton
              variant="secondary"
              size="sm"
              class="plugin-delete-btn"
              @click="emit('delete', { id: p.id, scope: p.scope })"
            >
              Delete
            </ThemedButton>
          </div>

          <!-- Expanded Details View -->
          <div v-if="expandedPlugins.has(`${p.scope}-${p.id}`)" class="plugin-expanded-body">
            <div v-if="p.hooks && Object.keys(p.hooks).length > 0" class="plugin-detail-section">
              <span class="detail-label">Active Hooks:</span>
              <ul class="detail-list">
                <li v-for="(val, hookName) in p.hooks" :key="hookName">
                  <code>{{ hookName }}</code>: <span>{{ val }}</span>
                </li>
              </ul>
            </div>

            <div v-if="p.mcpServers && Object.keys(p.mcpServers).length > 0" class="plugin-detail-section">
              <span class="detail-label">MCP Servers:</span>
              <ul class="detail-list">
                <li v-for="(srv, srvName) in p.mcpServers" :key="srvName">
                  <code>{{ srvName }}</code>: <span>{{ srv.command }} {{ srv.args?.join(' ') }}</span>
                </li>
              </ul>
            </div>

            <div v-if="p.systemPrompt" class="plugin-detail-section">
              <span class="detail-label">Injected System Prompt:</span>
              <pre class="plugin-prompt-preview">{{ p.systemPrompt }}</pre>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.settings-tab-panel {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.plugin-head-actions {
  display: flex;
  gap: 8px;
}

.plugin-presets-bar {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 14px;
  background: var(--bg-surface-secondary, rgba(255, 255, 255, 0.04));
  border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
  border-radius: 8px;
}

.plugin-presets-label {
  font-size: 0.82rem;
  font-weight: 500;
  color: var(--text-muted, #94a3b8);
}

.plugin-presets-list {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.plugin-preset-pill {
  padding: 4px 10px;
  border-radius: 6px;
  background: var(--bg-surface-tertiary, rgba(255, 255, 255, 0.08));
  border: 1px solid var(--border-color, rgba(255, 255, 255, 0.12));
  color: var(--text-primary, #f8fafc);
  font-size: 0.8rem;
  cursor: pointer;
  transition: all 0.15s ease;
}

.plugin-preset-pill:hover {
  background: var(--color-primary-subtle, rgba(99, 102, 241, 0.2));
  border-color: var(--color-primary, #6366f1);
}

/* Modal styles */
.plugin-modal-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.6);
  backdrop-filter: blur(2px);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 999;
}

.plugin-modal {
  width: 100%;
  max-width: 520px;
  background: var(--bg-surface-primary, #18181b);
  border: 1px solid var(--border-color, rgba(255, 255, 255, 0.12));
  border-radius: 12px;
  box-shadow: 0 20px 25px -5px rgba(0, 0, 0, 0.5);
  overflow: hidden;
}

.plugin-modal-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 16px 20px;
  border-bottom: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
}

.plugin-modal-title {
  margin: 0;
  font-size: 1.05rem;
  font-weight: 600;
  color: var(--text-primary, #f8fafc);
}

.plugin-modal-close {
  background: none;
  border: none;
  color: var(--text-muted, #94a3b8);
  font-size: 1.1rem;
  cursor: pointer;
}

.plugin-modal-body {
  padding: 20px;
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.plugin-source-tabs {
  display: flex;
  gap: 4px;
  padding: 4px;
  background: var(--bg-surface-secondary, rgba(255, 255, 255, 0.04));
  border-radius: 8px;
}

.plugin-source-tab {
  flex: 1;
  padding: 6px 10px;
  background: none;
  border: none;
  border-radius: 6px;
  color: var(--text-muted, #94a3b8);
  font-size: 0.8rem;
  font-weight: 500;
  cursor: pointer;
}

.plugin-source-tab.active {
  background: var(--bg-surface-tertiary, rgba(255, 255, 255, 0.12));
  color: var(--text-primary, #f8fafc);
}

.plugin-form-group {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.plugin-label {
  font-size: 0.82rem;
  font-weight: 500;
  color: var(--text-secondary, #cbd5e1);
}

.plugin-input, .plugin-textarea {
  width: 100%;
  padding: 8px 12px;
  background: var(--bg-input, rgba(0, 0, 0, 0.25));
  border: 1px solid var(--border-color, rgba(255, 255, 255, 0.12));
  border-radius: 6px;
  color: var(--text-primary, #f8fafc);
  font-size: 0.85rem;
}

.plugin-input:focus, .plugin-textarea:focus {
  outline: none;
  border-color: var(--color-primary, #6366f1);
}

.plugin-help-text {
  margin: 0;
  font-size: 0.76rem;
  color: var(--text-muted, #94a3b8);
}

.plugin-scope-options {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.plugin-radio-label {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 0.82rem;
  color: var(--text-primary, #f8fafc);
  cursor: pointer;
}

.plugin-radio-label.disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.plugin-modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  padding: 14px 20px;
  border-top: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
}

/* Cards List */
.plugin-cards {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.plugin-card {
  padding: 16px;
  background: var(--bg-surface-secondary, rgba(255, 255, 255, 0.03));
  border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
  border-radius: 10px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  transition: all 0.15s ease;
}

.plugin-card.disabled {
  opacity: 0.65;
}

.plugin-card-header {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: 16px;
}

.plugin-card-title-group {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.plugin-title-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.plugin-card-name {
  margin: 0;
  font-size: 0.95rem;
  font-weight: 600;
  color: var(--text-primary, #f8fafc);
}

.plugin-version-badge {
  font-size: 0.72rem;
  padding: 2px 6px;
  border-radius: 4px;
  background: rgba(255, 255, 255, 0.08);
  color: var(--text-muted, #94a3b8);
}

.plugin-scope-badge {
  font-size: 0.7rem;
  font-weight: 600;
  padding: 2px 6px;
  border-radius: 4px;
  text-transform: uppercase;
}

.badge-user {
  background: rgba(59, 130, 246, 0.15);
  color: #60a5fa;
  border: 1px solid rgba(59, 130, 246, 0.3);
}

.badge-project {
  background: rgba(16, 185, 129, 0.15);
  color: #34d399;
  border: 1px solid rgba(16, 185, 129, 0.3);
}

.plugin-card-desc {
  margin: 0;
  font-size: 0.82rem;
  color: var(--text-secondary, #cbd5e1);
  line-height: 1.4;
}

/* Switch */
.plugin-switch {
  position: relative;
  display: inline-block;
  width: 38px;
  height: 20px;
}

.plugin-switch input {
  opacity: 0;
  width: 0;
  height: 0;
}

.plugin-slider {
  position: absolute;
  cursor: pointer;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background-color: rgba(255, 255, 255, 0.2);
  transition: 0.2s;
  border-radius: 20px;
}

.plugin-slider:before {
  position: absolute;
  content: "";
  height: 14px;
  width: 14px;
  left: 3px;
  bottom: 3px;
  background-color: white;
  transition: 0.2s;
  border-radius: 50%;
}

input:checked + .plugin-slider {
  background-color: var(--color-primary, #6366f1);
}

input:checked + .plugin-slider:before {
  transform: translateX(18px);
}

/* Tags */
.plugin-tags-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}

.plugin-feature-tag {
  font-size: 0.72rem;
  padding: 3px 8px;
  border-radius: 4px;
  font-weight: 500;
}

.tag-mcp {
  background: rgba(168, 85, 247, 0.15);
  color: #c084fc;
}

.tag-hooks {
  background: rgba(245, 158, 11, 0.15);
  color: #fbbf24;
}

.tag-tools {
  background: rgba(14, 165, 233, 0.15);
  color: #38bdf8;
}

.tag-prompt {
  background: rgba(100, 116, 139, 0.2);
  color: #94a3b8;
}

.plugin-link {
  font-size: 0.75rem;
  color: var(--text-muted, #94a3b8);
  text-decoration: none;
  margin-left: auto;
}

.plugin-link:hover {
  color: var(--text-primary, #f8fafc);
  text-decoration: underline;
}

.plugin-card-footer {
  display: flex;
  justify-content: space-between;
  align-items: center;
  border-top: 1px solid var(--border-color, rgba(255, 255, 255, 0.05));
  padding-top: 8px;
}

.plugin-details-toggle {
  background: none;
  border: none;
  color: var(--text-muted, #94a3b8);
  font-size: 0.76rem;
  cursor: pointer;
  padding: 0;
}

.plugin-details-toggle:hover {
  color: var(--text-primary, #f8fafc);
}

.plugin-expanded-body {
  padding: 12px;
  background: rgba(0, 0, 0, 0.2);
  border-radius: 6px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.plugin-detail-section {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.detail-label {
  font-size: 0.74rem;
  font-weight: 600;
  color: var(--text-muted, #94a3b8);
}

.detail-list {
  margin: 0;
  padding-left: 18px;
  font-size: 0.78rem;
  color: var(--text-secondary, #cbd5e1);
}

.plugin-prompt-preview {
  margin: 0;
  padding: 8px;
  background: rgba(0, 0, 0, 0.3);
  border-radius: 4px;
  font-size: 0.75rem;
  white-space: pre-wrap;
  color: var(--text-secondary, #cbd5e1);
}

/* Empty state */
.plugin-empty-state {
  text-align: center;
  padding: 40px 20px;
  border: 1px dashed var(--border-color, rgba(255, 255, 255, 0.12));
  border-radius: 12px;
  background: var(--bg-surface-secondary, rgba(255, 255, 255, 0.02));
}

.plugin-empty-title {
  margin: 0 0 8px 0;
  font-size: 1rem;
  font-weight: 600;
  color: var(--text-primary, #f8fafc);
}

.plugin-empty-desc {
  margin: 0 0 16px 0;
  font-size: 0.85rem;
  color: var(--text-muted, #94a3b8);
}
</style>
