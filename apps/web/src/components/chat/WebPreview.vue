<script setup lang="ts">
import { ref, computed, onMounted, onBeforeUnmount } from 'vue';
import ThemedButton from '../ui/ThemedButton.vue';
import type { ProcessInfo } from '@locagens/shared';

const props = defineProps<{
  initialUrl?: string;
  initialHtml?: string;
  runningProcesses?: ProcessInfo[];
}>();

const previewMode = ref<'url' | 'html'>('url');
const urlInput = ref(props.initialUrl || 'http://localhost:3000');
const currentIframeUrl = ref(props.initialUrl || 'http://localhost:3000');
const htmlSource = ref(props.initialHtml || '<!DOCTYPE html>\n<html>\n<head>\n  <style>\n    body { font-family: system-ui, sans-serif; padding: 2rem; background: #f9f9f9; text-align: center; }\n    h1 { color: #3b82f6; }\n    .card { background: white; padding: 2rem; border-radius: 12px; box-shadow: 0 4px 12px rgba(0,0,0,0.08); max-width: 400px; margin: 0 auto; }\n  </style>\n</head>\n<body>\n  <div class="card">\n    <h1>✨ Live Web Preview</h1>\n    <p>Render components, HTML artifacts, or local dev servers live.</p>\n  </div>\n</body>\n</html>');

const deviceMode = ref<'desktop' | 'tablet' | 'mobile'>('desktop');
const iframeKey = ref(0);
const consoleLogs = ref<Array<{ type: 'log' | 'error' | 'warn'; text: string; time: string }>>([]);
const showConsole = ref(false);

const detectedServerUrls = computed(() => {
  const list: string[] = [];
  if (!props.runningProcesses) return list;

  for (const proc of props.runningProcesses) {
    if (proc.status !== 'running') continue;
    const match = proc.command.match(/--port\s+(\d+)|-p\s+(\d+)|:(\d{4,5})/);
    if (match) {
      const port = match[1] || match[2] || match[3];
      list.push(`http://localhost:${port}`);
    }
  }
  return list;
});

function applyUrl() {
  let url = urlInput.value.trim();
  if (!url) return;
  if (!url.startsWith('http://') && !url.startsWith('https://')) {
    url = `http://${url}`;
  }
  currentIframeUrl.value = url;
  iframeKey.value++;
}

function refreshIframe() {
  iframeKey.value++;
}

function openInNewTab() {
  if (previewMode.value === 'url') {
    window.open(currentIframeUrl.value, '_blank');
  } else {
    const blob = new Blob([htmlSource.value], { type: 'text/html' });
    const blobUrl = URL.createObjectURL(blob);
    window.open(blobUrl, '_blank');
  }
}

// Injected script to capture console logs from iframe
const injectedSrcDoc = computed(() => {
  const hookScript = `
    <script>
      (function() {
        function send(type, args) {
          try {
            var msg = Array.from(args).map(function(a) {
              return typeof a === 'object' ? JSON.stringify(a) : String(a);
            }).join(' ');
            window.parent.postMessage({ source: 'locagens-preview-console', type: type, text: msg }, '*');
          } catch(e) {}
        }
        var oldLog = console.log;
        console.log = function() { send('log', arguments); oldLog.apply(console, arguments); };
        var oldErr = console.error;
        console.error = function() { send('error', arguments); oldErr.apply(console, arguments); };
        var oldWarn = console.warn;
        console.warn = function() { send('warn', arguments); oldWarn.apply(console, arguments); };
      })();
    <\/script>
  `;
  return hookScript + htmlSource.value;
});

function handleMessage(e: MessageEvent) {
  if (e.data?.source === 'locagens-preview-console') {
    consoleLogs.value.push({
      type: e.data.type || 'log',
      text: e.data.text || '',
      time: new Date().toLocaleTimeString()
    });
    if (consoleLogs.value.length > 200) consoleLogs.value.shift();
  }
}

onMounted(() => {
  window.addEventListener('message', handleMessage);
});

onBeforeUnmount(() => {
  window.removeEventListener('message', handleMessage);
});
</script>

