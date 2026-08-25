<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from 'vue';
import type { Memory, MemoryCategory, MemoryScope, PermissionRule, ProviderMetadata, AgentPreset, SkillSummary, McpServerInfo, McpServerConfig } from '@locagens/shared';
import { useIsMobile } from '../../composables/useIsMobile';
import PermissionsTab from './PermissionsTab.vue';
import ProvidersTab from './ProvidersTab.vue';
import AgentPresetsTab from './AgentPresetsTab.vue';
import MemoryTab from './MemoryTab.vue';
import SkillsTab from './SkillsTab.vue';
import McpTab from './McpTab.vue';
import ServerTab from './ServerTab.vue';

const props = defineProps<{
  show: boolean;
  permissions: PermissionRule[];
  isLoading: boolean;
  providers: ProviderMetadata[];
  providersConfig: Record<string, any>;
  providersConfigLoading: boolean;
  presets: AgentPreset[];
  memories: Memory[];
  memoriesLoading: boolean;
  skills: SkillSummary[];
  skillsLoading: boolean;
  skillsUserRoot: string;
  skillsProjectRoot: string | null;
  skillsError?: string | null;
  skillsStatusMessage?: string | null;
  skillsInstalling?: boolean;
  mcpServers?: McpServerInfo[];
  mcpLoading?: boolean;
  mcpSaving?: boolean;
  mcpError?: string | null;
  mcpStatusMessage?: string | null;
  activeProjectPath: string;
  activeProjectName: string;
  activeTab?: TabId;
}>();

const emit = defineEmits<{
  (e: 'close'): void;
  (e: 'revoke', id: number): void;
  (e: 'clear-all'): void;
  (e: 'providers-saved'): void;
  (e: 'presets-saved'): void;
  (e: 'add-memory', payload: { scope: MemoryScope; category: MemoryCategory; content: string; projectPath?: string }): void;
  (e: 'update-memory', payload: { id: number; content: string }): void;
  (e: 'delete-memory', id: number): void;
  (e: 'clear-memories'): void;
  (e: 'refresh-skills'): void;
  (e: 'install-skill-file', payload: { target: 'user' | 'project'; file: File }): void;
  (e: 'create-skill', payload: { target: 'user' | 'project'; name: string; description: string; body: string }): void;
  (e: 'delete-skill', payload: { target: 'user' | 'project'; name: string }): void;
  (e: 'open-skills-folder', target: 'user' | 'project'): void;
  (e: 'refresh-mcp'): void;
  (e: 'save-mcp-server', config: McpServerConfig): void;
  (e: 'delete-mcp-server', name: string): void;
  (e: 'restart-mcp-server', name: string): void;
  (e: 'toggle-mcp-server', payload: { name: string; enabled: boolean }): void;
  (e: 'update:activeTab', value: TabId): void;
}>();

const TABS = [
  { id: 'permissions', label: 'Permissions' },
  { id: 'memory', label: 'Memory' },
  { id: 'skills', label: 'Skills' },
  { id: 'mcp', label: 'MCP' },
  { id: 'providers', label: 'Providers' },
  { id: 'agents', label: 'Agents' },
  { id: 'server', label: 'Server' }
] as const;
type TabId = (typeof TABS)[number]['id'];

const localActiveTab = ref<TabId>('permissions');
const isSidebarCollapsed = ref(false);
const isMobile = useIsMobile();

watch(() => props.activeTab, (newVal) => {
  if (newVal) {
    localActiveTab.value = newVal;
  }
}, { immediate: true });

function selectTab(tabId: TabId) {
  localActiveTab.value = tabId;
  emit('update:activeTab', tabId);
  if (isMobile.value) {
    isSidebarCollapsed.value = true;
  }
}

function onKey(e: KeyboardEvent) {
  if (props.show && e.key === 'Escape') {
    e.preventDefault();
    emit('close');
  }
}

watch(() => props.show, (newVal) => {
  if (newVal) {
    isSidebarCollapsed.value = isMobile.value;
  }
});

onMounted(() => {
  window.addEventListener('keydown', onKey);
  if (isMobile.value) {
    isSidebarCollapsed.value = true;
  }
});

onBeforeUnmount(() => window.removeEventListener('keydown', onKey));
</script>

