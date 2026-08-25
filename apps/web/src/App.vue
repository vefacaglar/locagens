<script setup lang="ts">
import { useAppShell } from './composables/useAppShell';
import type { Run } from '@locagens/shared';
import { defineAsyncComponent } from 'vue';
import AppSidebar from './components/AppSidebar.vue';
import MessageThread from './components/MessageThread.vue';
import ChatComposer from './components/ChatComposer.vue';
import ChatHeader from './components/ChatHeader.vue';
import AppDialog from './components/ui/AppDialog.vue';

// Demand-only surfaces (modals, settings, side panel, usage page) load as
// separate chunks so first paint only pays for the always-visible shell.
const AddProjectModal = defineAsyncComponent(() => import('./components/AddProjectModal.vue'));
const SettingsScreen = defineAsyncComponent(() => import('./components/settings/SettingsScreen.vue'));
const AgentTaskList = defineAsyncComponent(() => import('./components/AgentTaskList.vue'));
const PlanPanel = defineAsyncComponent(() => import('./components/PlanPanel.vue'));
const UsageLogsPage = defineAsyncComponent(() => import('./components/UsageLogsPage.vue'));
const TerminalDrawer = defineAsyncComponent(() => import('./components/chat/TerminalDrawer.vue'));
import { useProcesses } from './composables/useProcesses';
import { collectWorkspaceChanges, hasWorkspaceChangeSignals } from './lib/workspaceChanges';
import { collectAgentSummaries, collectAgentSummaryLinks } from './lib/messageGroups';
import { extractTaskListFromMessage } from './lib/messageDerived';

const {
  runs,
  agentPresets,
  loadAgentPresets,
  setMessagesContainer,
  settings,
  projects,
  permissions,
  memories,
  skills,
  mcp,
  plugins,
  showSettings,
  openSettings,
  closeSettings,
  chat,
  isMac,
  selectProject,
  selectProjectAndNewChat,
  submitProject,
  deleteProject,
  providersConfig,
  isProvidersConfigLoading,
  reloadProviders,
  showUsageLogsPage
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
import { useIsMobile } from './composables/useIsMobile';
import { usePlanApproval } from './composables/usePlanApproval';
import { STORAGE_KEYS, runStorageKeys } from './lib/storageKeys';

const { activeDialog, showConfirm } = useCustomDialog();
const isMobile = useIsMobile();

const settingsTab = ref<'permissions' | 'memory' | 'skills' | 'plugins' | 'mcp' | 'providers' | 'agents' | 'server'>('permissions');

async function handleOpenSettings(tab?: string) {
  if (tab === 'permissions' || tab === 'memory' || tab === 'skills' || tab === 'plugins' || tab === 'mcp' || tab === 'providers' || tab === 'agents' || tab === 'server') {
    settingsTab.value = tab;
  } else {
    settingsTab.value = 'permissions';
  }
  await openSettings();
}

function refreshSkills() {
  void skills.loadSkills(projects.activeProjectPath.value || undefined);
}

function installSkillFile(payload: { target: 'user' | 'project'; file: File }) {
  void skills.installSkillFile(
    payload.target,
    payload.file,
    projects.activeProjectPath.value || undefined
  );
}

function createSkill(payload: { target: 'user' | 'project'; name: string; description: string; body: string }) {
  void skills.createSkill({
    ...payload,
    projectPath: projects.activeProjectPath.value || undefined
  });
}

async function deleteSkill(payload: { target: 'user' | 'project'; name: string }) {
  if (!(await showConfirm(`Are you sure you want to delete skill "${payload.name}"?`))) return;
  await skills.deleteSkill(
    payload.target,
    payload.name,
    projects.activeProjectPath.value || undefined
  );
}

function openSkillsFolder(target: 'user' | 'project') {
  void skills.openFolder(
    target,
    projects.activeProjectPath.value || undefined
  );
}

async function deleteMcpServer(name: string) {
  if (!(await showConfirm(`Are you sure you want to delete MCP server "${name}"?`))) return;
  await mcp.deleteServer(name, projects.activeProjectPath.value || undefined);
}

async function deletePlugin(payload: { id: string; scope: any }) {
  if (!(await showConfirm(`Are you sure you want to delete plugin "${payload.id}"?`))) return;
  await plugins.deletePlugin(payload.id, payload.scope, projects.activeProjectPath.value || undefined);
}

watch(() => projects.activeProjectPath.value, (newPath) => {
  if (showSettings.value) {
    void skills.loadSkills(newPath || undefined);
    void plugins.loadPlugins(newPath || undefined);
    void mcp.loadServers(newPath || undefined);
  }
});

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
  if (isMobile.value) {
    isSidebarCollapsed.value = true;
    sidePanelCollapsed.value = true;
  }
}

