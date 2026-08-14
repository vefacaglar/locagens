<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch, nextTick } from 'vue';
import { useCustomDialog } from '../composables/useCustomDialog';

const { showAlert } = useCustomDialog();
import type { PermissionDecision } from '../api/client';
import type { MessageGroup } from '../lib/messageGroups';
import { MODES_LIST, type ChatMode } from '../composables/useComposerSettings';
import { useModelPicker } from '../composables/useModelPicker';
import { formatModelName, formatContextLimit, getReasoningDesc } from '../lib/modelFormat';
import type { ModelOption } from '../composables/useComposerSettings';
import type { AgentPreset } from '@locagens/shared';
import ConfirmationCard from './ConfirmationCard.vue';
import PermissionCard from './PermissionCard.vue';
import QuestionCard from './QuestionCard.vue';
import { messageTokenEstimate } from '../lib/messageDerived';

const props = defineProps<{
  taskInput: string;
  queuedTaskInput: string;
  isRunning: boolean;
  currentMode: ChatMode;
  bypassPermissions: boolean;
  selectedModel: string;
  selectedReasoningEffort: string;
  reasoningEffortOptions: { id: string; label: string }[];
  modelOptions: ModelOption[];
  activeModelDisplayName: string;
  agentPresets?: AgentPreset[];
  selectedPresetId?: string;
  focusSignal: number;
  confirmationGroup: MessageGroup | null;
  showPermission: boolean;
  permissionRequest: any;
  questionRequest: any;
  isLanding?: boolean;
  projectOptions?: { path: string; name: string }[];
  activeProjectPath?: string;
  messages?: any[];
  runUsageLabel?: string | null;
  runUsageTooltip?: string;
}>();

const emit = defineEmits<{
  (e: 'update:taskInput', value: string): void;
  (e: 'update:currentMode', value: ChatMode): void;
  (e: 'update:bypassPermissions', value: boolean): void;
  (e: 'update:selectedModel', value: string): void;
  (e: 'update:selectedReasoningEffort', value: string): void;
  (e: 'update:selectedPresetId', value: string): void;
  (e: 'send'): void;
  (e: 'queue'): void;
  (e: 'cancel'): void;
  (e: 'quick-reply', option: string): void;
  (e: 'permission-decision', decision: PermissionDecision): void;
  (e: 'question-answer', payload: { selections: string[][]; notes: string[] }): void;
  (e: 'select-project', path: string): void;
  (e: 'open-settings', tab?: string): void;
}>();

const textarea = ref<HTMLTextAreaElement | null>(null);
const fileInput = ref<HTMLInputElement | null>(null);
const attachedFiles = ref<{ name: string; content: string; extension: string; isImage: boolean }[]>([]);

function estimateTokens(text: string): number {
  if (!text) return 0;
  const cleanText = text.replace(/!\[([^\]]*)\]\(data:image\/[^)]+\)/g, '[Image]');
  const charCount = cleanText.length;
  const wordCount = cleanText.trim().split(/\s+/).length;
  return Math.round(Math.max(charCount / 3.7, wordCount * 1.3));
}

const draftTokens = computed(() => {
  let text = props.taskInput || '';
  let imageTokens = 0;
  for (const file of attachedFiles.value) {
    if (file.isImage) {
      imageTokens += 300;
    } else {
      const syntaxLang = file.extension ? file.extension.toLowerCase() : '';
      text += `\n\n### File: ${file.name}\n\`\`\`${syntaxLang}\n${file.content}\n\`\`\``;
    }
  }
  return estimateTokens(text) + imageTokens;
});

const contextTokens = computed(() => {
  let total = 0; // system prompt estimate
  if (props.messages) {
    // messageTokenEstimate is memoized per message object, so during streaming
    // only the one changed message is re-tokenized instead of the whole chat.
    for (const msg of props.messages) {
      total += messageTokenEstimate(msg);
    }
  }
  return total;
});

const activePreset = computed(() => {
  return props.agentPresets?.find(p => p.id === props.selectedPresetId) ?? null;
});

const effectiveModelCombined = computed(() => {
  if (activePreset.value) {
    const arch = activePreset.value.architect;
    return `${arch.providerId}:${arch.model}`;
  }
  return props.selectedModel;
});

const activeModelOption = computed(() => {
  return props.modelOptions.find(opt => opt.value === effectiveModelCombined.value);
});

const activeContextLimit = computed(() => {
  return activeModelOption.value?.contextLimit;
});

function formatTokensCompact(num: number): string {
  if (num >= 1000000) {
    const val = num / 1000000;
    const rounded = Math.round(val * 10) / 10;
    return rounded % 1 === 0 ? `${Math.round(rounded)}m` : `${rounded}m`;
  }
  if (num >= 1000) {
    return `${Math.round(num / 1000)}k`;
  }
  return String(num);
}

const formattedContext = computed(() => {
  const current = contextTokens.value + draftTokens.value;
  const limit = activeContextLimit.value;
  if (limit) {
    return `${formatTokensCompact(current)}/${formatTokensCompact(limit)}`;
  }
  return formatTokensCompact(current);
});

const usageParts = computed(() => {
  return props.runUsageLabel
    ? props.runUsageLabel.split(' · ').filter(Boolean)
    : [];
});

const canSend = computed(() => {
  return !props.isRunning && props.selectedModel && (props.taskInput.trim() || attachedFiles.value.length > 0);
});

const hasQueuedMessage = computed(() => props.queuedTaskInput.trim().length > 0);

const isButtonDisabled = computed(() => {
  if (props.isRunning) return false;
  return !props.selectedModel || (!props.taskInput.trim() && attachedFiles.value.length === 0);
});

function handleButtonClick() {
  if (props.isRunning) {
    emit('cancel');
  } else {
    handleSend();
  }
}

function triggerFileSelect() {
  fileInput.value?.click();
}

async function handleFileChange(event: Event) {
  const target = event.target as HTMLInputElement;
  if (!target.files || target.files.length === 0) return;

  for (const file of Array.from(target.files)) {
    if (file.size > 2 * 1024 * 1024) {
      await showAlert(`File too large (max 2MB): ${file.name}`);
      continue;
    }
    try {
      const extension = file.name.split('.').pop() || '';
      const isImage = file.type.startsWith('image/') || ['png', 'jpg', 'jpeg', 'gif', 'webp'].includes(extension.toLowerCase());
      const content = isImage ? await readFileAsDataURL(file) : await readFileAsText(file);
      attachedFiles.value.push({
        name: file.name,
        content,
        extension,
        isImage
      });
    } catch (err) {
      console.error(`Error reading file ${file.name}:`, err);
      await showAlert(`Failed to read file: ${file.name}`);
    }
  }
  target.value = '';
}

