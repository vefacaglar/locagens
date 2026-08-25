<script setup lang="ts">
import { computed, ref, watch, nextTick, onMounted, onUnmounted } from 'vue';
import type { Plan } from '@locagens/shared';
import type { AgentSummary } from '../lib/messageGroups';
import { renderMarkdown } from '../lib/markdown';
import { cleanedMessageContent } from '../lib/messageDerived';
import { changeDiffRows, type WorkspaceChange } from '../lib/workspaceChanges';
import ToolGroup from './ToolGroup.vue';
import ReasoningPanel from './ReasoningPanel.vue';
import ThemedButton from './ui/ThemedButton.vue';
import DiffView from './DiffView.vue';
import Spinner from './ui/Spinner.vue';
import WebPreview from './chat/WebPreview.vue';
import { api } from '../api/client';
import { runStorageKeys } from '../lib/storageKeys';
import { lineDiff, type DiffRow } from '../lib/diff';

const props = defineProps<{
  runId?: string | null;
  projectPath?: string | null;
  plan: Plan | null;
  changes?: WorkspaceChange[];
  agents?: AgentSummary[];
  showActions?: boolean;
  isOpen?: boolean;
  isRunning?: boolean;
}>();

const emit = defineEmits<{
  (e: 'close'): void;
  (e: 'view-agent', id: string): void;
  (e: 'start'): void;
  (e: 'revise'): void;
  (e: 'reject'): void;
  (e: 'resize', width: number): void;
  (e: 'resize-start'): void;
  (e: 'resize-end'): void;
}>();

// Drag resize handling
function initResize(e: MouseEvent) {
  e.preventDefault();
  emit('resize-start');
  document.addEventListener('mousemove', handleMouseMove);
  document.addEventListener('mouseup', handleMouseUp);
  document.body.style.cursor = 'ew-resize';
  document.body.style.userSelect = 'none';
}

function handleMouseMove(e: MouseEvent) {
  const newWidth = window.innerWidth - e.clientX;
  emit('resize', newWidth);
}

function handleMouseUp() {
  document.removeEventListener('mousemove', handleMouseMove);
  document.removeEventListener('mouseup', handleMouseUp);
  document.body.style.cursor = '';
  document.body.style.userSelect = '';
  emit('resize-end');
}

type StaticTab = 'plan' | 'agents' | 'review' | 'preview';
type PanelTab = StaticTab | `file:${string}`;

const activeTab = ref<PanelTab>('plan');
const openFileTabs = ref<string[]>([]);
const isSwitchingRun = ref(false);

const plan = ref<Plan | null>(null);
const changes = ref<WorkspaceChange[]>([]);
const agents = ref<AgentSummary[]>([]);

// Git Integration State
interface GitDiffFile {
  path: string;
  kind: 'created' | 'edited' | 'deleted' | 'moved';
  oldText: string;
  newText: string;
  isOpen: boolean;
  /** Full (unfiltered) lineDiff rows; DiffView collapses unchanged context itself. */
  diffRows: DiffRow[];
  added: number;
  deleted: number;
}

const isGitRepo = ref(false);
const hasGitChanges = ref(false);
const gitBranch = ref('');
const gitDiffFiles = ref<GitDiffFile[]>([]);
const isLoadingDiffs = ref(false);

const commitMessage = ref('');
const isGenerating = ref(false);
const commitStatus = ref<'idle' | 'processing' | 'success' | 'error'>('idle');
const gitError = ref<string | null>(null);
const showActionsDropdown = ref(false);

type GitAction = 'commit' | 'commit-push' | 'push';
const selectedGitAction = ref<GitAction>('commit');

const actionLabels: Record<GitAction, string> = {
  commit: 'Commit',
  'commit-push': 'Commit & Push',
  push: 'Push'
};

const availableGitActions: GitAction[] = ['commit', 'commit-push', 'push'];

const showCommitPopup = ref(false);

function toggleCommitPopup(e: Event) {
  e.stopPropagation();
  showCommitPopup.value = !showCommitPopup.value;
}

function closeDropdown() {
  showActionsDropdown.value = false;
  showCommitPopup.value = false;
}

onMounted(() => {
  document.addEventListener('click', closeDropdown);
});

onUnmounted(() => {
  document.removeEventListener('click', closeDropdown);
  if (gitStatusRefreshTimer) {
    clearTimeout(gitStatusRefreshTimer);
    gitStatusRefreshTimer = null;
  }
});

function toggleActionsDropdown(e: Event) {
  e.stopPropagation();
  showActionsDropdown.value = !showActionsDropdown.value;
}

function selectGitAction(action: GitAction) {
  selectedGitAction.value = action;
  showActionsDropdown.value = false;
}

async function fetchGitDiffDetails() {
  if (!props.projectPath) return;
  isLoadingDiffs.value = true;
  try {
    const data = await api.getGitDiffDetails(props.projectPath);
    if (data.files) {
      gitDiffFiles.value = data.files.map((file: any) => {
        const existing = gitDiffFiles.value.find(f => f.path === file.path);
        const isOpen = existing ? existing.isOpen : true;

        // Reuse the previous rows when the file content is unchanged so the
        // rows array keeps its identity — DiffView then preserves which hidden
        // ranges the user has expanded across periodic refreshes.
        if (existing && existing.oldText === file.oldText && existing.newText === file.newText) {
          return { ...existing, kind: file.kind, isOpen };
        }

        const diffRows = lineDiff(file.oldText, file.newText);
        let added = 0;
        let deleted = 0;
        for (const row of diffRows) {
          if (row.type === 'add') added++;
          else if (row.type === 'del') deleted++;
        }
        return {
          path: file.path,
          kind: file.kind,
          oldText: file.oldText,
          newText: file.newText,
          isOpen,
          diffRows,
          added,
          deleted
        };
      });
    }
  } catch (err) {
    console.error('Error fetching diff details:', err);
  } finally {
    isLoadingDiffs.value = false;
  }
}

// The changes prop refreshes every ~200ms while the agent streams edits, and
// each git status check chains a diff-details fetch + client-side line diffs.
// Debounce the changes-driven refresh; explicit triggers (tab switch, run end)
// still call checkGitStatus directly.
let gitStatusRefreshTimer: ReturnType<typeof setTimeout> | null = null;
const GIT_STATUS_REFRESH_DELAY_MS = 800;

function scheduleGitStatusRefresh() {
  if (gitStatusRefreshTimer) return;
  gitStatusRefreshTimer = setTimeout(() => {
    gitStatusRefreshTimer = null;
    if (activeTab.value === 'review' && props.isOpen && props.projectPath) {
      checkGitStatus();
    }
  }, GIT_STATUS_REFRESH_DELAY_MS);
}

async function checkGitStatus() {
  if (!props.projectPath) return;
  try {
    const data = await api.getGitStatus(props.projectPath);
    if (data.isGit) {
      isGitRepo.value = true;
      gitBranch.value = data.branch || 'main';
      hasGitChanges.value = data.hasChanges || false;
      await fetchGitDiffDetails();
    } else {
      isGitRepo.value = false;
      hasGitChanges.value = false;
      gitDiffFiles.value = [];
    }
  } catch (err) {
    console.error('Error checking git status:', err);
    isGitRepo.value = false;
    hasGitChanges.value = false;
    gitDiffFiles.value = [];
  }
}

async function generateCommitMessage() {
  if (!props.runId) return;
  isGenerating.value = true;
  gitError.value = null;
  try {
    const data = await api.generateCommitMessage(props.runId);
    commitMessage.value = data.message;
  } catch (err: any) {
    gitError.value = err.message || 'Failed to generate commit message';
  } finally {
    isGenerating.value = false;
  }
}

async function executeGitAction(action: GitAction) {
  if (!props.projectPath) return;
  commitStatus.value = 'processing';
  gitError.value = null;
  try {
    await api.gitCommit(props.projectPath, commitMessage.value, action);

    commitStatus.value = 'success';
    commitMessage.value = '';
    setTimeout(() => {
      checkGitStatus();
      commitStatus.value = 'idle';
    }, 3000);
  } catch (err: any) {
    commitStatus.value = 'error';
    gitError.value = err.message || 'Git operation failed';
  }
}

function resetGitForm() {
  commitMessage.value = '';
  gitError.value = null;
  commitStatus.value = 'idle';
  showCommitPopup.value = false;
}

watch(
  () => [activeTab.value, props.projectPath, props.isOpen] as const,
  ([tab, path, isOpen]) => {
    if (tab === 'review' && path && isOpen) {
      checkGitStatus();
    }
  },
  { immediate: true }
);
// These props are replaced immutably upstream (App.vue computeds produce new
// arrays/objects), so identity watching is sufficient — `deep: true` would
// traverse full RunMessage trees and whole file texts on every update.
watch(
  () => props.plan,
  (newPlan) => {
    plan.value = newPlan;
  },
  { immediate: true }
);

