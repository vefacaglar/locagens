<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import type { DiffRow } from '../lib/diff';

// Renders a unified diff from the FULL (unfiltered) lineDiff rows. Unchanged
// runs are collapsed behind clickable "show hidden lines" separators, and
// paired del/add lines get character-level inline highlighting of the segment
// that actually changed — matching editor-style review views.
const props = defineProps<{
  rows: DiffRow[];
  contextSize?: number;
}>();

interface Segment {
  text: string;
  hl: boolean;
}

type DisplayRow =
  | { kind: 'row'; index: number; row: DiffRow; segments: Segment[] | null }
  | { kind: 'separator'; start: number; end: number; count: number; expanded: boolean };

const DEFAULT_CONTEXT = 3;

/**
 * Character-level diff of a del/add line pair via common prefix/suffix. Returns
 * null when the lines are too dissimilar for an inline highlight to help.
 */
function charDiffSegments(oldText: string, newText: string): { del: Segment[]; add: Segment[] } | null {
  if (oldText === newText || (!oldText && !newText)) return null;

  const minLen = Math.min(oldText.length, newText.length);
  let prefix = 0;
  while (prefix < minLen && oldText[prefix] === newText[prefix]) prefix++;
  let suffix = 0;
  while (suffix < minLen - prefix && oldText[oldText.length - 1 - suffix] === newText[newText.length - 1 - suffix]) suffix++;

  const common = prefix + suffix;
  // Only highlight when the pair shares a meaningful amount of content;
  // otherwise the whole line is effectively different and row colors suffice.
  if (common === 0 || common < Math.max(oldText.length, newText.length) * 0.3) return null;

  const toSegments = (text: string): Segment[] => {
    const mid = text.slice(prefix, text.length - suffix);
    const segments: Segment[] = [];
    if (prefix) segments.push({ text: text.slice(0, prefix), hl: false });
    if (mid) segments.push({ text: mid, hl: true });
    if (suffix) segments.push({ text: text.slice(text.length - suffix), hl: false });
    return segments;
  };

  return { del: toSegments(oldText), add: toSegments(newText) };
}

/** Pairs each run of deletions with the run of additions that follows it. */
const segmentsByIndex = computed<Map<number, Segment[]>>(() => {
  const map = new Map<number, Segment[]>();
  const rows = props.rows;
  let i = 0;
  while (i < rows.length) {
    if (rows[i].type !== 'del') {
      i++;
      continue;
    }
    const delStart = i;
    while (i < rows.length && rows[i].type === 'del') i++;
    const addStart = i;
    while (i < rows.length && rows[i].type === 'add') i++;
    const pairCount = Math.min(addStart - delStart, i - addStart);
    for (let k = 0; k < pairCount; k++) {
      const pair = charDiffSegments(rows[delStart + k].text, rows[addStart + k].text);
      if (pair) {
        map.set(delStart + k, pair.del);
        map.set(addStart + k, pair.add);
      }
    }
  }
  return map;
});

// Hidden ranges the user has expanded, keyed "start-end"; reset per new diff.
const expandedRanges = ref(new Set<string>());
watch(() => props.rows, () => {
  expandedRanges.value = new Set();
});

const displayRows = computed<DisplayRow[]>(() => {
  const rows = props.rows;
  const context = props.contextSize ?? DEFAULT_CONTEXT;
  const n = rows.length;

  const keep = new Array<boolean>(n).fill(false);
  for (let i = 0; i < n; i++) {
    if (rows[i].type === 'add' || rows[i].type === 'del') {
      for (let k = Math.max(0, i - context); k <= Math.min(n - 1, i + context); k++) {
        keep[k] = true;
      }
    }
  }

  const result: DisplayRow[] = [];
  let i = 0;
  while (i < n) {
    if (keep[i]) {
      result.push({ kind: 'row', index: i, row: rows[i], segments: segmentsByIndex.value.get(i) ?? null });
      i++;
      continue;
    }
    let end = i;
    while (end < n && !keep[end]) end++;
    const expanded = expandedRanges.value.has(`${i}-${end}`);
    result.push({ kind: 'separator', start: i, end, count: end - i, expanded });
    if (expanded) {
      for (let k = i; k < end; k++) {
        result.push({ kind: 'row', index: k, row: rows[k], segments: null });
      }
    }
    i = end;
  }
  return result;
});

