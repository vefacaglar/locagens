<script setup lang="ts">
import { useAppShell } from './composables/useAppShell';
import type { Run } from '@locagens/shared';
import AppSidebar from './components/AppSidebar.vue';
import MessageThread from './components/MessageThread.vue';
import ChatComposer from './components/ChatComposer.vue';
import AddProjectModal from './components/AddProjectModal.vue';
import SettingsScreen from './components/settings/SettingsScreen.vue';
import AgentTaskList from './components/AgentTaskList.vue';
import PlanPanel from './components/PlanPanel.vue';
import UsageLogsPage from './components/UsageLogsPage.vue';
import { collectWorkspaceChanges, hasWorkspaceChangeSignals } from './lib/workspaceChanges';
import { collectAgentSummaries, collectAgentSummaryLinks } from './lib/messageGroups';

const {
  runs,
  agentPresets,
  loadAgentPresets,
  setMessagesContainer,
  settings,
  projects,
  permissions,
  memories,
  openSettings,
  chat,
  isMac,
  selectProject,
  selectProjectAndNewChat,
  submitProject,
  deleteProject,
  providersConfig,
  isProvidersConfigLoading,
  reloadProviders,
  showUsageLogsPage,
  appSettings,
  isAppSettingsLoading,
  loadAppSettings
} = useAppShell();

const {
  activeRunId,
  activeRun,
  isRunning,
  taskInput,
  queuedTaskInput,
  focusSignal,
  showPermissionModal,
  pendingPermissionRequest,
  pendingQuestionRequest,
  groupedMessages,
  visibleTitle,
  activeConfirmationGroup,
  messages,
  sidePanelMessages,
  sidePanelGroupedMessages,
  currentPlan,
  runUsage
} = chat;

import { ref, computed, watch, onMounted, onUnmounted, nextTick } from 'vue';
import { useCustomDialog } from './composables/useCustomDialog';

const { activeDialog } = useCustomDialog();

const settingsTab = ref<'permissions' | 'memory' | 'providers' | 'search' | 'agents' | 'server'>('permissions');

async function handleOpenSettings(tab?: string) {
  if (tab === 'permissions' || tab === 'memory' || tab === 'providers' || tab === 'search' || tab === 'agents' || tab === 'server') {
    settingsTab.value = tab;
  } else {
    settingsTab.value = 'permissions';
  }
  await openSettings();
  await loadAppSettings();
}

function handleEscape(e: KeyboardEvent) {
  if (e.key === 'Escape' && activeDialog.value) {
    activeDialog.value.resolve(false);
  }
}
onMounted(() => {
  window.addEventListener('keydown', handleEscape);
});
onUnmounted(() => {
  window.removeEventListener('keydown', handleEscape);
  clearSidePanelToggleTimer();
});

const isSidebarCollapsed = ref(false);
function toggleSidebar() {
  isSidebarCollapsed.value = !isSidebarCollapsed.value;
}

function handleSelectRun(run: Run) {
  chat.selectRun(run);
  showUsageLogsPage.value = false;
  if (window.innerWidth <= 760) {
    isSidebarCollapsed.value = true;
    sidePanelCollapsed.value = true;
  }
}

function handleNewChat() {
  chat.startNewRunSetup();
  showUsageLogsPage.value = false;
  if (window.innerWidth <= 760) {
    isSidebarCollapsed.value = true;
    sidePanelCollapsed.value = true;
  }
}

function handleSelectProjectAndNewChat(path: string) {
  selectProjectAndNewChat(path);
  showUsageLogsPage.value = false;
  if (window.innerWidth <= 760) {
    isSidebarCollapsed.value = true;
    sidePanelCollapsed.value = true;
  }
}

function handleSelectRunFromLogs(runId: string) {
  const run = runs.value.find(r => r.id === runId);
  if (run) {
    handleSelectRun(run);
  } else {
    alert("Chat session not found or has been deleted.");
  }
}