watch(
  () => props.changes,
  (newChanges) => {
    changes.value = newChanges ?? [];
    if (activeTab.value === 'review' && props.isOpen && props.projectPath) {
      scheduleGitStatusRefresh();
    }
  },
  { immediate: true }
);

watch(
  () => props.agents,
  (newAgents) => {
    const next = newAgents ?? [];
    // The parent rebuilds this array on every side-panel snapshot; skip the
    // empty→empty case so runs without sub-agents don't churn identity and
    // retrigger every watcher downstream (tab reset, auto-scroll) at 5Hz.
    if (next.length === 0 && agents.value.length === 0) return;
    agents.value = next;
  },
  { immediate: true }
);

watch(
  () => props.isRunning,
  (running) => {
    if (!running && activeTab.value === 'review' && props.isOpen && props.projectPath) {
      checkGitStatus();
    }
  }
);

// Non-git fallback: workspace changes render as inline diff accordions, same
// layout as the git file list. Open state is keyed by change id (default open);
// diff rows are cached per id + content so periodic snapshots don't re-diff
// unchanged files and DiffView keeps its expanded ranges.
const openChangeAccordions = ref<Record<string, boolean>>({});
const changeDiffCache = new Map<string, { oldText: string; newText: string; rows: DiffRow[] }>();

function isChangeOpen(id: string): boolean {
  return openChangeAccordions.value[id] ?? true;
}

function toggleChangeAccordion(id: string) {
  openChangeAccordions.value[id] = !isChangeOpen(id);
}

function diffRowsForChange(change: WorkspaceChange): DiffRow[] {
  const cached = changeDiffCache.get(change.id);
  if (cached && cached.oldText === change.oldText && cached.newText === change.newText) {
    return cached.rows;
  }
  const rows = changeDiffRows(change);
  changeDiffCache.set(change.id, { oldText: change.oldText, newText: change.newText, rows });
  return rows;
}

// Collapse/expand all review accordions (works on whichever list is active:
// git files or the non-git workspace changes).
const anyReviewAccordionOpen = computed(() => {
  if (isGitRepo.value) return gitDiffFiles.value.some(file => file.isOpen);
  return changes.value.some(change => isChangeOpen(change.id));
});

function toggleAllReviewAccordions() {
  const open = !anyReviewAccordionOpen.value;
  if (isGitRepo.value) {
    for (const file of gitDiffFiles.value) file.isOpen = open;
    return;
  }
  const next: Record<string, boolean> = {};
  for (const change of changes.value) next[change.id] = open;
  openChangeAccordions.value = next;
}

const doneCount = computed(() => plan.value?.tasks.filter(t => t.status === 'completed').length ?? 0);
const totalCount = computed(() => plan.value?.tasks.length ?? 0);
const totalAdded = computed(() => changes.value.reduce((sum, c) => sum + c.added, 0));
const totalDeleted = computed(() => changes.value.reduce((sum, c) => sum + c.deleted, 0));
const runningAgentCount = computed(() => agents.value.filter(agent => agent.status === 'running').length);

const totalGitAdded = computed(() => gitDiffFiles.value.reduce((sum, file) => sum + file.added, 0));
const totalGitDeleted = computed(() => gitDiffFiles.value.reduce((sum, file) => sum + file.deleted, 0));

const tabs = computed(() => {
  const list: { id: PanelTab; label: string }[] = [];
  if (plan.value) list.push({ id: 'plan', label: 'Plan' });
  if (agents.value.length) list.push({ id: 'agents', label: 'Agents' });
  if (changes.value.length || isGitRepo.value) list.push({ id: 'review', label: 'Review' });
  list.push({ id: 'preview', label: 'Preview' });
  for (const id of openFileTabs.value) {
    const change = changes.value.find(c => c.id === id);
    if (change) list.push({ id: `file:${id}`, label: shortPath(change.path) });
  }
  return list;
});

const activeChange = computed(() => {
  if (!activeTab.value.startsWith('file:')) return null;
  const id = activeTab.value.slice(5);
  return changes.value.find(c => c.id === id) ?? null;
});

// Full rows — DiffView collapses unchanged context behind expandable separators.
const activeDiffRows = computed(() => activeChange.value ? changeDiffRows(activeChange.value) : []);

watch(
  () => [plan.value?.id, agents.value.length, changes.value.length, isGitRepo.value] as const,
  () => {
    if (isSwitchingRun.value) return;
    openFileTabs.value = openFileTabs.value.filter(id => changes.value.some(c => c.id === id));
    if (tabs.value.some(tab => tab.id === activeTab.value)) return;
    activeTab.value = plan.value ? 'plan' : agents.value.length ? 'agents' : 'review';
  },
  { immediate: true }
);

watch(
  () => agents.value.length,
  (count, previous) => {
    if (isSwitchingRun.value) return;
    if (count > 0 && previous === 0) {
      activeTab.value = 'agents';
    }
  }
);

watch(
  () => plan.value?.version,
  (newVer, oldVer) => {
    if (isSwitchingRun.value) return;
    if (newVer !== undefined && oldVer !== undefined && newVer !== oldVer) {
      activeTab.value = 'plan';
    }
  }
);

function openFile(change: WorkspaceChange) {
  loadedState.value = null; // Clear loadedState on user action
  if (!openFileTabs.value.includes(change.id)) {
    openFileTabs.value.push(change.id);
  }
  activeTab.value = `file:${change.id}`;
}

function closeFileTab(id: string) {
  loadedState.value = null; // Clear loadedState on user action
  openFileTabs.value = openFileTabs.value.filter(x => x !== id);
  if (activeTab.value === `file:${id}`) {
    activeTab.value = changes.value.length ? 'review' : 'plan';
  }
}

function shortPath(path: string): string {
  const parts = path.split('/').filter(Boolean);
  return parts[parts.length - 1] || path || 'File';
}

/** Review header label: basename only (both sides for renames); the full path
 *  lives in the header's hover tooltip. */
function reviewFileName(path: string): string {
  if (path.includes(' -> ')) {
    return path.split(' -> ').map(part => shortPath(part)).join(' -> ');
  }
  return shortPath(path);
}

function statusLabel(status: string): string {
  if (status === 'completed') return 'Done';
  if (status === 'in_progress') return 'Doing';
  return 'To do';
}

function changeKindLabel(kind: WorkspaceChange['kind']): string {
  if (kind === 'created') return 'Created';
  if (kind === 'edited') return 'Edited';
  if (kind === 'deleted') return 'Deleted';
  if (kind === 'moved') return 'Moved';
  return 'Changed';
}

function compactNumber(value: number): string {
  if (value >= 1000) return `${(value / 1000).toFixed(value >= 10000 ? 0 : 1)}k`;
  return String(value);
}

// Inline accordion collapse states
const expandedTranscripts = ref<Record<string, boolean>>({});
const expandedReasoning = ref<Record<string, boolean>>({});

const loadedState = ref<{
  activeTab: PanelTab;
  openFileTabs: string[];
  expandedTranscripts: Record<string, boolean>;
  expandedReasoning: Record<string, boolean>;
} | null>(null);

function saveCurrentState() {
  if (!props.runId || loadedState.value) return;
  const state = {
    activeTab: activeTab.value,
    openFileTabs: openFileTabs.value,
    expandedTranscripts: expandedTranscripts.value,
    expandedReasoning: expandedReasoning.value
  };
  localStorage.setItem(runStorageKeys.panelState(props.runId), JSON.stringify(state));
}

