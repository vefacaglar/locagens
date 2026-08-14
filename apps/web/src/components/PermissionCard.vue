<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import type { PermissionPreview } from '@locagens/shared';
import type { PermissionDecision } from '../api/client';
import {
  PERMISSION_OPTIONS,
  actionLabel,
  actionQuestion,
  previewDiff
} from '../lib/permission';
import DiffView from './DiffView.vue';

const props = defineProps<{
  request: any;
}>();

const emit = defineEmits<{
  (e: 'decide', decision: PermissionDecision): void;
}>();

const selected = ref(0);

const preview = computed<PermissionPreview | null>(() => props.request?.preview ?? null);
const toolName = computed<string>(() => props.request?.toolCall?.function?.name ?? 'tool');

// For run_command, a grant matches by prefix: approving this command also covers
// any command that starts with it (e.g. "go build ./x/" -> "go build ./x/...").
// Relabel the "don't ask again" options to make that scope clear.
const options = computed(() => {
  if (toolName.value === 'run_command') {
    return PERMISSION_OPTIONS.map((o) => {
      if (o.decision === 'allow_project') return { ...o, label: 'Yes, and allow commands starting with this in this project' };
      if (o.decision === 'allow_always') return { ...o, label: 'Yes, and allow commands starting with this globally' };
      return o;
    });
  }
  if (toolName.value === 'search_web') {
    return PERMISSION_OPTIONS.map((o) => {
      if (o.decision === 'allow_project') return { ...o, label: 'Yes, and allow this search in this project' };
      if (o.decision === 'allow_always') return { ...o, label: 'Yes, and allow this search globally' };
      return o;
    });
  }
  if (toolName.value === 'fetch_url') {
    // fetch_url grants are scoped to the host, so a new host still asks while an
    // approved host runs silently — relabel the "don't ask again" options.
    return PERMISSION_OPTIONS.map((o) => {
      if (o.decision === 'allow_project') return { ...o, label: 'Yes, and allow this host in this project' };
      if (o.decision === 'allow_always') return { ...o, label: 'Yes, and allow this host globally' };
      return o;
    });
  }
  return PERMISSION_OPTIONS;
});
const headerPath = computed(() => preview.value?.path || preview.value?.absolutePath || '');
const question = computed(() => actionQuestion(preview.value, toolName.value));
const diffRows = computed(() => previewDiff(preview.value));
const hasDiff = computed(() => diffRows.value.length > 0);

// For non-file tools, show the most relevant detail instead of a path.
const previewDetail = computed(() => {
  const p = preview.value;
  if (!p) return '';
  if (p.action === 'command') return p.command ?? '';
  if (p.action === 'fetch') return p.url ?? '';
  if (p.action === 'move') return `${p.path || p.absolutePath} → ${p.destPath ?? ''}`;
  if (p.action === 'search') return p.query ?? '';
  return p.absolutePath;
});

// Raw arguments fallback when there is no structured preview (unknown tools).
const rawArgs = computed(() => {
  const args = props.request?.toolCall?.function?.arguments;
  if (!args) return '';
  try {
    return JSON.stringify(JSON.parse(args), null, 2);
  } catch {
    return args;
  }
});

function decide(decision: PermissionDecision) {
  emit('decide', decision);
}

// Reset the highlighted option whenever a new request appears.
watch(() => props.request, (req) => {
  if (req) selected.value = 0;
});

function onKeyDown(e: KeyboardEvent) {
  if (!props.request) return;

  if (e.key === 'Escape') {
    e.preventDefault();
    decide('deny');
  } else if (e.key === 'ArrowDown') {
    e.preventDefault();
    selected.value = (selected.value + 1) % options.value.length;
  } else if (e.key === 'ArrowUp') {
    e.preventDefault();
    selected.value = (selected.value - 1 + options.value.length) % options.value.length;
  } else if (e.key === 'Enter') {
    e.preventDefault();
    decide(options.value[selected.value].decision);
  } else if (/^[1-9]$/.test(e.key)) {
    e.preventDefault();
    const option = options.value[Number(e.key) - 1];
    if (option) decide(option.decision);
  }
}