// Side panel resizable width logic
const sidePanelWidth = ref(Number(localStorage.getItem('sidePanelWidth') || '480'));
const isResizing = ref(false);
function handleSidePanelResize(newWidth: number) {
  const clamped = Math.max(360, Math.min(800, newWidth));
  sidePanelWidth.value = clamped;
  localStorage.setItem('sidePanelWidth', String(clamped));
}

const currentProjectName = computed(() => {
  const current = projects.projectOptions.value.find(p => p.path === projects.activeProjectPath.value);
  return current ? current.name : 'Unknown Project';
});

// Compact cost summary for the active run, shown next to the breadcrumb once
// usage exists. Costs are tiny, so sub-dollar amounts keep 4 decimals.
const runUsageLabel = computed<string | null>(() => {
  const u = runUsage.value;
  if (!u || u.totalCalls === 0) return null;
  const cost = u.totalCost >= 1 ? `$${u.totalCost.toFixed(2)}` : `$${u.totalCost.toFixed(4)}`;
  return `${cost} · ${u.totalCalls} calls · ${u.avgCacheHitRate}% cache`;
});

const runUsageTooltip = computed<string>(() => {
  const u = runUsage.value;
  if (!u) return '';
  return u.byRole
    .map(r => `${r.agentRole}: $${r.cost.toFixed(4)} (${r.calls} calls, ${r.inputTokens.toLocaleString()} in / ${r.outputTokens.toLocaleString()} out)`)
    .join('\n');
});

// The live task checklist pinned above the composer. It is driven entirely by
// the latest <task_list> text block the assistant emitted, so it refreshes with
// every message and stays independent of the right-hand plan panel (which holds
// the stable plan document and only changes on an explicit revision request).
const currentTaskList = computed<string | null>(() => {
  const list = messages.value;
  if (!list) return null;
  for (let i = list.length - 1; i >= 0; i--) {
    const msg = list[i];
    if (msg.role !== 'assistant') continue;
    const match = msg.content.match(/<task_list>([\s\S]*?)<\/task_list>/);
    if (match) return match[1].trim();
  }
  return null;
});

// The side panel mirrors Codex / Claude desktop: it opens automatically as soon
// as the assistant creates a plan, edits workspace files, or starts sub-agents.
// Closing it collapses every panel tab until the user re-opens a thread link or
// switches chats.
const sidePanelCollapsed = ref(true);
const agentSummaryLinks = computed(() => collectAgentSummaryLinks(groupedMessages.value, isRunning.value));
const hasWorkspaceChanges = computed(() => hasWorkspaceChangeSignals(sidePanelMessages.value));
const workspaceChanges = computed(() =>
  sidePanelOpen.value ? collectWorkspaceChanges(sidePanelMessages.value, activeRun.value?.projectPath) : []
);
const agentSummaries = computed(() =>
  sidePanelOpen.value ? collectAgentSummaries(sidePanelGroupedMessages.value, isRunning.value) : []
);
const hasSidePanelContent = computed(() =>
  !!currentPlan.value || hasWorkspaceChanges.value || agentSummaryLinks.value.length > 0
);
const sidePanelOpen = computed(() =>
  hasSidePanelContent.value && !sidePanelCollapsed.value
);
const showSidePanelToggle = ref(false);
const SIDE_PANEL_TRANSITION_MS = 300;
let sidePanelToggleTimer: ReturnType<typeof setTimeout> | null = null;

function clearSidePanelToggleTimer() {
  if (sidePanelToggleTimer) {
    clearTimeout(sidePanelToggleTimer);
    sidePanelToggleTimer = null;
  }
}

watch([hasSidePanelContent, sidePanelOpen], ([hasContent, isOpen]) => {
  clearSidePanelToggleTimer();
  if (!hasContent || isOpen) {
    showSidePanelToggle.value = false;
    return;
  }

  sidePanelToggleTimer = setTimeout(() => {
    showSidePanelToggle.value = true;
    sidePanelToggleTimer = null;
  }, SIDE_PANEL_TRANSITION_MS);
}, { immediate: true });