function readFileAsText(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result as string);
    reader.onerror = () => reject(reader.error);
    reader.readAsText(file);
  });
}

function readFileAsDataURL(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result as string);
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
}

function removeAttachment(idx: number) {
  attachedFiles.value.splice(idx, 1);
}

function handleSend() {
  if (props.isRunning) {
    handleQueue();
    return;
  }

  if (!canSend.value) return;

  if (attachedFiles.value.length > 0) {
    let finalTask = props.taskInput;
    for (const file of attachedFiles.value) {
      if (file.isImage) {
        finalTask += `\n\n![${file.name}](${file.content})`;
      } else {
        const syntaxLang = file.extension ? file.extension.toLowerCase() : '';
        finalTask += `\n\n### File: ${file.name}\n\`\`\`${syntaxLang}\n${file.content}\n\`\`\``;
      }
    }
    emit('update:taskInput', finalTask);
    attachedFiles.value = [];
    nextTick(() => {
      emit('send');
    });
  } else {
    emit('send');
  }
}

function handleQueue() {
  if (!props.taskInput.trim() && attachedFiles.value.length === 0) return;

  if (attachedFiles.value.length > 0) {
    let finalTask = props.taskInput;
    for (const file of attachedFiles.value) {
      if (file.isImage) {
        finalTask += `\n\n![${file.name}](${file.content})`;
      } else {
        const syntaxLang = file.extension ? file.extension.toLowerCase() : '';
        finalTask += `\n\n### File: ${file.name}\n\`\`\`${syntaxLang}\n${file.content}\n\`\`\``;
      }
    }
    emit('update:taskInput', finalTask);
    attachedFiles.value = [];
    nextTick(() => emit('queue'));
  } else {
    emit('queue');
  }
}

const showModeMenu = ref(false);
const showModelMenu = ref(false);
const showReasoningMenu = ref(false);

function getModeLabel(modeId: string): string {
  return MODES_LIST.find(m => m.id === modeId)?.label ?? 'Chat';
}

function selectMode(modeId: ChatMode) {
  emit('update:currentMode', modeId);
  showModeMenu.value = false;
}

function selectModel(value: string) {
  emit('update:selectedModel', value);
  showModelMenu.value = false;
}

// --- Model picker dropdown: search / grouping / favorites / keyboard nav ---
// (state and behavior live in useModelPicker; this component keeps only the
// open/close state and the menu-switching keyboard shell)
const picker = useModelPicker({
  modelOptions: () => props.modelOptions,
  selectedModel: () => props.selectedModel,
  selectedReasoningEffort: () => props.selectedReasoningEffort,
  commitModel: (value) => emit('update:selectedModel', value),
  commitReasoningEffort: (value) => emit('update:selectedReasoningEffort', value)
});
const {
  searchQuery,
  groupedModels,
  isItemFocused,
  setFocusedItem,
  toggleFavorite,
  isStarred,
  toggleProviderCollapse,
  isProviderCollapsed
} = picker;

function handleAddProvider() {
  emit('open-settings', 'providers');
  showModelMenu.value = false;
}

function handleManagePresets() {
  emit('open-settings', 'agents');
  showPresetMenu.value = false;
}

watch(showModelMenu, (open) => {
  if (open) picker.onMenuOpened();
});

const activeReasoningLabel = computed(() =>
  props.reasoningEffortOptions.find(x => x.id === props.selectedReasoningEffort)?.label ?? 'Default'
);
const hasReasoningEffortOptions = computed(() => props.reasoningEffortOptions.length > 1);

function selectReasoningEffort(value: string) {
  emit('update:selectedReasoningEffort', value);
  showReasoningMenu.value = false;
}

// Optional dual-model agent presets (architect + coder). Only shown when at
// least one preset is configured. '' = single model (default).
const showPresetMenu = ref(false);
const hasPresets = computed(() => (props.agentPresets?.length ?? 0) > 0);
const activePresetLabel = computed(() => {
  const p = props.agentPresets?.find(x => x.id === props.selectedPresetId);
  return p ? p.displayName : 'Single model';
});
const selectedPresetIsActive = computed(() =>
  !!props.selectedPresetId && !!props.agentPresets?.some(x => x.id === props.selectedPresetId)
);

function selectPreset(value: string) {
  emit('update:selectedPresetId', value);
  showPresetMenu.value = false;
}

const showProjectMenu = ref(false);

// Ensure only one dropdown menu is open at a time
watch(showModeMenu, (val) => {
  if (val) {
    showModelMenu.value = false;
    showReasoningMenu.value = false;
    showPresetMenu.value = false;
    showProjectMenu.value = false;
  }
});
watch(showModelMenu, (val) => {
  if (val) {
    showModeMenu.value = false;
    showReasoningMenu.value = false;
    showPresetMenu.value = false;
    showProjectMenu.value = false;
  }
});
watch(showReasoningMenu, (val) => {
  if (val) {
    showModeMenu.value = false;
    showModelMenu.value = false;
    showPresetMenu.value = false;
    showProjectMenu.value = false;
  }
});
watch(showPresetMenu, (val) => {
  if (val) {
    showModeMenu.value = false;
    showModelMenu.value = false;
    showReasoningMenu.value = false;
    showProjectMenu.value = false;
  }
});
watch(showProjectMenu, (val) => {
  if (val) {
    showModeMenu.value = false;
    showModelMenu.value = false;
    showReasoningMenu.value = false;
    showPresetMenu.value = false;
  }
});

const activeProjectName = computed(() => {
  const current = props.projectOptions?.find(p => p.path === props.activeProjectPath);
  return current ? current.name : 'Select project...';
});

function selectProjectItem(path: string) {
  emit('select-project', path);
  showProjectMenu.value = false;
}

// Focus is driven by a signal counter incremented by the parent session.
watch(() => props.focusSignal, () => {
  requestAnimationFrame(() => textarea.value?.focus());
});

watch(() => props.isRunning, (running) => {
  if (!running) requestAnimationFrame(() => textarea.value?.focus());
});