function loadRunState(id: string | null | undefined) {
  let loadedActiveTab: PanelTab = 'plan';
  let loadedOpenFileTabs: string[] = [];
  let loadedTranscripts: Record<string, boolean> = {};
  let loadedReasoning: Record<string, boolean> = {};

  if (id) {
    const stored = localStorage.getItem(runStorageKeys.panelState(id));
    if (stored) {
      try {
        const state = JSON.parse(stored);
        if (state.activeTab) loadedActiveTab = state.activeTab;
        if (Array.isArray(state.openFileTabs)) loadedOpenFileTabs = state.openFileTabs;
        if (state.expandedTranscripts) loadedTranscripts = state.expandedTranscripts;
        if (state.expandedReasoning) loadedReasoning = state.expandedReasoning;
      } catch (e) {
        console.error('Failed to parse stored run state', e);
      }
    }
  }

  // Save to loadedState so we can re-apply it once the actual data loads
  loadedState.value = {
    activeTab: loadedActiveTab,
    openFileTabs: loadedOpenFileTabs,
    expandedTranscripts: loadedTranscripts,
    expandedReasoning: loadedReasoning
  };

  // Filter open file tabs based on current changes
  const currentChanges = props.changes ?? [];
  loadedOpenFileTabs = loadedOpenFileTabs.filter(fileId => currentChanges.some(c => c.id === fileId));

  // Update refs immediately for responsiveness
  openFileTabs.value = loadedOpenFileTabs;
  expandedTranscripts.value = loadedTranscripts;
  expandedReasoning.value = loadedReasoning;

  // Validate activeTab against current tabs
  const validTabIds: PanelTab[] = [];
  if (props.plan) validTabIds.push('plan');
  if (props.agents?.length) validTabIds.push('agents');
  if (props.changes?.length) validTabIds.push('review');
  for (const fileId of loadedOpenFileTabs) {
    validTabIds.push(`file:${fileId}`);
  }

  if (validTabIds.includes(loadedActiveTab)) {
    activeTab.value = loadedActiveTab;
  } else {
    activeTab.value = props.plan ? 'plan' : (props.agents?.length ? 'agents' : 'review');
  }
}

watch(
  () => props.runId,
  async (newId) => {
    isSwitchingRun.value = true;
    openChangeAccordions.value = {};
    changeDiffCache.clear();
    loadRunState(newId);
    await nextTick();
    isSwitchingRun.value = false;

    // Safety fallback: clear loadedState if watchers didn't trigger
    setTimeout(() => {
      if (loadedState.value) {
        loadedState.value = null;
      }
    }, 10000);
  },
  { immediate: true }
);

watch(
  [activeTab, openFileTabs, expandedTranscripts, expandedReasoning],
  () => {
    saveCurrentState();
  },
  { deep: true }
);

// Re-apply loaded state once the asynchronous plan/agents/changes data finishes loading
watch(
  () => [plan.value, changes.value, agents.value] as const,
  async () => {
    if (!loadedState.value) return;

    // Wait for auto-tab-switching watchers to finish their updates
    await nextTick();

    if (!loadedState.value) return;

    const state = loadedState.value;
    
    // Filter open file tabs based on the actual loaded changes
    const actualOpenFileTabs = state.openFileTabs.filter(fileId => 
      changes.value.some(c => c.id === fileId)
    );
    
    openFileTabs.value = actualOpenFileTabs;
    expandedTranscripts.value = state.expandedTranscripts;
    expandedReasoning.value = state.expandedReasoning;
    
    // Construct valid tab IDs based on the loaded data
    const validTabIds: PanelTab[] = [];
    if (plan.value) validTabIds.push('plan');
    if (agents.value.length) validTabIds.push('agents');
    if (changes.value.length) validTabIds.push('review');
    for (const fileId of actualOpenFileTabs) {
      validTabIds.push(`file:${fileId}`);
    }
    
    if (validTabIds.includes(state.activeTab)) {
      activeTab.value = state.activeTab;
    } else {
      activeTab.value = plan.value ? 'plan' : (agents.value.length ? 'agents' : 'review');
    }
    
    loadedState.value = null;
  }
);

const panelBody = ref<HTMLElement | null>(null);
const stickyBottomThreshold = 24;

function latestReasoningBody(root: HTMLElement | null): HTMLElement | undefined {
  const bodies = root?.querySelectorAll('.reasoning-terminal-container .plan-body');
  return bodies?.[bodies.length - 1] as HTMLElement | undefined;
}

watch(
  agents,
  () => {
    // This auto-scroll only concerns the Agents tab's streaming transcripts.
    // Without the tab guard it force-scrolled the whole panel body on every
    // 200ms snapshot, making the Review tab visibly jump while code streamed.
    if (activeTab.value !== 'agents') return;
    const el = panelBody.value;
    if (!el) return;

    const activeReasoningBodyBefore = latestReasoningBody(el);
    const reasoningWasAtBottom = activeReasoningBodyBefore
      ? activeReasoningBodyBefore.scrollHeight - activeReasoningBodyBefore.scrollTop - activeReasoningBodyBefore.clientHeight <= 60
      : true;
    const previousReasoningScrollTop = activeReasoningBodyBefore?.scrollTop ?? 0;

    const panelWasAtBottom = el.scrollHeight - el.scrollTop - el.clientHeight <= stickyBottomThreshold;
    
    nextTick(() => {
      const activeReasoningBodyAfter = latestReasoningBody(el);
      if (activeReasoningBodyAfter) {
        activeReasoningBodyAfter.scrollTop = reasoningWasAtBottom
          ? activeReasoningBodyAfter.scrollHeight
          : previousReasoningScrollTop;
      }

      if (panelWasAtBottom) {
        el.scrollTop = el.scrollHeight;
      }
    });
  }
);

function isReasoningExpanded(childId: string, children: any[]): boolean {
  const explicit = expandedReasoning.value[childId];
  if (explicit !== undefined) return explicit;
  
  // By default, only expand the latest reasoning panel of the agent's children
  let latestId = null;
  for (let i = children.length - 1; i >= 0; i--) {
    if (children[i].message.reasoningContent) {
      latestId = children[i].id;
      break;
    }
  }
  return childId === latestId;
}

function toggleReasoning(childId: string) {
  const willExpand = !expandedReasoning.value[childId];
  expandedReasoning.value[childId] = willExpand;
  if (willExpand) {
    nextTick(() => {
      const el = document.querySelector(`[data-reasoning-child-id="${childId}"]`);
      el?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    });
  }
}

function thoughtFor(child: any): string {
  return child.message.role === 'tool' ? '' : child.message.content;
}

function expandAgentTranscript(agentId: string) {
  loadedState.value = null; // Clear loadedState on user action
  activeTab.value = 'agents';
  expandedTranscripts.value[agentId] = true;
  nextTick(() => {
    const el = document.querySelector(`[data-agent-row-id="${agentId}"]`);
    el?.scrollIntoView({ behavior: 'smooth', block: 'center' });
  });
}

function selectTab(tab: PanelTab) {
  loadedState.value = null; // Clear loadedState on user action
  activeTab.value = tab;
}

function openFileInReview(filePath: string) {
  if (isGitRepo.value) {
    const file = gitDiffFiles.value.find(f => f.path === filePath || f.path.endsWith(filePath));
    if (file) {
      activeTab.value = 'review';
      file.isOpen = true;
      nextTick(() => {
        const el = document.querySelector(`[data-file-path="${file.path}"]`);
        el?.scrollIntoView({ behavior: 'smooth', block: 'start' });
      });
      return;
    }
  }
  if (changes.value) {
    const change = changes.value.find(c => c.displayPath === filePath || c.displayPath.endsWith(filePath));
    if (change) {
      openFile(change);
    }
  }
}

defineExpose({
  expandAgentTranscript,
  selectTab,
  openFileInReview
});
</script>