// Watch and persist sidePanelCollapsed changes on desktop, keyed by activeRunId
watch(sidePanelCollapsed, (val) => {
  if (window.innerWidth > 760 && activeRunId.value) {
    localStorage.setItem(`sidePanelCollapsed:${activeRunId.value}`, val ? 'true' : 'false');
  }
});

// Reset the collapsed state when switching chats using the stored preference, but keep it collapsed on mobile.
watch(activeRunId, (newId) => {
  if (window.innerWidth <= 760) {
    sidePanelCollapsed.value = true;
  } else if (newId) {
    const stored = localStorage.getItem(`sidePanelCollapsed:${newId}`);
    if (stored !== null) {
      sidePanelCollapsed.value = stored === 'true';
    } else {
      // Default to expanded (false) for a chat if no preference is saved yet
      sidePanelCollapsed.value = false;
    }
  } else {
    sidePanelCollapsed.value = true;
  }
}, { immediate: true });

// Automatically re-open the panel if it was closed and the assistant revises the plan (only on desktop).
watch(
  () => currentPlan.value?.version,
  (newVer, oldVer) => {
    if (newVer !== undefined && oldVer !== undefined && newVer !== oldVer) {
      if (window.innerWidth > 760) {
        sidePanelCollapsed.value = false;
      }
    }
  }
);

const planPanelRef = ref<any>(null);

async function openAgentTranscript(agentId: string) {
  sidePanelCollapsed.value = false;
  await nextTick();
  planPanelRef.value?.expandAgentTranscript?.(agentId);
}

async function openPlan() {
  sidePanelCollapsed.value = false;
  await nextTick();
  planPanelRef.value?.selectTab?.('plan');
}

async function openAgents() {
  sidePanelCollapsed.value = false;
  await nextTick();
  planPanelRef.value?.selectTab?.('agents');
}

async function openFileInReview(filePath: string) {
  sidePanelCollapsed.value = false;
  await nextTick();
  planPanelRef.value?.openFileInReview?.(filePath);
}

// --- Plan approval actions (Start / Revise / Reject) ----------------------
// While in plan mode, the panel offers three choices once a plan is presented.
// The decision is tracked per plan version so the buttons reappear if the
// assistant produces a revised plan.
const isPlanMode = computed(() => settings.currentMode.value === 'plan');
const planKey = computed(() =>
  currentPlan.value ? `${currentPlan.value.id}:${currentPlan.value.version}` : ''
);
const decidedPlanKey = ref('');

const latestPlanProposalMessageIndex = computed(() => {
  const list = groupedMessages.value;
  if (!list) return -1;
  for (let i = list.length - 1; i >= 0; i--) {
    const group = list[i];
    if (group.type === 'tool_group') {
      const hasUpdatePlan = group.toolCalls.some(tc => tc.function?.name === 'update_plan');
      if (hasUpdatePlan) return i;
    }
  }
  return -1;
});

const showPlanActions = computed(() => {
  if (!isPlanMode.value || !currentPlan.value) return false;

  const planIndex = latestPlanProposalMessageIndex.value;
  if (planIndex === -1) return false;

  // If there is any user message after the latest plan proposal, the user has already interacted with it
  for (let i = planIndex + 1; i < groupedMessages.value.length; i++) {
    if (groupedMessages.value[i].type === 'user') {
      return false;
    }
  }

  return decidedPlanKey.value !== planKey.value;
});

watch(activeRunId, () => { decidedPlanKey.value = ''; });

// Start: approve the plan, switch to build (accept edits) mode, and kick off
// implementation with a follow-up message. We flip the mode FIRST and wait a
// tick so the continue request (and the system prompt it rebuilds) is already
// in Build mode before the approval message is sent — otherwise the model can
// reply as if it were still in Plan mode and ask to switch again.
async function startPlan() {
  decidedPlanKey.value = planKey.value;
  settings.currentMode.value = 'accept_edits';
  await nextTick();
  chat.sendQuickReply(
    "I approve this plan. We are now in Build mode — implement it step by step right away. Do not ask me to switch modes; you are already in Build mode."
  );
}