function adjustHeight() {
  const el = textarea.value;
  if (!el) return;
  if (!props.taskInput) {
    el.style.height = '';
    return;
  }
  const lineHeight = 24;
  const maxHeight = 240;
  // Base height from explicit newlines
  el.style.height = 'auto';
  const newlineCount = el.value.split('\n').length;
  const baseHeight = lineHeight * newlineCount;
  // Use scrollHeight if content wraps (long single line) and stays under the cap
  const targetHeight = Math.min(Math.max(baseHeight, el.scrollHeight), maxHeight);
  el.style.height = targetHeight + 'px';
}

watch(() => props.taskInput, (val) => {
  if (!val) {
    if (textarea.value) textarea.value.style.height = '';
  } else {
    nextTick(adjustHeight);
  }
});

function handleDocumentClick(e: MouseEvent) {
  const target = e.target as HTMLElement;
  if (!target.closest('.mode-selector-wrap')) showModeMenu.value = false;
  if (!target.closest('.model-dropdown-wrap')) showModelMenu.value = false;
  if (!target.closest('.reasoning-dropdown-wrap')) showReasoningMenu.value = false;
  if (!target.closest('.preset-dropdown-wrap')) showPresetMenu.value = false;
  if (!target.closest('.project-dropdown-wrap')) showProjectMenu.value = false;
}

function handleKeyDown(e: KeyboardEvent) {
  // 1. If mode menu is open:
  if (showModeMenu.value) {
    const modeKeys: Record<string, ChatMode> = {
      '1': 'accept_edits',
      '2': 'plan',
      '3': 'full_access'
    };
    if (modeKeys[e.key]) {
      e.preventDefault();
      selectMode(modeKeys[e.key]);
    }
    return;
  }

  // 2. If model menu is open:
  if (showModelMenu.value) {
    if (e.key === 'Escape') {
      e.preventDefault();
      showModelMenu.value = false;
    } else if (e.key === 'Tab') {
      e.preventDefault();
      showModelMenu.value = false;
      if (hasPresets.value) {
        showPresetMenu.value = true;
      }
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      picker.moveFocus(1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      picker.moveFocus(-1);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const item = picker.focusedItem.value;
      if (item) {
        if (item.type === 'action' && item.id === 'add-provider') {
          handleAddProvider();
        } else if (item.type === 'model' && item.option) {
          selectModel(item.option.value);
        }
      }
    } else if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      const item = picker.focusedItem.value;
      if (item && item.type === 'model' && item.option) {
        e.preventDefault();
        picker.cycleReasoningOnFocused(e.key === 'ArrowRight' ? 1 : -1);
      }
    }
    return;
  }

  // 3. If preset menu is open:
  if (showPresetMenu.value) {
    if (e.key === 'Escape') {
      e.preventDefault();
      showPresetMenu.value = false;
    } else if (e.key === 'Tab') {
      e.preventDefault();
      showPresetMenu.value = false;
      showModelMenu.value = true;
    }
    return;
  }
}

onMounted(() => {
  document.addEventListener('click', handleDocumentClick);
  window.addEventListener('keydown', handleKeyDown);
  requestAnimationFrame(() => {
    textarea.value?.focus();
    adjustHeight();
  });
});

onBeforeUnmount(() => {
  document.removeEventListener('click', handleDocumentClick);
  window.removeEventListener('keydown', handleKeyDown);
});
</script>

