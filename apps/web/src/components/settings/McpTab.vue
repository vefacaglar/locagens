<script setup lang="ts">
import { ref, computed } from 'vue';
import type { McpServerConfig, McpServerInfo, McpServerScope, McpTransportType } from '@locagens/shared';
import ThemedSelect from '../ui/ThemedSelect.vue';
import ThemedButton from '../ui/ThemedButton.vue';

const props = defineProps<{
  servers: McpServerInfo[];
  isLoading: boolean;
  isSaving: boolean;
  error: string | null;
  statusMessage: string | null;
  activeProjectPath?: string | null;
  activeProjectName?: string | null;
}>();

const emit = defineEmits<{
  (e: 'refresh'): void;
  (e: 'save', config: McpServerConfig): void;
  (e: 'delete', name: string): void;
  (e: 'restart', name: string): void;
  (e: 'toggle', payload: { name: string; enabled: boolean }): void;
}>();

// Form state
const showForm = ref(false);
const isEditing = ref(false);
const editingName = ref('');
const isJsonMode = ref(false);
const rawJsonText = ref('');
const jsonError = ref<string | null>(null);

// Form model
const formScope = ref<McpServerScope>('user');
const formName = ref('');
const formTransport = ref<McpTransportType>('stdio');
const formCommand = ref('npx');
const formArgs = ref('');
const formUrl = ref('');
const formEnvEntries = ref<Array<{ key: string; value: string }>>([{ key: '', value: '' }]);

// Expanded tools state
const expandedServerTools = ref<Record<string, boolean>>({});

function toggleExpandedTools(name: string) {
  expandedServerTools.value[name] = !expandedServerTools.value[name];
}

function isExpanded(name: string) {
  return !!expandedServerTools.value[name];
}

const targetOptions = computed(() => [
  { value: 'user', label: 'Global (all projects)' },
  {
    value: 'project',
    label: `Project${props.activeProjectPath ? ` - ${props.activeProjectName || 'Current'}` : ' (select project first)'}`,
    disabled: !props.activeProjectPath
  }
]);

const transportOptions = [
  { value: 'stdio', label: 'stdio (Local CLI / Subprocess)' },
  { value: 'sse', label: 'sse (Remote HTTP / SSE)' }
];

const PRESETS = [
  {
    id: 'github',
    label: 'GitHub',
    description: 'Interact with GitHub repos, issues, and PRs',
    command: 'npx',
    args: '-y @modelcontextprotocol/server-github',
    env: [{ key: 'GITHUB_PERSONAL_ACCESS_TOKEN', value: '' }]
  },
  {
    id: 'postgres',
    label: 'PostgreSQL',
    description: 'Query and inspect PostgreSQL databases',
    command: 'npx',
    args: '-y @modelcontextprotocol/server-postgres postgresql://localhost/mydb',
    env: []
  },
  {
    id: 'sqlite',
    label: 'SQLite',
    description: 'Inspect and query local SQLite databases',
    command: 'npx',
    args: '-y @modelcontextprotocol/server-sqlite --db-path ./database.sqlite',
    env: []
  },
  {
    id: 'filesystem',
    label: 'Filesystem',
    description: 'Access local directory trees via MCP',
    command: 'npx',
    args: '-y @modelcontextprotocol/server-filesystem .',
    env: []
  },
  {
    id: 'brave-search',
    label: 'Brave Search',
    description: 'Web search using Brave Search API',
    command: 'npx',
    args: '-y @modelcontextprotocol/server-brave-search',
    env: [{ key: 'BRAVE_API_KEY', value: '' }]
  }
];

function applyPreset(preset: (typeof PRESETS)[0]) {
  formScope.value = props.activeProjectPath ? 'project' : 'user';
  formName.value = preset.id;
  formTransport.value = 'stdio';
  formCommand.value = preset.command;
  formArgs.value = preset.args;
  formEnvEntries.value =
    preset.env.length > 0 ? preset.env.map((e) => ({ ...e })) : [{ key: '', value: '' }];
  showForm.value = true;
  isEditing.value = false;
  isJsonMode.value = false;
  jsonError.value = null;
}