<template>
  <aside class="workspace-panel">
    <div class="panel-resize-handle" @mousedown="initResize"></div>
    <header class="workspace-panel-header">
      <div class="panel-tabs" role="tablist" aria-label="Workspace panel">
        <button
          v-for="tab in tabs"
          :key="tab.id"
          type="button"
          class="panel-tab"
          :class="{ active: activeTab === tab.id }"
          @click="selectTab(tab.id)"
        >
          <span>{{ tab.label }}</span>
          <span
            v-if="tab.id.startsWith('file:')"
            class="panel-tab-close"
            title="Close file tab"
            @click.stop="closeFileTab(tab.id.slice(5))"
          >
            x
          </span>
        </button>
      </div>
      <button type="button" class="workspace-panel-close" title="Hide side panel" @click="$emit('close')">
        <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <rect width="18" height="18" x="3" y="3" rx="2" />
          <path d="M15 3v18" />
        </svg>
      </button>
    </header>

    <div ref="panelBody" class="workspace-panel-body">
      <section v-if="activeTab === 'plan'" class="panel-section">
        <div class="panel-section-heading">
          <div>
            <h2>{{ plan?.title || 'Plan' }}</h2>
            <p v-if="totalCount">{{ doneCount }} / {{ totalCount }} completed</p>
          </div>
        </div>

        <div v-if="plan?.body" class="plan-panel-prose" v-html="renderMarkdown(plan.body, 'plan-' + plan.id + '-' + plan.version)"></div>

        <ul v-if="plan && plan.tasks.length" class="plan-tasks">
          <li
            v-for="(task, index) in plan.tasks"
            :key="index"
            class="plan-task"
            :class="task.status"
          >
            <span class="plan-task-box" :class="task.status"></span>
            <span class="plan-task-text">{{ task.text }}</span>
            <span v-if="task.status !== 'completed'" class="plan-task-status" :class="task.status">{{ statusLabel(task.status) }}</span>
          </li>
        </ul>

        <p v-else-if="!plan?.body" class="panel-empty">
          The assistant's plan will appear here once it drafts one.
        </p>
      </section>

      <section v-else-if="activeTab === 'agents'" class="panel-section">
        <div class="panel-section-heading">
          <div>
            <h2>Background agents</h2>
            <p>{{ runningAgentCount }} running · {{ agents.length }} total</p>
          </div>
        </div>

        <div v-if="agents.length" class="agent-list">
          <article
            v-for="agent in agents"
            :key="agent.id"
            class="agent-row"
            :class="{ running: agent.status === 'running', expanded: expandedTranscripts[agent.id] }"
            :data-agent-row-id="agent.id"
          >
            <!-- Premium Header Row -->
            <div class="agent-row-header" @click="expandedTranscripts[agent.id] = !expandedTranscripts[agent.id]">
              <div class="agent-header-left">
                <!-- Status Dot with Pulse -->
                <div class="agent-status-indicator">
                  <span class="agent-status-dot" :class="agent.status"></span>
                  <span v-if="agent.status === 'running'" class="agent-status-pulse"></span>
                </div>
                
                <div class="agent-info">
                  <div class="agent-title-row">
                    <h3 class="agent-title">{{ agent.title }}</h3>
                    <span class="agent-role-badge" :class="agent.role">{{ agent.roleLabel }}</span>
                  </div>
                  <div class="agent-sub-info">
                    <span v-if="agent.model" class="agent-model-name">
                      <svg class="meta-icon" style="margin-right: 4px;" xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <rect x="2" y="2" width="20" height="8" rx="2" ry="2"></rect>
                        <rect x="2" y="14" width="20" height="8" rx="2" ry="2"></rect>
                        <line x1="6" y1="6" x2="6.01" y2="6"></line>
                        <line x1="6" y1="18" x2="6.01" y2="18"></line>
                      </svg>
                      {{ agent.model }}
                    </span>
                  </div>
                </div>
              </div>
              
              <!-- Chevron Expand Action -->
              <button 
                type="button" 
                class="agent-expand-toggle-btn" 
                @click.stop="expandedTranscripts[agent.id] = !expandedTranscripts[agent.id]"
                :title="expandedTranscripts[agent.id] ? 'Hide transcript' : 'View transcript'"
              >
                <svg 
                  class="chevron-icon" 
                  :class="{ rotated: expandedTranscripts[agent.id] }" 
                  xmlns="http://www.w3.org/2000/svg" 
                  width="14" 
                  height="14" 
                  viewBox="0 0 24 24" 
                  fill="none" 
                  stroke="currentColor" 
                  stroke-width="2.5" 
                  stroke-linecap="round" 
                  stroke-linejoin="round"
                >
                  <path d="m6 9 6 6 6-6"></path>
                </svg>
              </button>
            </div>

            <!-- Structured Metadata Bar -->
            <div class="agent-metadata-bar">
              <div class="meta-item tokens">
                <svg class="meta-icon" xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"></path>
                </svg>
                <span>{{ compactNumber(agent.tokenEstimate) }} tokens</span>
              </div>
              <div class="meta-item tools">
                <svg class="meta-icon" xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"></path>
                </svg>
                <span>{{ agent.toolUseCount }} tool {{ agent.toolUseCount === 1 ? 'use' : 'uses' }}</span>
              </div>
              <div class="meta-item status-badge" :class="agent.status">
                <span>{{ agent.status === 'running' ? 'Running' : 'Done' }}</span>
              </div>
            </div>

            <!-- Inline Accordion Transcript -->
            <div v-if="expandedTranscripts[agent.id] && agent.children" class="agent-transcript-body">
              <template v-for="child in agent.children" :key="child.id">
                <ReasoningPanel
                  v-if="child.message.reasoningContent"
                  :content="child.message.reasoningContent"
                  :expanded="isReasoningExpanded(child.id, agent.children)"
                  :data-reasoning-child-id="child.id"
                  @toggle="toggleReasoning(child.id)"
                />

                <ToolGroup
                  v-if="child.type === 'tool_group'"
                  :id="child.message.id"
                  :thought="thoughtFor(child)"
                  :tool-calls="child.toolCalls"
                  :tool-responses="child.toolResponses"
                  @open-plan="selectTab('plan')"
                  @view-file-in-review="openFileInReview"
                />

                <div
                  v-else-if="child.type === 'assistant' && cleanedMessageContent(child.message)"
                  class="coder-text markdown-body"
                  v-html="renderMarkdown(cleanedMessageContent(child.message), child.message.id)"
                ></div>
              </template>
            </div>
          </article>
        </div>

        <p v-else class="panel-empty">
          Sub-agents will appear here when the architect delegates work.
        </p>
      </section>

      <section v-else-if="activeTab === 'review'" class="panel-section">
        <div class="panel-section-heading">
          <div>
            <h2>Review</h2>
            <p>{{ isGitRepo ? gitDiffFiles.length : changes.length }} changed {{ (isGitRepo ? gitDiffFiles.length : changes.length) === 1 ? 'item' : 'items' }}</p>
          </div>
          
          <div class="review-header-actions">
            <button
              v-if="isGitRepo ? gitDiffFiles.length : changes.length"
              type="button"
              class="review-fold-all-btn"
              :title="anyReviewAccordionOpen ? 'Collapse all' : 'Expand all'"
              @click="toggleAllReviewAccordions"
            >
              <svg v-if="anyReviewAccordionOpen" viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                <path d="m4.5 2.5 3.5 3 3.5-3M4.5 13.5l3.5-3 3.5 3" />
              </svg>
              <svg v-else viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                <path d="m4.5 5.5 3.5-3 3.5 3M4.5 10.5l3.5 3 3.5-3" />
              </svg>
            </button>

            <!-- Commit Popup Trigger Wrapper -->
            <div v-if="isGitRepo" class="commit-popup-wrapper">
              <button
                type="button"
                class="commit-trigger-btn"
                @click.stop="toggleCommitPopup"
              >
                <svg viewBox="0 0 16 16" width="12" height="12" fill="currentColor" style="margin-right: 4px;">
                  <path fill-rule="evenodd" d="M11.75 2.5a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5zm-2.25.75a2.25 2.25 0 1 1 3 2.122V6A2.5 2.5 0 0 1 10 8.5H6a1 1 0 0 0-1 1v1.378a2.251 2.251 0 1 1-1.5 0V5.122a2.25 2.25 0 1 1 1.5 0v2.878h4A1 1 0 0 0 10 7V5.372a2.25 2.25 0 0 1-.5-1.622zM4.25 12a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5zM3.5 3.25a.75.75 0 1 1 1.5 0 .75.75 0 0 1-1.5 0z"/>
                </svg>
                Commit...
              </button>

              <!-- Commit Modal Teleport -->
              <Teleport to="body">
                <div v-if="showCommitPopup" class="commit-modal-backdrop" @click="closeDropdown">
                  <div class="git-commit-card modal-content" @click.stop>
                    <div class="git-header">
                      <span class="git-icon">
                        <svg viewBox="0 0 16 16" width="14" height="14" fill="currentColor">
                          <path fill-rule="evenodd" d="M11.75 2.5a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5zm-2.25.75a2.25 2.25 0 1 1 3 2.122V6A2.5 2.5 0 0 1 10 8.5H6a1 1 0 0 0-1 1v1.378a2.251 2.251 0 1 1-1.5 0V5.122a2.25 2.25 0 1 1 1.5 0v2.878h4A1 1 0 0 0 10 7V5.372a2.25 2.25 0 0 1-.5-1.622zM4.25 12a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5zM3.5 3.25a.75.75 0 1 1 1.5 0 .75.75 0 0 1-1.5 0z"/>
                        </svg>
                      </span>
                      <span class="git-branch">on branch <strong>{{ gitBranch }}</strong></span>
                    </div>

                    <div class="git-commit-form">
                      <!-- Warning message if clean and not pushing -->
                      <div v-if="!hasGitChanges && selectedGitAction !== 'push'" class="git-clean-msg">
                        No local changes to commit. Make edits or select <strong>Push</strong> to push local commits.
                      </div>

                      <!-- Buttoned Input -->
                      <div v-else class="git-input-wrapper">
                        <textarea
                          v-model="commitMessage"
                          placeholder="Commit message..."
                          class="git-commit-input"
                          :disabled="commitStatus === 'processing'"
                          rows="3"
                        ></textarea>
                        <button
                          type="button"
                          class="git-generate-btn"
                          :disabled="isGenerating || commitStatus === 'processing'"
                          @click="generateCommitMessage"
                          title="AI Generate Message"
                        >
                          <Spinner v-if="isGenerating" :size="12" track="var(--control-border-focus)" />
                          <span v-else>Generate</span>
                        </button>
                      </div>

                      <!-- Status Alerts -->
                      <div v-if="commitStatus === 'error' && gitError" class="git-alert error">
                        {{ gitError }}
                      </div>
                      <div v-if="commitStatus === 'success'" class="git-alert success">
                        Successfully executed!
                      </div>

                      <!-- Action Buttons Row -->
                      <div class="git-actions-row">
                        <button
                          type="button"
                          class="git-cancel-btn"
                          :disabled="commitStatus === 'processing'"
                          @click="resetGitForm"
                        >
                          Cancel
                        </button>

                        <!-- Nested Split Button -->
                        <div class="git-split-button-container">
                          <button
                            type="button"
                            class="git-main-btn"
                            :disabled="commitStatus === 'processing' || (!commitMessage.trim() && selectedGitAction !== 'push') || (!hasGitChanges && selectedGitAction !== 'push')"
                            @click="executeGitAction(selectedGitAction)"
                          >
                            <Spinner v-if="commitStatus === 'processing'" class="spinner-inline" :size="12" track="var(--control-border-focus)" />
                            <span v-else>{{ actionLabels[selectedGitAction] }}</span>
                          </button>

                          <button
                            type="button"
                            class="git-arrow-btn"
                            :disabled="commitStatus === 'processing'"
                            @click="toggleActionsDropdown"
                            title="More actions"
                          >
                            <svg viewBox="0 0 16 16" width="10" height="10" fill="currentColor">
                              <path d="M7.247 11.14 2.451 5.658C1.885 5.013 2.345 4 3.204 4h9.592a1 1 0 0 1 .753 1.659l-4.796 5.48a1 1 0 0 1-1.506 0z"/>
                            </svg>
                          </button>

                          <!-- Action Dropdown Menu -->
                          <div v-if="showActionsDropdown" class="git-dropdown-menu">
                            <button
                              v-for="action in availableGitActions"
                              :key="action"
                              type="button"
                              class="git-dropdown-item"
                              :class="{ active: selectedGitAction === action }"
                              @click="selectGitAction(action)"
                            >
                              {{ actionLabels[action] }}
                            </button>
                          </div>
                        </div>
                      </div>
                    </div>
                  </div>
                </div>
              </Teleport>
            </div>

            <div v-if="isGitRepo ? gitDiffFiles.length : changes.length" class="change-total">
              <span class="add">+{{ isGitRepo ? totalGitAdded : totalAdded }}</span>
              <span class="del">-{{ isGitRepo ? totalGitDeleted : totalDeleted }}</span>
            </div>
          </div>
        </div>

        <!-- Git Diffs list (accordion layout) -->
        <div v-if="isGitRepo" class="git-diff-list">
          <div v-if="isLoadingDiffs" class="panel-empty-small">
            <span class="spinner-small inline"></span> Loading repository changes...
          </div>
          <template v-else-if="gitDiffFiles.length">
            <div
              v-for="file in gitDiffFiles"
              :key="file.path"
              class="git-file-accordion"
              :class="{ collapsed: !file.isOpen }"
              :data-file-path="file.path"
            >
              <!-- Accordion Header -->
              <button
                type="button"
                class="git-file-header"
                :data-path="file.path"
                @click="file.isOpen = !file.isOpen"
              >
                <span class="git-file-chevron" :class="{ rotated: !file.isOpen }">
                  <svg viewBox="0 0 16 16" width="10" height="10" fill="currentColor">
                    <path fill-rule="evenodd" d="M1.646 4.646a.5.5 0 0 1 .708 0L8 10.293l5.646-5.647a.5.5 0 0 1 .708.708l-6 6a.5.5 0 0 1-.708 0l-6-6a.5.5 0 0 1 0-.708z"/>
                  </svg>
                </span>
                <span class="git-file-title">
                  <span class="git-file-path truncate">{{ reviewFileName(file.path) }}</span>
                  <span class="git-file-meta">
                    <span v-if="file.added || file.deleted" class="git-file-stats">
                      <span v-if="file.added" class="add">+{{ file.added }}</span>
                      <span v-if="file.deleted" class="del">-{{ file.deleted }}</span>
                    </span>
                    <span class="git-file-badge" :class="file.kind">{{ changeKindLabel(file.kind) }}</span>
                  </span>
                </span>
              </button>

              <!-- Accordion Content (Diff View) -->
              <div v-show="file.isOpen" class="git-file-diff-container">
                <DiffView v-if="file.diffRows.length" :rows="file.diffRows" />
                <p v-else class="panel-empty-small">
                  No changes in this file.
                </p>
              </div>
            </div>
          </template>
          <p v-else-if="!hasGitChanges" class="panel-empty-small">
            No changed files in the workspace.
          </p>
        </div>

        <!-- Fallback for non-git workspaces: same accordion + inline diff layout -->
        <div v-else-if="changes.length" class="git-diff-list">
          <div
            v-for="change in changes"
            :key="change.id"
            class="git-file-accordion"
            :class="{ collapsed: !isChangeOpen(change.id) }"
          >
            <button
              type="button"
              class="git-file-header"
              :data-path="change.displayPath"
              @click="toggleChangeAccordion(change.id)"
            >
              <span class="git-file-chevron" :class="{ rotated: !isChangeOpen(change.id) }">
                <svg viewBox="0 0 16 16" width="10" height="10" fill="currentColor">
                  <path fill-rule="evenodd" d="M1.646 4.646a.5.5 0 0 1 .708 0L8 10.293l5.646-5.647a.5.5 0 0 1 .708.708l-6 6a.5.5 0 0 1-.708 0l-6-6a.5.5 0 0 1 0-.708z"/>
                </svg>
              </span>
              <span class="git-file-title">
                <span class="git-file-path truncate">{{ reviewFileName(change.displayPath) }}</span>
                <span class="git-file-meta">
                  <span v-if="change.added || change.deleted" class="git-file-stats">
                    <span v-if="change.added" class="add">+{{ change.added }}</span>
                    <span v-if="change.deleted" class="del">-{{ change.deleted }}</span>
                  </span>
                  <span class="git-file-badge" :class="change.kind">{{ changeKindLabel(change.kind) }}</span>
                  <span
                    class="git-file-open-tab"
                    title="Open in tab"
                    @click.stop="openFile(change)"
                  >
                    <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                      <path d="M6.5 3.5h-3a1 1 0 0 0-1 1v8a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1v-3M9.5 2.5h4v4M13.5 2.5 7 9" />
                    </svg>
                  </span>
                </span>
              </span>
            </button>

            <div v-show="isChangeOpen(change.id)" class="git-file-diff-container">
              <div v-if="change.kind === 'moved'" class="rename-block">
                <span>{{ change.oldText }}</span>
                <span>-></span>
                <span>{{ change.newText }}</span>
              </div>
              <DiffView v-else-if="diffRowsForChange(change).length" :rows="diffRowsForChange(change)" />
              <p v-else class="panel-empty-small">
                No inline diff is available for this change.
              </p>
            </div>
          </div>
        </div>



        <p v-if="changes.length === 0 && !isGitRepo" class="panel-empty">
          File changes will appear here after the assistant edits the workspace.
        </p>
      </section>

      <section v-else-if="activeChange" class="panel-section file-section">
        <div class="panel-section-heading file-heading">
          <div>
            <h2>{{ activeChange.path }}</h2>
            <p>{{ changeKindLabel(activeChange.kind) }} by {{ activeChange.tool }}</p>
          </div>
          <div class="change-total">
            <span v-if="activeChange.added" class="add">+{{ activeChange.added }}</span>
            <span v-if="activeChange.deleted" class="del">-{{ activeChange.deleted }}</span>
          </div>
        </div>

        <div v-if="activeChange.kind === 'moved'" class="rename-block">
          <span>{{ activeChange.oldText }}</span>
          <span>-></span>
          <span>{{ activeChange.newText }}</span>
        </div>

        <DiffView v-else-if="activeDiffRows.length" :rows="activeDiffRows" />

        <p v-else class="panel-empty">
          No inline diff is available for this change.
        </p>
      </section>

      <section v-else-if="activeTab === 'preview'" class="panel-section preview-panel-section">
        <WebPreview />
      </section>
    </div>

    <footer v-if="showActions && plan && activeTab === 'plan'" class="plan-panel-actions">
      <ThemedButton variant="primary" class="plan-action-btn" @click="$emit('start')">Start building</ThemedButton>
      <ThemedButton variant="secondary" class="plan-action-btn" @click="$emit('revise')">Revise</ThemedButton>
      <ThemedButton variant="danger" class="plan-action-btn" @click="$emit('reject')">Reject</ThemedButton>
    </footer>
  </aside>