<template>
  <footer class="composer-wrap">
    <div class="composer-container" style="position: relative;">
      <ConfirmationCard :group="confirmationGroup" @reply="emit('quick-reply', $event)" />

      <PermissionCard
        :request="showPermission ? permissionRequest : null"
        @decide="emit('permission-decision', $event)"
      />

      <QuestionCard
        :request="questionRequest"
        @submit="emit('question-answer', $event)"
      />

      <!-- Token info bar (placed outside the input box) -->
      <div v-if="messages && messages.length > 0" class="composer-token-info">
        <div class="composer-info-strip">
          <span class="composer-info-metric context-tokens" title="Total context (system prompt + history + draft) sent to model">
            <svg class="composer-info-icon" xmlns="http://www.w3.org/2000/svg" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M4 7h16"/>
              <path d="M4 12h16"/>
              <path d="M4 17h10"/>
            </svg>
            <span class="composer-info-label">Context</span>
            <span class="composer-info-value">{{ formattedContext }}</span>
          </span>
          <template v-if="usageParts.length">
            <span class="composer-info-divider"></span>
            <span
              v-for="(part, idx) in usageParts"
              :key="`${part}-${idx}`"
              class="composer-info-metric composer-usage"
              :title="runUsageTooltip"
            >
              <span v-if="idx === 0" class="composer-info-label">Cost</span>
              <span v-else-if="part.includes('calls')" class="composer-info-label">Calls</span>
              <span v-else-if="part.includes('cache')" class="composer-info-label">Cache</span>
              <span class="composer-info-value">{{ part.replace(' calls', '').replace(' cache', '') }}</span>
            </span>
          </template>
        </div>
      </div>

      <div v-if="hasQueuedMessage" class="queued-message-preview">
        <span class="queued-label">Queued</span>
        <span class="queued-text">{{ queuedTaskInput }}</span>
      </div>

      <div class="composer-input-box" style="position: relative; z-index: 2;">
        <!-- Attached Files List -->
        <div v-if="attachedFiles.length > 0" class="composer-attachments">
          <div
            v-for="(file, idx) in attachedFiles"
            :key="idx"
            class="attachment-pill"
            :class="{ 'image-attachment-pill': file.isImage }"
          >
            <img
              v-if="file.isImage"
              class="attachment-preview"
              :src="file.content"
              :alt="file.name"
            />
            <svg v-else class="file-icon" xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/>
              <path d="M14 2v4a2 2 0 0 0 2 2h4"/>
            </svg>
            <span class="attachment-name" :title="file.name">{{ file.name }}</span>
            <button class="remove-attachment-btn" @click="removeAttachment(idx)" title="Remove file">
              <svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="M18 6 6 18"/>
                <path d="m6 6 12 12"/>
              </svg>
            </button>
          </div>
        </div>

        <textarea
          ref="textarea"
          rows="1"
          :value="taskInput"
          :placeholder="isRunning ? 'Queue a follow-up...' : 'Type a message...'"
          @input="emit('update:taskInput', ($event.target as HTMLTextAreaElement).value)"
          @keydown.enter.exact.prevent="handleSend"
        />
        <button
          class="composer-send-btn"
          :class="{ 'stop-mode': isRunning }"
          :disabled="isButtonDisabled"
          :title="isRunning ? 'Cancel generation' : 'Send message'"
          @click="handleButtonClick"
        >
          <svg v-if="isRunning" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="currentColor">
            <rect x="5" y="5" width="14" height="14" rx="1.5" />
          </svg>
          <svg v-else xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
            <line x1="12" y1="19" x2="12" y2="5"></line>
            <polyline points="5 12 12 5 19 12"></polyline>
          </svg>
        </button>
      </div>

      <div class="composer-menu-row" style="position: relative; z-index: 2;">
        <div class="composer-menu-left">
          <div class="mode-selector-wrap">
            <button
              class="mode-pill-btn"
              :class="{ 'mode-pill-danger': currentMode === 'full_access' }"
              @click.stop="showModeMenu = !showModeMenu"
            >
              <span class="mode-pill-text">{{ getModeLabel(currentMode) }}</span>
            </button>

            <div v-if="showModeMenu" class="mode-popup-menu">
              <header class="mode-popup-header">
                <span class="mode-popup-title">Mode</span>
              </header>
              <ul class="mode-popup-list">
                <li
                  v-for="modeItem in MODES_LIST"
                  :key="modeItem.id"
                  :class="{ active: currentMode === modeItem.id, 'mode-item-danger': modeItem.id === 'full_access' }"
                  @click.stop="selectMode(modeItem.id)"
                >
                  <span class="mode-item-name">{{ modeItem.label }}</span>
                  <span class="mode-item-shortcut">{{ modeItem.shortcut }}</span>
                </li>
                <li class="mode-popup-divider"></li>
                <li @click.stop="emit('update:bypassPermissions', !bypassPermissions)">
                  <span class="mode-item-name">Bypass permissions</span>
                  <span class="bypass-toggle-badge" :class="{ enabled: bypassPermissions }">
                    {{ bypassPermissions ? 'Disable' : 'Enable' }}
                  </span>
                </li>
              </ul>
            </div>
          </div>

          <input
            type="file"
            ref="fileInput"
            style="display: none;"
            multiple
            @change="handleFileChange"
          />
          <button
            class="mode-pill-btn attach-file-btn"
            title="Attach files"
            :disabled="isRunning"
            @click="triggerFileSelect"
          >
            <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
              <path d="M5 12h14"/>
              <path d="M12 5v14"/>
            </svg>
          </button>

          <!-- Project selection dropdown (only on landing screen) -->
          <div v-if="isLanding" class="project-dropdown-wrap">
            <button class="mode-pill-btn" @click.stop="showProjectMenu = !showProjectMenu" title="Active Project">
              <svg class="folder-icon" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="color: var(--muted); opacity: 0.85; margin-right: 6px;">
                <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>
              </svg>
              <span class="mode-pill-text">{{ activeProjectName }}</span>
            </button>

            <div v-if="showProjectMenu" class="mode-popup-menu project-popup-menu">
              <header class="mode-popup-header">
                <span class="mode-popup-title">Active Project</span>
              </header>
              <ul class="mode-popup-list">
                <li
                  v-for="project in projectOptions"
                  :key="project.path"
                  :class="{ active: activeProjectPath === project.path }"
                  @click.stop="selectProjectItem(project.path)"
                >
                  <span class="mode-item-name" style="display: flex; align-items: center; gap: 6px;">
                    <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="color: var(--faint);">
                      <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>
                    </svg>
                    {{ project.name }}
                  </span>
                </li>
              </ul>
            </div>
          </div>
        </div>

        <div class="composer-status-section">
          <!-- Optional dual-model agent preset selector (next to the model picker). -->
          <div v-if="hasPresets" class="preset-dropdown-wrap model-dropdown-wrap">
            <button
              class="model-select-display-btn preset-select-btn"
              :class="{ 'preset-active': !!selectedPresetId }"
              @click.stop="showPresetMenu = !showPresetMenu"
            >
              {{ activePresetLabel }}
            </button>
            <div v-if="showPresetMenu" class="model-dropdown-list preset-dropdown-list">
              <!-- Manage presets button -->
              <button
                class="add-provider-btn"
                @click.stop="handleManagePresets"
              >
                <svg class="plus-icon" xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                  <circle cx="12" cy="12" r="3"/>
                  <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/>
                </svg>
                Manage presets
              </button>

              <div class="preset-section-header">
                AGENT CONFIGURATIONS
              </div>

              <!-- Scrollable presets list -->
              <div class="models-list-scroll">
                <!-- Single Model option -->
                <div
                  class="preset-item-row"
                  :class="{ active: !selectedPresetId }"
                  @click.stop="selectPreset('')"
                >
                  <span class="preset-name-wrap">
                    <svg class="preset-icon" xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                      <circle cx="12" cy="12" r="10"/>
                      <circle cx="12" cy="12" r="4"/>
                    </svg>
                    Single model
                  </span>
                  <span class="preset-desc">No delegation</span>
                </div>

                <!-- Preset Options -->
                <div
                  v-for="preset in agentPresets"
                  :key="preset.id"
                  class="preset-item-row"
                  :class="{ active: selectedPresetId === preset.id }"
                  @click.stop="selectPreset(preset.id)"
                >
                  <div class="preset-info-col">
                    <span class="preset-name-wrap">
                      <svg class="preset-icon" xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                        <path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/>
                        <circle cx="9" cy="7" r="4"/>
                        <path d="M22 21v-2a4 4 0 0 0-3-3.87"/>
                        <path d="M16 3.13a4 4 0 0 1 0 7.75"/>
                      </svg>
                      {{ preset.displayName }}
                    </span>
                    <span class="preset-flow-desc">
                      {{ formatModelName(preset.architect.model) }}
                      <span class="preset-flow-arrow">→</span>
                      {{ formatModelName(preset.coder.model) }}
                    </span>
                  </div>
                </div>
              </div>
            </div>
          </div>

          <span v-if="hasPresets" class="status-divider">/</span>

          <div class="model-dropdown-wrap">
            <button
              class="model-select-display-btn"
              :disabled="selectedPresetIsActive"
              :title="selectedPresetIsActive ? 'Architect model is set by the selected agent preset' : ''"
              @click.stop="showModelMenu = !showModelMenu"
            >
              {{ selectedPresetIsActive ? 'Architect: preset' : activeModelDisplayName }}
            </button>
            <div v-if="showModelMenu && !selectedPresetIsActive" class="model-dropdown-list">
              <!-- Add new provider button -->
              <button
                class="add-provider-btn"
                :class="{ focused: isItemFocused('add-provider') }"
                @click.stop="handleAddProvider"
                @mouseenter="setFocusedItem('add-provider')"
              >
                <svg class="plus-icon" xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                  <line x1="12" y1="5" x2="12" y2="19"></line>
                  <line x1="5" y1="12" x2="19" y2="12"></line>
                </svg>
                Add new provider
              </button>

              <!-- Search input -->
              <div class="search-input-wrap">
                <svg class="search-icon" xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                  <circle cx="11" cy="11" r="8"></circle>
                  <line x1="21" y1="21" x2="16.65" y2="16.65"></line>
                </svg>
                <input
                  :ref="(el: unknown) => { picker.searchInputRef.value = el as HTMLInputElement | null; }"
                  v-model="searchQuery"
                  type="text"
                  placeholder="Search models"
                  class="model-search-input"
                  @click.stop
                />
              </div>

              <!-- Models List scroll container -->
              <div class="models-list-scroll" :ref="(el: unknown) => { picker.scrollContainerRef.value = el as HTMLElement | null; }">
                <div
                  v-for="group in groupedModels"
                  :key="group.providerId"
                  class="provider-group"
                >
                  <div class="provider-group-header" @click.stop="toggleProviderCollapse(group.providerId)">
                    <span class="provider-name-wrap">
                      <!-- Inline custom SVGs for providers -->
                      <svg v-if="group.providerId.includes('deepseek')" class="provider-logo" xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M12 21.5c-4.5-2.5-7.5-6.5-7.5-11.5 0-3.5 2.5-6 6-6s6 2.5 6 6c0 5-3 9-6.5 11.5Z"/>
                        <path d="M7 9.5c0-.8.7-1.5 1.5-1.5s1.5.7 1.5 1.5"/>
                      </svg>
                      <svg v-else-if="group.providerId.includes('opencode')" class="provider-logo" xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                        <rect x="3" y="3" width="18" height="18" rx="2" ry="2"/>
                        <line x1="9" y1="9" x2="15" y2="15"/>
                        <line x1="15" y1="9" x2="9" y2="15"/>
                      </svg>
                      <svg v-else-if="group.providerId.includes('openai')" class="provider-logo" xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <circle cx="12" cy="12" r="10"/>
                        <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/>
                        <path d="M2 12h20"/>
                      </svg>
                      <svg v-else-if="group.providerId.includes('anthropic')" class="provider-logo" xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M4 20l8-16 8 16"/>
                        <path d="M6 16h12"/>
                      </svg>
                      <svg v-else-if="group.providerId.includes('google')" class="provider-logo" xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5"/>
                      </svg>
                      <svg v-else class="provider-logo" xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <rect x="3" y="11" width="18" height="10" rx="2"/>
                        <circle cx="12" cy="5" r="2"/>
                        <path d="M12 7v4"/>
                        <line x1="8" y1="16" x2="8" y2="16"/>
                        <line x1="16" y1="16" x2="16" y2="16"/>
                      </svg>
                      {{ group.providerDisplayName.toUpperCase() }}
                    </span>
                    <svg
                      class="chevron-icon"
                      :class="{ collapsed: isProviderCollapsed(group.providerId) }"
                      xmlns="http://www.w3.org/2000/svg"
                      width="10"
                      height="10"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="2.5"
                    >
                      <polyline points="6 9 12 15 18 9"></polyline>
                    </svg>
                  </div>

                  <div v-if="!isProviderCollapsed(group.providerId)" class="provider-models-list">
                    <div
                      v-for="option in group.models"
                      :key="option.value"
                      class="model-item-row"
                      :class="{
                        active: selectedModel === option.value,
                        focused: isItemFocused(option.value)
                      }"
                      @click.stop="selectModel(option.value)"
                      @mouseenter="setFocusedItem(option.value)"
                    >
                      <span class="model-name-text">
                        {{ formatModelName(option.model) }}
                        <span class="model-context-badge" v-if="option.contextLimit">
                          {{ formatContextLimit(option.contextLimit) }}
                        </span>
                      </span>

                      <span
                        v-if="option.reasoningOptions && option.reasoningOptions.length > 0 && (selectedModel === option.value || isItemFocused(option.value))"
                        class="model-thinking-label"
                      >
                        Thinking: {{ selectedModel === option.value ? activeReasoningLabel : 'Default' }}
                      </span>

                      <button
                        class="star-btn"
                        :class="{ starred: isStarred(option.value) }"
                        @click.stop="toggleFavorite(option.value)"
                        title="Toggle favorite"
                      >
                        <svg
                          xmlns="http://www.w3.org/2000/svg"
                          width="12"
                          height="12"
                          viewBox="0 0 24 24"
                          :fill="isStarred(option.value) ? 'currentColor' : 'none'"
                          stroke="currentColor"
                          stroke-width="2.2"
                        >
                          <polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2"></polygon>
                        </svg>
                      </button>
                    </div>
                  </div>
                </div>

                <div v-if="groupedModels.length === 0" class="empty-search-state">
                  No models found
                </div>
              </div>

              <!-- Footer bar -->
              <div class="model-dropdown-footer">
                <span><span class="footer-key">↑</span><span class="footer-key">↓</span> navigate</span>
                <span><span class="footer-key">Tab</span> switch agent</span>
                <span><span class="footer-key">←</span><span class="footer-key">→</span> thinking</span>
              </div>
            </div>
          </div>

          <span v-if="hasReasoningEffortOptions" class="status-divider">/</span>
          <div v-if="hasReasoningEffortOptions" class="reasoning-dropdown-wrap model-dropdown-wrap">
            <button
              class="model-select-display-btn parameter-pill"
              title="Reasoning effort"
              @click.stop="showReasoningMenu = !showReasoningMenu"
            >
              {{ activeReasoningLabel }}
            </button>
            <div v-if="showReasoningMenu" class="model-dropdown-list reasoning-dropdown-list">
              <div class="preset-section-header">
                REASONING EFFORT
              </div>
              <div class="models-list-scroll">
                <div
                  v-for="option in reasoningEffortOptions"
                  :key="option.id"
                  class="preset-item-row"
                  :class="{ active: selectedReasoningEffort === option.id }"
                  @click.stop="selectReasoningEffort(option.id)"
                >
                  <div class="preset-info-col">
                    <span class="preset-name-wrap">
                      <span class="reasoning-dot" :class="option.id"></span>
                      {{ option.label }}
                    </span>
                    <span class="preset-flow-desc" style="padding-left: 14px;">
                      {{ getReasoningDesc(option.id) }}
                    </span>
                  </div>
                </div>
              </div>
            </div>
          </div>
          <span class="status-indicator-ring" :class="{ active: isRunning }">
            <span class="ring-dot"></span>
          </span>
        </div>
      </div>
    </div>
  </footer>