// Revise: stay in plan mode and focus the composer so the user can describe the
// changes they want to the plan.
function revisePlan() {
  decidedPlanKey.value = planKey.value;
  focusSignal.value++;
}

// Reject: tell the model the plan is turned down so it does NOT implement it,
// and stay in plan mode. Without sending this message the model never learns it
// was rejected and may go ahead and build the plan anyway.
async function rejectPlan() {
  decidedPlanKey.value = planKey.value;
  if (settings.currentMode.value !== 'plan') {
    settings.currentMode.value = 'plan';
    await nextTick();
  }
  chat.sendQuickReply(
    "I reject this plan. Do NOT implement it or make any changes. Stay in Plan mode and wait for my further instructions."
  );
}

// Automatically collapse left and right panels when screen is resized to mobile width (760px)
const isWindowResizing = ref(false);
let resizeTimeout: ReturnType<typeof setTimeout> | null = null;
let lastWidth = window.innerWidth;

function handleWindowResize() {
  isWindowResizing.value = true;
  if (resizeTimeout) clearTimeout(resizeTimeout);
  resizeTimeout = setTimeout(() => {
    isWindowResizing.value = false;
  }, 150);

  const currentWidth = window.innerWidth;
  if (currentWidth <= 760 && lastWidth > 760) {
    if (!isSidebarCollapsed.value) isSidebarCollapsed.value = true;
    if (!sidePanelCollapsed.value) sidePanelCollapsed.value = true;
  }
  lastWidth = currentWidth;
}

onMounted(() => {
  window.addEventListener('resize', handleWindowResize);
  if (window.innerWidth <= 760) {
    isSidebarCollapsed.value = true;
    sidePanelCollapsed.value = true;
  }
});

onUnmounted(() => {
  window.removeEventListener('resize', handleWindowResize);
});
</script>

