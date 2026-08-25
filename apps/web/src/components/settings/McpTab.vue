<script setup lang="ts">
import { ref, computed } from 'vue';
import type { McpServerConfig, McpServerInfo, McpServerScope, McpTransportType } from '@locagens/shared';

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

// Form & Modal state
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
const formCommand = ref('');
const formArgs = ref('');
const formUrl = ref('');
const formEnvEntries = ref<Array<{ key: string; value: string }>>([{ key: '', value: '' }]);

// Expanded tools state
const expandedServerTools = ref<Record<string, boolean>>({});

function toggleExpandedTools(name: string) {
  expandedServerTools.value[name] = !expandedServerTools.value[name];
}

const PRESETS = [
  {
    id: 'github',
    label: 'GitHub',
    description: 'Interact with GitHub repositories, issues, and PRs',
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
    description: 'Inspect and query local SQLite database files',
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
  formName.value = preset.id;
  formTransport.value = 'stdio';
  formCommand.value = preset.command;
  formArgs.value = preset.args;
  formEnvEntries.value =
    preset.env.length > 0 ? preset.env.map((e) => ({ ...e })) : [{ key: '', value: '' }];
  showForm.value = true;
  isEditing.value = false;
  isJsonMode.value = false;
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

function openEditForm(server: McpServerInfo) {
  formScope.value = server.config.scope;
  formName.value = server.config.name;
  formTransport.value = server.config.transport;
  formCommand.value = server.config.command || '';
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
      showForm.value = false;
    } catch (err: any) {
      jsonError.value = err?.message || 'Invalid JSON syntax.';
    }
    return;
  }

  const name = formName.value.trim().toLowerCase();
  if (!name) return;

  const envObj: Record<string, string> = {};
  for (const entry of formEnvEntries.value) {
    const k = entry.key.trim();
    if (k) {
      envObj[k] = entry.value;
    }
  }

  // Parse args
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
  showForm.value = false;
}

const userServers = computed(() => props.servers.filter((s) => s.config.scope === 'user'));
const projectServers = computed(() => props.servers.filter((s) => s.config.scope === 'project'));
</script>