</template>

<style scoped>
.composer-container {
  width: min(1000px, 100%);
  margin: 0 auto;
  display: flex;
  flex-direction: column;
}

.composer-input-box {
  position: relative;
  background: var(--control-bg);
  border: 1px solid var(--control-border);
  border-radius: 14px;
  padding: 13px 88px 13px 16px;
  box-shadow:
    0 18px 45px var(--composer-input-shadow),
    inset 0 1px 0 var(--composer-input-inset-shadow);
  transition: border-color 0.2s ease, box-shadow 0.2s ease, background 0.2s ease;
}

.composer-input-box:focus-within {
  border-color: var(--control-border-focus);
  box-shadow:
    0 20px 55px var(--composer-input-focus-shadow),
    0 0 0 1px var(--composer-input-focus-border-shadow),
    inset 0 1px 0 var(--composer-input-focus-inset-shadow);
}

.composer-token-info {
  display: flex;
  justify-content: flex-end;
  align-items: center;
  margin-bottom: 8px;
  font-size: 0.75rem;
  color: var(--faint);
  user-select: none;
  padding: 0 4px;
}

.draft-tokens {
  display: flex;
  align-items: center;
  gap: 5px;
}

.token-dot {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: var(--composer-token-inactive);
  transition: all 0.2s ease;
}