<template>
  <transition name="settings-fade">
    <div v-if="show" class="settings-overlay" @click.self="emit('close')">
      <div class="settings-card">
        <aside class="settings-sidebar" :class="{ collapsed: isSidebarCollapsed }">
          <div class="settings-sidebar-header">
            <span class="settings-sidebar-label" title="Settings">
              <svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="settings-sidebar-icon">
                <circle cx="12" cy="12" r="3"/>
                <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/>
              </svg>
            </span>
            <button
              class="settings-sidebar-close-btn"
              @click="isSidebarCollapsed = true"
              title="Close Menu"
            >
              <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <line x1="18" y1="6" x2="6" y2="18"></line>
                <line x1="6" y1="6" x2="18" y2="18"></line>
              </svg>
            </button>
          </div>

          <div class="settings-tabs-list">
            <button
              v-for="tab in TABS"
              :key="tab.id"
              class="settings-tab-btn"
              :class="{ active: localActiveTab === tab.id }"
              @click="selectTab(tab.id)"
            >
              {{ tab.label }}
            </button>
          </div>

          <button class="settings-close-btn" @click="emit('close')">
            <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="m15 18-6-6 6-6"/>
            </svg>
            Close Settings
          </button>
        </aside>

        <main class="settings-main">
          <header class="settings-main-header">
            <div class="settings-main-header-inner">
              <div class="settings-header-left">
                <button
                  class="settings-sidebar-toggle-btn"
                  :class="{ active: !isSidebarCollapsed }"
                  @click="isSidebarCollapsed = !isSidebarCollapsed"
                  title="Toggle Sidebar"
                >
                  <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
                    <line x1="9" y1="3" x2="9" y2="21"></line>
                  </svg>
                </button>
                <div class="settings-breadcrumb">
                  <span class="breadcrumb-project">Settings</span>
                  <span class="breadcrumb-separator">/</span>
                  <span class="breadcrumb-chat-title">{{ TABS.find(t => t.id === localActiveTab)?.label }}</span>
                </div>
              </div>
            </div>
          </header>

          <section class="settings-scroll-area">
            <div class="settings-container-wrap">
              <PermissionsTab
                v-if="localActiveTab === 'permissions'"
                :permissions="permissions"
                :is-loading="isLoading"
                @revoke="emit('revoke', $event)"
                @clear-all="emit('clear-all')"
              />
              <MemoryTab
                v-else-if="localActiveTab === 'memory'"
                :memories="memories"
                :is-loading="memoriesLoading"
                :active-project-path="activeProjectPath"
                :active-project-name="activeProjectName"
                @add="emit('add-memory', $event)"
                @update="emit('update-memory', $event)"
                @delete="emit('delete-memory', $event)"
                @clear-all="emit('clear-memories')"
              />
              <SkillsTab
                v-else-if="localActiveTab === 'skills'"
                :skills="skills"
                :is-loading="skillsLoading"
                :is-installing="skillsInstalling"
                :user-root="skillsUserRoot"
                :project-root="skillsProjectRoot"
                :active-project-path="activeProjectPath"
                :active-project-name="activeProjectName"
                :error="skillsError"
                :status-message="skillsStatusMessage"
                @refresh="emit('refresh-skills')"
                @install-file="emit('install-skill-file', $event)"
                @create-skill="emit('create-skill', $event)"
                @delete-skill="emit('delete-skill', $event)"
                @open-folder="emit('open-skills-folder', $event)"
              />
              <McpTab
                v-else-if="localActiveTab === 'mcp'"
                :servers="mcpServers || []"
                :is-loading="!!mcpLoading"
                :is-saving="!!mcpSaving"
                :error="mcpError || null"
                :status-message="mcpStatusMessage || null"
                :active-project-path="activeProjectPath"
                :active-project-name="activeProjectName"
                @refresh="emit('refresh-mcp')"
                @save="emit('save-mcp-server', $event)"
                @delete="emit('delete-mcp-server', $event)"
                @restart="emit('restart-mcp-server', $event)"
                @toggle="emit('toggle-mcp-server', $event)"
              />
              <ProvidersTab
                v-else-if="localActiveTab === 'providers'"
                :providers-config="providersConfig"
                :is-loading="providersConfigLoading"
                @saved="emit('providers-saved')"
              />
              <AgentPresetsTab
                v-else-if="localActiveTab === 'agents'"
                :providers="providers"
                :presets="presets"
                @saved="emit('presets-saved')"
              />
              <ServerTab v-else-if="localActiveTab === 'server'" />
            </div>
          </section>
        </main>
      </div>
    </div>
  </transition>