</template>

<style scoped>
.workspace-panel {
  display: flex;
  flex-direction: column;
  min-height: 0;
  border: none;
  border-left: 1px solid var(--control-border);
  border-radius: 0;
  margin: 0;
  padding-top: var(--shell-inset);
  background: var(--card-bg);
  overflow: hidden;
  opacity: 1;
  transform: translateX(0);
  position: relative;
  transition:
    margin 0.3s cubic-bezier(0.4, 0, 0.2, 1),
    opacity 0.2s ease,
    transform 0.3s cubic-bezier(0.4, 0, 0.2, 1),
    border-color 0.2s ease;
}

.panel-resize-handle {
  position: absolute;
  top: 0;
  left: 0;
  width: 6px;
  height: 100%;
  cursor: ew-resize;
  z-index: 1000;
  background: transparent;
  transition: background 0.15s ease;
}

.panel-resize-handle:hover,
.panel-resize-handle:active {
  background: var(--panel-resize-hover);
  border-left: 1px solid var(--border-soft);
}

.workspace-panel.collapsed {
  margin: 0;
  border-color: transparent;
  opacity: 0;
  pointer-events: none;
  transform: translateX(16px);
}

@media (max-width: 760px) {
  .workspace-panel {
    position: fixed !important;
    top: 0;
    right: 0;
    bottom: 0;
    width: 100vw !important;
    max-width: none !important;
    height: 100% !important;
    z-index: 1001;
    transform: translateX(0);
    opacity: 1 !important;
    pointer-events: auto !important;
    box-shadow: none;
    transition: transform 0.3s cubic-bezier(0.4, 0, 0.2, 1);
    border-left: none !important;
  }

  .workspace-panel.collapsed {
    transform: translateX(100%) !important;
    margin: 0;
    opacity: 1 !important;
    pointer-events: none !important;
  }
}