.token-dot.active.thinking {
  background: var(--warning);
  box-shadow: 0 0 6px var(--warning);
  animation: pulseDot 1.2s infinite alternate;
}

.token-dot.active.generating {
  background: var(--success);
  box-shadow: 0 0 6px var(--success);
  animation: pulseDot 1.2s infinite alternate;
}

.token-dot.last-response {
  background: var(--muted);
}

.context-tokens {
  color: var(--text);
}

.composer-info-strip {
  display: inline-flex;
  align-items: center;
  gap: 0;
  min-height: 28px;
  max-width: 100%;
  padding: 4px 7px;
  border-radius: 10px;
  background:
    linear-gradient(180deg, rgba(255, 255, 255, 0.055), rgba(255, 255, 255, 0.025));
  border: 1px solid rgba(255, 255, 255, 0.07);
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.04);
}

.composer-info-metric {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  min-width: 0;
  padding: 0 7px;
  white-space: nowrap;
  line-height: 1.2;
}

.composer-info-icon {
  color: rgba(255, 255, 255, 0.44);
  flex-shrink: 0;
}

.composer-info-divider {
  width: 1px;
  height: 14px;
  margin: 0 4px;
  background: rgba(255, 255, 255, 0.1);
  flex-shrink: 0;
}

.composer-info-label {
  color: rgba(255, 255, 255, 0.42);
  font-size: 0.66rem;
  font-weight: 500;
  letter-spacing: 0;
}

.composer-info-value {
  color: rgba(255, 255, 255, 0.84);
  font-variant-numeric: tabular-nums;
  font-weight: 500;
}

.composer-usage {
  color: var(--text);
}

.queued-message-preview {
  position: relative;
  z-index: 3;
  display: flex;
  align-items: baseline;
  gap: 10px;
  width: 100%;
  margin-bottom: 8px;
  padding: 9px 12px;
  background: var(--composer-queued-msg-bg);
  border: 1px solid var(--composer-queued-msg-border);
  border-radius: 8px;
  color: var(--muted);
  font-size: 0.86rem;
  font-style: italic;
  box-shadow: 0 8px 22px var(--composer-dropdown-shadow);
}

.queued-label {
  flex: 0 0 auto;
  color: var(--faint);
  font-size: 0.72rem;
  text-transform: uppercase;
}

.queued-text {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

@keyframes pulseDot {
  0% { opacity: 0.4; }
  100% { opacity: 1; }
}

.composer-input-box textarea {
  display: block;
  width: 100%;
  height: 24px;
  min-height: 24px;
  max-height: 240px; /* 10 lines * 24px line-height */
  line-height: 24px;
  resize: none;
  overflow-y: auto;
  border: 0;
  outline: none;
  background: transparent;
  color: var(--text);
  font-size: 0.96rem;
  padding: 0;
  margin: 0;
}

.composer-send-btn {
  position: absolute;
  bottom: 9px;
  right: 11px;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  background: var(--btn-primary-bg);
  color: var(--btn-primary-color);
  border: none;
  border-radius: 50%;
  cursor: pointer;
  box-shadow: 0 8px 18px var(--composer-shadow);
  transition: all 0.2s cubic-bezier(0.4, 0, 0.2, 1);
}

.composer-send-btn:hover:not(:disabled) {
  background: var(--btn-primary-hover-bg);
  transform: translateY(-1px);
  box-shadow: 0 10px 24px var(--composer-dropdown-shadow);
}

.composer-send-btn:active:not(:disabled) {
  background: var(--btn-primary-active-bg);
  transform: translateY(0);
}

.composer-send-btn:disabled {
  opacity: 0.35;
  cursor: not-allowed;
  background: var(--surface-strong);
  color: var(--faint);
  border: 1px solid var(--border);
}

.composer-send-btn.stop-mode {
  background: var(--danger-soft);
  color: var(--danger);
  border: 1px solid var(--danger-border);
}

.composer-send-btn.stop-mode:hover {
  background: var(--danger-soft-strong);
}

.composer-send-btn.stop-mode:active {
  background: var(--btn-danger-active-bg);
}

.composer-menu-row {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-top: 11px;
  padding: 6px;
  gap: 10px;
}

.composer-menu-left {
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 6px;
  flex: 1 1 auto;
}

.mode-selector-wrap,
.project-dropdown-wrap {
  position: relative;
}

.project-popup-menu {
  width: 250px;
}

.mode-pill-btn {
  display: flex;
  align-items: center;
  min-height: 30px;
  background: transparent;
  border: 1px solid transparent;
  border-radius: 8px;
  padding: 5px 10px;
  color: var(--muted);
  font-size: 0.8rem;
  cursor: pointer;
  transition: all 0.2s ease;
  user-select: none;
  gap: 8px;
}

.mode-pill-btn:hover {
  border-color: var(--control-border);
  color: var(--text);
  background: var(--nav-action-hover-bg);
}

/* Full Access is a no-confirmation mode — flag it red wherever it surfaces. */
.mode-pill-btn.mode-pill-danger {
  color: var(--danger);
  border-color: var(--danger-border);
  background: var(--danger-soft);
}

.mode-pill-btn.mode-pill-danger:hover {
  color: var(--danger);
  background: var(--danger-soft-strong);
}

.mode-pill-text {
  font-weight: 500;
}

.mode-popup-menu {
  position: absolute;
  bottom: calc(100% + 8px);
  left: 0;
  width: 230px;
  background: var(--composer-dropdown-bg);
  border: 1px solid var(--composer-dropdown-border);
  border-radius: 10px;
  box-shadow: 0 12px 30px var(--composer-dropdown-shadow);
  z-index: 1000;
  overflow: hidden;
  padding: 6px;
  animation: menuAppear 0.15s ease-out;
}

@keyframes menuAppear {
  from { opacity: 0; transform: translateY(4px); }
  to { opacity: 1; transform: translateY(0); }
}

.mode-popup-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 6px 10px 4px;
  border-bottom: 1px solid var(--perm-code-border);
}

