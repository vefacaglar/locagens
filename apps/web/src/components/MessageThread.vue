<script setup lang="ts">
import { ref, watch, nextTick, onUnmounted } from 'vue';
import type { Run, Plan } from '@locagens/shared';
import type { AgentSummary, MessageGroup } from '../lib/messageGroups';
import { renderMarkdown, formatSystemErrorMessage, capturePreScrollStates, restorePreScrollStates } from '../lib/markdown';
import { cleanedMessageContent, extractPlanFromMessage, messageTokenEstimate } from '../lib/messageDerived';
import { formatTime } from '../lib/format';
import ToolGroup from './ToolGroup.vue';
import ReasoningPanel from './ReasoningPanel.vue';

const props = defineProps<{
  activeRun: Run | null;
  groupedMessages: MessageGroup[];
  isRunning: boolean;
  plan?: Plan | null;
  planPanelOpen?: boolean;
  agentSummaries?: AgentSummary[];
}>();

const emit = defineEmits<{
  (e: 'open-plan'): void;
  (e: 'open-agents'): void;
  (e: 'view-agent', id: string): void;
  (e: 'view-file-in-review', path: string): void;
}>();

const planDoneCount = computed(() => props.plan?.tasks.filter(t => t.status === 'completed').length ?? 0);

const copiedMessageId = ref<string | null>(null);
const containerEl = ref<HTMLElement | null>(null);

function copyTextWithStatus(value: string, messageId: string) {
  navigator.clipboard.writeText(value);
  copiedMessageId.value = messageId;
  setTimeout(() => {
    if (copiedMessageId.value === messageId) {
      copiedMessageId.value = null;
    }
  }, 2000);
}

// User message collapse/expand states
const expandedUserMessages = ref<Record<string, boolean>>({});
const userMessageExpandable = ref<Record<string, boolean>>({});

function checkBubbleHeight(el: HTMLElement | null, messageId: string) {
  if (!el) return;
  // User message content never changes after send, so measure only once —
  // reading scrollHeight forces a layout pass.
  if (messageId in userMessageExpandable.value) return;
  const body = el.querySelector('.user-raw-message') as HTMLElement | null;
  if (!body) return;

  // 3 lines * 1.55 line-height * 15px is around 70px.
  // We use 74px to be safe against rounding.
  userMessageExpandable.value[messageId] = body.scrollHeight > 74;
}

// Stable per-message ref callbacks: an inline `:ref="(el) => ..."` closure gets
// a new identity every render, which makes Vue re-invoke it on every patch.
const bubbleRefCallbacks = new Map<string, (el: unknown) => void>();

function bubbleRef(messageId: string) {
  let callback = bubbleRefCallbacks.get(messageId);
  if (!callback) {
    callback = (el: unknown) => checkBubbleHeight(el as HTMLElement | null, messageId);
    bubbleRefCallbacks.set(messageId, callback);
  }
  return callback;
}

function handleUserMessageClick(event: MouseEvent, messageId: string) {
  const target = event.target as HTMLElement;
  // If user clicked a link or image, do not toggle collapse/expand
  if (target.tagName === 'A' || target.tagName === 'IMG' || target.closest('a')) {
    return;
  }
  if (userMessageExpandable.value[messageId]) {
    expandedUserMessages.value[messageId] = true;
  }
}

// Reset collapse states when the active run changes
watch(() => props.activeRun?.id, () => {
  expandedUserMessages.value = {};
  userMessageExpandable.value = {};
  bubbleRefCallbacks.clear();
});

// Plan Accordion State
const expandedPlans = ref<Record<string, boolean>>({});

function togglePlan(messageId: string) {
  expandedPlans.value[messageId] = !(expandedPlans.value[messageId] ?? true);
}

function isPlanExpanded(messageId: string): boolean {
  // Default to expanded without writing reactive state during render.
  return expandedPlans.value[messageId] ?? true;
}

// Reasoning Accordion State (coordinated: newest open, older auto-collapsed).
// Tracked separately from expandedPlans so the latest-open-others-closed rule
// only applies to reasoning panels.
const expandedReasoning = ref<Record<string, boolean>>({});