</template>

<style scoped>
.settings-overlay {
  position: fixed;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  z-index: 2500;
  background: var(--settings-overlay-bg);
  backdrop-filter: blur(12px);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
}

.settings-card {
  width: min(980px, 100%);
  height: min(680px, 90vh);
  background: var(--settings-card-bg);
  border: 1px solid var(--settings-card-border);
  border-radius: 16px;
  box-shadow: 
    0 30px 70px var(--settings-card-shadow),
    0 0 1px var(--settings-card-inset-shadow) inset;
  display: flex;
  overflow: hidden;
}

.settings-sidebar {
  width: 220px;
  background: var(--settings-sidebar-bg);
  border-right: 1px solid var(--settings-sidebar-border);
  padding: 20px;
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
  transition: width 0.25s cubic-bezier(0.4, 0, 0.2, 1),
              padding 0.25s cubic-bezier(0.4, 0, 0.2, 1),
              opacity 0.25s cubic-bezier(0.4, 0, 0.2, 1),
              border-color 0.25s cubic-bezier(0.4, 0, 0.2, 1);
  overflow: hidden;
  white-space: nowrap;
}

.settings-sidebar.collapsed {
  width: 0 !important;
  padding-left: 0 !important;
  padding-right: 0 !important;
  opacity: 0 !important;
  border-right-color: transparent !important;
  pointer-events: none;
}

.settings-header-left {
  display: flex;
  align-items: center;
}

.settings-sidebar-toggle-btn {
  display: none;
  background: transparent;
  border: none;
  color: var(--muted);
  cursor: pointer;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border-radius: 6px;
  transition: all 0.2s ease;
  margin-right: 12px;
}

.settings-sidebar-toggle-btn:hover {
  color: var(--text);
  background: var(--settings-sidebar-toggle-hover);
}

.settings-sidebar-toggle-btn.active {
  color: var(--text);
}

.settings-sidebar-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 24px;
  padding-left: 6px;
}

.settings-sidebar-close-btn {
  display: none;
}

.settings-sidebar-label {
  display: flex;
  align-items: center;
  color: var(--text);
}

.settings-sidebar-icon {
  width: 24px;
  height: 24px;
  color: var(--text);
  opacity: 0.8;
  transition: opacity 0.2s ease, transform 0.3s ease;
}

.settings-sidebar-label:hover .settings-sidebar-icon {
  opacity: 1;
  transform: rotate(25deg);
}

.settings-tabs-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  flex: 1;
}

.settings-tab-btn {
  width: 100%;
  padding: 10px 12px;
  font-size: 0.92rem;
  font-weight: 400;
  color: var(--muted);
  background: transparent;
  border: 1px solid transparent;
  border-radius: 8px;
  text-align: left;
  cursor: pointer;
  display: flex;
  align-items: center;
  gap: 8px;
  transition: all 0.2s cubic-bezier(0.4, 0, 0.2, 1);
}

.settings-tab-btn:hover {
  background: var(--settings-tab-btn-hover);
  color: var(--text);
}

.settings-tab-btn.active {
  background: var(--sidebar-active);
  border-color: var(--settings-tab-btn-active-border);
  color: var(--text);
  font-weight: 400;
  box-shadow: 0 2px 8px var(--settings-tab-btn-shadow);
  transform: translateX(2px);
}

.settings-close-btn {
  width: 100%;
  margin-top: auto;
  padding: 10px 12px;
  font-size: 0.9rem;
  font-weight: 500;
  color: var(--faint);
  background: transparent;
  border: 1px solid transparent;
  border-radius: 8px;
  text-align: left;
  cursor: pointer;
  display: flex;
  align-items: center;
  gap: 8px;
  transition: all 0.2s ease;
}

.settings-close-btn:hover {
  color: var(--text);
  background: var(--settings-close-btn-hover);
}

.settings-close-btn svg {
  transition: transform 0.2s ease;
}

.settings-close-btn:hover svg {
  transform: translateX(-3px);
}




.settings-main {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  background: var(--settings-main-bg);
}

.settings-main-header {
  border-bottom: 1px solid var(--settings-sidebar-border);
  padding: 0 24px;
}

.settings-main-header-inner {
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 58px;
}

.settings-breadcrumb {
  display: flex;
  align-items: center;
  font-size: 0.95rem;
  font-weight: 400;
  color: var(--muted);
}