<template>
  <div
    class="app-shell"
    :class="{
      'sidebar-collapsed': isSidebarCollapsed,
      'panel-available': hasSidePanelContent && !showUsageLogsPage,
      'panel-open': sidePanelOpen && !showUsageLogsPage,
      'is-resizing': isResizing,
      'window-resizing': isWindowResizing
    }"
    :style="{ '--side-panel-w': `${sidePanelWidth}px` }"
  >
    <!-- Full-width draggable strip across the very top of the window (desktop
         only). Fills the dead zone above the header bars so double-click-to-
         zoom works across the entire window width, not just the header areas. -->
    <div class="window-drag-strip"></div>
    <AppSidebar
      :project-options="projects.projectOptions.value"
      :active-project-path="projects.activeProjectPath.value"
      :runs="runs"
      :active-run-id="activeRunId"
      :is-sidebar-collapsed="isSidebarCollapsed"
      :show-usage-logs-page="showUsageLogsPage"
      @new-chat="handleNewChat"
      @add-project="projects.openAddProjectModal"
      @select-project="selectProject"
      @select-project-and-new-chat="handleSelectProjectAndNewChat"
      @select-run="handleSelectRun"
      @delete-project="deleteProject"
      @open-settings="handleOpenSettings"
      @toggle-sidebar="toggleSidebar"
      @open-usage-logs="showUsageLogsPage = true"
    />

    <main class="chat-shell" :class="{ 'landing-mode': !activeRun && !showUsageLogsPage }">
      <header class="chat-header">
        <div class="chat-header-inner">
          <div class="thread-title">
            <button v-if="isSidebarCollapsed" class="panel-toggle-btn expand-sidebar-btn" @click="toggleSidebar" title="Expand Sidebar">
              <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <rect width="18" height="18" x="3" y="3" rx="2" />
                <path d="M9 3v18" />
              </svg>
            </button>
            
            <div v-if="showUsageLogsPage" class="project-breadcrumb">
              <svg class="folder-icon" xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <line x1="18" y1="20" x2="18" y2="10"/>
                <line x1="12" y1="20" x2="12" y2="4"/>
                <line x1="6" y1="20" x2="6" y2="14"/>
              </svg>
              <span class="breadcrumb-project" style="font-weight: 500;">Usage Logs</span>
            </div>
            <div v-else-if="activeRun" class="project-breadcrumb">
              <svg class="folder-icon open-folder" xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="m6 14 1.45-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.55 6a2 2 0 0 1-1.94 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2A2 2 0 0 0 12.07 6H20a2 2 0 0 1 2 2v2"/>
              </svg>
              <span class="breadcrumb-project">{{ currentProjectName }}</span>
              <span class="breadcrumb-separator">/</span>
              <span class="breadcrumb-chat-title">{{ visibleTitle }}</span>
            </div>
            
          </div>
          <div class="header-actions">
            <button
              v-if="showSidePanelToggle"
              class="panel-toggle-btn"
              type="button"
              title="Open side panel"
              @click="sidePanelCollapsed = false"
            >
              <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <rect width="18" height="18" x="3" y="3" rx="2" />
                <path d="M15 3v18" />
              </svg>
            </button>
          </div>
        </div>
      </header>

      <div v-if="activeRun" class="header-fade-overlay"></div>

      <template v-if="showUsageLogsPage">
        <UsageLogsPage @select-run="handleSelectRunFromLogs" />
      </template>

      <template v-else-if="activeRun">
        <section :ref="setMessagesContainer" class="messages-scroll">
          <MessageThread
            :active-run="activeRun"
            :grouped-messages="groupedMessages"
            :is-running="isRunning"
            :plan="currentPlan"
            :plan-panel-open="sidePanelOpen"
            :agent-summaries="agentSummaryLinks"
            @open-plan="openPlan"
            @open-agents="openAgents"
            @view-agent="openAgentTranscript"
            @view-file-in-review="openFileInReview"
          />
        </section>

        <div v-if="currentTaskList" class="pinned-task-list-wrap">
          <AgentTaskList
            :task-list-text="currentTaskList"
          />
        </div>

        <ChatComposer
          v-model:task-input="taskInput"
          v-model:current-mode="settings.currentMode.value"
          v-model:bypass-permissions="settings.bypassPermissions.value"
          v-model:selected-model="settings.selectedModelCombined.value"
          v-model:selected-reasoning-effort="settings.selectedReasoningEffort.value"
          v-model:selected-preset-id="settings.selectedPresetId.value"
          :is-running="isRunning"
          :queued-task-input="queuedTaskInput"
          :model-options="settings.modelOptions.value"
          :reasoning-effort-options="settings.reasoningEffortOptions.value"
          :active-model-display-name="settings.activeModelDisplayName.value"
          :agent-presets="agentPresets"
          :focus-signal="focusSignal"
          :confirmation-group="activeConfirmationGroup"
          :show-permission="showPermissionModal"
          :permission-request="pendingPermissionRequest"
          :question-request="pendingQuestionRequest"
          :messages="messages"
          :run-usage-label="runUsageLabel"
          :run-usage-tooltip="runUsageTooltip"
          @open-settings="handleOpenSettings"
          @send="chat.handleSendTask"
          @queue="chat.handleQueueTask"
          @quick-reply="chat.sendQuickReply"
          @permission-decision="chat.handlePermissionDecision"
          @question-answer="chat.handleQuestionAnswer"
          @cancel="chat.cancelActiveRun"
        />
      </template>

      <template v-else>
        <div class="landing-center-wrap">
          <ChatComposer
            v-model:task-input="taskInput"
            v-model:current-mode="settings.currentMode.value"
            v-model:bypass-permissions="settings.bypassPermissions.value"
          v-model:selected-model="settings.selectedModelCombined.value"
          v-model:selected-reasoning-effort="settings.selectedReasoningEffort.value"
          v-model:selected-preset-id="settings.selectedPresetId.value"
            :is-running="isRunning"
            :queued-task-input="queuedTaskInput"
          :model-options="settings.modelOptions.value"
          :reasoning-effort-options="settings.reasoningEffortOptions.value"
          :active-model-display-name="settings.activeModelDisplayName.value"
            :agent-presets="agentPresets"
            :focus-signal="focusSignal"
            :confirmation-group="activeConfirmationGroup"
            :show-permission="showPermissionModal"
            :permission-request="pendingPermissionRequest"
            :question-request="pendingQuestionRequest"
            :is-landing="true"
            :project-options="projects.projectOptions.value"
            :active-project-path="projects.activeProjectPath.value"
            :messages="messages"
            @open-settings="handleOpenSettings"
            @send="chat.handleSendTask"
            @queue="chat.handleQueueTask"
            @quick-reply="chat.sendQuickReply"
            @permission-decision="chat.handlePermissionDecision"
            @question-answer="chat.handleQuestionAnswer"
            @select-project="selectProject"
            @cancel="chat.cancelActiveRun"
          />
        </div>
      </template>
    </main>

    <PlanPanel
      ref="planPanelRef"
      v-if="sidePanelOpen && !showUsageLogsPage"
      :run-id="activeRunId"
      :project-path="activeRun?.projectPath || projects.activeProjectPath.value"
      :isOpen="sidePanelOpen && !showUsageLogsPage"
      :is-running="isRunning"
      :plan="currentPlan"
      :changes="workspaceChanges"
      :agents="agentSummaries"
      :show-actions="showPlanActions"
      @close="sidePanelCollapsed = true"
      @view-agent="openAgentTranscript"
      @start="startPlan"
      @revise="revisePlan"
      @reject="rejectPlan"
      @resize="handleSidePanelResize"
      @resize-start="isResizing = true"
      @resize-end="isResizing = false"
    />

    <!-- Backdrop overlays for mobile drawer menus -->
    <div v-if="!isSidebarCollapsed" class="sidebar-backdrop" @click="toggleSidebar"></div>
    <div v-if="sidePanelOpen" class="panel-backdrop" @click="sidePanelCollapsed = true"></div>

    <AddProjectModal
      :show="projects.showAddProjectModal.value"
      :is-mac="isMac"
      :is-submitting="projects.isSubmittingProject.value"
      v-model:name="projects.newProjectName.value"
      v-model:path="projects.newProjectPath.value"
      @close="projects.closeAddProjectModal"
      @browse="projects.browseFolder"
      @submit="submitProject"
    />

    <SettingsScreen
      v-model:active-tab="settingsTab"
      :show="permissions.showSettings.value"
      :permissions="permissions.permissions.value"
      :is-loading="permissions.isLoading.value"
      :providers="chat.providers.value"
      :providers-config="providersConfig"
      :providers-config-loading="isProvidersConfigLoading"
      :presets="agentPresets"
      :memories="memories.memories.value"
      :memories-loading="memories.isLoading.value"
      :active-project-path="projects.activeProjectPath.value"
      :active-project-name="projects.activeProject.value?.name || ''"
      :settings="appSettings"
      :settings-loading="isAppSettingsLoading"
      @close="permissions.closeSettings"
      @revoke="permissions.revokePermission"
      @clear-all="permissions.clearPermissions"
      @providers-saved="reloadProviders"
      @presets-saved="loadAgentPresets"
      @settings-saved="loadAppSettings"
      @add-memory="memories.addMemory"
      @update-memory="memories.updateMemory($event.id, $event.content)"
      @delete-memory="memories.deleteMemory"
      @clear-memories="memories.clearMemories"
    />

    <!-- Custom Dialog Modal (Alert/Confirm) -->
    <Transition name="fade">
      <div v-if="activeDialog" class="modal-overlay" @click="activeDialog.resolve(false)">
        <div class="modal-card dialog-modal-card" @click.stop>
          <header class="modal-header">
            <h3>{{ activeDialog.title || (activeDialog.type === 'confirm' ? 'Confirmation' : 'Notification') }}</h3>
            <button class="close-modal-btn" @click="activeDialog.resolve(false)">Close</button>
          </header>
          <div class="modal-body dialog-modal-body">
            <div class="dialog-content-wrapper">
              <div class="dialog-icon-wrap" :class="activeDialog.type">
                <svg v-if="activeDialog.type === 'confirm'" xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                  <circle cx="12" cy="12" r="10"></circle>
                  <path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3"></path>
                  <line x1="12" y1="17" x2="12.01" y2="17"></line>
                </svg>
                <svg v-else xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                  <path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z"></path>
                  <line x1="12" y1="9" x2="12" y2="13"></line>
                  <line x1="12" y1="17" x2="12.01" y2="17"></line>
                </svg>
              </div>
              <p class="dialog-message">{{ activeDialog.message }}</p>
            </div>
          </div>
          <footer class="modal-footer">
            <button v-if="activeDialog.type === 'confirm'" class="ghost-button" @click="activeDialog.resolve(false)">
              Cancel
            </button>
            <button class="primary-button" @click="activeDialog.resolve(true)">
              {{ activeDialog.type === 'confirm' ? 'Confirm' : 'OK' }}
            </button>
          </footer>
        </div>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.pinned-task-list-wrap {
  flex: 0 0 auto;
  padding: 0 24px;
  margin-right: 8px;
}

