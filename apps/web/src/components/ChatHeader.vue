<script setup lang="ts">
defineProps<{
  isSidebarCollapsed: boolean;
  showUsageLogsPage: boolean;
  hasActiveRun: boolean;
  currentProjectName: string;
  visibleTitle: string;
  showSidePanelToggle: boolean;
  runningProcessesCount?: number;
}>();

const emit = defineEmits<{
  (e: 'toggle-sidebar'): void;
  (e: 'open-side-panel'): void;
  (e: 'toggle-terminal'): void;
}>();
</script>

<template>
  <header class="chat-header">
    <div class="chat-header-inner">
      <div class="thread-title">
        <button v-if="isSidebarCollapsed" class="panel-toggle-btn expand-sidebar-btn" @click="emit('toggle-sidebar')" title="Expand Sidebar">
          <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <rect width="18" height="18" x="3" y="3" rx="2" />
            <path d="M9 3v18" />
          </svg>
        </button>

        <div v-if="showUsageLogsPage" class="project-breadcrumb">
          <svg class="folder-icon" xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <line x1="18" y1="20" x2="18" y2="10"/>
            <line x1="12" y1="20" x2="12" y2="4"/>
            <line x1="6" y1="20" x2="6" y2="14"/>
          </svg>
          <span class="breadcrumb-project truncate" style="font-weight: 500;">Usage Logs</span>
        </div>
        <div v-else-if="hasActiveRun" class="project-breadcrumb">
          <svg class="folder-icon open-folder" xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="m6 14 1.45-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.55 6a2 2 0 0 1-1.94 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2A2 2 0 0 0 12.07 6H20a2 2 0 0 1 2 2v2"/>
          </svg>
          <span class="breadcrumb-project truncate">{{ currentProjectName }}</span>
          <span class="breadcrumb-separator">/</span>
          <span class="breadcrumb-chat-title truncate">{{ visibleTitle }}</span>
        </div>
      </div>
      <div class="header-actions">
        <button
          class="panel-toggle-btn terminal-toggle-btn"
          :class="{ 'has-running': (runningProcessesCount ?? 0) > 0 }"
          type="button"
          :title="(runningProcessesCount ?? 0) > 0 ? `${runningProcessesCount} server(s) running - Open Terminal` : 'Open Dev Servers & Terminal'"
          @click="emit('toggle-terminal')"
        >
          <span class="terminal-bolt-icon">⚡</span>
          <span v-if="(runningProcessesCount ?? 0) > 0" class="running-count-badge">{{ runningProcessesCount }}</span>
        </button>

        <button
          v-if="showSidePanelToggle"
          class="panel-toggle-btn"
          type="button"
          title="Open side panel"
          @click="emit('open-side-panel')"
        >
          <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <rect width="18" height="18" x="3" y="3" rx="2" />
            <path d="M15 3v18" />
          </svg>
        </button>
      </div>
    </div>
  </header>
</template>

<style scoped>
.chat-header {
  display: block;
  /* Match the side cards' top inset so all three header rows share a baseline. */
  margin-top: var(--shell-inset);
  min-height: 0;
  padding: 0;
  border-bottom: none;
}

.chat-header-inner {
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: var(--top-bar-h);
  width: 100%;
  padding: 0 24px;
  box-sizing: border-box;
}

.thread-title {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 10px;
}

.project-breadcrumb {
  display: flex;
  align-items: center;
  font-size: 0.93rem;
  font-weight: 500;
  color: var(--text);
  min-width: 0;
  flex: 1;
}

.project-breadcrumb .folder-icon {
  margin-right: 8px;
  color: var(--muted);
  flex-shrink: 0;
}

.breadcrumb-project {
  color: var(--breadcrumb-project-color);
  font-weight: 400;
  max-width: 180px;
  flex-shrink: 0;
}

.breadcrumb-separator {
  margin: 0 8px;
  color: var(--faint);
  user-select: none;
  flex-shrink: 0;
}

.breadcrumb-chat-title {
  color: var(--text);
  font-weight: 650;
  flex: 1;
  min-width: 0;
}

.terminal-toggle-btn {
  position: relative;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.terminal-bolt-icon {
  font-size: 13px;
  color: var(--text-secondary);
}

.terminal-toggle-btn.has-running .terminal-bolt-icon {
  color: #3fb950;
}

.running-count-badge {
  position: absolute;
  top: 2px;
  right: 2px;
  min-width: 13px;
  height: 13px;
  padding: 0 3px;
  border-radius: 7px;
  background: #238636;
  color: #ffffff;
  font-size: 9px;
  font-weight: 700;
  display: flex;
  align-items: center;
  justify-content: center;
  line-height: 1;
}
</style>