function handleNewChat() {
  chat.startNewRunSetup();
  showUsageLogsPage.value = false;
  if (isMobile.value) {
    isSidebarCollapsed.value = true;
    sidePanelCollapsed.value = true;
  }
}

function handleSelectProjectAndNewChat(path: string) {
  selectProjectAndNewChat(path);
  showUsageLogsPage.value = false;
  if (isMobile.value) {
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
const sidePanelWidth = ref(Number(localStorage.getItem(STORAGE_KEYS.sidePanelWidth) || '480'));
const isResizing = ref(false);
function handleSidePanelResize(newWidth: number) {
  const clamped = Math.max(360, Math.min(800, newWidth));
  sidePanelWidth.value = clamped;
  localStorage.setItem(STORAGE_KEYS.sidePanelWidth, String(clamped));
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
  // extractTaskListFromMessage is memoized per message object, so this scan
  // only re-runs the regex for the one message that changed in a flush.
  for (let i = list.length - 1; i >= 0; i--) {
    const msg = list[i];
    if (msg.role !== 'assistant') continue;
    const taskList = extractTaskListFromMessage(msg);
    if (taskList) return taskList;
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

// --- Background Processes & Terminal Drawer ---
const showTerminalDrawer = ref(false);
const processesManager = useProcesses(() => projects.activeProjectPath.value);

function toggleTerminalDrawer() {
  showTerminalDrawer.value = !showTerminalDrawer.value;
  if (showTerminalDrawer.value) {
    processesManager.startPolling();
  } else {
    processesManager.stopPolling();
  }
}

// Watch and persist sidePanelCollapsed changes on desktop, keyed by activeRunId
watch(sidePanelCollapsed, (val) => {
  if (!isMobile.value && activeRunId.value) {
    localStorage.setItem(runStorageKeys.sidePanelCollapsed(activeRunId.value), val ? 'true' : 'false');
  }
});

// Reset the collapsed state when switching chats using the stored preference, but keep it collapsed on mobile.
watch(activeRunId, (newId) => {
  if (isMobile.value) {
    sidePanelCollapsed.value = true;
  } else if (newId) {
    const stored = localStorage.getItem(runStorageKeys.sidePanelCollapsed(newId));
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
      if (!isMobile.value) {
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

// Props + listeners shared by the two <ChatComposer> render branches (active
// run vs landing). v-model bindings and branch-specific props stay inline so
// the differences between the branches remain visible at the call sites.
const composerBindings = computed(() => ({
  isRunning: isRunning.value,
  queuedTaskInput: queuedTaskInput.value,
  modelOptions: settings.modelOptions.value,
  reasoningEffortOptions: settings.reasoningEffortOptions.value,
  activeModelDisplayName: settings.activeModelDisplayName.value,
  agentPresets: agentPresets.value,
  focusSignal: focusSignal.value,
  confirmationGroup: activeConfirmationGroup.value,
  showPermission: showPermissionModal.value,
  permissionRequest: pendingPermissionRequest.value,
  questionRequest: pendingQuestionRequest.value,
  messages: messages.value,
  onOpenSettings: handleOpenSettings,
  onSend: chat.handleSendTask,
  onQueue: chat.handleQueueTask,
  onQuickReply: chat.sendQuickReply,
  onPermissionDecision: chat.handlePermissionDecision,
  onQuestionAnswer: chat.handleQuestionAnswer,
  onCancel: chat.cancelActiveRun
}));

// Plan approval actions (Start / Revise / Reject) live in usePlanApproval.
const { showPlanActions, startPlan, revisePlan, rejectPlan } = usePlanApproval({
  currentMode: settings.currentMode,
  currentPlan,
  groupedMessages,
  activeRunId,
  focusSignal,
  sendQuickReply: chat.sendQuickReply
});

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
  if (isMobile.value) {
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
      <ChatHeader
        :is-sidebar-collapsed="isSidebarCollapsed"
        :show-usage-logs-page="showUsageLogsPage"
        :has-active-run="!!activeRun"
        :current-project-name="currentProjectName"
        :visible-title="visibleTitle"
        :show-side-panel-toggle="showSidePanelToggle"
        :running-processes-count="processesManager.runningCount.value"
        @toggle-sidebar="toggleSidebar"
        @open-side-panel="sidePanelCollapsed = false"
        @toggle-terminal="toggleTerminalDrawer"
      />

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
          v-bind="composerBindings"
          v-model:task-input="taskInput"
          v-model:current-mode="settings.currentMode.value"
          v-model:bypass-permissions="settings.bypassPermissions.value"
          v-model:selected-model="settings.selectedModelCombined.value"
          v-model:selected-reasoning-effort="settings.selectedReasoningEffort.value"
          v-model:selected-preset-id="settings.selectedPresetId.value"
          :run-usage-label="runUsageLabel"
          :run-usage-tooltip="runUsageTooltip"
        />
      </template>

      <template v-else>
        <div class="landing-center-wrap">
          <ChatComposer
            v-bind="composerBindings"
            v-model:task-input="taskInput"
            v-model:current-mode="settings.currentMode.value"
            v-model:bypass-permissions="settings.bypassPermissions.value"
            v-model:selected-model="settings.selectedModelCombined.value"
            v-model:selected-reasoning-effort="settings.selectedReasoningEffort.value"
            v-model:selected-preset-id="settings.selectedPresetId.value"
            :is-landing="true"
            :project-options="projects.projectOptions.value"
            :active-project-path="projects.activeProjectPath.value"
            @select-project="selectProject"
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
      v-if="projects.showAddProjectModal.value"
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
      v-if="showSettings"
      v-model:active-tab="settingsTab"
      :show="showSettings"
      :permissions="permissions.permissions.value"
      :is-loading="permissions.isLoading.value"
      :providers="chat.providers.value"
      :providers-config="providersConfig"
      :providers-config-loading="isProvidersConfigLoading"
      :presets="agentPresets"
      :memories="memories.memories.value"
      :memories-loading="memories.isLoading.value"
      :skills="skills.skills.value"
      :skills-loading="skills.isLoading.value"
      :skills-user-root="skills.userRoot.value"
      :skills-project-root="skills.projectRoot.value"
      :skills-error="skills.error.value"
      :skills-status-message="skills.statusMessage.value"
      :skills-installing="skills.isInstalling.value"
      :mcp-servers="mcp.servers.value"
      :mcp-loading="mcp.isLoading.value"
      :mcp-saving="mcp.isSaving.value"
      :mcp-error="mcp.error.value"
      :mcp-status-message="mcp.statusMessage.value"
      :plugins="plugins.plugins.value"
      :plugin-templates="plugins.templates.value"
      :plugins-user-dir="plugins.userPluginsDir.value"
      :plugins-project-dir="plugins.projectPluginsDir.value"
      :plugins-loading="plugins.isLoading.value"
      :plugins-installing="plugins.isInstalling.value"
      :plugins-error="plugins.error.value"
      :plugins-status-message="plugins.statusMessage.value"
      :active-project-path="projects.activeProjectPath.value"
      :active-project-name="projects.activeProject.value?.name || ''"
      @close="closeSettings"
      @revoke="permissions.revokePermission"
      @clear-all="permissions.clearPermissions"
      @providers-saved="reloadProviders"
      @presets-saved="loadAgentPresets"
      @add-memory="memories.addMemory"
      @update-memory="memories.updateMemory($event.id, $event.content)"
      @delete-memory="memories.deleteMemory"
      @clear-memories="memories.clearMemories"
      @refresh-skills="refreshSkills"
      @install-skill-file="installSkillFile"
      @create-skill="createSkill"
      @delete-skill="deleteSkill"
      @open-skills-folder="openSkillsFolder"
      @refresh-mcp="mcp.loadServers(projects.activeProjectPath.value || undefined)"
      @save-mcp-server="mcp.saveServer($event, projects.activeProjectPath.value || undefined)"
      @delete-mcp-server="deleteMcpServer"
      @restart-mcp-server="mcp.restartServer($event, projects.activeProjectPath.value || undefined)"
      @toggle-mcp-server="mcp.toggleServer($event.name, $event.enabled, projects.activeProjectPath.value || undefined)"
      @refresh-plugins="plugins.loadPlugins(projects.activeProjectPath.value || undefined)"
      @install-plugin="plugins.installPlugin"
      @toggle-plugin="plugins.togglePlugin($event.id, $event.enabled, $event.scope, projects.activeProjectPath.value || undefined)"
      @delete-plugin="deletePlugin"
    />

    <!-- Terminal / Dev Servers Drawer -->
    <TerminalDrawer
      :is-open="showTerminalDrawer"
      :processes="processesManager.processes.value"
      :active-process="processesManager.activeProcess.value"
      :logs="processesManager.logs.value"
      :is-spawning="processesManager.isSpawning.value"
      :error="processesManager.error.value"
      @close="toggleTerminalDrawer"
      @select="processesManager.selectProcess"
      @spawn="processesManager.spawn"
      @kill="processesManager.kill"
      @restart="processesManager.restart"
    />

    <!-- Custom Dialog Modal (Alert/Confirm) -->
    <AppDialog :dialog="activeDialog" />
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

</style>