function openCreateForm() {
  formScope.value = props.activeProjectPath ? 'project' : 'user';
  formName.value = '';
  formTransport.value = 'stdio';
  formCommand.value = 'npx';
  formArgs.value = '';
  formUrl.value = '';
  formEnvEntries.value = [{ key: '', value: '' }];
  isEditing.value = false;
  editingName.value = '';
  isJsonMode.value = false;
  rawJsonText.value = '';
  jsonError.value = null;
  showForm.value = true;
}

function cancelForm() {
  showForm.value = false;
  isEditing.value = false;
  editingName.value = '';
  jsonError.value = null;
}

function openEditForm(server: McpServerInfo) {
  formScope.value = server.config.scope;
  formName.value = server.config.name;
  formTransport.value = server.config.transport;
  formCommand.value = server.config.command || 'npx';
  formArgs.value = (server.config.args || []).join(' ');
  formUrl.value = server.config.url || '';

  const entries: Array<{ key: string; value: string }> = [];
  if (server.config.env) {
    for (const [k, v] of Object.entries(server.config.env)) {
      entries.push({ key: k, value: v });
    }
  }
  if (entries.length === 0) entries.push({ key: '', value: '' });
  formEnvEntries.value = entries;

  isEditing.value = true;
  editingName.value = server.config.name;
  isJsonMode.value = false;
  rawJsonText.value = '';
  jsonError.value = null;
  showForm.value = true;
}

function addEnvRow() {
  formEnvEntries.value.push({ key: '', value: '' });
}

function removeEnvRow(index: number) {
  formEnvEntries.value.splice(index, 1);
  if (formEnvEntries.value.length === 0) {
    formEnvEntries.value.push({ key: '', value: '' });
  }
}

function handleSave() {
  jsonError.value = null;

  if (isJsonMode.value) {
    try {
      const parsed = JSON.parse(rawJsonText.value);
      if (!parsed || typeof parsed !== 'object') throw new Error('Invalid JSON format.');
      const name = String(parsed.name || formName.value).trim().toLowerCase();
      if (!name) throw new Error('Server name is required.');

      const config: McpServerConfig = {
        name,
        scope: formScope.value,
        transport: parsed.transport === 'sse' ? 'sse' : 'stdio',
        command: parsed.command,
        args: Array.isArray(parsed.args) ? parsed.args : undefined,
        env: parsed.env,
        url: parsed.url,
        enabled: parsed.enabled !== false,
        projectPath: formScope.value === 'project' ? props.activeProjectPath || undefined : undefined
      };
      emit('save', config);
      cancelForm();
    } catch (err: any) {
      jsonError.value = err?.message || 'Invalid JSON syntax.';
    }
    return;
  }

  const name = formName.value.trim().toLowerCase().replace(/[^a-z0-9_-]/g, '-');
  if (!name) return;

  const envObj: Record<string, string> = {};
  for (const entry of formEnvEntries.value) {
    const k = entry.key.trim();
    if (k) {
      envObj[k] = entry.value;
    }
  }

  let parsedArgs: string[] = [];
  if (formArgs.value.trim()) {
    parsedArgs = formArgs.value
      .trim()
      .split(/\s+/)
      .filter((a) => a.length > 0);
  }

  const config: McpServerConfig = {
    name,
    scope: formScope.value,
    transport: formTransport.value,
    command: formTransport.value === 'stdio' ? formCommand.value.trim() : undefined,
    args: formTransport.value === 'stdio' ? parsedArgs : undefined,
    url: formTransport.value === 'sse' ? formUrl.value.trim() : undefined,
    env: Object.keys(envObj).length > 0 ? envObj : undefined,
    enabled: true,
    projectPath: formScope.value === 'project' ? props.activeProjectPath || undefined : undefined
  };

  emit('save', config);
  cancelForm();
}