// System Error Accordion State
const expandedErrors = ref<Record<string, boolean>>({});

function toggleError(id: string) {
  expandedErrors.value[id] = !expandedErrors.value[id];
}

// The most recent group (assistant or tool_group) that carries reasoning.
const latestReasoningId = computed(() => {
  for (let i = props.groupedMessages.length - 1; i >= 0; i--) {
    const g = props.groupedMessages[i];
    // Coder reasoning lives inside CoderGroup, which coordinates its own panels.
    if (g.type !== 'coder_group' && g.message?.reasoningContent) {
      return g.id;
    }
  }
  return null;
});

function isReasoningExpanded(groupId: string): boolean {
  const explicit = expandedReasoning.value[groupId];
  if (explicit !== undefined) return explicit;
  // Default: only the latest reasoning panel is open.
  return groupId === latestReasoningId.value;
}

function toggleReasoning(groupId: string) {
  const willExpand = !isReasoningExpanded(groupId);
  expandedReasoning.value[groupId] = willExpand;
  if (willExpand) {
    nextTick(() => {
      const el = document.querySelector(`[data-reasoning-group-id="${groupId}"]`);
      el?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    });
  }
}

// When a new reasoning panel appears, collapse the previous latest and open the
// new one — the old panel stays visible (and re-expandable), it just closes.
watch(latestReasoningId, (newId, oldId) => {
  if (oldId && oldId !== newId) {
    expandedReasoning.value[oldId] = false;
  }
  if (newId) {
    expandedReasoning.value[newId] = true;
  }
});

import { computed } from 'vue';



const hasSystemErrorInHistory = computed(() => {
  return props.groupedMessages.some(g => g.type === 'system');
});

// Index of the last non-coder group. Any coder_group after it belongs to the
// in-flight delegation batch, so every such window (parallel sub-agents too)
// shows the live "working…" state, not just the last one.
const lastNonCoderIdx = computed(() => {
  for (let i = props.groupedMessages.length - 1; i >= 0; i--) {
    if (props.groupedMessages[i].type !== 'coder_group') return i;
  }
  return -1;
});

watch(() => props.groupedMessages, () => {
  if (!props.isRunning) return;

  // Query the current active reasoning body before DOM updates to check if it's at the bottom.
  // Fall back to true if no reasoning panel exists yet so it scrolls on first render.
  const reasoningBodiesBefore = containerEl.value?.querySelectorAll('.reasoning-terminal-container .plan-body') ?? [];
  const activeReasoningBodyBefore = reasoningBodiesBefore[reasoningBodiesBefore.length - 1] as HTMLElement | undefined;
  const wasAtBottom = activeReasoningBodyBefore 
    ? (activeReasoningBodyBefore.scrollHeight - activeReasoningBodyBefore.scrollTop - activeReasoningBodyBefore.clientHeight <= 60)
    : true;
  const prevScrollTop = activeReasoningBodyBefore ? activeReasoningBodyBefore.scrollTop : 0;

  // Only the streaming (last) message's code blocks can change, so scope the
  // scroll capture/restore to that message — measuring every pre in the thread
  // forced a full layout pass per flush, growing with chat length.
  const liveMessageId = props.groupedMessages[props.groupedMessages.length - 1]?.message.id;
  const preScrollStates = capturePreScrollStates(containerEl.value, liveMessageId);

  nextTick(() => {
    // The live reasoning panel may sit in either an assistant message or a tool
    // group, so scroll the last rendered reasoning body regardless of wrapper.
    const reasoningBodiesAfter = containerEl.value?.querySelectorAll('.reasoning-terminal-container .plan-body') ?? [];
    const activeReasoningBodyAfter = reasoningBodiesAfter[reasoningBodiesAfter.length - 1] as HTMLElement | undefined;
    if (activeReasoningBodyAfter) {
      if (wasAtBottom) {
        activeReasoningBodyAfter.scrollTop = activeReasoningBodyAfter.scrollHeight;
      } else {
        activeReasoningBodyAfter.scrollTop = prevScrollTop;
      }
    }

    // Restore pre scroll states for the streaming message only
    restorePreScrollStates(containerEl.value, preScrollStates, liveMessageId);
  });
});