.mode-popup-title {
  color: var(--faint);
  font-size: 0.72rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.mode-popup-list {
  list-style: none;
  padding: 4px 0 0 0;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.mode-popup-list li {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 7px 10px;
  border-radius: 6px;
  color: var(--muted);
  font-size: 0.82rem;
  cursor: pointer;
  transition: all 0.15s ease;
}

.mode-popup-list li:hover {
  background: var(--attachment-pill-bg);
  color: var(--text);
}

.mode-popup-list li.active {
  color: var(--text);
  background: var(--composer-menu-active-bg);
}

.mode-popup-list li.mode-item-danger .mode-item-name {
  color: var(--danger);
}

.mode-popup-list li.mode-item-danger.active {
  background: var(--danger-soft);
}

.mode-item-name {
  display: flex;
  align-items: center;
  gap: 8px;
}

.mode-item-shortcut {
  font-family: monospace;
  font-size: 0.75rem;
  color: var(--faint);
  background: var(--composer-menu-shortcut-bg);
  padding: 1px 5px;
  border-radius: 3px;
  border: 1px solid var(--composer-dropdown-border);
}

.mode-popup-divider {
  height: 1px;
  background: var(--perm-code-border);
  margin: 4px 0;
  padding: 0 !important;
  pointer-events: none;
}

.bypass-toggle-badge {
  font-size: 0.72rem;
  padding: 2px 6px;
  border-radius: 4px;
  background: var(--attachment-pill-bg);
  color: var(--muted);
  transition: all 0.2s ease;
  font-weight: 500;
}

.bypass-toggle-badge.enabled {
  background: var(--success-soft);
  color: var(--success);
}

.composer-status-section {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 6px;
  min-width: 0;
  flex: 0 1 auto;
  color: var(--faint);
  font-size: 0.8rem;
}

.model-dropdown-wrap {
  position: relative;
}

.model-select-display-btn {
  max-width: 220px;
  background: transparent;
  color: var(--muted);
  font-size: 0.8rem;
  cursor: pointer;
  transition: color 0.2s ease, background 0.2s ease, border-color 0.2s ease;
  padding: 5px 8px;
  border-radius: 7px;
  border: 1px solid transparent;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.model-select-display-btn:hover {
  color: var(--text);
  background: var(--nav-action-hover-bg);
  border-color: var(--control-border);
}

.model-select-display-btn:disabled {
  cursor: default;
  opacity: 0.55;
}

.model-select-display-btn:disabled:hover {
  background: transparent;
  color: var(--muted);
}

/* Dual-model preset selector: emphasize when a preset (not single model) is on. */
.preset-select-btn.preset-active {
  color: var(--text);
  background: var(--composer-preset-active-bg);
  border-color: var(--composer-preset-active-border);
  font-weight: 600;
}

.model-dropdown-item .preset-sub {
  margin-left: auto;
  padding-left: 12px;
  font-size: 0.72rem;
  color: var(--faint);
  white-space: nowrap;
}

.model-dropdown-list {
  position: absolute;
  bottom: calc(100% + 8px);
  right: 0;
  width: 335px;
  height: 380px;
  max-height: 80vh;
  display: flex;
  flex-direction: column;
  background: var(--composer-dropdown-bg);
  border: 1px solid var(--composer-dropdown-border);
  border-radius: 12px;
  box-shadow: 0 20px 45px var(--composer-dropdown-shadow);
  z-index: 1000;
  overflow: hidden;
  padding: 0;
  animation: menuAppear 0.15s ease-out;
}

.add-provider-btn {
  width: calc(100% - 16px);
  margin: 8px 8px 4px 8px;
  display: flex;
  align-items: center;
  gap: 8px;
  height: 32px;
  padding: 0 10px;
  background: transparent;
  border: 1px solid transparent;
  border-radius: 6px;
  color: var(--muted);
  font-size: 0.8rem;
  font-weight: 500;
  cursor: pointer;
  text-align: left;
  transition: all 0.15s ease;
  flex-shrink: 0;
}

.add-provider-btn:hover,
.add-provider-btn.focused {
  background: var(--attachment-pill-bg);
  color: var(--text);
  border-color: var(--control-border);
}

.plus-icon {
  opacity: 0.75;
}

.search-input-wrap {
  position: relative;
  margin: 4px 8px 8px 8px;
  flex-shrink: 0;
}

.search-icon {
  position: absolute;
  left: 10px;
  top: 50%;
  transform: translateY(-50%);
  color: var(--faint);
  pointer-events: none;
}

.model-search-input {
  width: 100%;
  height: 34px;
  padding: 0 12px 0 32px;
  background: var(--control-bg);
  border: 1px solid var(--control-border);
  border-radius: 6px;
  color: var(--text);
  font-size: 0.82rem;
  outline: none;
  transition: all 0.2s ease;
}

.model-search-input:focus {
  border-color: var(--control-border-focus);
  background: var(--control-bg-focus);
  box-shadow: 0 0 0 1px var(--composer-input-focus-border-shadow);
}

.models-list-scroll {
  flex: 1;
  overflow-y: auto;
  padding: 0 8px 8px 8px;
  scrollbar-width: thin;
}

.provider-group {
  margin-top: 10px;
}

.provider-group:first-child {
  margin-top: 4px;
}

.provider-group-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 6px;
  font-size: 0.72rem;
  font-weight: 600;
  letter-spacing: 0.5px;
  color: var(--faint);
  user-select: none;
  cursor: pointer;
  border-radius: 4px;
  transition: background 0.15s ease, color 0.15s ease;
}

.provider-group-header:hover {
  background: var(--attachment-pill-bg);
  color: var(--muted);
}

.provider-name-wrap {
  display: flex;
  align-items: center;
  gap: 8px;
}

.provider-logo {
  opacity: 0.8;
  flex-shrink: 0;
}

.chevron-icon {
  transition: transform 0.2s ease;
  opacity: 0.6;
}

.chevron-icon.collapsed {
  transform: rotate(-90deg);
}

.model-item-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 36px;
  padding: 0 8px 0 10px;
  margin-top: 2px;
  border-radius: 6px;
  color: var(--muted);
  font-size: 0.82rem;
  cursor: pointer;
  transition: all 0.15s ease;
  user-select: none;
}

.model-item-row:hover,
.model-item-row.focused {
  background: var(--attachment-pill-bg);
  color: var(--text);
}

.model-item-row.active {
  color: var(--text);
  background: var(--composer-menu-active-bg);
  font-weight: 500;
}

.model-name-text {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  text-align: left;
}

.model-context-badge {
  font-size: 0.72rem;
  color: var(--faint);
  font-weight: 400;
}

.model-thinking-label {
  font-size: 0.72rem;
  color: var(--faint);
  margin-left: auto;
  margin-right: 8px;
  white-space: nowrap;
}

.star-btn {
  background: transparent;
  border: none;
  color: var(--faint);
  cursor: pointer;
  padding: 4px;
  border-radius: 4px;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: all 0.15s ease;
}

.star-btn:hover {
  color: var(--warning);
  background: rgba(255, 193, 7, 0.1);
}

.star-btn.starred {
  color: var(--warning);
}

.empty-search-state {
  padding: 24px;
  text-align: center;
  font-size: 0.8rem;
  color: var(--faint);
  font-style: italic;
}

.model-dropdown-footer {
  height: 28px;
  background: var(--composer-queued-msg-bg);
  border-top: 1px solid var(--composer-dropdown-border);
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 12px;
  font-size: 0.68rem;
  color: var(--faint);
  user-select: none;
  flex-shrink: 0;
}

.footer-key {
  font-family: monospace;
  background: var(--composer-menu-shortcut-bg);
  border: 1px solid var(--composer-dropdown-border);
  padding: 0px 4px;
  margin: 0 2px;
  border-radius: 3px;
  color: var(--muted);
  font-weight: 600;
}

.status-divider {
  color: var(--composer-indicator-border);
  user-select: none;
}

.parameter-pill {
  color: var(--muted);
  padding: 5px 7px;
  border-radius: 7px;
  background: var(--composer-parameter-bg);
  border: 1px solid transparent;
}

.reasoning-dropdown-list {
  width: 260px !important;
  height: auto !important;
  max-height: 280px !important;
}

.reasoning-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--muted);
  display: inline-block;
  flex-shrink: 0;
}

