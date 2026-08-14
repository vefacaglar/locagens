<script setup lang="ts">
import { ref, computed, watch, nextTick } from 'vue';
import type { MessageGroup } from '../lib/messageGroups';
import { renderMarkdown, cleanMessageContent } from '../lib/markdown';
import { capturePreScrollStates, restorePreScrollStates } from '../lib/domScroll';
import ToolGroup from './ToolGroup.vue';
import ReasoningPanel from './ReasoningPanel.vue';

const props = defineProps<{
  children: MessageGroup[];
  active?: boolean;
  /** The sub-agent's name (its delegated task title), shown in the header. */
  title?: string;
}>();

const emit = defineEmits<{
  (e: 'open-plan'): void;
  (e: 'view-file-in-review', path: string): void;
}>();

const expanded = ref(false);

// Sub-agent role drives the badge: coder vs the lighter utility tier.
const badgeLabel = computed(() =>
  props.children[0]?.message.agentRole === 'utility' ? 'Utility' : 'Coder'
);

// The sub-agent model label, taken from the first child that carries one.
const model = computed(() => props.children.find(c => c.message.model)?.message.model ?? '');

// Number of tool steps the sub-agent(s) ran, for a compact summary in the header.
const stepCount = computed(() =>
  props.children.reduce((n, c) => n + (c.type === 'tool_group' ? Math.max(c.toolCalls.length, 1) : 0), 0)
);

// Reasoning collapse state, local to this box (newest open, older collapsed).
const expandedReasoning = ref<Record<string, boolean>>({});
const latestReasoningId = computed(() => {
  for (let i = props.children.length - 1; i >= 0; i--) {
    if (props.children[i].message.reasoningContent) return props.children[i].id;
  }
  return null;
});
function isReasoningExpanded(id: string): boolean {
  const explicit = expandedReasoning.value[id];
  if (explicit !== undefined) return explicit;
  return id === latestReasoningId.value;
}
function toggleReasoning(id: string) {
  expandedReasoning.value[id] = !isReasoningExpanded(id);
}
watch(latestReasoningId, (newId, oldId) => {
  if (oldId && oldId !== newId) expandedReasoning.value[oldId] = false;
  if (newId) expandedReasoning.value[newId] = true;
});

// Standalone tool messages carry their JSON result as `content`; never surface
// that as a thought blob — the ToolGroup accordion shows a trimmed result.
function thoughtFor(child: MessageGroup): string {
  return child.message.role === 'tool' ? '' : child.message.content;
}

// Keep the latest sub-agent output in view while it streams.
const bodyEl = ref<HTMLElement | null>(null);
const stickyBottomThreshold = 24;

function isAtBottom(el: HTMLElement, threshold = stickyBottomThreshold) {
  return el.scrollHeight - el.scrollTop - el.clientHeight <= threshold;
}

watch(() => props.children.map(c =>
  `${c.id}:${c.message.content.length}:${c.message.reasoningContent?.length ?? 0}`
).join('|'), () => {
  if (!props.active || !expanded.value || !bodyEl.value) return;

  const wasAtBottom = isAtBottom(bodyEl.value);
  const prevScrollTop = bodyEl.value.scrollTop;
  const preScrollStates = capturePreScrollStates(bodyEl.value);

  nextTick(() => {
    if (bodyEl.value) {
      if (wasAtBottom) {
        bodyEl.value.scrollTop = bodyEl.value.scrollHeight;
      } else {
        bodyEl.value.scrollTop = prevScrollTop;
      }
      restorePreScrollStates(bodyEl.value, preScrollStates);
    }
  });
});

watch(expanded, (isExpanded) => {
  if (isExpanded) {
    nextTick(() => {
      if (bodyEl.value) {
        bodyEl.value.scrollTop = bodyEl.value.scrollHeight;
      }
    });
  }
});
</script>

<template>
  <div class="coder-group" :class="{ 'is-active': active }">
    <header class="coder-group-header" @click="expanded = !expanded">
      <div class="coder-group-head-left">
        <span class="agent-badge coder-badge">{{ badgeLabel }}</span>
        <span v-if="title" class="coder-group-title truncate">{{ title }}</span>
        <span v-if="model" class="coder-group-model truncate">{{ model }}</span>
        <span v-if="active" class="coder-group-status">working…</span>
        <span v-else-if="stepCount" class="coder-group-steps">{{ stepCount }} step{{ stepCount === 1 ? '' : 's' }}</span>
      </div>
      <button class="coder-group-toggle">{{ expanded ? 'Hide' : 'Show' }}</button>
    </header>

    <div v-if="expanded" ref="bodyEl" class="coder-group-body">
      <template v-for="child in children" :key="child.id">
        <ReasoningPanel
          v-if="child.message.reasoningContent"
          :content="child.message.reasoningContent"
          :expanded="isReasoningExpanded(child.id)"
          @toggle="toggleReasoning(child.id)"
        />

        <ToolGroup
          v-if="child.type === 'tool_group'"
          :thought="thoughtFor(child)"
          :tool-calls="child.toolCalls"
          :tool-responses="child.toolResponses"
          @open-plan="emit('open-plan')"
          @view-file-in-review="emit('view-file-in-review', $event)"
        />

        <div
          v-else-if="child.type === 'assistant' && cleanMessageContent(child.message.content)"
          class="coder-text markdown-body"
          v-html="renderMarkdown(cleanMessageContent(child.message.content), child.message.id)"
        ></div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.coder-group {
  margin: 1px 0;
  border: 1px solid var(--border);
  border-left: 2px solid var(--muted);
  border-radius: 10px;
  background: var(--surface);
  overflow: hidden;
}

.coder-group.is-active {
  border-left-color: var(--planner);
}

.coder-group-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 8px 12px;
  cursor: pointer;
  user-select: none;
}

.coder-group-head-left {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.agent-badge.coder-badge {
  font-size: 0.66rem;
  font-weight: 600;
  letter-spacing: 0.04em;
  text-transform: uppercase;
  color: var(--text);
  background: var(--surface-strong);
  border: 1px solid var(--border);
  border-radius: 5px;
  padding: 1px 6px;
}

.coder-group-title {
  font-size: 0.8rem;
  font-weight: 600;
  color: var(--text);
  min-width: 0;
}

.coder-group-model {
  font-size: 0.72rem;
  color: var(--faint);
  font-family: monospace;
  flex-shrink: 0;
}

.coder-group-status {
  font-size: 0.72rem;
  color: var(--muted);
  font-style: italic;
}

.coder-group-steps {
  font-size: 0.72rem;
  color: var(--faint);
  font-family: monospace;
}

.coder-group-toggle {
  flex-shrink: 0;
  background: transparent;
  color: var(--faint);
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 2px 8px;
  font-size: 0.72rem;
  cursor: pointer;
  transition: all 0.2s ease;
}

.coder-group-toggle:hover {
  color: var(--text);
  background: var(--perm-code-bg);
}

.coder-group-body {
  max-height: 340px;
  overflow-y: auto;
  padding: 4px 14px 12px;
  border-top: 1px solid var(--border);
  scroll-behavior: smooth;
}

.coder-text {
  color: var(--msg-thought-text);
  line-height: 1.6;
  font-size: 0.9rem;
  margin: 8px 0;
}
</style>