// Lightbox state
const activeLightboxImage = ref<string | null>(null);

function handleImageClick(e: MouseEvent) {
  const target = e.target as HTMLElement;
  if (target && target.tagName === 'IMG' && target.closest('.user-markdown-body')) {
    activeLightboxImage.value = (target as HTMLImageElement).src;
  }
}

function handleKeyDown(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    activeLightboxImage.value = null;
  }
}

watch(activeLightboxImage, (newVal) => {
  if (newVal) {
    window.addEventListener('keydown', handleKeyDown);
  } else {
    window.removeEventListener('keydown', handleKeyDown);
  }
});

const elapsedSeconds = ref(0);
let timerInterval: any = null;
const startTime = ref<number | null>(null);

watch([() => props.isRunning, () => props.activeRun?.id], ([running]) => {
  if (running) {
    startTime.value = Date.now();
    elapsedSeconds.value = 0;
    if (timerInterval) clearInterval(timerInterval);
    timerInterval = setInterval(() => {
      if (startTime.value) {
        elapsedSeconds.value = Math.floor((Date.now() - startTime.value) / 1000);
      }
    }, 1000);
  } else {
    if (timerInterval) {
      clearInterval(timerInterval);
      timerInterval = null;
    }
    startTime.value = null;
    elapsedSeconds.value = 0;
  }
}, { immediate: true });

onUnmounted(() => {
  window.removeEventListener('keydown', handleKeyDown);
  if (timerInterval) clearInterval(timerInterval);
});

const formattedElapsedTime = computed(() => {
  const m = Math.floor(elapsedSeconds.value / 60);
  const s = elapsedSeconds.value % 60;
  return `${m}:${s.toString().padStart(2, '0')}`;
});
</script>

