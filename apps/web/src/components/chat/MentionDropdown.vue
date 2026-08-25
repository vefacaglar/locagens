<script setup lang="ts">
const props = defineProps<{
  files: string[];
  selectedIndex: number;
  isOpen: boolean;
}>();

const emit = defineEmits<{
  (e: 'select', file: string): void;
  (e: 'close'): void;
}>();

function getFileIcon(file: string): string {
  const ext = file.split('.').pop()?.toLowerCase() || '';
  if (['ts', 'js', 'vue', 'jsx', 'tsx'].includes(ext)) return '⚡';
  if (['json', 'yaml', 'yml', 'toml'].includes(ext)) return '⚙️';
  if (['md', 'txt', 'doc'].includes(ext)) return '📝';
  if (['css', 'scss', 'less'].includes(ext)) return '🎨';
  if (['html', 'svg'].includes(ext)) return '🌐';
  if (['py', 'rb', 'go', 'rs', 'java', 'c', 'cpp'].includes(ext)) return '💻';
  if (['png', 'jpg', 'jpeg', 'gif', 'webp'].includes(ext)) return '🖼️';
  return '📄';
}

function getFileName(file: string): string {
  return file.split('/').pop() || file;
}

function getFileDir(file: string): string {
  const parts = file.split('/');
  parts.pop();
  return parts.join('/');
}
</script>

<template>
  <div v-if="isOpen && files.length > 0" class="mention-dropdown">
    <div class="mention-dropdown-header">
      <span class="mention-header-title">Files (@mention)</span>
      <span class="mention-header-hint">↑↓ navigate · ↵ select · esc dismiss</span>
    </div>
    <div class="mention-list">
      <button
        v-for="(file, idx) in files"
        :key="file"
        type="button"
        class="mention-item"
        :class="{ active: idx === selectedIndex }"
        @mousedown.prevent="emit('select', file)"
      >
        <span class="mention-icon">{{ getFileIcon(file) }}</span>
        <div class="mention-details truncate">
          <span class="mention-name">{{ getFileName(file) }}</span>
          <span v-if="getFileDir(file)" class="mention-dir truncate">{{ getFileDir(file) }}</span>
        </div>
      </button>
    </div>
  </div>
</template>

<style scoped>
.mention-dropdown {
  position: absolute;
  bottom: calc(100% + 8px);
  left: 12px;
  width: calc(100% - 24px);
  max-width: 440px;
  max-height: 240px;
  background: var(--surface, #1e1e1e);
  border: 1px solid var(--border, #333);
  border-radius: 8px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  z-index: 1000;
  backdrop-filter: blur(8px);
}

.mention-dropdown-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 6px 10px;
  background: var(--surface-input, rgba(0, 0, 0, 0.2));
  border-bottom: 1px solid var(--border, #333);
}

.mention-header-title {
  font-size: 11px;
  font-weight: 600;
  color: var(--text-secondary, #888);
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.mention-header-hint {
  font-size: 10px;
  color: var(--text-tertiary, #666);
}

.mention-list {
  display: flex;
  flex-direction: column;
  overflow-y: auto;
  padding: 4px;
  gap: 2px;
}

.mention-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 8px;
  border-radius: 5px;
  background: transparent;
  border: none;
  color: var(--text-primary, #fff);
  cursor: pointer;
  text-align: left;
  transition: background 0.1s ease;
  font-size: 12px;
  width: 100%;
}

.mention-item:hover,
.mention-item.active {
  background: color-mix(in srgb, var(--accent, #648cff) 18%, transparent);
  color: var(--text-primary, #fff);
}

.mention-icon {
  font-size: 13px;
  flex-shrink: 0;
}

.mention-details {
  display: flex;
  align-items: baseline;
  gap: 6px;
  min-width: 0;
}

.mention-name {
  font-weight: 500;
  color: var(--text-primary, #fff);
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
}

.mention-dir {
  font-size: 11px;
  color: var(--text-secondary, #888);
}
</style>
