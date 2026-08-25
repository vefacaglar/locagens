<script setup lang="ts">
import { computed, ref } from 'vue';

const props = defineProps<{
  diff?: string;
  filePath?: string;
  oldContent?: string;
  newContent?: string;
}>();

const copied = ref(false);

interface DiffLine {
  type: 'add' | 'del' | 'context' | 'header';
  oldLineNumber?: number;
  newLineNumber?: number;
  text: string;
}

const parsedDiff = computed(() => {
  const lines: DiffLine[] = [];
  const rawDiff = props.diff || '';
  if (!rawDiff.trim()) return { lines, additions: 0, deletions: 0 };

  const rawLines = rawDiff.split('\n');
  let oldLine = 0;
  let newLine = 0;
  let additions = 0;
  let deletions = 0;

  for (const line of rawLines) {
    if (line.startsWith('@@')) {
      const match = line.match(/@@\s*-(\d+)(?:,\d+)?\s*\+(\d+)(?:,\d+)?\s*@@/);
      if (match) {
        oldLine = parseInt(match[1], 10);
        newLine = parseInt(match[2], 10);
      }
      lines.push({ type: 'header', text: line });
    } else if (line.startsWith('+') && !line.startsWith('+++')) {
      additions++;
      lines.push({
        type: 'add',
        newLineNumber: newLine++,
        text: line.slice(1)
      });
    } else if (line.startsWith('-') && !line.startsWith('---')) {
      deletions++;
      lines.push({
        type: 'del',
        oldLineNumber: oldLine++,
        text: line.slice(1)
      });
    } else {
      // Context or meta line
      if (!line.startsWith('diff --git') && !line.startsWith('index ') && !line.startsWith('--- ') && !line.startsWith('+++ ')) {
        lines.push({
          type: 'context',
          oldLineNumber: oldLine++,
          newLineNumber: newLine++,
          text: line.startsWith(' ') ? line.slice(1) : line
        });
      }
    }
  }

  return { lines, additions, deletions };
});

async function copyDiff() {
  if (!props.diff) return;
  try {
    await navigator.clipboard.writeText(props.diff);
    copied.value = true;
    setTimeout(() => {
      copied.value = false;
    }, 2000);
  } catch {
    /* ignore clipboard error */
  }
}
</script>

<template>
  <div class="diff-viewer">
    <div class="diff-header">
      <div class="diff-file-info">
        <span class="diff-file-icon">📄</span>
        <span class="diff-file-path truncate">{{ filePath || 'Diff Output' }}</span>
        <div class="diff-stats">
          <span v-if="parsedDiff.additions > 0" class="stat-add">+{{ parsedDiff.additions }}</span>
          <span v-if="parsedDiff.deletions > 0" class="stat-del">-{{ parsedDiff.deletions }}</span>
        </div>
      </div>
      <div class="diff-actions">
        <button
          type="button"
          class="diff-btn-copy"
          :title="copied ? 'Copied!' : 'Copy diff'"
          @click="copyDiff"
        >
          {{ copied ? '✓ Copied' : 'Copy' }}
        </button>
      </div>
    </div>

    <!-- Unified Diff Table -->
    <div class="diff-content">
      <table class="diff-table">
        <tbody>
          <tr
            v-for="(line, idx) in parsedDiff.lines"
            :key="idx"
            class="diff-row"
            :class="line.type"
          >
            <td class="diff-ln old-ln">
              {{ line.oldLineNumber || '' }}
            </td>
            <td class="diff-ln new-ln">
              {{ line.newLineNumber || '' }}
            </td>
            <td class="diff-sign">
              <span v-if="line.type === 'add'">+</span>
              <span v-else-if="line.type === 'del'">-</span>
              <span v-else-if="line.type === 'header'">@</span>
            </td>
            <td class="diff-text">
              <pre>{{ line.text }}</pre>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>

<style scoped>
.diff-viewer {
  border: 1px solid var(--border, #30363d);
  border-radius: 8px;
  background: var(--surface, #161b22);
  overflow: hidden;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  margin: 8px 0;
}

.diff-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 6px 12px;
  background: var(--surface-input, rgba(0, 0, 0, 0.25));
  border-bottom: 1px solid var(--border, #30363d);
}

.diff-file-info {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.diff-file-icon {
  font-size: 12px;
}

.diff-file-path {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-primary, #ffffff);
}

.diff-stats {
  display: flex;
  gap: 4px;
  font-size: 11px;
  font-weight: 600;
}

.stat-add {
  color: #3fb950;
}

.stat-del {
  color: #f85149;
}

.diff-actions {
  display: flex;
  gap: 6px;
}

.diff-btn-copy {
  background: transparent;
  border: 1px solid var(--border, #30363d);
  color: var(--text-secondary, #8b949e);
  padding: 2px 8px;
  font-size: 11px;
  border-radius: 4px;
  cursor: pointer;
  transition: all 0.15s ease;
}

.diff-btn-copy:hover {
  background: var(--surface-hover, rgba(255, 255, 255, 0.06));
  color: var(--text-primary, #ffffff);
}

.diff-content {
  max-height: 400px;
  overflow: auto;
}

.diff-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 11px;
  line-height: 1.5;
}

.diff-row.add {
  background: rgba(46, 160, 67, 0.15);
}

.diff-row.del {
  background: rgba(248, 81, 73, 0.15);
}

.diff-row.header {
  background: rgba(56, 139, 253, 0.1);
  color: #58a6ff;
}

.diff-ln {
  width: 38px;
  padding: 0 6px;
  text-align: right;
  color: var(--text-tertiary, #6e7681);
  user-select: none;
  font-size: 10px;
  border-right: 1px solid rgba(255, 255, 255, 0.05);
}

.diff-sign {
  width: 16px;
  text-align: center;
  user-select: none;
  font-weight: 600;
}

.diff-row.add .diff-sign {
  color: #3fb950;
}

.diff-row.del .diff-sign {
  color: #f85149;
}

.diff-text {
  padding: 0 8px;
  white-space: pre;
}

.diff-text pre {
  margin: 0;
  font-family: inherit;
  font-size: inherit;
  color: var(--text-primary, #c9d1d9);
}
</style>