<template>
  <div v-if="!activeRun" class="empty-chat"></div>

  <div v-else ref="containerEl" class="messages-inner" @click="handleImageClick">
    <div class="user-message-container">
      <article 
        class="user-bubble"
        :class="{ 
          'is-collapsed': !expandedUserMessages[activeRun?.id || ''], 
          'is-expandable': userMessageExpandable[activeRun?.id || ''] 
        }"
        @click="handleUserMessageClick($event, activeRun?.id || '')"
        :ref="bubbleRef(activeRun?.id || '')"
      >
        <div
          class="user-raw-message user-markdown-body"
          v-html="renderMarkdown(activeRun?.task || '', activeRun?.id || 'active-run')"
        ></div>
      </article>
      <div class="user-response-footer">
        <button 
          class="copy-button-icon" 
          title="Copy message"
          @click.stop="copyTextWithStatus(activeRun?.task || '', activeRun?.id || '')"
        >
          <svg v-if="copiedMessageId === activeRun?.id" class="check-icon" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
            <path d="M20 6 9 17l-5-5"/>
          </svg>
          <svg v-else class="copy-icon" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <rect width="14" height="14" x="8" y="8" rx="2" ry="2"/>
            <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>
          </svg>
        </button>
      </div>
    </div>

    <!-- Inline Plan Link -->
    <div v-if="plan" class="plan-accordion-row">
      <header class="step-row" @click="emit('open-plan')">
        <svg class="step-row-toggle" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="m9 18 6-6-6-6"></path>
        </svg>
        <span class="step-row-label">Plan: {{ plan.title }}</span>
        <span v-if="plan.tasks.length" style="font-size: 0.72rem; color: var(--faint); font-family: monospace; margin-left: 6px; user-select: none;">
          {{ planDoneCount }} / {{ plan.tasks.length }} tasks completed
        </span>
      </header>
    </div>

    <!-- Inline Background Agents Link -->
    <div v-if="(agentSummaries?.length ?? 0) > 0" class="agents-accordion-row">
      <header class="step-row" @click="emit('open-agents')">
        <svg class="step-row-toggle" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="m9 18 6-6-6-6"></path>
        </svg>
        <span class="step-row-label">Background Agents</span>
        <span style="font-size: 0.72rem; color: var(--faint); font-family: monospace; margin-left: 6px; user-select: none;">
          {{ agentSummaries?.filter(agent => agent.status === 'running').length ?? 0 }} running · {{ agentSummaries?.length }} total
        </span>
      </header>
    </div>

    <template v-for="(group, idx) in groupedMessages" :key="group.id">
      <div v-if="group.type === 'user'" class="user-message-container">
        <article 
          class="user-bubble"
          :class="{ 
            'is-collapsed': !expandedUserMessages[group.message.id], 
            'is-expandable': userMessageExpandable[group.message.id] 
          }"
          @click="handleUserMessageClick($event, group.message.id)"
          :ref="bubbleRef(group.message.id)"
        >
          <div
            class="user-raw-message user-markdown-body"
            v-html="renderMarkdown(group.message.content || '', group.message.id)"
          ></div>
        </article>
        <div class="user-response-footer">
          <button 
            class="copy-button-icon" 
            title="Copy message"
            @click.stop="copyTextWithStatus(group.message.content, group.message.id)"
          >
            <svg v-if="copiedMessageId === group.message.id" class="check-icon" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
              <path d="M20 6 9 17l-5-5"/>
            </svg>
            <svg v-else class="copy-icon" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <rect width="14" height="14" x="8" y="8" rx="2" ry="2"/>
              <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>
            </svg>
          </button>
        </div>
      </div>

      <template v-else-if="group.type === 'tool_group'">
        <!-- Reasoning persists even after the turn commits its tool calls. -->
        <ReasoningPanel
          v-if="group.message.reasoningContent"
          :content="group.message.reasoningContent"
          :expanded="isReasoningExpanded(group.id)"
          :is-thinking="isRunning && idx === groupedMessages.length - 1 && !group.toolCalls?.length && !group.message.content?.trim()"
          :data-reasoning-group-id="group.id"
          @toggle="toggleReasoning(group.id)"
        />
        <ToolGroup
          :id="group.message.id"
          :thought="group.message.role === 'tool' ? '' : group.message.content"
          :tool-calls="group.toolCalls"
          :tool-responses="group.toolResponses"
          :agent-role="group.message.agentRole"
          :model="group.message.model"
          @open-plan="emit('open-plan')"
          @view-file-in-review="emit('view-file-in-review', $event)"
        />
      </template>

      <div v-else-if="group.type === 'coder_group'" class="coder-shortcut-row" :data-agent-group-id="group.id">
        <button
          type="button"
          class="coder-shortcut-btn step-row"
          @click="emit('view-agent', group.id)"
        >
          <svg class="arrow-right-icon step-row-toggle" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
            <path d="m9 18 6-6-6-6"></path>
          </svg>
          <span class="step-row-label">
            {{ group.children?.[0]?.message.agentRole === 'utility' ? 'Utility' : 'Coder' }}: {{ group.title }}
          </span>
          <template v-if="isRunning && idx > lastNonCoderIdx">
            <span class="status-dot pending" title="Running..."></span>
          </template>
        </button>
      </div>

      <article
        v-else-if="group.type === 'assistant'"
        class="assistant-message"
        :class="{ 
          'coder-message': group.message.agentRole === 'coder',
          'is-generating': isRunning && idx === groupedMessages.length - 1
        }"
      >
        <div class="assistant-meta">
          <span v-if="group.message.agentRole === 'coder'" class="agent-badge coder-badge">Coder</span>
          <span>{{ group.message.providerDisplayName }} / {{ group.message.model }}</span>
          <span>{{ formatTime(group.message.createdAt) }}</span>
        </div>

        <!-- AI Plan Accordion (hidden while the side plan panel is showing it) -->
        <div v-if="extractPlanFromMessage(group.message) && !props.planPanelOpen" class="plan-terminal-container">
          <header class="terminal-header" @click="togglePlan(group.message.id + '-plan')">
            <div class="terminal-header-left">
              <svg class="header-icon" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <circle cx="12" cy="12" r="3"></circle>
                <circle cx="12" cy="5" r="1"></circle>
                <circle cx="12" cy="19" r="1"></circle>
                <circle cx="5" cy="12" r="1"></circle>
                <circle cx="19" cy="12" r="1"></circle>
                <path d="M12 8v3M12 13v3M8 12h3M13 12h3"></path>
              </svg>
              <span class="terminal-title">AI Plan / Reasoning</span>
            </div>
            <button class="terminal-toggle-btn">
              {{ isPlanExpanded(group.message.id + '-plan') ? 'Collapse' : 'Expand' }}
            </button>
          </header>
          <div v-if="isPlanExpanded(group.message.id + '-plan')" class="terminal-body plan-body">
            <pre class="plan-text">{{ extractPlanFromMessage(group.message) }}</pre>
          </div>
        </div>

        <!-- AI Reasoning (Inner Monologue) Accordion -->
        <ReasoningPanel
          v-if="group.message.reasoningContent"
          :content="group.message.reasoningContent"
          :expanded="isReasoningExpanded(group.id)"
          :data-reasoning-group-id="group.id"
          @toggle="toggleReasoning(group.id)"
        />

        <div 
          class="markdown-body" 
          v-html="renderMarkdown(cleanedMessageContent(group.message), group.message.id)"
        ></div>
        <div class="assistant-response-footer" style="display: flex; align-items: center; gap: 8px; margin-top: 10px;">
          <button 
            class="copy-button-icon" 
            title="Copy entire response"
            @click="copyTextWithStatus(group.message.content, group.message.id)"
          >
            <svg v-if="copiedMessageId === group.message.id" class="check-icon" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
              <path d="M20 6 9 17l-5-5"/>
            </svg>
            <svg v-else class="copy-icon" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <rect width="14" height="14" x="8" y="8" rx="2" ry="2"/>
              <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>
            </svg>
          </button>
          <span class="response-tokens-badge" style="font-size: 0.72rem; color: var(--faint); font-family: monospace; user-select: none;">
            {{ messageTokenEstimate(group.message) }} tokens
          </span>
          <span v-if="isRunning && idx === groupedMessages.length - 1" class="working-loader-text">
            <span>· Working on</span>
            <span class="animating-dots"><span>.</span><span>.</span><span>.</span></span>
            <span class="elapsed-time">({{ formattedElapsedTime }})</span>
          </span>
        </div>
      </article>

      <div v-else-if="group.type === 'system'" class="system-error-accordion" :class="{ 'is-expanded': expandedErrors[group.id] }">
        <header class="step-row" @click="toggleError(group.id)">
          <svg class="step-row-toggle" :class="{ rotated: expandedErrors[group.id] }" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="m9 18 6-6-6-6"></path>
          </svg>
          <span class="step-row-label">System Error</span>
          <span style="font-size: 0.72rem; color: var(--faint); font-family: monospace; margin-left: 6px; user-select: none;">
            {{ formatTime(group.message.createdAt) }}
          </span>
        </header>

        <div v-if="expandedErrors[group.id]" class="error-details">
          <div class="error-bubble">
            <div class="error-icon-wrap">
              <svg class="error-icon" xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                <circle cx="12" cy="12" r="10"></circle>
                <line x1="12" y1="8" x2="12" y2="12"></line>
                <line x1="12" y1="16" x2="12.01" y2="16"></line>
              </svg>
            </div>
            <div class="error-text">
              {{ formatSystemErrorMessage(group.message.content) }}
            </div>
          </div>
        </div>
      </div>
    </template>

    <div v-if="activeRun.status === 'failed' && !hasSystemErrorInHistory" class="system-error-accordion" :class="{ 'is-expanded': expandedErrors['trailing-error'] }">
      <header class="step-row" @click="toggleError('trailing-error')">
        <svg class="step-row-toggle" :class="{ rotated: expandedErrors['trailing-error'] }" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="m9 18 6-6-6-6"></path>
        </svg>
        <span class="step-row-label">System Error</span>
        <span style="font-size: 0.72rem; color: var(--faint); font-family: monospace; margin-left: 6px; user-select: none;">
          {{ formatTime(activeRun.updatedAt) }}
        </span>
      </header>

      <div v-if="expandedErrors['trailing-error']" class="error-details">
        <div class="error-bubble">
          <div class="error-icon-wrap">
            <svg class="error-icon" xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="12" cy="12" r="10"></circle>
              <line x1="12" y1="8" x2="12" y2="12"></line>
              <line x1="12" y1="16" x2="12.01" y2="16"></line>
            </svg>
          </div>
          <div class="error-text">
            {{ formatSystemErrorMessage(activeRun.errorMessage || 'Chat generation failed.') }}
          </div>
        </div>
      </div>
    </div>

    <!-- Lightbox Modal -->
    <Transition name="fade">
      <div v-if="activeLightboxImage" class="lightbox-overlay" @click="activeLightboxImage = null">
        <div class="lightbox-content" @click.stop>
          <img :src="activeLightboxImage" class="lightbox-img" alt="Enlarged screenshot" />
          <button class="lightbox-close-btn" @click="activeLightboxImage = null">
            <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <line x1="18" y1="6" x2="6" y2="18"></line>
              <line x1="6" y1="6" x2="18" y2="18"></line>
            </svg>
          </button>
        </div>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