<template>
  <div class="web-preview-container">
    <!-- Top Control Bar -->
    <header class="preview-toolbar">
      <div class="preview-mode-switch">
        <button
          type="button"
          class="preview-tab-btn"
          :class="{ active: previewMode === 'url' }"
          @click="previewMode = 'url'"
        >
          Dev Server / URL
        </button>
        <button
          type="button"
          class="preview-tab-btn"
          :class="{ active: previewMode === 'html' }"
          @click="previewMode = 'html'"
        >
          HTML Code
        </button>
      </div>

      <!-- URL Mode Address Bar -->
      <div v-if="previewMode === 'url'" class="preview-address-bar">
        <input
          v-model="urlInput"
          type="text"
          class="preview-url-input"
          placeholder="http://localhost:3000, 5173, 8080…"
          @keydown.enter="applyUrl"
        />
        <ThemedButton variant="secondary" size="sm" @click="applyUrl">
          Go
        </ThemedButton>
      </div>

      <!-- Quick Detected Servers -->
      <div v-if="detectedServerUrls.length > 0 && previewMode === 'url'" class="detected-urls">
        <button
          v-for="u in detectedServerUrls"
          :key="u"
          type="button"
          class="detected-url-pill"
          @click="urlInput = u; applyUrl()"
        >
          {{ u }}
        </button>
      </div>

      <!-- Device Frame Controls -->
      <div class="preview-device-controls">
        <button
          type="button"
          class="device-btn"
          :class="{ active: deviceMode === 'desktop' }"
          title="Desktop (100%)"
          @click="deviceMode = 'desktop'"
        >
          🖥️
        </button>
        <button
          type="button"
          class="device-btn"
          :class="{ active: deviceMode === 'tablet' }"
          title="Tablet (768px)"
          @click="deviceMode = 'tablet'"
        >
          📱
        </button>
        <button
          type="button"
          class="device-btn"
          :class="{ active: deviceMode === 'mobile' }"
          title="Mobile (375px)"
          @click="deviceMode = 'mobile'"
        >
          📲
        </button>
      </div>

      <!-- Actions -->
      <div class="preview-actions">
        <button
          type="button"
          class="preview-icon-btn"
          title="Refresh preview"
          @click="refreshIframe"
        >
          🔄
        </button>
        <button
          type="button"
          class="preview-icon-btn"
          title="Open in new window"
          @click="openInNewTab"
        >
          ↗️
        </button>
        <button
          type="button"
          class="preview-icon-btn"
          :class="{ active: showConsole }"
          title="Toggle browser console logs"
          @click="showConsole = !showConsole"
        >
          🪵
        </button>
      </div>
    </header>

    <!-- Main Workspace -->
    <div class="preview-viewport-wrap" :class="deviceMode">
      <div class="preview-frame-container">
        <!-- URL Iframe -->
        <iframe
          v-if="previewMode === 'url'"
          :key="`url-${iframeKey}`"
          :src="currentIframeUrl"
          class="preview-iframe"
          sandbox="allow-scripts allow-same-origin allow-forms allow-popups"
        />

        <!-- Raw HTML Iframe -->
        <iframe
          v-else
          :key="`html-${iframeKey}`"
          :srcdoc="injectedSrcDoc"
          class="preview-iframe"
          sandbox="allow-scripts allow-same-origin allow-forms allow-popups"
        />
      </div>
    </div>

    <!-- Console Drawer -->
    <div v-if="showConsole" class="preview-console">
      <div class="preview-console-head">
        <span>Console Output</span>
        <button type="button" class="preview-console-clear" @click="consoleLogs = []">
          Clear
        </button>
      </div>
      <div class="preview-console-body">
        <div
          v-for="(log, idx) in consoleLogs"
          :key="idx"
          class="console-log-row"
          :class="log.type"
        >
          <span class="console-time">{{ log.time }}</span>
          <span class="console-text">{{ log.text }}</span>
        </div>
        <div v-if="consoleLogs.length === 0" class="console-empty">
          No console logs captured yet.
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.web-preview-container {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--preview-bg);
  overflow: hidden;
  font-family: inherit;
}

.preview-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  background: var(--preview-toolbar-bg);
  border-bottom: 1px solid var(--preview-border);
  flex-wrap: wrap;
}