<template>
  <div class="mcp-tab">
    <!-- Header -->
    <div class="mcp-header">
      <div>
        <h3 class="mcp-title">Model Context Protocol (MCP)</h3>
        <p class="mcp-subtitle">
          Connect external MCP servers (GitHub, PostgreSQL, Filesystem, APIs) to give your agents custom tools and capabilities.
        </p>
      </div>
      <div class="mcp-actions">
        <button class="btn btn-secondary btn-sm" :disabled="isLoading" @click="emit('refresh')">
          <span v-if="isLoading" class="spinner-sm" />
          <span v-else>↻ Refresh</span>
        </button>
        <button class="btn btn-primary btn-sm" @click="openCreateForm">
          + Add Server
        </button>
      </div>
    </div>

    <!-- Feedback messages -->
    <div v-if="error" class="alert alert-error">
      {{ error }}
    </div>
    <div v-if="statusMessage" class="alert alert-success">
      {{ statusMessage }}
    </div>

    <!-- Quick Presets Carousel/Bar -->
    <div class="presets-section">
      <span class="presets-label">Quick Presets:</span>
      <div class="presets-pills">
        <button
          v-for="p in PRESETS"
          :key="p.id"
          class="preset-pill"
          :title="p.description"
          @click="applyPreset(p)"
        >
          {{ p.label }}
        </button>
      </div>
    </div>

    <!-- Modal / Drawer Form -->
    <div v-if="showForm" class="form-modal-backdrop" @click.self="showForm = false">
      <div class="form-modal">
        <div class="modal-header">
          <h4>{{ isEditing ? `Edit Server "${editingName}"` : 'Add MCP Server' }}</h4>
          <button class="btn-close" @click="showForm = false">×</button>
        </div>

        <div class="modal-body">
          <div class="mode-switch-bar">
            <button
              class="mode-btn"
              :class="{ active: !isJsonMode }"
              @click="isJsonMode = false"
            >
              Form
            </button>
            <button
              class="mode-btn"
              :class="{ active: isJsonMode }"
              @click="isJsonMode = true"
            >
              Raw JSON
            </button>
          </div>

          <!-- JSON Mode -->
          <div v-if="isJsonMode" class="form-group">
            <label class="form-label">Configuration JSON (Claude / Cursor format)</label>
            <textarea
              v-model="rawJsonText"
              class="form-textarea json-editor"
              rows="10"
              placeholder='{
  "command": "npx",
  "args": ["-y", "@modelcontextprotocol/server-github"],
  "env": {
    "GITHUB_PERSONAL_ACCESS_TOKEN": "..."
  }
}'
            />
            <div v-if="jsonError" class="field-error">{{ jsonError }}</div>
          </div>

          <!-- Structured Form Mode -->
          <div v-else class="form-fields">
            <!-- Scope -->
            <div class="form-row">
              <label class="form-label">Scope</label>
              <div class="radio-group">
                <label class="radio-label">
                  <input v-model="formScope" type="radio" value="user" :disabled="isEditing" />
                  <span>Global (All Projects)</span>
                </label>
                <label v-if="activeProjectPath" class="radio-label">
                  <input v-model="formScope" type="radio" value="project" :disabled="isEditing" />
                  <span>Active Project ({{ activeProjectName || 'Current' }})</span>
                </label>
              </div>
            </div>

            <!-- Server Name -->
            <div class="form-group">
              <label class="form-label">Server Name</label>
              <input
                v-model="formName"
                type="text"
                class="form-input"
                placeholder="e.g. github, postgres, my-server"
                :disabled="isEditing"
                pattern="[a-z0-9_-]+"
                required
              />
              <span class="field-hint">Lowercase alphanumeric, dash and underscore.</span>
            </div>

            <!-- Transport -->
            <div class="form-group">
              <label class="form-label">Transport Type</label>
              <select v-model="formTransport" class="form-select">
                <option value="stdio">stdio (Local CLI / Subprocess)</option>
                <option value="sse">sse (Remote HTTP / SSE)</option>
              </select>
            </div>

            <!-- stdio fields -->
            <template v-if="formTransport === 'stdio'">
              <div class="form-group">
                <label class="form-label">Command</label>
                <input
                  v-model="formCommand"
                  type="text"
                  class="form-input"
                  placeholder="e.g. npx, uvx, node, python"
                  required
                />
              </div>

              <div class="form-group">
                <label class="form-label">Arguments</label>
                <input
                  v-model="formArgs"
                  type="text"
                  class="form-input"
                  placeholder="e.g. -y @modelcontextprotocol/server-github"
                />
              </div>
            </template>

            <!-- sse fields -->
            <template v-if="formTransport === 'sse'">
              <div class="form-group">
                <label class="form-label">Server URL</label>
                <input
                  v-model="formUrl"
                  type="url"
                  class="form-input"
                  placeholder="e.g. http://localhost:8000/sse"
                  required
                />
              </div>
            </template>

            <!-- Environment Variables -->
            <div class="form-group">
              <div class="env-header">
                <label class="form-label">Environment Variables / Headers</label>
                <button type="button" class="btn-link" @click="addEnvRow">+ Add Variable</button>
              </div>
              <div
                v-for="(row, idx) in formEnvEntries"
                :key="idx"
                class="env-row"
              >
                <input
                  v-model="row.key"
                  type="text"
                  class="form-input env-key"
                  placeholder="KEY (e.g. API_TOKEN)"
                />
                <input
                  v-model="row.value"
                  type="password"
                  class="form-input env-val"
                  placeholder="Value"
                />
                <button
                  type="button"
                  class="btn-icon-del"
                  title="Remove variable"
                  @click="removeEnvRow(idx)"
                >
                  ✕
                </button>
              </div>
            </div>
          </div>
        </div>

        <div class="modal-footer">
          <button class="btn btn-secondary" @click="showForm = false">Cancel</button>
          <button
            class="btn btn-primary"
            :disabled="isSaving || (!isJsonMode && !formName)"
            @click="handleSave"
          >
            <span v-if="isSaving" class="spinner-sm" />
            <span v-else>{{ isEditing ? 'Save Changes' : 'Connect Server' }}</span>
          </button>
        </div>
      </div>
    </div>

    <!-- Servers List -->
    <div class="servers-container">
      <!-- Active Project Servers -->
      <div v-if="activeProjectPath && projectServers.length > 0" class="servers-group">
        <h4 class="group-heading">Project Servers ({{ activeProjectName || 'Active Project' }})</h4>
        <div class="server-cards-grid">
          <div
            v-for="server in projectServers"
            :key="server.config.name"
            class="server-card"
            :class="{ disabled: !server.config.enabled }"
          >
            <div class="server-card-top">
              <div class="server-ident">
                <span class="server-name">{{ server.config.name }}</span>
                <span class="badge badge-scope">project</span>
                <span class="badge badge-transport">{{ server.config.transport }}</span>
              </div>
              <div class="server-status-badge" :class="server.status">
                <span class="status-dot" />
                <span class="status-text">{{ server.status }}</span>
              </div>
            </div>

            <div class="server-details">
              <div v-if="server.config.transport === 'stdio'" class="command-line">
                <code>{{ server.config.command }} {{ (server.config.args || []).join(' ') }}</code>
              </div>
              <div v-else class="command-line">
                <code>{{ server.config.url }}</code>
              </div>
              <div v-if="server.error" class="server-error-text">
                {{ server.error }}
              </div>
            </div>

            <!-- Tools dropdown -->
            <div v-if="server.tools.length > 0" class="tools-summary">
              <button class="tools-toggle-btn" @click="toggleExpandedTools(server.config.name)">
                <span>{{ server.tools.length }} {{ server.tools.length === 1 ? 'tool' : 'tools' }} available</span>
                <span class="arrow-icon">{{ expandedServerTools[server.config.name] ? '▲' : '▼' }}</span>
              </button>
              <div v-if="expandedServerTools[server.config.name]" class="tools-drawer">
                <div v-for="tool in server.tools" :key="tool.name" class="tool-item">
                  <div class="tool-name">{{ tool.originalName }}</div>
                  <div v-if="tool.description" class="tool-desc">{{ tool.description }}</div>
                </div>
              </div>
            </div>

            <!-- Actions Bar -->
            <div class="server-card-actions">
              <button
                class="btn-action-text"
                :title="server.config.enabled ? 'Disable server' : 'Enable server'"
                @click="emit('toggle', { name: server.config.name, enabled: !server.config.enabled })"
              >
                {{ server.config.enabled ? 'Disable' : 'Enable' }}
              </button>
              <button
                v-if="server.config.enabled"
                class="btn-action-text"
                title="Restart connection"
                @click="emit('restart', server.config.name)"
              >
                Restart
              </button>
              <button
                class="btn-action-text"
                title="Edit configuration"
                @click="openEditForm(server)"
              >
                Edit
              </button>
              <button
                class="btn-action-text text-danger"
                title="Delete server"
                @click="emit('delete', server.config.name)"
              >
                Delete
              </button>
            </div>
          </div>
        </div>
      </div>

      <!-- Global / User Servers -->
      <div class="servers-group">
        <h4 class="group-heading">Global Servers (Available Across All Projects)</h4>
        <div v-if="userServers.length === 0" class="empty-servers">
          <p>No global MCP servers configured yet.</p>
          <button class="btn btn-secondary btn-sm" @click="openCreateForm">
            + Add First MCP Server
          </button>
        </div>

        <div v-else class="server-cards-grid">
          <div
            v-for="server in userServers"
            :key="server.config.name"
            class="server-card"
            :class="{ disabled: !server.config.enabled }"
          >
            <div class="server-card-top">
              <div class="server-ident">
                <span class="server-name">{{ server.config.name }}</span>
                <span class="badge badge-scope">global</span>
                <span class="badge badge-transport">{{ server.config.transport }}</span>
              </div>
              <div class="server-status-badge" :class="server.status">
                <span class="status-dot" />
                <span class="status-text">{{ server.status }}</span>
              </div>
            </div>

            <div class="server-details">
              <div v-if="server.config.transport === 'stdio'" class="command-line">
                <code>{{ server.config.command }} {{ (server.config.args || []).join(' ') }}</code>
              </div>
              <div v-else class="command-line">
                <code>{{ server.config.url }}</code>
              </div>
              <div v-if="server.error" class="server-error-text">
                {{ server.error }}
              </div>
            </div>

            <!-- Tools dropdown -->
            <div v-if="server.tools.length > 0" class="tools-summary">
              <button class="tools-toggle-btn" @click="toggleExpandedTools(server.config.name)">
                <span>{{ server.tools.length }} {{ server.tools.length === 1 ? 'tool' : 'tools' }} available</span>
                <span class="arrow-icon">{{ expandedServerTools[server.config.name] ? '▲' : '▼' }}</span>
              </button>
              <div v-if="expandedServerTools[server.config.name]" class="tools-drawer">
                <div v-for="tool in server.tools" :key="tool.name" class="tool-item">
                  <div class="tool-name">{{ tool.originalName }}</div>
                  <div v-if="tool.description" class="tool-desc">{{ tool.description }}</div>
                </div>
              </div>
            </div>

            <!-- Actions Bar -->
            <div class="server-card-actions">
              <button
                class="btn-action-text"
                :title="server.config.enabled ? 'Disable server' : 'Enable server'"
                @click="emit('toggle', { name: server.config.name, enabled: !server.config.enabled })"
              >
                {{ server.config.enabled ? 'Disable' : 'Enable' }}
              </button>
              <button
                v-if="server.config.enabled"
                class="btn-action-text"
                title="Restart connection"
                @click="emit('restart', server.config.name)"
              >
                Restart
              </button>
              <button
                class="btn-action-text"
                title="Edit configuration"
                @click="openEditForm(server)"
              >
                Edit
              </button>
              <button
                class="btn-action-text text-danger"
                title="Delete server"
                @click="emit('delete', server.config.name)"
              >
                Delete
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.mcp-tab {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.mcp-header {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: 16px;
}

.mcp-title {
  margin: 0 0 4px;
  font-size: 16px;
  font-weight: 600;
  color: var(--text-color, #ffffff);
}

.mcp-subtitle {
  margin: 0;
  font-size: 13px;
  color: var(--text-muted, #8b949e);
  line-height: 1.4;
}

.mcp-actions {
  display: flex;
  gap: 8px;
  flex-shrink: 0;
}

.presets-section {
  display: flex;
  align-items: center;
  gap: 8px;
  background: var(--bg-surface, #161b22);
  padding: 8px 12px;
  border-radius: 6px;
  border: 1px solid var(--border-color, #30363d);
  flex-wrap: wrap;
}

.presets-label {
  font-size: 12px;
  color: var(--text-muted, #8b949e);
  font-weight: 500;
}

.presets-pills {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}

.preset-pill {
  background: var(--bg-subtle, #21262d);
  border: 1px solid var(--border-color, #30363d);
  color: var(--text-color, #c9d1d9);
  padding: 3px 8px;
  font-size: 12px;
  border-radius: 12px;
  cursor: pointer;
  transition: all 0.15s ease;
}

.preset-pill:hover {
  background: var(--accent-color, #1f6feb);
  color: #fff;
  border-color: var(--accent-color, #1f6feb);
}

/* Modal styles */
.form-modal-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.7);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
  backdrop-filter: blur(2px);
}

.form-modal {
  background: var(--bg-surface, #161b22);
  border: 1px solid var(--border-color, #30363d);
  border-radius: 8px;
  width: 90%;
  max-width: 540px;
  max-height: 85vh;
  display: flex;
  flex-direction: column;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
}

.modal-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 12px 16px;
  border-bottom: 1px solid var(--border-color, #30363d);
}

.modal-header h4 {
  margin: 0;
  font-size: 15px;
  font-weight: 600;
}

.btn-close {
  background: transparent;
  border: none;
  color: var(--text-muted, #8b949e);
  font-size: 18px;
  cursor: pointer;
}

.modal-body {
  padding: 16px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 16px;
  border-top: 1px solid var(--border-color, #30363d);
}

.mode-switch-bar {
  display: flex;
  border: 1px solid var(--border-color, #30363d);
  border-radius: 4px;
  overflow: hidden;
  margin-bottom: 8px;
}

.mode-btn {
  flex: 1;
  background: var(--bg-subtle, #21262d);
  border: none;
  color: var(--text-muted, #8b949e);
  padding: 6px;
  font-size: 12px;
  cursor: pointer;
}

.mode-btn.active {
  background: var(--accent-color, #1f6feb);
  color: #fff;
  font-weight: 500;
}

.json-editor {
  font-family: monospace;
  font-size: 12px;
  background: #0d1117;
  color: #c9d1d9;
}

.form-fields {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.form-group {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.form-label {
  font-size: 12px;
  font-weight: 500;
  color: var(--text-color, #c9d1d9);
}

.field-hint {
  font-size: 11px;
  color: var(--text-muted, #8b949e);
}

.field-error {
  font-size: 11px;
  color: #f85149;
  margin-top: 4px;
}

.form-input,
.form-select,
.form-textarea {
  background: #0d1117;
  border: 1px solid var(--border-color, #30363d);
  border-radius: 4px;
  color: var(--text-color, #c9d1d9);
  padding: 6px 8px;
  font-size: 13px;
}

.form-input:focus,
.form-select:focus,
.form-textarea:focus {
  outline: none;
  border-color: var(--accent-color, #1f6feb);
}

.radio-group {
  display: flex;
  gap: 16px;
  margin-top: 4px;
}

.radio-label {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--text-color, #c9d1d9);
  cursor: pointer;
}

.env-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.btn-link {
  background: none;
  border: none;
  color: var(--accent-color, #58a6ff);
  font-size: 11px;
  cursor: pointer;
  padding: 0;
}

.env-row {
  display: flex;
  gap: 6px;
  align-items: center;
  margin-bottom: 4px;
}

.env-key {
  flex: 1;
}

.env-val {
  flex: 1;
}

.btn-icon-del {
  background: none;
  border: none;
  color: var(--text-muted, #8b949e);
  cursor: pointer;
  padding: 4px;
  font-size: 12px;
}

.btn-icon-del:hover {
  color: #f85149;
}

/* Server Cards */
.servers-container {
  display: flex;
  flex-direction: column;
  gap: 20px;
}

.servers-group {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.group-heading {
  margin: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-muted, #8b949e);
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.server-cards-grid {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.server-card {
  background: var(--bg-surface, #161b22);
  border: 1px solid var(--border-color, #30363d);
  border-radius: 6px;
  padding: 12px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.server-card.disabled {
  opacity: 0.6;
}

.server-card-top {
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.server-ident {
  display: flex;
  align-items: center;
  gap: 8px;
}

.server-name {
  font-weight: 600;
  font-size: 14px;
  color: var(--text-color, #ffffff);
}

.badge {
  font-size: 10px;
  padding: 2px 6px;
  border-radius: 10px;
  text-transform: uppercase;
  font-weight: 600;
}

.badge-scope {
  background: #21262d;
  color: #8b949e;
  border: 1px solid #30363d;
}

.badge-transport {
  background: #1f2937;
  color: #60a5fa;
  border: 1px solid #3b82f6;
}

.server-status-badge {
  display: flex;
  align-items: center;
  gap: 5px;
  font-size: 11px;
  font-weight: 500;
}

.status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
}

.server-status-badge.connected .status-dot {
  background: #2ea043;
}
.server-status-badge.connected .status-text {
  color: #3fb950;
}

.server-status-badge.connecting .status-dot {
  background: #d29922;
}
.server-status-badge.connecting .status-text {
  color: #d29922;
}

.server-status-badge.error .status-dot {
  background: #f85149;
}
.server-status-badge.error .status-text {
  color: #f85149;
}

.server-status-badge.disabled .status-dot {
  background: #6e7681;
}
.server-status-badge.disabled .status-text {
  color: #8b949e;
}

.server-details {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.command-line {
  font-size: 12px;
  background: #0d1117;
  padding: 4px 8px;
  border-radius: 4px;
  overflow-x: auto;
}

.server-error-text {
  font-size: 12px;
  color: #f85149;
}

.tools-summary {
  background: #0d1117;
  border: 1px solid var(--border-color, #30363d);
  border-radius: 4px;
  overflow: hidden;
}

.tools-toggle-btn {
  width: 100%;
  background: none;
  border: none;
  padding: 6px 8px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  color: #58a6ff;
  font-size: 11px;
  cursor: pointer;
}

.tools-drawer {
  padding: 6px 8px;
  border-top: 1px solid var(--border-color, #30363d);
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.tool-item {
  font-size: 11px;
}

.tool-name {
  font-family: monospace;
  color: #7ee787;
}

.tool-desc {
  color: var(--text-muted, #8b949e);
  margin-top: 1px;
}

.server-card-actions {
  display: flex;
  gap: 12px;
  border-top: 1px solid var(--border-color, #21262d);
  padding-top: 6px;
}

.btn-action-text {
  background: none;
  border: none;
  color: #58a6ff;
  font-size: 12px;
  cursor: pointer;
  padding: 0;
}

.btn-action-text:hover {
  text-decoration: underline;
}

.btn-action-text.text-danger {
  color: #f85149;
}

.empty-servers {
  background: var(--bg-surface, #161b22);
  border: 1px dashed var(--border-color, #30363d);
  padding: 24px;
  border-radius: 6px;
  text-align: center;
  color: var(--text-muted, #8b949e);
  font-size: 13px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
}

.alert {
  padding: 8px 12px;
  border-radius: 4px;
  font-size: 12px;
}

.alert-error {
  background: rgba(248, 81, 73, 0.15);
  border: 1px solid #f85149;
  color: #ff7b72;
}

.alert-success {
  background: rgba(46, 160, 67, 0.15);
  border: 1px solid #2ea043;
  color: #56d364;
}

.spinner-sm {
  display: inline-block;
  width: 12px;
  height: 12px;
  border: 2px solid rgba(255, 255, 255, 0.3);
  border-radius: 50%;
  border-top-color: #fff;
  animation: spin 0.8s linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