/* Vertical rhythm — one rule for every block type. The whole assistant turn
   (reasoning, tool calls, coder rows, plan terminals, final prose) flows as a
   single tightly-spaced stream; a user message opens a new turn with real air.
   This replaces the old per-pair negative-margin patch that special-cased some
   adjacencies and left reasoning panels with the full gap around them. */
.messages-inner > * {
  margin-top: 8px;
}

.messages-inner > .user-message-container {
  margin-top: 30px;
  margin-bottom: 14px;
}

.messages-inner > .plan-thread-link {
  margin-top: 18px;
}

.messages-inner > *:first-child {
  margin-top: 0;
}
/* Coder sub-agent messages: subtle left accent + monochrome badge so the
   architect's stream is visually distinct from delegated coder work. */
.assistant-message.coder-message {
  border-left: 2px solid var(--border);
  padding-left: 14px;
  margin-left: 2px;
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

.plan-thread-link {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  text-align: left;
  padding: 10px 14px;
  border: 1px solid var(--border);
  border-radius: 10px;
  background: var(--surface-strong);
  color: var(--text);
  cursor: pointer;
  transition: border-color 0.15s ease, background 0.15s ease;
}

.plan-thread-link:hover {
  border-color: var(--muted);
}

.plan-thread-link-label {
  flex: 1 1 auto;
  font-size: 0.86rem;
  font-weight: 550;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.plan-thread-link-count {
  flex: 0 0 auto;
  font-variant-numeric: tabular-nums;
  font-family: monospace;
  font-size: 0.72rem;
  color: var(--faint);
}

.plan-thread-link-action {
  flex: 0 0 auto;
  font-size: 0.74rem;
  color: var(--muted);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 2px 8px;
}


/* Terminal Container Styling */
.plan-terminal-container,
.thinking-terminal-container {
  margin: 12px 0 18px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: 8px;
  overflow: hidden;
  font-family: 'Fira Code', 'Courier New', monospace;
  box-shadow: 0 4px 15px var(--btn-secondary-shadow);
  transition: border-color 0.2s ease;
}

.plan-terminal-container:hover,
.thinking-terminal-container:hover {
  border-color: var(--msg-container-border);
}

.terminal-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  background: var(--surface);
  padding: 8px 14px;
  cursor: pointer;
  user-select: none;
  border-bottom: 1px solid var(--border);
}

.terminal-header-left {
  display: flex;
  align-items: center;
  gap: 6px;
}

.header-icon {
  color: var(--planner);
  flex-shrink: 0;
}

.terminal-title {
  color: var(--msg-metadata-color);
  font-size: 0.8rem;
  font-weight: 500;
}

.terminal-toggle-btn {
  background: transparent;
  color: var(--faint);
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 2px 8px;
  font-size: 0.72rem;
  cursor: pointer;
  transition: all 0.2s ease;
}

.terminal-toggle-btn:hover {
  color: var(--text);
  background: var(--surface-strong);
  border-color: var(--border);
}

.terminal-body {
  padding: 12px;
  max-height: 220px;
  overflow-y: auto;
  font-size: 0.78rem;
  line-height: 1.5;
  display: flex;
  flex-direction: column;
  gap: 4px;
  scroll-behavior: smooth;
}

.plan-body {
  max-height: 200px;
}

.plan-text {
  margin: 0;
  white-space: pre-wrap;
  word-break: break-word;
  color: var(--muted);
  font-family: inherit;
  font-size: inherit;
}

.terminal-log-line {
  display: flex;
  align-items: flex-start;
  gap: 8px;
}

.log-timestamp {
  color: var(--msg-copy-btn);
  flex-shrink: 0;
  user-select: none;
}

.log-prompt {
  color: var(--msg-success-stroke);
  flex-shrink: 0;
  user-select: none;
}

.log-text {
  word-break: break-all;
}

.log-text.info {
  color: var(--msg-thought-text);
}

.log-text.success {
  color: var(--msg-success-stroke);
}

.log-text.warning {
  color: var(--msg-warning-text);
}

.log-text.danger {
  color: var(--msg-danger-text);
}

.terminal-cursor-line {
  display: flex;
  align-items: center;
  gap: 8px;
}

.terminal-cursor {
  color: var(--msg-success-stroke);
  animation: blink 1s infinite step-end;
}

@keyframes blink {
  from, to { opacity: 1; }
  50% { opacity: 0; }
}

.copy-button-icon {
  display: flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: 0;
  padding: 4px;
  cursor: pointer;
  color: var(--faint);
  border-radius: 4px;
  transition: all 0.2s ease;
}

.copy-button-icon:hover {
  color: var(--text);
  background: var(--surface-strong);
}

.user-bubble {
  max-width: 100%;
  align-self: stretch;
  padding: 14px 16px;
  border: none;
  border-radius: 16px;
  background: var(--user-bubble-bg);
  box-shadow:
    0 12px 32px var(--user-bubble-shadow-color),
    inset 0 1px 0 var(--user-bubble-border);
  transition: background 0.2s ease, max-height 0.3s cubic-bezier(0.4, 0, 0.2, 1), box-shadow 0.3s ease;
  overflow: hidden;
  position: relative;
}

.user-bubble.is-expandable.is-collapsed {
  cursor: pointer;
}

.user-bubble.is-expandable.is-collapsed:hover {
  background: var(--user-bubble-hover-bg);
}

.user-bubble.is-collapsed.is-expandable {
  max-height: 6.5rem;
  box-shadow:
    0 12px 32px var(--user-bubble-shadow-color),
    inset 0 1px 0 var(--user-bubble-border),
    inset 0 -36px 36px -18px var(--user-bubble-fade-color);
}

.user-bubble.is-expanded.is-expandable {
  max-height: 2000px;
  box-shadow:
    0 12px 32px var(--user-bubble-shadow-color),
    inset 0 1px 0 var(--user-bubble-border);
}

.user-message-container {
  display: flex;
  flex-direction: column;
  align-items: stretch;
  max-width: 100%;
  align-self: stretch;
}

.user-response-footer {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  margin-top: 6px;
  padding-right: 8px;
}

.user-raw-message {
  color: var(--text);
  font-family: inherit;
  font-size: 0.94rem;
  line-height: 1.55;
  white-space: normal;
  word-break: break-word;
}
.user-markdown-body :deep(p) {
  margin: 0;
}
.user-markdown-body :deep(p + p) {
  margin-top: 8px;
}
.user-markdown-body :deep(img) {
  width: 80px;
  height: 80px;
  object-fit: cover;
  border-radius: 8px;
  margin-top: 8px;
  display: block;
  border: 1px solid var(--border);
  cursor: zoom-in;
  transition: transform 0.2s cubic-bezier(0.16, 1, 0.3, 1), border-color 0.2s ease, box-shadow 0.2s ease;
}

.user-markdown-body :deep(img):hover {
  transform: scale(1.05);
  border-color: var(--planner);
  box-shadow: 0 4px 12px var(--user-bubble-shadow-color);
}

/* Lightbox Modal */
.lightbox-overlay {
  position: fixed;
  top: 0;
  left: 0;
  width: 100vw;
  height: 100vh;
  background: var(--lightbox-overlay-bg);
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 9999;
  cursor: zoom-out;
}

.lightbox-content {
  position: relative;
  max-width: 90%;
  max-height: 90%;
  display: flex;
  align-items: center;
  justify-content: center;
}

.lightbox-img {
  max-width: 100%;
  max-height: 90vh;
  object-fit: contain;
  border-radius: 12px;
  box-shadow: 0 20px 50px var(--lightbox-image-shadow);
  border: 1px solid var(--lightbox-image-border);
  cursor: default;
  user-select: none;
}

.lightbox-close-btn {
  position: absolute;
  top: -40px;
  right: -40px;
  background: var(--lightbox-close-btn-bg);
  border: 1px solid var(--lightbox-close-btn-border);
  color: var(--text);
  border-radius: 50%;
  width: 36px;
  height: 36px;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: all 0.2s ease;
}

.lightbox-close-btn:hover {
  background: var(--lightbox-close-btn-hover-bg);
  transform: scale(1.1);
}

/* Close button responsiveness for small screens */
@media (max-width: 768px) {
  .lightbox-close-btn {
    top: 10px;
    right: 10px;
    background: var(--lightbox-close-btn-mobile-bg);
    border: 1px solid var(--lightbox-close-btn-mobile-border);
  }
}

/* Transition Animations */
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.25s ease;
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}