@media (max-width: 760px) {
  .pinned-task-list-wrap {
    margin-right: 0;
  }
}

.landing-center-wrap {
  flex: 1;
  display: flex;
  flex-direction: column;
  justify-content: center;
  align-items: stretch;
  width: 100%;
  margin: 0 auto;
  padding: 40px 0;
  box-sizing: border-box;
  gap: 24px;
}

.chat-shell {
  position: relative;
}

.header-fade-overlay {
  position: absolute;
  top: 56px; /* 12px margin-top + 44px header height */
  left: 0;
  right: 0;
  height: 24px;
  background: linear-gradient(to bottom, var(--bg), transparent);
  pointer-events: none;
  z-index: 10;
}

.chat-header {
  display: block;
  /* Match the side cards' top inset so all three header rows share a baseline. */
  margin-top: var(--shell-inset);
  min-height: 0;
  padding: 0;
  border-bottom: none;
}

.chat-header-inner {
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: var(--top-bar-h);
  width: 100%;
  padding: 0 24px;
  box-sizing: border-box;
}

.thread-title {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 10px;
}

.project-breadcrumb {
  display: flex;
  align-items: center;
  font-size: 0.93rem;
  font-weight: 500;
  color: var(--text);
  min-width: 0;
  flex: 1;
}