onMounted(() => window.addEventListener('keydown', onKeyDown, true));
onBeforeUnmount(() => window.removeEventListener('keydown', onKeyDown, true));
</script>

<template>
  <transition name="slide-up">
    <div v-if="request" class="prompt-card prompt-card--floating cc-permission-card">
      <header class="cc-perm-header">
        <span class="cc-perm-action">{{ actionLabel(preview?.action) }}</span>
        <span v-if="headerPath" class="cc-perm-path truncate">{{ headerPath }}</span>
        <span class="cc-perm-tool">{{ toolName }}</span>
      </header>

      <!-- File diff (edit / create / delete) -->
      <div v-if="hasDiff" class="cc-diff-scroll">
        <DiffView :rows="diffRows" />
      </div>

      <!-- Detail preview (read / list / mkdir / move / search / command) -->
      <div v-else-if="preview" class="cc-perm-pathbox">
        <code>{{ previewDetail }}</code>
      </div>

      <!-- Fallback for tools without a structured preview -->
      <div v-else-if="rawArgs" class="cc-perm-pathbox">
        <code>{{ rawArgs }}</code>
      </div>

      <p class="cc-perm-question">{{ question }}</p>

      <ul class="cc-perm-options">
        <li
          v-for="(option, idx) in options"
          :key="option.decision"
          class="cc-perm-option"
          :class="{ active: idx === selected }"
          @mouseenter="selected = idx"
          @click="decide(option.decision)"
        >
          <span class="cc-perm-number">{{ idx + 1 }}.</span>
          <span class="cc-perm-label">{{ option.label }}</span>
          <span v-if="option.hint" class="cc-perm-hint">{{ option.hint }}</span>
        </li>
      </ul>
    </div>
  </transition>
</template>

<style scoped>
/* Shell + positioning come from the global .prompt-card / .prompt-card--floating;
   this card keeps its slightly stronger shadow. */
.cc-permission-card {
  padding: 14px;
  gap: 10px;
  box-shadow: 0 14px 36px var(--permission-card-shadow);
}

.cc-perm-header {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 0.82rem;
}

.cc-perm-action {
  font-weight: 600;
  color: var(--text);
  background: var(--surface-strong);
  padding: 2px 8px;
  border-radius: 6px;
}

.cc-perm-path {
  font-family: monospace;
  color: var(--muted);
}

.cc-perm-tool {
  margin-left: auto;
  font-family: monospace;
  font-size: 0.72rem;
  color: var(--faint);
}

/* Diff rendering is shared with the review panel (DiffView); this wrapper only
   caps the height so long diffs scroll inside the card. */
.cc-diff-scroll {
  max-height: 280px;
  overflow: auto;
}

.cc-perm-pathbox {
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 8px 12px;
  /* Keep long commands/args contained: a small box that scrolls inside itself
     instead of stretching the whole card and pushing the options off-screen. */
  max-height: 180px;
  overflow: auto;
}

.cc-perm-pathbox code {
  font-family: monospace;
  font-size: 0.78rem;
  color: var(--muted);
  white-space: pre-wrap;
  word-break: break-all;
}

.cc-perm-question {
  margin: 2px 0 0;
  font-size: 0.88rem;
  color: var(--text);
}

/* Numbered options */
.cc-perm-options {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.cc-perm-option {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 10px;
  border-radius: 6px;
  cursor: pointer;
  font-size: 0.85rem;
  color: var(--muted);
  border-left: 2px solid transparent;
  transition: background 0.12s ease, color 0.12s ease, border-color 0.12s ease;
}

.cc-perm-option.active {
  background: var(--surface-strong);
  color: var(--text);
  border-left-color: var(--success);
}

.cc-perm-number {
  color: var(--faint);
  font-variant-numeric: tabular-nums;
}

.cc-perm-option.active .cc-perm-number {
  color: var(--muted);
}

.cc-perm-label {
  flex: 1 1 auto;
}

.cc-perm-hint {
  font-family: monospace;
  font-size: 0.72rem;
  color: var(--muted);
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 1px 6px;
}
</style>