.fade-enter-active .lightbox-img {
  transition: transform 0.25s cubic-bezier(0.34, 1.56, 0.64, 1);
}
.fade-leave-active .lightbox-img {
  transition: transform 0.2s ease;
}

.fade-enter-from .lightbox-img {
  transform: scale(0.92);
}
.fade-leave-to .lightbox-img {
  transform: scale(0.95);
}
.working-loader-text {
  font-size: 0.72rem;
  color: var(--muted);
  font-style: italic;
  display: inline-flex;
  align-items: center;
  gap: 2px;
  margin-left: 4px;
}

.working-loader-text .elapsed-time {
  color: var(--faint);
  font-style: normal;
  margin-left: 2px;
  font-variant-numeric: tabular-nums;
}

.animating-dots {
  display: inline-flex;
}

.animating-dots span {
  animation: dotBlink 1.4s infinite both;
  width: 4px;
  text-align: center;
}

.animating-dots span:nth-child(2) {
  animation-delay: .2s;
}

.animating-dots span:nth-child(3) {
  animation-delay: .4s;
}

@keyframes dotBlink {
  0% { opacity: .2; }
  20% { opacity: 1; }
  100% { opacity: .2; }
}

@keyframes messageEnter {
  from {
    opacity: 0;
    transform: translateY(12px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}

.user-message-container,
.assistant-message,
.plan-thread-link,
.coder-shortcut-row,
.system-error-accordion,
.plan-accordion-row,
.agents-accordion-row,
:deep(.tool-group-wrap),
:deep(.reasoning-terminal-container) {
  animation: messageEnter 0.35s cubic-bezier(0.16, 1, 0.3, 1) both;
}

.assistant-message {
  color: var(--assistant-msg-color);
  line-height: 1.65;
}

.assistant-meta {
  color: var(--assistant-meta-color);
}

.coder-shortcut-row {
  width: 100%;
}

/* The row, label, icon and right-aligned toggle all come from the shared
   .step-row classes (style.css). Only the navigate nudge stays component-local. */
.coder-shortcut-btn:hover .arrow-right-icon {
  transform: translateX(1px);
}

.system-error-accordion {
  width: 100%;
  margin: 2px 0;
}

.system-error-accordion .step-row-icon {
  color: var(--danger);
}

.system-error-accordion .step-row-label {
  color: var(--danger);
  font-weight: 550;
}

.error-details {
  padding: 8px 0 12px;
}

.error-details .error-bubble {
  margin-top: 0;
}

.plan-accordion-row,
.agents-accordion-row {
  width: 100%;
  margin: 6px 0;
}

.check-icon {
  stroke: var(--msg-success-stroke);
}
</style>
