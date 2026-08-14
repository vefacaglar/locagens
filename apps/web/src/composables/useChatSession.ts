import { computed, ref, shallowRef, type ComputedRef, type Ref } from 'vue';
import { tryOnScopeDispose } from '@vueuse/core';
import type { ProviderMetadata, Run, RunMessage, RunStatus, Plan, RunUsageSummary } from '@locagens/shared';
import { api, type PermissionDecision, type RunEventStream } from '../api/client';
import { ACTIVE_STATUSES } from '../lib/format';
import { STORAGE_KEYS } from '../lib/storageKeys';
import { groupMessages, type MessageGroup } from '../lib/messageGroups';
import { getConfirmationOptions } from '../lib/confirmation';
import { useCustomDialog } from './useCustomDialog';
import {
  clearDraftComposerSettings,
  saveLastUsedComposerSettings,
  saveRunComposerSettings,
  type PersistedComposerSettings
} from './useComposerSettings';

interface ChatSessionOptions {
  activeRunId: Ref<string | null>;
  activeRun: Ref<Run | null>;
  providers: Ref<ProviderMetadata[]>;
  runs: Ref<Run[]>;
  selectedModelCombined: Ref<string>;
  selectedReasoningEffort: Ref<string>;
  effectiveReasoningEffort: ComputedRef<string | undefined>;
  // The effective main/architect model (preset architect or single-model pick)
  // and the optional dual-model fields, both derived in useComposerSettings.
  effectiveModel: ComputedRef<{ providerId: string; model: string }>;
  agentRunFields: ComputedRef<{
    coderProviderId?: string;
    coderModel?: string;
    coderReasoningEffort?: string;
    utilityProviderId?: string;
    utilityModel?: string;
    utilityReasoningEffort?: string;
    agentPreset?: string;
  }>;
  currentMode: Ref<string>;
  bypassPermissions: Ref<boolean>;
  selectedPresetId: Ref<string>;
  activeProject: ComputedRef<{ path: string; name: string } | undefined>;
  activeProjectPath: Ref<string>;
}

/**
 * Core chat state machine: provider/run/message lists, the live SSE stream,
 * sending and continuing runs, cancellation and permission handling.
 * The providers and runs refs are owned by the caller and shared with the
 * project/settings composables to avoid circular dependencies.
 */
