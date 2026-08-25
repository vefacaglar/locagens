<script setup lang="ts">
import type { CodeSymbol } from '@locagens/shared';

export interface MentionItem {
  type: 'file' | 'symbol';
  name: string;
  subText: string;
  icon: string;
  kind?: string;
  filePath: string;
  line?: number;
  symbol?: CodeSymbol;
}

const props = defineProps<{
  items: MentionItem[];
  selectedIndex: number;
  isOpen: boolean;
}>();

const emit = defineEmits<{
  (e: 'select', item: MentionItem): void;
  (e: 'close'): void;
}>();
</script>

<template>
  <div v-if="isOpen && items.length > 0" class="mention-dropdown">
    <div class="mention-dropdown-header">
      <span class="mention-header-title">Files & Symbols (@mention)</span>
      <span class="mention-header-hint">↑↓ navigate · ↵ select · esc dismiss</span>
    </div>
    <div class="mention-list">
      <button
        v-for="(item, idx) in items"
        :key="`${item.type}-${item.filePath}-${item.name}-${idx}`"
        type="button"
        class="mention-item"
        :class="{ active: idx === selectedIndex }"
        @mousedown.prevent="emit('select', item)"
      >
        <span class="mention-icon">{{ item.icon }}</span>
        <div class="mention-details truncate">
          <span class="mention-name">{{ item.name }}</span>
          <span v-if="item.kind" class="mention-kind-badge" :class="item.kind">{{ item.kind }}</span>
          <span v-if="item.subText" class="mention-dir truncate">{{ item.subText }}</span>
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
  max-width: 480px;
  max-height: 260px;
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

.mention-kind-badge {
  font-size: 9px;
  font-weight: 600;
  padding: 1px 4px;
  border-radius: 3px;
  text-transform: uppercase;
}

.mention-kind-badge.function {
  background: rgba(160, 100, 255, 0.2);
  color: #b088ff;
}

.mention-kind-badge.class {
  background: rgba(56, 139, 253, 0.2);
  color: #58a6ff;
}

.mention-kind-badge.interface {
  background: rgba(46, 160, 67, 0.2);
  color: #3fb950;
}

.mention-kind-badge.type {
  background: rgba(210, 153, 34, 0.2);
  color: #d29922;
}

.mention-dir {
  font-size: 11px;
  color: var(--text-secondary, #888);
}
</style>