const userServers = computed(() => props.servers.filter((s) => s.config.scope === 'user'));
const projectServers = computed(() => props.servers.filter((s) => s.config.scope === 'project'));
</script>

<template>
  <div class="settings-tab-panel">
    <!-- Header -->
    <header class="settings-section-head">
      <div>
        <h3 class="settings-section-title">Model Context Protocol (MCP)</h3>
        <p class="settings-section-desc">
          Connect external MCP servers (GitHub, PostgreSQL, Filesystem, SQLite, custom APIs) to extend your assistant with powerful external tools.
        </p>
      </div>
      <div class="mcp-head-actions">
        <ThemedButton
          :variant="showForm ? 'secondary' : 'primary'"
          size="sm"
          :disabled="isSaving"
          @click="showForm ? cancelForm() : openCreateForm()"
        >
          {{ showForm ? 'Cancel' : '+ Add server' }}
        </ThemedButton>
        <ThemedButton
          variant="secondary"
          size="sm"
          :disabled="isLoading || isSaving"
          @click="emit('refresh')"
        >
          Refresh
        </ThemedButton>
      </div>
    </header>

    <!-- Feedback banners -->
    <p v-if="error" class="skills-banner skills-banner-error">{{ error }}</p>
    <p v-else-if="statusMessage" class="skills-banner">{{ statusMessage }}</p>

    <!-- Quick Presets -->
    <div class="mcp-presets-bar">
      <span class="mcp-presets-label">Quick templates:</span>
      <div class="mcp-presets-list">
        <button
          v-for="p in PRESETS"
          :key="p.id"
          type="button"
          class="mcp-preset-pill"
          :title="p.description"
          @click="applyPreset(p)"
        >
          {{ p.label }}
        </button>
      </div>
    </div>

    <!-- Inline Add / Edit Form -->
    <div v-if="showForm" class="skills-form-wrap">
      <div class="mcp-form-head">
        <h4 class="skills-form-title">
          {{ isEditing ? `Edit Server "${editingName}"` : 'Add MCP Server' }}
        </h4>
        <div class="mcp-mode-toggle">
          <button
            type="button"
            class="mcp-mode-btn"
            :class="{ active: !isJsonMode }"
            @click="isJsonMode = false"
          >
            Form
          </button>
          <button
            type="button"
            class="mcp-mode-btn"
            :class="{ active: isJsonMode }"
            @click="isJsonMode = true"
          >
            Raw JSON
          </button>
        </div>
      </div>

      <!-- Raw JSON Mode -->
      <template v-if="isJsonMode">
        <label class="skills-field">
          <span class="skills-field-label">Claude Desktop / Cursor format JSON</span>
          <textarea
            v-model="rawJsonText"
            class="text-input skills-textarea mcp-json-area"
            rows="8"
            placeholder='{
  "name": "github",
  "command": "npx",
  "args": ["-y", "@modelcontextprotocol/server-github"],
  "env": {
    "GITHUB_PERSONAL_ACCESS_TOKEN": "ghp_..."
  }
}'
          ></textarea>
        </label>
        <p v-if="jsonError" class="skills-banner skills-banner-error">{{ jsonError }}</p>
      </template>

      <!-- Visual Form Mode -->
      <template v-else>
        <div class="skills-form-row">
          <label class="skills-field">
            <span class="skills-field-label">Scope</span>
            <ThemedSelect
              v-model="formScope"
              :options="targetOptions"
              :disabled="isEditing"
            />
          </label>
          <label class="skills-field">
            <span class="skills-field-label">Transport</span>
            <ThemedSelect
              v-model="formTransport"
              :options="transportOptions"
            />
          </label>
          <label class="skills-field flex-2">
            <span class="skills-field-label">Server Name (ID)</span>
            <input
              v-model="formName"
              type="text"
              class="text-input skills-input"
              placeholder="e.g. github, postgres, my-server"
              maxlength="48"
              :disabled="isEditing"
            />
          </label>
        </div>

        <template v-if="formTransport === 'stdio'">
          <div class="skills-form-row">
            <label class="skills-field">
              <span class="skills-field-label">Command</span>
              <input
                v-model="formCommand"
                type="text"
                class="text-input skills-input"
                placeholder="e.g. npx, uvx, node, python"
              />
            </label>
            <label class="skills-field flex-2">
              <span class="skills-field-label">Arguments</span>
              <input
                v-model="formArgs"
                type="text"
                class="text-input skills-input"
                placeholder="e.g. -y @modelcontextprotocol/server-github"
              />
            </label>
          </div>
        </template>

        <template v-else>
          <label class="skills-field">
            <span class="skills-field-label">Server URL (SSE endpoint)</span>
            <input
              v-model="formUrl"
              type="url"
              class="text-input skills-input"
              placeholder="e.g. http://localhost:8000/sse"
            />
          </label>
        </template>

        <!-- Environment Variables -->
        <div class="mcp-env-section">
          <div class="mcp-env-head">
            <span class="skills-field-label">Environment Variables / API Keys</span>
            <button type="button" class="skills-btn-link" @click="addEnvRow">
              + Add Variable
            </button>
          </div>
          <div
            v-for="(row, idx) in formEnvEntries"
            :key="idx"
            class="mcp-env-row"
          >
            <input
              v-model="row.key"
              type="text"
              class="text-input skills-input mcp-env-key"
              placeholder="KEY (e.g. GITHUB_TOKEN)"
            />
            <input
              v-model="row.value"
              type="password"
              class="text-input skills-input mcp-env-val"
              placeholder="Value"
            />
            <button
              type="button"
              class="mcp-btn-del"
              title="Remove variable"
              @click="removeEnvRow(idx)"
            >
              ✕
            </button>
          </div>
        </div>
      </template>

      <div class="skills-form-actions">
        <ThemedButton
          variant="primary"
          size="sm"
          :disabled="isSaving || (!isJsonMode && !formName.trim())"
          @click="handleSave"
        >
          {{ isSaving ? 'Connecting…' : (isEditing ? 'Save Changes' : 'Connect Server') }}
        </ThemedButton>
        <ThemedButton variant="secondary" size="sm" @click="cancelForm">
          Cancel
        </ThemedButton>
      </div>
    </div>

    <div v-if="isLoading" class="settings-empty">Loading MCP servers…</div>

    <template v-else>
      <!-- Active Project Servers Section -->
      <section v-if="activeProjectPath" class="skills-group">
        <div class="skills-group-head">
          <div>
            <h4 class="skills-group-title">
              Project servers
              <span class="skills-project-label">· {{ activeProjectName || 'Current Project' }}</span>
            </h4>
          </div>
        </div>

        <ul v-if="projectServers.length > 0" class="skills-list">
          <li
            v-for="server in projectServers"
            :key="`project-${server.config.name}`"
            class="skills-item"
            :class="{ disabled: !server.config.enabled }"
          >
            <div class="skills-item-head">
              <div class="skills-item-title-wrap">
                <span class="skills-name">{{ server.config.name }}</span>
                <span class="skills-badge project">project</span>
                <span class="mcp-badge-transport">{{ server.config.transport }}</span>
                <span class="mcp-status-pill" :class="server.status">
                  <span class="mcp-status-dot" />
                  {{ server.status }}
                </span>
              </div>
              <div class="skills-item-actions">
                <button
                  v-if="server.tools.length > 0"
                  class="skills-btn-link"
                  @click="toggleExpandedTools(`proj-${server.config.name}`)"
                >
                  {{ isExpanded(`proj-${server.config.name}`) ? 'Hide tools' : `${server.tools.length} tools` }}
                </button>
                <button
                  class="skills-btn-link"
                  @click="emit('toggle', { name: server.config.name, enabled: !server.config.enabled })"
                >
                  {{ server.config.enabled ? 'Disable' : 'Enable' }}
                </button>
                <button
                  v-if="server.config.enabled"
                  class="skills-btn-link"
                  @click="emit('restart', server.config.name)"
                >
                  Reconnect
                </button>
                <button
                  class="skills-btn-link"
                  @click="openEditForm(server)"
                >
                  Edit
                </button>
                <button
                  class="skills-btn-link delete"
                  title="Delete server"
                  @click="emit('delete', server.config.name)"
                >
                  Delete
                </button>
              </div>
            </div>

            <p class="mcp-cmd-line">
              <code v-if="server.config.transport === 'stdio'">{{ server.config.command }} {{ (server.config.args || []).join(' ') }}</code>
              <code v-else>{{ server.config.url }}</code>
            </p>

            <p v-if="server.error" class="mcp-error-line">{{ server.error }}</p>

            <!-- Tools preview list -->
            <div
              v-if="isExpanded(`proj-${server.config.name}`) && server.tools.length > 0"
              class="skills-body-preview"
            >
              <div
                v-for="tool in server.tools"
                :key="tool.name"
                class="mcp-tool-preview-item"
              >
                <span class="mcp-tool-fn-name">{{ tool.originalName }}</span>
                <span v-if="tool.description" class="mcp-tool-fn-desc">{{ tool.description }}</span>
              </div>
            </div>
          </li>
        </ul>
        <p v-else class="skills-empty-hint">
          No project MCP servers configured. Project servers are defined in <code>.locagens/mcp.json</code>.
        </p>
      </section>

      <!-- Global / User Servers Section -->
      <section class="skills-group">
        <div class="skills-group-head">
          <div>
            <h4 class="skills-group-title">Global servers (All projects)</h4>
          </div>
        </div>

        <ul v-if="userServers.length > 0" class="skills-list">
          <li
            v-for="server in userServers"
            :key="`user-${server.config.name}`"
            class="skills-item"
            :class="{ disabled: !server.config.enabled }"
          >
            <div class="skills-item-head">
              <div class="skills-item-title-wrap">
                <span class="skills-name">{{ server.config.name }}</span>
                <span class="skills-badge user">global</span>
                <span class="mcp-badge-transport">{{ server.config.transport }}</span>
                <span class="mcp-status-pill" :class="server.status">
                  <span class="mcp-status-dot" />
                  {{ server.status }}
                </span>
              </div>
              <div class="skills-item-actions">
                <button
                  v-if="server.tools.length > 0"
                  class="skills-btn-link"
                  @click="toggleExpandedTools(`user-${server.config.name}`)"
                >
                  {{ isExpanded(`user-${server.config.name}`) ? 'Hide tools' : `${server.tools.length} tools` }}
                </button>
                <button
                  class="skills-btn-link"
                  @click="emit('toggle', { name: server.config.name, enabled: !server.config.enabled })"
                >
                  {{ server.config.enabled ? 'Disable' : 'Enable' }}
                </button>
                <button
                  v-if="server.config.enabled"
                  class="skills-btn-link"
                  @click="emit('restart', server.config.name)"
                >
                  Reconnect
                </button>
                <button
                  class="skills-btn-link"
                  @click="openEditForm(server)"
                >
                  Edit
                </button>
                <button
                  class="skills-btn-link delete"
                  title="Delete server"
                  @click="emit('delete', server.config.name)"
                >
                  Delete
                </button>
              </div>
            </div>

            <p class="mcp-cmd-line">
              <code v-if="server.config.transport === 'stdio'">{{ server.config.command }} {{ (server.config.args || []).join(' ') }}</code>
              <code v-else>{{ server.config.url }}</code>
            </p>

            <p v-if="server.error" class="mcp-error-line">{{ server.error }}</p>

            <!-- Tools preview list -->
            <div
              v-if="isExpanded(`user-${server.config.name}`) && server.tools.length > 0"
              class="skills-body-preview"
            >
              <div
                v-for="tool in server.tools"
                :key="tool.name"
                class="mcp-tool-preview-item"
              >
                <span class="mcp-tool-fn-name">{{ tool.originalName }}</span>
                <span v-if="tool.description" class="mcp-tool-fn-desc">{{ tool.description }}</span>
              </div>
            </div>
          </li>
        </ul>
        <p v-else class="skills-empty-hint">
          No global MCP servers configured yet. Click <strong>+ Add server</strong> or pick a quick template above to connect one.
        </p>
      </section>
    </template>
  </div>