.preview-panel-section {
  padding: 0 !important;
  height: 100%;
  display: flex;
  flex-direction: column;
}

.workspace-panel-header {
  display: flex;
  align-items: center;
  gap: 10px;
  height: var(--top-bar-h);
  padding: 0 var(--card-pad-x);
  border-bottom: none;
  flex: 0 0 auto;
}

.panel-tabs {
  min-width: 0;
  flex: 1;
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: nowrap;
  overflow-x: auto;
  overflow-y: hidden;
  scrollbar-width: none;
}

.panel-tabs::-webkit-scrollbar {
  display: none;
}

.panel-tab {
  flex: 0 0 auto;
  display: inline-flex;
  align-items: center;
  gap: 7px;
  max-width: 150px;
  min-height: 30px;
  padding: 5px 10px;
  border: 1px solid transparent;
  border-radius: 8px;
  background: var(--panel-tab-bg);
  color: var(--muted);
  font-size: 0.82rem;
  cursor: pointer;
}

.panel-tab span:first-child {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.panel-tab:hover,
.panel-tab.active {
  background: var(--panel-tab-hover-bg);
  border-color: var(--panel-tab-hover-border);
  color: var(--text);
}

.panel-tab-close {
  flex: 0 0 auto;
  width: 16px;
  height: 16px;
  border-radius: 4px;
  background: transparent;
  color: var(--faint);
  cursor: pointer;
  line-height: 1;
}

.panel-tab-close:hover {
  background: var(--surface-strong);
  color: var(--text);
}

.workspace-panel-close {
  margin-left: auto;
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  background: var(--panel-close-bg);
  border: none;
  border-radius: 6px;
  color: var(--muted);
  cursor: pointer;
  padding: 0;
  transition: all 0.2s ease;
}

.workspace-panel-close:hover {
  color: var(--text);
  background: var(--panel-close-hover-bg);
}

.workspace-panel-body {
  min-height: 0;
  flex: 1;
  overflow-y: auto;
  scrollbar-gutter: stable;
}

.panel-section {
  padding: var(--card-pad-x);
}

.panel-section-heading {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 14px;
}

.panel-section-heading h2 {
  margin: 0;
  font-size: 0.95rem;
  font-weight: 650;
  color: var(--text);
  word-break: break-word;
}

.panel-section-heading p {
  margin: 4px 0 0;
  font-size: 0.76rem;
  color: var(--faint);
}

.change-total,
.change-stats {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.8rem;
}

.add {
  color: var(--msg-success-stroke);
}

.del {
  color: var(--plan-del-color);
}

.change-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.agent-list {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.agent-row {
  padding: 16px;
  border: 1px solid var(--agent-row-border);
  border-radius: 12px;
  background: var(--shimmer-glow);
  box-shadow: 0 4px 12px var(--attachment-shadow);
  display: flex;
  flex-direction: column;
  gap: 12px;
  transition: border-color 0.2s cubic-bezier(0.4, 0, 0.2, 1), 
              background 0.2s cubic-bezier(0.4, 0, 0.2, 1), 
              box-shadow 0.2s cubic-bezier(0.4, 0, 0.2, 1),
              transform 0.15s ease;
}

.agent-row:hover {
  border-color: var(--border);
  background: var(--surface);
  box-shadow: 0 6px 16px var(--composer-shadow);
  transform: translateY(-1px);
}

.agent-row.running {
  border-color: var(--success-border);
  background: var(--success-soft);
}

.agent-row.expanded {
  background: var(--shimmer-glow);
  border-color: var(--border);
  box-shadow: inset 0 1px 0 var(--control-border), 0 8px 24px var(--composer-shadow);
}

.agent-row-header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  cursor: pointer;
  user-select: none;
  gap: 12px;
}

.agent-header-left {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  min-width: 0;
  flex: 1;
}

.agent-status-indicator {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 10px;
  height: 10px;
  margin-top: 5px;
  flex-shrink: 0;
}

.agent-status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--faint);
  z-index: 2;
  transition: all 0.3s ease;
}

.agent-status-dot.running {
  background: var(--success);
}

.agent-status-dot.done {
  background: var(--success);
}

.agent-status-pulse {
  position: absolute;
  width: 20px;
  height: 20px;
  border-radius: 50%;
  background: var(--success-border);
  animation: agent-pulse-ring 2s infinite ease-out;
  z-index: 1;
}

@keyframes agent-pulse-ring {
  0% {
    transform: scale(0.3);
    opacity: 1;
  }
  100% {
    transform: scale(1);
    opacity: 0;
  }
}

.agent-info {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
  flex: 1;
}

.agent-title-row {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.agent-title {
  margin: 0;
  color: var(--text);
  font-size: 0.9rem;
  font-weight: 600;
  line-height: 1.35;
  overflow-wrap: anywhere;
}

.agent-role-badge {
  font-size: 0.74rem;
  font-style: italic;
  color: var(--faint);
  flex-shrink: 0;
}

.agent-sub-info {
  display: flex;
  align-items: center;
  font-size: 0.74rem;
  color: var(--faint);
}

.agent-model-name {
  display: flex;
  align-items: center;
  gap: 4px;
}

.meta-icon {
  color: var(--faint);
  flex-shrink: 0;
  opacity: 0.7;
}

.agent-expand-toggle-btn {
  background: var(--surface);
  border: 1px solid var(--border-soft);
  border-radius: 6px;
  color: var(--muted);
  width: 24px;
  height: 24px;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  flex-shrink: 0;
  transition: all 0.2s ease;
}

.agent-expand-toggle-btn:hover {
  background: var(--control-bg-hover);
  border-color: var(--border);
  color: var(--text);
}

.agent-metadata-bar {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  padding-top: 8px;
  border-top: 1px dashed var(--border-soft);
  font-size: 0.74rem;
  color: var(--muted);
}

.meta-item {
  display: flex;
  align-items: center;
  gap: 5px;
}

.meta-item.status-badge {
  margin-left: auto;
  font-size: 0.66rem;
  font-weight: 600;
  padding: 1px 6px;
  border-radius: 4px;
  text-transform: uppercase;
}

.meta-item.status-badge.running {
  background: var(--success-soft);
  color: var(--success);
}

.meta-item.status-badge.done {
  background: var(--control-border);
  color: var(--faint);
}

.change-row {
  width: 100%;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 9px 10px;
  border-radius: 7px;
  background: transparent;
  color: var(--text);
  cursor: pointer;
  text-align: left;
}

.change-row:hover {
  background: var(--surface);
}

.change-main {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.change-path {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 0.86rem;
}

.change-kind {
  color: var(--faint);
  font-size: 0.72rem;
}

.file-heading h2 {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.82rem;
}

.rename-block {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr);
  gap: 10px;
  padding: 12px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--surface);
  color: var(--muted);
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.78rem;
}

