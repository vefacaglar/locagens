<script setup lang="ts">
import { ref, watch, nextTick } from 'vue';
import type { ProcessInfo, ProcessLogEntry } from '@locagens/shared';
import ThemedButton from '../ui/ThemedButton.vue';

const props = defineProps<{
  isOpen: boolean;
  processes: ProcessInfo[];
  activeProcess: ProcessInfo | null;
  logs: ProcessLogEntry[];
  isSpawning: boolean;
  error: string | null;
}>();

const emit = defineEmits<{
  (e: 'close'): void;
  (e: 'select', id: string): void;
  (e: 'spawn', command: string): void;
  (e: 'kill', id: string): void;
  (e: 'restart', id: string): void;
}>();

const newCommandInput = ref('');
const autoScroll = ref(true);
const logContainer = ref<HTMLElement | null>(null);

function handleSpawn() {
  const cmd = newCommandInput.value.trim();
  if (!cmd) return;
  emit('spawn', cmd);
  newCommandInput.value = '';
}

function scrollToBottom() {
  if (autoScroll.value && logContainer.value) {
    logContainer.value.scrollTop = logContainer.value.scrollHeight;
  }
}

watch(
  () => props.logs.length,
  () => {
    nextTick(scrollToBottom);
  }
);

function formatTime(iso: string): string {
  try {
    const d = new Date(iso);
    return d.toLocaleTimeString();
  } catch {
    return iso;
  }
}
</script>

<template>
  <div v-if="isOpen" class="terminal-drawer">
    <!-- Top Header -->
    <header class="terminal-header">
      <div class="terminal-header-title-wrap">
        <span class="terminal-icon">⚡</span>
        <span class="terminal-title">Dev Servers & Processes</span>
        <span v-if="processes.length > 0" class="terminal-proc-count">
          {{ processes.filter(p => p.status === 'running').length }} running
        </span>
      </div>
      <div class="terminal-header-actions">
        <button
          type="button"
          class="terminal-close-btn"
          title="Close terminal drawer"
          @click="emit('close')"
        >
          ✕
        </button>
      </div>
    </header>

    <div class="terminal-body">
      <!-- Left Sidebar: Process List & Launch Form -->
      <aside class="terminal-sidebar">
        <form class="terminal-spawn-form" @submit.prevent="handleSpawn">
          <div class="terminal-input-wrap">
            <span class="terminal-prompt-sign">$</span>
            <input
              v-model="newCommandInput"
              type="text"
              class="terminal-cmd-input"
              placeholder="e.g. npm run dev, python server.py"
              :disabled="isSpawning"
            />
          </div>
          <ThemedButton
            variant="primary"
            size="sm"
            type="submit"
            :disabled="isSpawning || !newCommandInput.trim()"
          >
            {{ isSpawning ? 'Starting…' : 'Run' }}
          </ThemedButton>
        </form>

        <p v-if="error" class="terminal-sidebar-error">{{ error }}</p>

        <!-- Process Items -->
        <div class="terminal-proc-list">
          <div
            v-for="p in processes"
            :key="p.id"
            class="terminal-proc-item"
            :class="{ active: activeProcess?.id === p.id }"
            @click="emit('select', p.id)"
          >
            <div class="proc-item-top">
              <span class="proc-status-dot" :class="p.status" />
              <span class="proc-cmd truncate" :title="p.command">{{ p.command }}</span>
            </div>
            <div class="proc-item-meta">
              <span v-if="p.pid" class="proc-pid">PID: {{ p.pid }}</span>
              <span class="proc-time">{{ formatTime(p.startedAt) }}</span>
            </div>
          </div>
          <div v-if="processes.length === 0" class="terminal-empty-list">
            No background processes active. Type a command above to start a server or background process.
          </div>
        </div>
      </aside>

      <!-- Right Main: Live Console Log Output -->
      <main class="terminal-main">
        <div v-if="activeProcess" class="terminal-toolbar">
          <div class="terminal-active-meta truncate">
            <span class="proc-status-dot" :class="activeProcess.status" />
            <strong class="truncate">{{ activeProcess.command }}</strong>
            <span class="proc-badge" :class="activeProcess.status">{{ activeProcess.status }}</span>
          </div>
          <div class="terminal-toolbar-actions">
            <label class="terminal-scroll-toggle">
              <input v-model="autoScroll" type="checkbox" />
              <span>Auto-scroll</span>
            </label>
            <ThemedButton
              v-if="activeProcess.status === 'running'"
              variant="danger"
              size="sm"
              @click="emit('kill', activeProcess.id)"
            >
              Stop (Kill)
            </ThemedButton>
            <ThemedButton
              v-else
              variant="secondary"
              size="sm"
              @click="emit('restart', activeProcess.id)"
            >
              Restart
            </ThemedButton>
          </div>
        </div>

        <!-- Terminal screen -->
        <div ref="logContainer" class="terminal-screen">
          <template v-if="activeProcess && logs.length > 0">
            <div
              v-for="(log, idx) in logs"
              :key="idx"
              class="terminal-log-line"
              :class="log.stream"
            >
              <span class="log-time">{{ formatTime(log.timestamp) }}</span>
              <pre class="log-text">{{ log.text }}</pre>
            </div>
          </template>
          <div v-else-if="activeProcess" class="terminal-waiting">
            Waiting for process output…
          </div>
          <div v-else class="terminal-waiting">
            Select or start a background process to view live console logs.
          </div>
        </div>
      </main>
    </div>
  </div>
</template>