.reasoning-dot.default { background: var(--text); }
.reasoning-dot.none { background: var(--faint); }
.reasoning-dot.minimal { background: #5bc0de; }
.reasoning-dot.low { background: var(--success); }
.reasoning-dot.medium { background: var(--warning); }
.reasoning-dot.high { background: #f0ad4e; }
.reasoning-dot.xhigh { background: #d9534f; }
.reasoning-dot.max { background: var(--danger); }

/* Redesigned Preset Dropdown List overrides */
.preset-dropdown-list {
  width: 290px !important;
  height: auto !important;
  max-height: 320px !important;
}

.preset-section-header {
  padding: 6px 12px;
  font-size: 0.68rem;
  font-weight: 700;
  letter-spacing: 0.5px;
  color: var(--faint);
  user-select: none;
}

.preset-item-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 12px;
  margin-top: 2px;
  border-radius: 6px;
  color: var(--muted);
  font-size: 0.82rem;
  cursor: pointer;
  transition: all 0.15s ease;
  user-select: none;
}

.preset-item-row:hover {
  background: var(--attachment-pill-bg);
  color: var(--text);
}

.preset-item-row.active {
  color: var(--text);
  background: var(--composer-menu-active-bg);
  font-weight: 500;
}

.preset-name-wrap {
  display: flex;
  align-items: center;
  gap: 8px;
  font-weight: 500;
}

.preset-icon {
  opacity: 0.75;
  flex-shrink: 0;
}

.preset-desc {
  font-size: 0.72rem;
  color: var(--faint);
}

.preset-info-col {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
  width: 100%;
}

.preset-flow-desc {
  font-size: 0.72rem;
  color: var(--faint);
  padding-left: 20px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  text-align: left;
}

.preset-flow-arrow {
  margin: 0 4px;
  color: var(--faint);
}

.status-indicator-ring {
  width: 16px;
  height: 16px;
  border-radius: 50%;
  border: 1.5px solid var(--composer-indicator-border);
  display: flex;
  align-items: center;
  justify-content: center;
  transition: border-color 0.3s ease;
}

.status-indicator-ring.active {
  border-color: var(--success);
}

.ring-dot {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: var(--composer-indicator-dot);
  transition: background 0.3s ease;
}

.status-indicator-ring.active .ring-dot {
  background: var(--success);
  animation: pulseDot 1.2s infinite alternate;
}

/* File attachments styling inside the input box */
.composer-attachments {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 10px;
  padding-bottom: 10px;
  border-bottom: 1px solid var(--composer-dropdown-border);
}

.attachment-pill {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: var(--attachment-pill-bg);
  border: 1px solid var(--attachment-pill-border);
  border-radius: 6px;
  padding: 4px 8px;
  font-size: 0.78rem;
  color: var(--text);
  box-shadow: 0 2px 5px var(--attachment-shadow);
}

.attachment-pill .file-icon {
  color: var(--muted);
  flex-shrink: 0;
}

.image-attachment-pill {
  padding: 4px 6px 4px 4px;
}

.attachment-preview {
  width: 36px;
  height: 36px;
  object-fit: cover;
  border-radius: 6px;
  border: 1px solid var(--attachment-pill-border);
  flex-shrink: 0;
  background: var(--surface-elevated);
}

.attachment-name {
  max-width: 150px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.remove-attachment-btn {
  background: transparent;
  color: var(--faint);
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  padding: 2px;
  border-radius: 3px;
  transition: all 0.2s ease;
}

.remove-attachment-btn:hover {
  color: var(--danger);
  background: var(--danger-soft);
}

/* Attach file button styling */
.attach-file-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 6px;
  width: 30px;
  height: 30px;
  border-radius: 8px;
  flex-shrink: 0;
}
</style>