.rename-block span {
  overflow-wrap: anywhere;
}

.plan-panel-prose {
  font-size: 0.86rem;
  line-height: 1.55;
  color: var(--muted);
  margin-bottom: 16px;
  word-break: break-word;
}

.plan-panel-prose :deep(p) {
  margin: 0.5em 0;
}

.plan-panel-prose :deep(h1),
.plan-panel-prose :deep(h2),
.plan-panel-prose :deep(h3),
.plan-panel-prose :deep(h4) {
  color: var(--text);
  font-weight: 650;
  line-height: 1.3;
  margin: 1em 0 0.4em;
}

.plan-panel-prose :deep(h1) { font-size: 1rem; }
.plan-panel-prose :deep(h2) { font-size: 0.94rem; }
.plan-panel-prose :deep(h3),
.plan-panel-prose :deep(h4) { font-size: 0.88rem; }

.plan-panel-prose :deep(ul),
.plan-panel-prose :deep(ol) {
  padding-left: 1.25em;
  margin: 0.5em 0;
}

.plan-panel-prose :deep(li) {
  margin: 0.25em 0;
}

.plan-panel-prose :deep(strong) {
  color: var(--text);
}

.plan-panel-prose :deep(a) {
  color: var(--planner);
  text-decoration: none;
  transition: color 0.15s ease;
}

.plan-panel-prose :deep(a:hover) {
  color: var(--text);
  text-decoration: underline;
}

.plan-panel-prose :deep(code) {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.82em;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 1px 5px;
}

.plan-panel-prose :deep(pre) {
  margin: 0;
  padding: 14px;
  overflow: auto;
  max-height: 480px;
  background: transparent;
  border: none;
  border-radius: 0;
  white-space: pre !important;
  word-break: normal !important;
}

.plan-panel-prose :deep(pre code) {
  background: none;
  border: 0;
  padding: 0;
}

.plan-panel-prose :deep(table) {
  width: 100%;
  border-collapse: collapse;
  margin: 0.6em 0;
  font-size: 0.8rem;
}

.plan-panel-prose :deep(th),
.plan-panel-prose :deep(td) {
  border: 1px solid var(--border);
  padding: 5px 8px;
  text-align: left;
  vertical-align: top;
}

.plan-panel-prose :deep(th) {
  color: var(--text);
  background: var(--surface);
  font-weight: 600;
}

.plan-panel-prose :deep(blockquote) {
  border-left: 2px solid var(--border);
  margin: 0.6em 0;
  padding-left: 12px;
  color: var(--faint);
}

.panel-empty {
  font-size: 0.85rem;
  color: var(--faint);
  line-height: 1.5;
  margin: 0;
}

.plan-tasks {
  list-style: none;
  padding: 0;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.plan-task {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 7px 8px;
  border-radius: 6px;
  transition: background 0.15s ease, color 0.15s ease;
}

.plan-task:hover {
  background: var(--plan-task-hover-bg);
}

.plan-task.in_progress {
  background: var(--plan-task-hover-bg);
}

.plan-task.in_progress:hover {
  background: var(--plan-task-progress-hover-bg);
}

.plan-task-box {
  flex-shrink: 0;
  width: 14px;
  height: 14px;
  border-radius: 4px;
  border: 1.5px solid var(--faint);
  margin-top: 2px;
}

.plan-task-box.completed {
  background: var(--success);
  border-color: var(--success);
}

.plan-task-box.in_progress {
  border-color: var(--text);
  background: repeating-linear-gradient(
    45deg,
    var(--faint),
    var(--faint) 2px,
    transparent 2px,
    transparent 4px
  );
}

.plan-task-text {
  flex: 1 1 auto;
  font-size: 0.84rem;
  color: var(--text);
  line-height: 1.45;
  transition: color 0.15s ease;
}

.plan-task.completed .plan-task-text {
  color: var(--faint);
  text-decoration: line-through;
}

.plan-task.completed:hover .plan-task-text {
  color: var(--muted);
}

.plan-task-status {
  flex: 0 0 auto;
  font-size: 0.66rem;
  font-family: monospace;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--faint);
  margin-top: 2px;
}

.plan-task-status.in_progress {
  color: var(--text);
}

.plan-task-status.completed {
  color: var(--success);
}

.plan-panel-actions {
  flex: 0 0 auto;
  display: flex;
  gap: 8px;
  padding: 12px 16px;
  border-top: 1px solid var(--border);
}

.plan-action-btn {
  flex: 1 1 auto;
}

/* Timeline Transcript Design */
.agent-transcript-body {
  margin-top: 8px;
  padding-top: 16px;
  border-top: 1px solid var(--border-soft);
  display: flex;
  flex-direction: column;
  gap: 12px;
  position: relative;
  padding-left: 14px;
}

.agent-transcript-body::before {
  content: '';
  position: absolute;
  top: 16px;
  bottom: 16px;
  left: 2px;
  width: 1px;
  background: var(--border-soft);
}

.agent-transcript-body :deep(.reasoning-terminal-container),
.agent-transcript-body :deep(.tool-group-wrap),
.agent-transcript-body :deep(.coder-text) {
  position: relative;
}

.agent-transcript-body :deep(.reasoning-terminal-container)::before,
.agent-transcript-body :deep(.tool-group-wrap)::before,
.agent-transcript-body :deep(.coder-text)::before {
  content: '';
  position: absolute;
  left: -17px;
  top: 14px;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--border-soft);
  border: 2px solid var(--card-bg);
  z-index: 2;
}

.agent-transcript-body :deep(.reasoning-terminal-container) {
  margin: 4px 0 !important;
}

.agent-transcript-body :deep(.step-row) {
  padding: 6px 8px !important;
  border-radius: 6px !important;
  font-size: 0.8rem !important;
  background: var(--shimmer-glow);
}

.agent-transcript-body :deep(.step-row:hover) {
  color: var(--text);
  background: var(--surface);
}

.agent-transcript-body :deep(.coder-tool-model) {
  color: var(--muted);
}

.coder-text {
  color: var(--muted);
  line-height: 1.55;
  font-size: 0.84rem;
  margin: 4px 0;
  background: var(--shimmer-glow);
  padding: 10px 12px;
  border-radius: 8px;
  border: 1px solid var(--border-soft);
}

.chevron-icon {
  transition: transform 0.2s ease;
  color: inherit;
  display: block;
}

.chevron-icon.rotated {
  transform: rotate(180deg);
}

@keyframes commitScaleUp {
  from { transform: scale(0.97); opacity: 0; }
  to { transform: scale(1); opacity: 1; }
}

.commit-modal-backdrop {
  position: fixed;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background: var(--backdrop-bg);
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
  z-index: 9999;
  display: flex;
  align-items: center;
  justify-content: center;
  /* fadeIn comes from the global styles/modals.css keyframes. */
  animation: fadeIn 0.2s cubic-bezier(0.16, 1, 0.3, 1) forwards;
}

/* Git Commit Card Styling */
.git-commit-card {
  width: min(500px, 90vw);
  background: var(--surface-elevated);
  border: 1px solid var(--border);
  box-shadow: 0 20px 50px var(--composer-shadow);
  padding: 24px;
  border-radius: 14px;
  display: flex;
  flex-direction: column;
  gap: 16px;
  animation: commitScaleUp 0.25s cubic-bezier(0.16, 1, 0.3, 1) forwards;
}

.review-header-actions {
  display: flex;
  align-items: center;
  gap: 12px;
}

.commit-popup-wrapper {
  position: relative;
  display: inline-block;
}

.commit-trigger-btn {
  background: var(--btn-primary-bg);
  border: none;
  border-radius: 8px;
  color: var(--text);
  font-size: 0.76rem;
  font-weight: 600;
  padding: 6px 12px;
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  transition: background 0.15s ease;
}

.commit-trigger-btn:hover {
  background: var(--btn-primary-hover-bg);
}

.git-header {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 0.8rem;
  color: var(--muted);
}

.git-icon {
  color: var(--btn-primary-bg);
  display: flex;
  align-items: center;
}

.git-branch {
  font-family: system-ui, -apple-system, sans-serif;
}

.git-branch strong {
  color: var(--text);
  font-weight: 600;
}