.preview-mode-switch {
  display: flex;
  border: 1px solid var(--preview-border);
  border-radius: 6px;
  overflow: hidden;
}

.preview-tab-btn {
  background: transparent;
  border: none;
  color: var(--preview-text-muted);
  padding: 4px 8px;
  font-size: 11px;
  cursor: pointer;
  transition: all 0.15s ease;
}

.preview-tab-btn.active {
  background: var(--preview-tab-active-bg);
  color: var(--preview-tab-active-color);
  font-weight: 600;
}

.preview-address-bar {
  display: flex;
  align-items: center;
  gap: 4px;
  flex: 1;
  min-width: 160px;
}

.preview-url-input {
  flex: 1;
  background: var(--preview-input-bg);
  border: 1px solid var(--preview-input-border);
  border-radius: 6px;
  color: var(--preview-text);
  font-size: 11px;
  padding: 5px 8px;
  outline: none;
}

.detected-urls {
  display: flex;
  gap: 4px;
}

.detected-url-pill {
  background: var(--preview-chip-bg);
  border: 1px solid var(--preview-chip-border);
  color: var(--preview-text);
  font-size: 10px;
  padding: 2px 6px;
  border-radius: 10px;
  cursor: pointer;
}

.preview-device-controls {
  display: flex;
  gap: 2px;
  border: 1px solid var(--preview-border);
  border-radius: 6px;
  padding: 2px;
}

.device-btn {
  background: transparent;
  border: none;
  padding: 2px 6px;
  font-size: 12px;
  cursor: pointer;
  border-radius: 4px;
  transition: background 0.1s ease;
}

.device-btn.active {
  background: var(--preview-hover-bg);
}

.preview-actions {
  display: flex;
  align-items: center;
  gap: 4px;
}

.preview-icon-btn {
  background: transparent;
  border: 1px solid var(--preview-border);
  border-radius: 6px;
  padding: 4px 6px;
  font-size: 11px;
  cursor: pointer;
  color: var(--preview-text);
  transition: all 0.15s ease;
}

.preview-icon-btn:hover,
.preview-icon-btn.active {
  background: var(--preview-hover-bg);
}

.preview-viewport-wrap {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--preview-viewport-bg);
  overflow: auto;
  padding: 12px;
}

.preview-frame-container {
  width: 100%;
  height: 100%;
  background: var(--preview-viewport-paper);
  border-radius: 6px;
  overflow: hidden;
  box-shadow: 0 8px 30px rgba(0, 0, 0, 0.4);
  transition: width 0.25s ease, height 0.25s ease;
}

.preview-viewport-wrap.tablet .preview-frame-container {
  width: 768px;
  height: 1024px;
  max-height: 100%;
}

.preview-viewport-wrap.mobile .preview-frame-container {
  width: 375px;
  height: 667px;
  max-height: 100%;
}

.preview-iframe {
  width: 100%;
  height: 100%;
  border: none;
  display: block;
}

.preview-console {
  height: 140px;
  border-top: 1px solid var(--preview-border);
  background: var(--preview-console-bg);
  display: flex;
  flex-direction: column;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
}

.preview-console-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 4px 10px;
  background: var(--preview-console-head-bg);
  font-size: 10px;
  font-weight: 600;
  color: var(--preview-text-muted);
  text-transform: uppercase;
}

.preview-console-clear {
  background: none;
  border: none;
  color: var(--preview-text-muted);
  font-size: 10px;
  cursor: pointer;
}

.preview-console-clear:hover {
  color: var(--preview-text);
}

.preview-console-body {
  flex: 1;
  overflow-y: auto;
  padding: 6px 10px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.console-log-row {
  display: flex;
  gap: 8px;
  font-size: 11px;
  line-height: 1.4;
}

.console-log-row.error {
  color: var(--log-stderr);
}

.console-log-row.warn {
  color: var(--log-warn);
}

.console-time {
  color: var(--log-time);
  font-size: 10px;
}

.console-empty {
  color: var(--log-time);
  font-size: 11px;
  text-align: center;
  padding: 20px 0;
}
</style>