export function useChatSession(options: ChatSessionOptions) {
  const { showAlert } = useCustomDialog();
  const providers = options.providers;
  const runs = options.runs;
  // shallowRef: the arrays are always replaced wholesale (never mutated in
  // place), so deep-proxying every RunMessage — including large rawResponse
  // blobs — would only add per-property-read overhead on the render hot path.
  const messages = shallowRef<RunMessage[]>([]);
  const sidePanelMessages = shallowRef<RunMessage[]>([]);

  const activeRunId = options.activeRunId;
  const activeRun = options.activeRun;
  const isRunning = ref(false);
  const currentPlan = ref<Plan | null>(null);
  // Aggregated cost/token totals for the active run (shown in the chat header).
  const runUsage = ref<RunUsageSummary | null>(null);

  const taskInput = ref('');
  const queuedTaskInput = ref('');
  const focusSignal = ref(0);

  const showPermissionModal = ref(false);
  const pendingPermissionRequest = ref<any>(null);
  // The active ask_user_question request (questions to render), or null.
  const pendingQuestionRequest = ref<any>(null);

  let eventSource: RunEventStream | null = null;
  let pendingPermissionPollTimer: ReturnType<typeof setTimeout> | null = null;
  let isDisposed = false;
  let liveMessageFlushFrame: number | null = null;
  let sidePanelSnapshotTimer: ReturnType<typeof setTimeout> | null = null;
  let usageDebounceTimer: ReturnType<typeof setTimeout> | null = null;
  // Raw SSE updates stay in a plain Map so high-frequency token/sub-agent
  // events do not touch Vue reactivity until the scheduled flush runs.
  const pendingMessageUpdates = new Map<string, RunMessage>();
  // Persistent id → index lookup for the messages array, maintained across
  // flushes so applying updates is O(changed) instead of rebuilding a full
  // index over every message per animation frame.
  const messageIndexById = new Map<string, number>();
  const SIDE_PANEL_SNAPSHOT_DELAY_MS = 200;

  const groupedMessages = computed<MessageGroup[]>(() => groupMessages(messages.value));
  const sidePanelGroupedMessages = computed<MessageGroup[]>(() => groupMessages(sidePanelMessages.value));

  const visibleTitle = computed(() => activeRun.value?.title || 'New chat');

  /** The trailing assistant message that is awaiting a yes/no confirmation. */
  const activeConfirmationGroup = computed<MessageGroup | null>(() => {
    if (groupedMessages.value.length === 0) return null;
    const last = groupedMessages.value[groupedMessages.value.length - 1];
    if (last && last.type === 'assistant' && canShowConfirmation(last)) {
      return last;
    }
    return null;
  });

  function requestFocus() {
    focusSignal.value++;
  }

  function permissionRequestKey(request: any): string {
    const toolCall = request?.toolCall;
    return `${request?.runId ?? ''}:${toolCall?.id ?? ''}:${toolCall?.function?.name ?? ''}:${toolCall?.function?.arguments ?? ''}`;
  }

  function clearPermissionRequestIfCurrent(key: string) {
    if (!pendingPermissionRequest.value || permissionRequestKey(pendingPermissionRequest.value) === key) {
      showPermissionModal.value = false;
      pendingPermissionRequest.value = null;
    }
  }

  function clearPendingPermissionPoll() {
    if (pendingPermissionPollTimer) {
      clearTimeout(pendingPermissionPollTimer);
      pendingPermissionPollTimer = null;
    }
  }

  async function refreshPendingPermission(runId: string): Promise<boolean> {
    if (isDisposed || activeRun.value?.id !== runId) return false;
    const pending = await api.getRunPending(runId);
    if (!pending || activeRun.value?.id !== runId) return false;

    if (pending.permissionRequest) {
      pendingPermissionRequest.value = pending.permissionRequest;
      showPermissionModal.value = true;
      return true;
    }
    return false;
  }

  function schedulePendingPermissionRefresh(runId: string, delays = [0, 80, 250]) {
    clearPendingPermissionPoll();
    const [delay, ...rest] = delays;
    if (delay === undefined) return;

    pendingPermissionPollTimer = setTimeout(() => {
      pendingPermissionPollTimer = null;
      void refreshPendingPermission(runId).then((found) => {
        if (!found) schedulePendingPermissionRefresh(runId, rest);
      });
    }, delay);
  }

  function canShowConfirmation(group: MessageGroup): boolean {
    if (!getConfirmationOptions(group.message.content)) return false;
    if (isRunning.value) return false;

    const idx = groupedMessages.value.findIndex(g => g.id === group.id);
    if (idx === -1) return false;
    for (let k = idx + 1; k < groupedMessages.value.length; k++) {
      if (groupedMessages.value[k].type === 'user') return false;
    }
    return true;
  }

  // --- Loading -------------------------------------------------------------

  async function loadProviders() {
    const data = await api.getProviders();
    if (data) providers.value = data;
  }

  async function loadRuns() {
    const data = await api.getRuns();
    if (data) runs.value = data;
  }

  async function loadMessages(runId: string) {
    const data = await api.getMessages(runId);
    if (data) {
      replaceMessages(data);
    }
  }

  // --- Selection -----------------------------------------------------------

  async function selectRun(run: Run) {
    clearPendingPermissionPoll();
    flushAllMessageSurfaces();
    eventSource?.close();
    eventSource = null;
    clearPendingMessageUpdates();

    // The runs list is kept fresh by the active-run poll and finishEventStream,
    // so use the current copy instead of a blocking refetch on the switch path.
    const latestRun = runs.value.find(r => r.id === run.id) || run;

    activeRunId.value = latestRun.id;
    localStorage.setItem(STORAGE_KEYS.activeRunId, latestRun.id);
    activeRun.value = { ...latestRun };
    options.activeProjectPath.value = latestRun.projectPath || options.activeProjectPath.value;
    taskInput.value = '';
    showPermissionModal.value = false;
    pendingPermissionRequest.value = null;
    pendingQuestionRequest.value = null;

    // These four loads are independent of each other — fetch them in parallel
    // so opening a run costs one round-trip instead of four sequential ones.
    const [, plan] = await Promise.all([
      loadMessages(latestRun.id),
      api.getRunPlan(latestRun.id),
      refreshRunUsage(latestRun.id),
      hydratePendingRequest(latestRun.id)
    ]);
    currentPlan.value = plan;

    if (ACTIVE_STATUSES.includes(latestRun.status)) {
      isRunning.value = true;
      connectEventSource(latestRun.id);
    } else {
      isRunning.value = false;
    }
    requestFocus();
  }

  async function refreshRunUsage(runId: string) {
    try {
      const summary = await api.getRunUsage(runId);
      if (activeRun.value?.id === runId) runUsage.value = summary;
    } catch {
      // Usage is informational only; never let it break run selection/teardown.
      if (activeRun.value?.id === runId) runUsage.value = null;
    }
  }

  function clearUsageDebounceTimer() {
    if (usageDebounceTimer) {
      clearTimeout(usageDebounceTimer);
      usageDebounceTimer = null;
    }
  }

  function requestUsageRefresh(runId: string) {
    if (usageDebounceTimer) return;
    usageDebounceTimer = setTimeout(async () => {
      usageDebounceTimer = null;
      if (activeRun.value?.id === runId) {
        await refreshRunUsage(runId);
      }
    }, 1500);
  }

  async function hydratePendingRequest(runId: string) {
    const pending = await api.getRunPending(runId);
    if (!pending || activeRun.value?.id !== runId) return;

    if (pending.permissionRequest) {
      pendingPermissionRequest.value = pending.permissionRequest;
      showPermissionModal.value = true;
    }
    if (pending.questionRequest) {
      pendingQuestionRequest.value = pending.questionRequest;
    }
  }

  function startNewRunSetup() {
    // Starting a new chat while another run is still generating must NOT cancel
    // that run — it keeps going on the backend. We just stop watching its live
    // stream here; selecting it again later reconnects and reloads its history.
    clearPendingPermissionPoll();
    clearUsageDebounceTimer();
    flushAllMessageSurfaces();
    eventSource?.close();
    eventSource = null;
    clearPendingMessageUpdates();

    // Clear draft settings so it initializes with last used settings
    clearDraftComposerSettings();

    isRunning.value = false;
    activeRunId.value = null;
    localStorage.removeItem(STORAGE_KEYS.activeRunId);
    activeRun.value = null;
    resetMessageSurfaces();
    currentPlan.value = null;
    runUsage.value = null;
    taskInput.value = '';
    queuedTaskInput.value = '';
    requestFocus();
  }

  // --- Live event stream ---------------------------------------------------

  function cancelLiveMessageFlush() {
    if (liveMessageFlushFrame !== null) {
      cancelAnimationFrame(liveMessageFlushFrame);
      liveMessageFlushFrame = null;
    }
  }

  function cancelSidePanelSnapshot() {
    if (sidePanelSnapshotTimer) {
      clearTimeout(sidePanelSnapshotTimer);
      sidePanelSnapshotTimer = null;
    }
  }

  function syncSidePanelMessages() {
    cancelSidePanelSnapshot();
    if (isDisposed) return;
    sidePanelMessages.value = messages.value;
  }

  function scheduleSidePanelSnapshot() {
    if (isDisposed || sidePanelSnapshotTimer) return;
    sidePanelSnapshotTimer = setTimeout(() => {
      sidePanelSnapshotTimer = null;
      syncSidePanelMessages();
    }, SIDE_PANEL_SNAPSHOT_DELAY_MS);
  }

  function applyPendingMessageUpdates(): boolean {
    if (pendingMessageUpdates.size === 0) return false;
    const updates = Array.from(pendingMessageUpdates.values());
    pendingMessageUpdates.clear();

    const next = messages.value.slice();

    for (const message of updates) {
      const index = messageIndexById.get(message.id);
      if (index !== undefined) {
        next[index] = message;
      } else {
        messageIndexById.set(message.id, next.length);
        next.push(message);
      }
    }

    messages.value = next;
    return true;
  }

  function flushLiveMessages() {
    if (isDisposed) return;
    cancelLiveMessageFlush();

    if (applyPendingMessageUpdates()) {
      scheduleSidePanelSnapshot();
    }
  }

  function flushAllMessageSurfaces() {
    if (isDisposed) return;
    cancelLiveMessageFlush();
    applyPendingMessageUpdates();
    syncSidePanelMessages();
  }

  function scheduleLiveMessageFlush() {
    if (isDisposed || liveMessageFlushFrame !== null) return;
    liveMessageFlushFrame = requestAnimationFrame(() => {
      liveMessageFlushFrame = null;
      flushLiveMessages();
    });
  }

  function clearPendingMessageUpdates() {
    cancelLiveMessageFlush();
    pendingMessageUpdates.clear();
  }

  function rebuildMessageIndex(list: RunMessage[]) {
    messageIndexById.clear();
    for (let i = 0; i < list.length; i++) {
      messageIndexById.set(list[i].id, i);
    }
  }

  function resetMessageSurfaces() {
    clearPendingMessageUpdates();
    cancelSidePanelSnapshot();
    messages.value = [];
    sidePanelMessages.value = [];
    messageIndexById.clear();
  }

  function replaceMessages(nextMessages: RunMessage[]) {
    clearPendingMessageUpdates();
    cancelSidePanelSnapshot();
    messages.value = nextMessages;
    sidePanelMessages.value = nextMessages;
    rebuildMessageIndex(nextMessages);
  }

  function queueMessageUpdate(message: RunMessage) {
    if (isDisposed) return;
    pendingMessageUpdates.set(message.id, message);
    scheduleLiveMessageFlush();
  }

  function connectEventSource(runId: string) {
    clearPendingPermissionPoll();
    flushAllMessageSurfaces();
    eventSource?.close();
    clearPendingMessageUpdates();
    eventSource = api.openEvents(runId);

    eventSource.onmessage = (event) => {
      const data = JSON.parse(event.data);
      if (data.type === 'connected') return;

      if (data.type === 'status_changed') {
        updateRunStatus(runId, data.status);
        if (data.status === 'done' || data.status === 'failed' || data.status === 'cancelled') {
          showPermissionModal.value = false;
          pendingPermissionRequest.value = null;
        }
        if (data.status !== 'awaiting_input') {
          pendingQuestionRequest.value = null;
        }
        // The backend closes the SSE stream on a terminal status_changed, so a
        // following run_completed event never arrives. Drive the queued-message
        // send (and stream teardown) from the terminal status itself.
        if (data.status === 'done') {
          finishEventStream({ sendQueuedMessage: true });
        }
      }

      if (data.type === 'permission_requested' && activeRun.value?.id === runId) {
        pendingPermissionRequest.value = data;
        showPermissionModal.value = true;
      }

      if (data.type === 'question_requested' && activeRun.value?.id === runId) {
        pendingQuestionRequest.value = data;
      }

      if (data.type === 'message_created' && activeRun.value?.id === runId) {
        queueMessageUpdate(data.message);
      }

      if (data.type === 'message_updated' && activeRun.value?.id === runId) {
        queueMessageUpdate(data.message);
      }

      if (data.type === 'plan_updated' && activeRun.value?.id === runId) {
        currentPlan.value = data.plan;
      }

      if (data.type === 'run_title_changed') {
        if (activeRun.value?.id === runId) {
          activeRun.value = { ...activeRun.value, title: data.title };
        }
        runs.value = runs.value.map(r => r.id === runId ? { ...r, title: data.title } : r);
      }

      if (data.type === 'run_completed') {
        if (activeRun.value?.id === runId) updateRunStatus(runId, 'done');
        finishEventStream({ sendQueuedMessage: true });
      }

      if (data.type === 'run_failed') {
        if (activeRun.value?.id === runId) {
          activeRun.value = { ...activeRun.value, errorMessage: data.errorMessage, status: 'failed' };
        }
        runs.value = runs.value.map(r => r.id === runId ? { ...r, errorMessage: data.errorMessage, status: 'failed' } : r);
        finishEventStream();
      }

      // Request a throttled usage refresh on any activity to update the cost info bar in real-time
      requestUsageRefresh(runId);
    };

    eventSource.onerror = () => finishEventStream();
  }

  function updateRunStatus(runId: string, status: RunStatus) {
    if (activeRun.value?.id === runId) {
      activeRun.value = { ...activeRun.value, status };
    }
    runs.value = runs.value.map(r => r.id === runId ? { ...r, status } : r);
  }

  function finishEventStream(options: { sendQueuedMessage?: boolean } = {}) {
    clearPendingPermissionPoll();
    clearUsageDebounceTimer();
    // Terminal SSE events may close the stream before a scheduled render tick,
    // so force the latest buffered message into reactive state before teardown.
    flushAllMessageSurfaces();
    eventSource?.close();
    eventSource = null;
    isRunning.value = false;
    loadRuns();
    if (activeRun.value) refreshRunUsage(activeRun.value.id);

    if (options.sendQueuedMessage && queuedTaskInput.value.trim()) {
      // Capture + clear synchronously so a second finishEventStream call (e.g.
      // both a terminal status_changed and run_completed arriving) can't re-send.
      const queued = queuedTaskInput.value;
      queuedTaskInput.value = '';
      requestAnimationFrame(() => {
        if (!isRunning.value) {
          taskInput.value = queued;
          handleSendTask();
        }
      });
    }
  }

  // --- Sending -------------------------------------------------------------

  function currentComposerSettings(): PersistedComposerSettings {
    return {
      selectedModel: options.selectedModelCombined.value,
      reasoningEffort: options.selectedReasoningEffort.value,
      selectedPreset: options.selectedPresetId.value,
      mode: options.currentMode.value,
      bypassPermissions: options.bypassPermissions.value
    };
  }

  async function handleSendTask() {
    if (!taskInput.value.trim() || isRunning.value || !options.selectedModelCombined.value) return;

    // Record last used settings in localStorage
    saveLastUsedComposerSettings(currentComposerSettings());

    const { providerId, model } = options.effectiveModel.value;
    const agentFields = options.agentRunFields.value;
    isRunning.value = true;

    const isContinuation = activeRun.value && !ACTIVE_STATUSES.includes(activeRun.value.status);

    if (isContinuation) {
      const runId = activeRun.value!.id;
      const currentTask = taskInput.value;
      taskInput.value = '';

      try {
        await api.continueRun(runId, {
          task: currentTask,
          providerId,
          model,
          reasoningEffort: options.effectiveReasoningEffort.value,
          mode: options.currentMode.value,
          bypassPermissions: options.bypassPermissions.value,
          ...agentFields
        });
      } catch (err: any) {
        isRunning.value = false;
        taskInput.value = currentTask; // restore input on error
        await showAlert(err.message);
        return;
      }

      await loadMessages(runId);
      await loadRuns();
      connectEventSource(runId);
    } else {
      resetMessageSurfaces();
      currentPlan.value = null;

      try {
        const run = await api.createRun({
          task: taskInput.value,
          projectPath: options.activeProject.value?.path,
          projectName: options.activeProject.value?.name,
          providerId,
          model,
          reasoningEffort: options.effectiveReasoningEffort.value,
          mode: options.currentMode.value,
          bypassPermissions: options.bypassPermissions.value,
          ...agentFields
        });
        taskInput.value = '';

        // Initialize settings for the new run
        saveRunComposerSettings(run.id, currentComposerSettings());

        activeRunId.value = run.id;
        localStorage.setItem(STORAGE_KEYS.activeRunId, run.id);
        activeRun.value = run;
        runs.value.unshift(run);
        connectEventSource(run.id);
      } catch (err: any) {
        isRunning.value = false;
        await showAlert(err.message);
      }
    }
  }

  async function sendQuickReply(option: string) {
    taskInput.value = option;
    if (isRunning.value) {
      handleQueueTask();
    } else {
      await handleSendTask();
    }
  }

  function handleQueueTask() {
    if (!isRunning.value || !taskInput.value.trim()) return;
    queuedTaskInput.value = taskInput.value;
    taskInput.value = '';
    requestFocus();
  }

  async function cancelActiveRun() {
    if (!activeRunId.value) return;
    const runId = activeRunId.value;
    try {
      const response = await api.cancelRun(runId);
      if (response.ok || response.status === 400) {
        updateRunStatus(runId, 'cancelled');
        finishEventStream();
      } else {
        const err = await response.json().catch(() => ({}));
        await showAlert(err.error || 'Cancel request failed.');
      }
    } catch (err) {
      console.error('Failed to contact server for cancellation:', err);
      updateRunStatus(runId, 'failed');
      finishEventStream();
    }
  }

  async function handleQuestionAnswer(payload: { selections: string[][]; notes: string[] }) {
    if (!activeRunId.value) return;
    try {
      await api.answerQuestion(activeRunId.value, payload.selections, payload.notes);
      pendingQuestionRequest.value = null;
    } catch (err: any) {
      await showAlert(err.message || 'Connection error.');
    }
  }

  async function handlePermissionDecision(decision: PermissionDecision) {
    if (!activeRunId.value) return;
    const runId = activeRunId.value;
    const requestKey = permissionRequestKey(pendingPermissionRequest.value);
    try {
      await api.sendPermissionDecision(runId, decision);
      clearPermissionRequestIfCurrent(requestKey);
      schedulePendingPermissionRefresh(runId);
    } catch (err: any) {
      await showAlert(err.message || 'Connection error.');
    }
  }

  function disconnect() {
    clearPendingPermissionPoll();
    clearUsageDebounceTimer();
    flushAllMessageSurfaces();
    eventSource?.close();
    eventSource = null;
    clearPendingMessageUpdates();
    cancelSidePanelSnapshot();
  }

  tryOnScopeDispose(() => {
    clearPendingPermissionPoll();
    clearUsageDebounceTimer();
    flushAllMessageSurfaces();
    isDisposed = true;
    clearPendingMessageUpdates();
    cancelSidePanelSnapshot();
  });

  return {
    providers,
    runs,
    messages,
    sidePanelMessages,
    activeRunId,
    activeRun,
    isRunning,
    currentPlan,
    runUsage,
    taskInput,
    queuedTaskInput,
    focusSignal,
    showPermissionModal,
    pendingPermissionRequest,
    pendingQuestionRequest,
    groupedMessages,
    sidePanelGroupedMessages,
    visibleTitle,
    activeConfirmationGroup,
    loadProviders,
    loadRuns,
    selectRun,
    startNewRunSetup,
    handleSendTask,
    handleQueueTask,
    sendQuickReply,
    cancelActiveRun,
    handlePermissionDecision,
    handleQuestionAnswer,
    disconnect
  };
}