.git-commit-form {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

/* Buttoned Input */
.git-input-wrapper {
  position: relative;
  display: flex;
  flex-direction: column;
}

.git-commit-input {
  width: 100%;
  box-sizing: border-box;
  background: var(--surface-strong);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 10px 80px 10px 12px;
  color: var(--text);
  font-size: 0.85rem;
  font-family: inherit;
  resize: vertical;
  line-height: 1.4;
  outline: none;
  transition: border-color 0.2s ease, box-shadow 0.2s ease;
}

.git-commit-input:focus {
  border-color: var(--btn-primary-bg);
  box-shadow: 0 0 0 2px var(--btn-primary-shadow);
}

.git-generate-btn {
  position: absolute;
  right: 6px;
  bottom: 6px;
  background: var(--control-border);
  border: 1px solid var(--border-soft);
  border-radius: 6px;
  color: var(--text);
  font-size: 0.72rem;
  padding: 4px 8px;
  cursor: pointer;
  transition: all 0.2s ease;
  z-index: 2;
  font-weight: 500;
}

.git-generate-btn:hover:not(:disabled) {
  background: var(--control-border-focus);
  border-color: var(--muted);
}

.git-generate-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

/* Action Buttons Row */
.git-actions-row {
  display: flex;
  justify-content: flex-end;
  align-items: center;
  gap: 8px;
}

.git-cancel-btn {
  background: transparent;
  border: 1px solid transparent;
  border-radius: 8px;
  color: var(--muted);
  font-size: 0.8rem;
  padding: 6px 12px;
  cursor: pointer;
  transition: all 0.2s ease;
}

.git-cancel-btn:hover:not(:disabled) {
  color: var(--text);
  background: var(--control-bg-hover);
}

.git-cancel-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

/* Nested Split Button */
.git-split-button-container {
  position: relative;
  display: inline-flex;
  border-radius: 8px;
  overflow: visible;
}

.git-main-btn {
  background: var(--btn-primary-bg);
  border: none;
  border-radius: 8px 0 0 8px !important;
  color: var(--text);
  font-size: 0.8rem;
  font-weight: 500;
  padding: 6px 12px;
  cursor: pointer;
  transition: background 0.2s ease;
  display: flex;
  align-items: center;
  gap: 6px;
}

.git-main-btn:hover:not(:disabled) {
  background: var(--btn-primary-hover-bg);
}

.git-main-btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.git-arrow-btn {
  background: var(--btn-primary-bg);
  border: none;
  border-left: 1px solid var(--control-border-focus);
  border-radius: 0 8px 8px 0 !important;
  color: var(--text);
  padding: 6px 8px;
  cursor: pointer;
  transition: background 0.2s ease;
  display: flex;
  align-items: center;
  justify-content: center;
}

.git-arrow-btn:hover:not(:disabled) {
  background: var(--btn-primary-hover-bg);
}

.git-arrow-btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

/* Dropdown Popup */
.git-dropdown-menu {
  position: absolute;
  bottom: 100%;
  right: 0;
  margin-bottom: 4px;
  background: var(--card-bg);
  border: 1px solid var(--border-soft);
  border-radius: 8px;
  box-shadow: 0 4px 16px var(--composer-dropdown-shadow);
  z-index: 100;
  min-width: 140px;
  padding: 4px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.git-dropdown-item {
  width: 100%;
  background: transparent;
  border: none;
  border-radius: 6px;
  color: var(--muted);
  font-size: 0.78rem;
  padding: 6px 10px;
  text-align: left;
  cursor: pointer;
  transition: all 0.2s ease;
}

.git-dropdown-item:hover {
  background: var(--composer-dropdown-item-hover);
  color: var(--text);
}

.git-dropdown-item.active {
  background: var(--btn-primary-shadow);
  color: var(--btn-primary-bg);
  font-weight: 500;
}

/* Alerts */
.git-alert {
  border-radius: 8px;
  padding: 8px 12px;
  font-size: 0.78rem;
  display: flex;
  align-items: center;
  line-height: 1.4;
}

.git-alert.error {
  background: var(--danger-soft);
  border: 1px solid var(--danger-border);
  color: var(--danger);
}

.git-alert.success {
  background: var(--success-soft);
  border: 1px solid var(--success-border);
  color: var(--success);
}

.spinner-inline {
  margin-right: 4px;
}

.git-clean-msg {
  font-size: 0.8rem;
  color: var(--muted);
  background: var(--shimmer-glow);
  border: 1px dashed var(--border-soft);
  border-radius: 8px;
  padding: 12px;
  text-align: center;
  line-height: 1.4;
}

/* Git Accordion Styles */
.git-diff-list {
  display: flex;
  flex-direction: column;
  gap: 0;
  margin-bottom: 20px;
  margin-left: calc(-1 * var(--card-pad-x));
  margin-right: calc(-1 * var(--card-pad-x));
  border-top: 1px solid var(--border-soft);
  border-bottom: 1px solid var(--border-soft);
  background: transparent;
  border-radius: 0 !important;
}

.git-file-accordion {
  border: none;
  border-bottom: 1px solid var(--border-soft);
  background: transparent;
  display: flex;
  flex-direction: column;
  transition: background 0.2s ease;
  border-radius: 0 !important;
}

.git-file-accordion:last-child {
  border-bottom: none;
}

.git-file-header {
  position: sticky;
  top: 0;
  z-index: 10;
  width: 100%;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px var(--card-pad-x);
  background: var(--card-bg);
  border: none;
  border-bottom: 1px solid var(--border-soft);
  color: var(--text);
  font-family: inherit;
  font-size: 0.82rem;
  line-height: 1.4;
  cursor: pointer;
  text-align: left;
  transition: background 0.15s ease;
  box-shadow: 0 1px 0 var(--border-soft);
  border-radius: 0 !important;
}

.git-file-header:hover {
  background: var(--control-bg-hover) !important;
}

/* Full-path tooltip: headers show only the file name; hovering reveals the
   complete workspace-relative location (Claude-Desktop-style). */
.git-file-header::after {
  content: attr(data-path);
  position: absolute;
  top: calc(100% + 4px);
  left: 28px;
  z-index: 30;
  max-width: min(440px, calc(100% - 40px));
  padding: 8px 12px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--card-bg);
  color: var(--text);
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.72rem;
  font-weight: 400;
  line-height: 1.5;
  white-space: normal;
  overflow-wrap: anywhere;
  box-shadow: 0 6px 18px rgba(0, 0, 0, 0.35);
  opacity: 0;
  pointer-events: none;
  transition: opacity 0.12s ease;
  transition-delay: 0s;
}

.git-file-header:hover::after {
  opacity: 1;
  transition-delay: 0.35s;
}

.git-file-chevron {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--muted);
  transition: transform 0.25s cubic-bezier(0.4, 0, 0.2, 1);
}

.git-file-chevron.rotated {
  transform: rotate(-90deg);
}

.git-file-title {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  min-width: 0;
}

.git-file-path {
  font-weight: 500;
  line-height: 1.4;
  padding-bottom: 3px;
  margin-bottom: -3px;
}

.git-file-badge {
  font-size: 0.68rem;
  padding: 2px 6px;
  border-radius: 0 !important;
  text-transform: uppercase;
  font-weight: 600;
  letter-spacing: 0.03em;
}

.git-file-badge.created {
  background: var(--success-soft);
  color: var(--success);
}

.git-file-badge.edited {
  background: var(--btn-primary-shadow);
  color: var(--btn-primary-bg);
}

.git-file-badge.deleted {
  background: var(--danger-soft);
  color: var(--plan-del-color);
}

.git-file-badge.moved {
  background: var(--status-warning-bg);
  color: var(--warning);
}

.git-file-diff-container {
  font-size: 0.8rem;
  background: var(--attachment-shadow);
}

.git-file-diff-container .diff-view {
  border: none;
  border-radius: 0 !important;
  background: transparent;
}

.panel-empty-small {
  padding: 16px;
  text-align: center;
  font-size: 0.78rem;
  color: var(--muted);
}

.git-file-meta {
  display: inline-flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}

.git-file-stats {
  display: inline-flex;
  gap: 6px;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.72rem;
  font-weight: 600;
}

.git-file-stats .add {
  color: var(--success);
}

.git-file-stats .del {
  color: var(--plan-del-color);
}

.git-file-open-tab {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 3px;
  color: var(--muted);
  transition: color 0.15s ease;
}

.git-file-open-tab:hover {
  color: var(--text);
}

.review-fold-all-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 5px;
  border: 1px solid var(--border-soft);
  border-radius: 6px;
  background: transparent;
  color: var(--muted);
  cursor: pointer;
  transition: color 0.15s ease, background 0.15s ease;
}

.review-fold-all-btn:hover {
  background: var(--control-bg-hover);
  color: var(--text);
}
</style>