.breadcrumb-project {
  display: none;
  color: var(--muted);
  font-weight: 400;
}

.breadcrumb-separator {
  display: none;
  margin: 0 8px;
  color: var(--faint);
  user-select: none;
}

.breadcrumb-chat-title {
  font-weight: 400;
  color: var(--muted);
}



.settings-scroll-area {
  flex: 1;
  overflow-y: auto;
  padding: 24px;
  scrollbar-gutter: stable;
}

.settings-container-wrap {
  width: 100%;
  max-width: 800px;
  margin: 0 auto;
}

/* Shared panel style overrides for tab components */
.settings-scroll-area :deep(.settings-tab-panel) {
  width: 100%;
}

.settings-scroll-area :deep(.settings-section-head) {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 18px;
}

.settings-scroll-area :deep(.settings-section-title) {
  margin: 0 0 4px;
  font-size: 1.1rem;
  font-weight: 400;
  color: var(--muted);
}

.settings-scroll-area :deep(.settings-section-desc) {
  margin: 0;
  font-size: 0.82rem;
  color: var(--muted);
  line-height: 1.5;
  max-width: 460px;
}

.settings-scroll-area :deep(.settings-section-desc code) {
  font-family: monospace;
  background: var(--surface-strong);
  padding: 1px 5px;
  border-radius: 4px;
}

.settings-scroll-area :deep(.settings-empty) {
  padding: 40px 12px;
  text-align: center;
  color: var(--faint);
  font-size: 0.88rem;
  font-style: italic;
}

.settings-scroll-area :deep(.settings-section-title) {
  display: none;
}

/* Transitions */
.settings-fade-enter-active,
.settings-fade-leave-active {
  transition: opacity 0.25s ease;
}

.settings-fade-enter-from,
.settings-fade-leave-to {
  opacity: 0;
}

.settings-fade-enter-active .settings-card {
  animation: scaleUp 0.3s cubic-bezier(0.34, 1.56, 0.64, 1);
}

.settings-fade-leave-active .settings-card {
  animation: scaleDown 0.2s ease-in forwards;
}

@keyframes scaleUp {
  from {
    transform: scale(0.95);
    opacity: 0;
  }
  to {
    transform: scale(1);
    opacity: 1;
  }
}

@keyframes scaleDown {
  from {
    transform: scale(1);
    opacity: 1;
  }
  to {
    transform: scale(0.95);
    opacity: 0;
  }
}

@media (max-width: 760px) {
  .settings-overlay {
    padding: 0;
    backdrop-filter: none;
    background: var(--settings-main-bg);
  }

  .settings-card {
    position: relative;
    width: 100vw;
    height: 100vh;
    height: 100dvh;
    border-radius: 0;
    border: none;
  }

  .settings-sidebar {
    position: absolute;
    left: 0;
    top: 0;
    width: 100vw !important;
    height: 100vh !important;
    height: 100dvh !important;
    z-index: 100;
    background: var(--settings-sidebar-bg);
    padding: 24px 20px;
    border-right: none;
  }

  .settings-sidebar-header {
    display: flex;
    margin-bottom: 24px;
  }

  .settings-sidebar-close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    color: var(--muted);
    cursor: pointer;
    width: 28px;
    height: 28px;
    border-radius: 6px;
    transition: all 0.2s ease;
  }

  .settings-sidebar-close-btn:hover {
    color: var(--text);
    background: var(--settings-sidebar-toggle-hover);
  }

  .settings-tabs-list {
    gap: 8px;
  }

  .settings-tab-btn {
    padding: 12px 14px;
    font-size: 1rem;
  }

  .settings-tab-btn.active {
    transform: none;
  }

  .settings-close-btn {
    padding: 12px 14px;
    font-size: 0.95rem;
    display: flex;
  }

  .settings-main-header {
    padding: 0 16px;
  }

  .settings-main-header-inner {
    height: 48px;
  }

  .settings-scroll-area {
    padding: 16px;
  }

  .settings-sidebar-toggle-btn {
    display: flex;
  }

  .breadcrumb-project,
  .breadcrumb-separator {
    display: inline;
  }
}

@media (max-width: 480px) {
  .settings-tab-btn {
    padding: 10px 12px;
    font-size: 0.95rem;
  }

  .settings-close-btn {
    padding: 10px 12px;
    font-size: 0.9rem;
  }
}
</style>
