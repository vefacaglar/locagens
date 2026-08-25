import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import type { ProviderMetadata, Run, AgentPreset } from '@locagens/shared';
import { api } from '../api/client';
import { useChatAutoScroll } from './useChatAutoScroll';
import { useChatSession } from './useChatSession';
import { useComposerSettings } from './useComposerSettings';
import { useProjects } from './useProjects';
import { usePermissions } from './usePermissions';
import { useMemories } from './useMemories';
import { useSkills } from './useSkills';
import { useMcpServers } from './useMcpServers';
import { useAsyncResource } from './useAsyncResource';
import { ACTIVE_STATUSES } from '../lib/format';
import { STORAGE_KEYS } from '../lib/storageKeys';

export function useAppShell() {
  const providers = ref<ProviderMetadata[]>([]);
  const agentPresets = ref<AgentPreset[]>([]);
  const runs = ref<Run[]>([]);
  const messagesContainer = ref<HTMLElement | null>(null);
  const showSettings = ref(false);
  const {
    data: providersConfig,
    isLoading: isProvidersConfigLoading,
    load: loadProvidersConfig
  } = useAsyncResource<Record<string, any>>(() => api.getProvidersConfig(), {});

  const activeRunId = ref<string | null>(localStorage.getItem(STORAGE_KEYS.activeRunId));
  const activeRun = ref<Run | null>(null);
  const showUsageLogsPage = ref(false);
  let runsRefreshTimer: ReturnType<typeof setInterval> | null = null;

  watch(activeRunId, () => {
    if (activeRunId.value) {
      showUsageLogsPage.value = false;
    }
  });

  const settings = useComposerSettings(providers, agentPresets, activeRunId, activeRun);

  async function loadAgentPresets() {
    agentPresets.value = (await api.getAgentPresets()) ?? [];
  }

  async function reloadProviders() {
    await Promise.all([
      loadProvidersConfig(),
      chat.loadProviders()
    ]);
  }

  const projects = useProjects(runs);
  const permissions = usePermissions();
  const memories = useMemories();
  const skills = useSkills();
  const mcp = useMcpServers();

  // Opening the settings screen loads all settings tabs so they are ready when
  // the user switches between them.
  async function openSettings() {
    showSettings.value = true;
    await Promise.all([
      permissions.loadPermissions(),
      memories.loadMemories(),
      skills.loadSkills(projects.activeProjectPath.value || undefined),
      mcp.loadServers(projects.activeProjectPath.value || undefined),
      loadProvidersConfig(),
      loadAgentPresets()
    ]);
  }

  function closeSettings() {
    showSettings.value = false;
  }
  const chat = useChatSession({
    activeRunId,
    activeRun,
    providers,
    runs,
    selectedModelCombined: settings.selectedModelCombined,
    selectedReasoningEffort: settings.selectedReasoningEffort,
    effectiveReasoningEffort: settings.effectiveReasoningEffort,
    effectiveModel: settings.effectiveModel,
    agentRunFields: settings.agentRunFields,
    currentMode: settings.currentMode,
    bypassPermissions: settings.bypassPermissions,
    selectedPresetId: settings.selectedPresetId,
    activeProject: projects.activeProject,
    activeProjectPath: projects.activeProjectPath
  });

  const isMac = computed(() =>
    navigator.userAgent.toLowerCase().includes('mac') || navigator.platform.toLowerCase().includes('mac')
  );

  useChatAutoScroll(messagesContainer, chat.messages, chat.isRunning, chat.activeRunId);

  function stopRunsRefreshTimer() {
    if (runsRefreshTimer) {
      clearInterval(runsRefreshTimer);
      runsRefreshTimer = null;
    }
  }

  watch(
    () => runs.value.some(run => ACTIVE_STATUSES.includes(run.status)),
    (hasActiveRun) => {
      stopRunsRefreshTimer();
      if (!hasActiveRun) return;

      runsRefreshTimer = setInterval(() => {
        void chat.loadRuns();
      }, 3000);
    },
    { immediate: true }
  );

  function setMessagesContainer(el: unknown) {
    messagesContainer.value = el instanceof HTMLElement ? el : null;
  }

  function selectProject(projectPath: string) {
    if (projects.activeProjectPath.value !== projectPath) {
      projects.activeProjectPath.value = projectPath;
    }
  }

  function selectProjectAndNewChat(projectPath: string) {
    projects.activeProjectPath.value = projectPath;
    chat.startNewRunSetup();
  }

  async function submitProject() {
    const path = await projects.submitNewProject();
    if (path) selectProject(path);
  }

  async function deleteProject(projectPath: string) {
    const fallback = await projects.deleteProject(projectPath);
    if (fallback !== null) selectProject(fallback);
  }

  async function initialize() {
    // These four loads are independent — fetch them in one parallel wave so
    // startup costs one round-trip instead of four sequential ones.
    await Promise.all([
      chat.loadProviders(),
      loadAgentPresets(),
      projects.loadProjects(),
      chat.loadRuns()
    ]);
    settings.ensureDefaultModel();

    if (runs.value.length > 0) {
      const storedRunId = localStorage.getItem(STORAGE_KEYS.activeRunId);
      const storedRun = storedRunId ? runs.value.find(r => r.id === storedRunId) : null;
      if (storedRun) {
        await chat.selectRun(storedRun);
      } else {
        await chat.selectRun(runs.value[0]);
      }
    } else if (projects.projectOptions.value.length > 0) {
      projects.activeProjectPath.value = projects.projectOptions.value[0].path;
    }
  }

  onMounted(initialize);
  onBeforeUnmount(() => {
    stopRunsRefreshTimer();
    chat.disconnect();
  });

  return {
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
  };
}