<style scoped>
.terminal-drawer {
  position: fixed;
  bottom: 0;
  left: 0;
  right: 0;
  height: 320px;
  background: var(--terminal-bg);
  border-top: 1px solid var(--terminal-border);
  box-shadow: 0 -8px 30px var(--terminal-shadow);
  display: flex;
  flex-direction: column;
  z-index: 1100;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
}

.terminal-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 6px 14px;
  background: var(--terminal-header-bg);
  border-bottom: 1px solid var(--terminal-border);
}

.terminal-header-title-wrap {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  font-weight: 600;
  color: var(--terminal-text);
}

.terminal-icon {
  font-size: 13px;
  color: var(--terminal-icon);
}

.terminal-proc-count {
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 10px;
  background: var(--terminal-status-running-soft);
  color: var(--terminal-status-running);
  font-weight: 500;
}

.terminal-close-btn {
  background: none;
  border: none;
  color: var(--terminal-text-muted);
  font-size: 14px;
  cursor: pointer;
  padding: 2px 6px;
}

.terminal-close-btn:hover {
  color: var(--terminal-text);
}

.terminal-body {
  display: flex;
  flex: 1;
  min-height: 0;
}

.terminal-sidebar {
  width: 280px;
  border-right: 1px solid var(--terminal-border);
  display: flex;
  flex-direction: column;
  background: var(--terminal-sidebar-bg);
}

.terminal-spawn-form {
  display: flex;
  gap: 6px;
  padding: 8px;
  border-bottom: 1px solid var(--terminal-border);
}

.terminal-input-wrap {
  display: flex;
  align-items: center;
  gap: 4px;
  flex: 1;
  background: var(--terminal-input-bg);
  border: 1px solid var(--terminal-input-border);
  border-radius: 6px;
  padding: 0 6px;
}

.terminal-prompt-sign {
  color: var(--terminal-text-faint);
  font-size: 11px;
}

.terminal-cmd-input {
  width: 100%;
  border: none;
  background: transparent;
  color: var(--terminal-text);
  font-size: 11px;
  font-family: inherit;
  outline: none;
  padding: 5px 0;
}

.terminal-sidebar-error {
  margin: 4px 8px;
  font-size: 10px;
  color: var(--danger);
}

.terminal-proc-list {
  flex: 1;
  overflow-y: auto;
  padding: 4px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.terminal-proc-item {
  padding: 6px 8px;
  border-radius: 6px;
  cursor: pointer;
  display: flex;
  flex-direction: column;
  gap: 3px;
  border: 1px solid transparent;
  transition: all 0.1s ease;
}

.terminal-proc-item:hover {
  background: var(--terminal-hover-bg);
}

.terminal-proc-item.active {
  background: var(--terminal-active-bg);
  border-color: var(--terminal-active-border);
}

.proc-item-top {
  display: flex;
  align-items: center;
  gap: 6px;
}

.proc-cmd {
  font-size: 11px;
  font-weight: 500;
  color: var(--terminal-text);
}

.proc-item-meta {
  display: flex;
  justify-content: space-between;
  font-size: 10px;
  color: var(--terminal-text-muted);
}

.proc-status-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex-shrink: 0;
}

.proc-status-dot.running {
  background: var(--terminal-status-running);
  box-shadow: 0 0 6px var(--terminal-status-running);
}

.proc-status-dot.stopped {
  background: var(--terminal-status-stopped);
}

.proc-status-dot.error {
  background: var(--terminal-status-error);
}

.terminal-empty-list {
  padding: 16px 12px;
  font-size: 11px;
  line-height: 1.4;
  color: var(--terminal-text-faint);
  text-align: center;
}

.terminal-main {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
  background: var(--terminal-screen-bg);
}

.terminal-toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 6px 12px;
  background: var(--terminal-header-bg);
  border-bottom: 1px solid var(--terminal-border);
}

.terminal-active-meta {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 11px;
  color: var(--terminal-text);
}

.proc-badge {
  font-size: 9px;
  font-weight: 600;
  text-transform: uppercase;
  padding: 1px 5px;
  border-radius: 4px;
}

.proc-badge.running {
  background: var(--terminal-status-running-soft);
  color: var(--terminal-status-running);
}

.proc-badge.stopped {
  background: var(--terminal-status-stopped-soft);
  color: var(--terminal-status-stopped);
}

.terminal-toolbar-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.terminal-scroll-toggle {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: 11px;
  color: var(--terminal-text-muted);
  cursor: pointer;
  user-select: none;
}

.terminal-screen {
  flex: 1;
  overflow-y: auto;
  padding: 10px 12px;
  display: flex;
  flex-direction: column;
  gap: 2px;
  font-size: 11px;
  line-height: 1.45;
}

.terminal-log-line {
  display: flex;
  gap: 8px;
  align-items: flex-start;
  white-space: pre-wrap;
  word-break: break-all;
}

.log-time {
  color: var(--log-time);
  font-size: 10px;
  flex-shrink: 0;
  user-select: none;
}

.log-text {
  margin: 0;
  font-family: inherit;
  font-size: inherit;
  white-space: pre-wrap;
  word-break: break-all;
  flex: 1;
}

.terminal-log-line.stdout .log-text {
  color: var(--log-stdout);
}

.terminal-log-line.stderr .log-text {
  color: var(--log-stderr);
}

.terminal-log-line.system .log-text {
  color: var(--log-system);
  font-style: italic;
}

.terminal-waiting {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  color: var(--log-time);
  font-size: 12px;
}
</style>