</template>

<style scoped>
.mcp-head-actions {
  display: flex;
  gap: 8px;
  align-items: center;
}

.mcp-presets-bar {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 16px;
  padding: 8px 12px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 8px;
  flex-wrap: wrap;
}

.mcp-presets-label {
  font-size: 11px;
  font-weight: 500;
  color: var(--text-secondary);
}

.mcp-presets-list {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}

.mcp-preset-pill {
  background: var(--surface-input, var(--surface));
  border: 1px solid var(--border);
  color: var(--text-secondary);
  padding: 3px 9px;
  font-size: 11px;
  font-weight: 500;
  border-radius: 12px;
  cursor: pointer;
  transition: all 0.15s ease;
}

.mcp-preset-pill:hover {
  background: color-mix(in srgb, var(--accent, #648cff) 15%, transparent);
  color: var(--text-primary);
  border-color: var(--accent, #648cff);
}

.mcp-form-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 4px;
}

.mcp-mode-toggle {
  display: flex;
  border: 1px solid var(--border);
  border-radius: 6px;
  overflow: hidden;
}

.mcp-mode-btn {
  background: var(--surface);
  border: none;
  color: var(--text-secondary);
  padding: 4px 10px;
  font-size: 11px;
  cursor: pointer;
  transition: background 0.15s ease;
}

.mcp-mode-btn.active {
  background: var(--accent, #648cff);
  color: #fff;
  font-weight: 600;
}

.mcp-json-area {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
}

.mcp-env-section {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-top: 4px;
}

.mcp-env-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.mcp-env-row {
  display: flex;
  gap: 8px;
  align-items: center;
}

.mcp-env-key {
  flex: 1;
}

.mcp-env-val {
  flex: 1.5;
}

.mcp-btn-del {
  background: none;
  border: none;
  color: var(--text-secondary);
  cursor: pointer;
  padding: 4px 6px;
  font-size: 12px;
  transition: color 0.15s ease;
}

.mcp-btn-del:hover {
  color: var(--danger, #c44);
}

.mcp-badge-transport {
  font-size: 10px;
  font-weight: 600;
  padding: 2px 6px;
  border-radius: 4px;
  text-transform: uppercase;
  background: rgba(160, 100, 255, 0.15);
  color: #b088ff;
  letter-spacing: 0.5px;
}

.mcp-status-pill {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 10px;
  font-weight: 600;
  padding: 2px 7px;
  border-radius: 10px;
  text-transform: capitalize;
}

.mcp-status-pill.connected {
  background: rgba(46, 160, 67, 0.15);
  color: #3fb950;
}

.mcp-status-pill.connecting {
  background: rgba(210, 153, 34, 0.15);
  color: #d29922;
}

.mcp-status-pill.error {
  background: rgba(248, 81, 73, 0.15);
  color: #f85149;
}

.mcp-status-pill.disabled {
  background: rgba(110, 118, 129, 0.15);
  color: #8b949e;
}

.mcp-status-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;
}

.mcp-cmd-line {
  margin: 2px 0 0;
  font-size: 11px;
  color: var(--text-tertiary, var(--text-secondary));
  word-break: break-all;
}

.mcp-cmd-line code {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
}

.mcp-error-line {
  margin: 2px 0 0;
  font-size: 11px;
  color: var(--danger, #c44);
}

.mcp-tool-preview-item {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 4px 0;
}

.mcp-tool-preview-item:not(:last-child) {
  border-bottom: 1px dashed var(--border);
  padding-bottom: 6px;
}

.mcp-tool-fn-name {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 11px;
  font-weight: 600;
  color: var(--accent, #648cff);
}

.mcp-tool-fn-desc {
  font-size: 11px;
  line-height: 1.4;
  color: var(--text-secondary);
}

.skills-item.disabled {
  opacity: 0.6;
}

/* Base style rules matching SkillsTab and SettingsScreen */
.skills-banner {
  margin: 0 0 14px;
  padding: 10px 12px;
  border-radius: 8px;
  font-size: 12px;
  line-height: 1.4;
  background: var(--surface);
  border: 1px solid var(--border);
  color: var(--text-secondary);
  word-break: break-all;
}

.skills-banner-error {
  border-color: color-mix(in srgb, var(--danger, #c44) 40%, var(--border));
  color: var(--danger, #c44);
}

.skills-form-wrap {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px;
  margin-bottom: 20px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 10px;
}

.skills-form-title {
  margin: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
}

.skills-form-row {
  display: flex;
  gap: 12px;
}

.skills-field {
  display: flex;
  flex-direction: column;
  gap: 6px;
  flex: 1;
}

.skills-field.flex-2 {
  flex: 2;
}

.skills-field-label {
  font-size: 11px;
  font-weight: 500;
  color: var(--text-secondary);
}

.skills-input,
.skills-textarea {
  width: 100%;
  box-sizing: border-box;
  padding: 8px 10px;
  font-size: 12px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--surface-input, var(--surface));
  color: var(--text-primary);
  outline: none;
}

.skills-textarea {
  resize: vertical;
}

.skills-form-actions {
  display: flex;
  gap: 8px;
  margin-top: 4px;
}

.skills-group {
  margin-bottom: 22px;
}

.skills-group-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 10px;
}

.skills-group-title {
  margin: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
}

.skills-project-label {
  font-weight: 500;
  color: var(--text-secondary);
}

.skills-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.skills-item {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 12px 14px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--surface);
  transition: border-color 0.15s ease;
}

.skills-item:hover {
  border-color: var(--border-hover, var(--border));
}

.skills-item-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.skills-item-title-wrap {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.skills-name {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
}

.skills-badge {
  font-size: 10px;
  font-weight: 600;
  padding: 2px 6px;
  border-radius: 4px;
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.skills-badge.user {
  background: rgba(100, 140, 255, 0.15);
  color: #648cff;
}

.skills-badge.project {
  background: rgba(80, 200, 120, 0.15);
  color: #50c878;
}

.skills-item-actions {
  display: flex;
  align-items: center;
  gap: 10px;
}

.skills-btn-link {
  background: none;
  border: none;
  padding: 0;
  font-size: 11px;
  color: var(--text-secondary);
  cursor: pointer;
  text-decoration: underline;
  transition: color 0.15s ease;
}

.skills-btn-link:hover {
  color: var(--text-primary);
}

.skills-btn-link.delete {
  color: var(--danger, #c44);
}

.skills-btn-link.delete:hover {
  color: color-mix(in srgb, var(--danger, #c44) 80%, black);
}

.skills-body-preview {
  margin-top: 6px;
  padding: 10px 12px;
  background: var(--surface-strong, rgba(0, 0, 0, 0.04));
  border: 1px solid var(--border);
  border-radius: 6px;
  overflow-x: auto;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.skills-empty-hint {
  margin: 0;
  font-size: 12px;
  line-height: 1.45;
  color: var(--text-secondary);
}

.skills-empty-hint code {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
}
</style>