.project-breadcrumb .folder-icon {
  margin-right: 8px;
  color: var(--muted);
  flex-shrink: 0;
}

.breadcrumb-project {
  color: var(--breadcrumb-project-color);
  font-weight: 400;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 180px;
  flex-shrink: 0;
}

.breadcrumb-separator {
  margin: 0 8px;
  color: var(--faint);
  user-select: none;
  flex-shrink: 0;
}

.breadcrumb-chat-title {
  color: var(--text);
  font-weight: 650;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex: 1;
  min-width: 0;
}

/* Breadcrumb usage styling removed */

/* Dialog Modal Custom Styling */
.dialog-modal-card {
  width: min(420px, 90%);
}

.dialog-content-wrapper {
  display: flex;
  align-items: flex-start;
  gap: 16px;
  padding: 8px 0;
}

.dialog-icon-wrap {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 40px;
  height: 40px;
  border-radius: 50%;
  flex-shrink: 0;
}

.dialog-icon-wrap.confirm {
  background: var(--dialog-confirm-icon-bg);
  color: var(--dialog-confirm-icon-color);
}

.dialog-icon-wrap.alert {
  background: var(--dialog-alert-icon-bg);
  color: var(--dialog-alert-icon-color);
}

.dialog-message {
  margin: 0;
  font-size: 0.95rem;
  line-height: 1.5;
  color: var(--text);
  padding-top: 8px;
}
</style>