function toggleRange(separator: { start: number; end: number; expanded: boolean }) {
  const key = `${separator.start}-${separator.end}`;
  const next = new Set(expandedRanges.value);
  if (separator.expanded) next.delete(key);
  else next.add(key);
  expandedRanges.value = next;
}
</script>

<template>
  <div class="diff-view">
    <template v-for="entry in displayRows" :key="entry.kind === 'separator' ? `sep-${entry.start}` : `row-${entry.index}`">
      <button
        v-if="entry.kind === 'separator'"
        type="button"
        class="diff-separator"
        :title="entry.expanded ? 'Collapse unmodified lines' : 'Expand unmodified lines'"
        @click="toggleRange(entry)"
      >
        <span class="diff-separator-chevron" :class="{ expanded: entry.expanded }">
          <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
            <path d="m3.5 6 4.5 4.5L12.5 6" />
          </svg>
        </span>
        <span class="diff-separator-label">{{ entry.count }} unmodified {{ entry.count === 1 ? 'line' : 'lines' }}</span>
      </button>

      <div v-else class="diff-row" :class="entry.row.type">
        <span class="diff-line old">{{ entry.row.oldNo ?? '' }}</span>
        <span class="diff-line new">{{ entry.row.newNo ?? '' }}</span>
        <span class="diff-mark">{{ entry.row.type === 'add' ? '+' : entry.row.type === 'del' ? '-' : ' ' }}</span>
        <code v-if="entry.segments">
          <span
            v-for="(segment, si) in entry.segments"
            :key="si"
            :class="segment.hl ? `inline-hl inline-${entry.row.type}` : undefined"
          >{{ segment.text }}</span>
        </code>
        <code v-else>{{ entry.row.text || ' ' }}</code>
      </div>
    </template>
  </div>
</template>

<style scoped>
.diff-view {
  border: 1px solid var(--border);
  border-radius: 8px;
  overflow: hidden;
  background: var(--bg);
}

.diff-row,
.diff-line,
.diff-mark {
  border-radius: 0 !important;
}

.diff-row {
  display: grid;
  grid-template-columns: 42px 42px 18px minmax(0, 1fr);
  min-height: 24px;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.76rem;
  line-height: 1.45;
}

.diff-row.add {
  background: var(--diff-add-bg);
}

.diff-row.del {
  background: var(--diff-del-bg);
}

.diff-line,
.diff-mark {
  color: var(--faint);
  user-select: none;
  text-align: right;
  padding: 3px 6px;
  border-right: 1px solid var(--composer-menu-border);
}

.diff-mark {
  text-align: center;
}

.diff-row code {
  min-width: 0;
  display: block;
  padding: 3px 8px;
  color: var(--text);
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.inline-hl {
  border-radius: 2px;
}

.inline-hl.inline-add {
  background: var(--diff-add-hl);
}

.inline-hl.inline-del {
  background: var(--diff-del-hl);
}

.diff-separator {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 10px;
  border: none;
  border-top: 1px solid var(--border-soft);
  border-bottom: 1px solid var(--border-soft);
  border-radius: 0 !important;
  background: var(--shimmer-glow);
  color: var(--muted);
  font-family: inherit;
  font-size: 0.76rem;
  font-weight: 500;
  cursor: pointer;
  user-select: none;
  text-align: left;
  transition: background 0.15s ease;
}

.diff-separator:hover {
  background: var(--control-bg-hover);
  color: var(--text);
}

.diff-separator-chevron {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  transition: transform 0.2s ease;
}

.diff-separator-chevron.expanded {
  transform: rotate(180deg);
}
</style>
