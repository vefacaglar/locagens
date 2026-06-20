<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue';
import type { Run } from '@locagens/shared';
import { ACTIVE_STATUSES, DEFAULT_PROJECT_PATH } from '../lib/format';

interface ProjectOption {
  path: string;
  name: string;
  count: number;
}

const props = defineProps<{
  projectOptions: ProjectOption[];
  activeProjectPath: string;
  runs: Run[];
  activeRunId: string | null;
  isSidebarCollapsed: boolean;
  showUsageLogsPage: boolean;
}>();

const emit = defineEmits<{
  (e: 'new-chat'): void;
  (e: 'add-project'): void;
  (e: 'select-project', path: string): void;
  (e: 'select-project-and-new-chat', path: string): void;
  (e: 'select-run', run: Run): void;
  (e: 'delete-project', path: string): void;
  (e: 'open-settings'): void;
  (e: 'toggle-sidebar'): void;
  (e: 'open-usage-logs'): void;
}>();

const expandedProjects = ref<Record<string, boolean>>((() => {
  try {
    const stored = localStorage.getItem('expandedProjects');
    return stored ? JSON.parse(stored) : {};
  } catch (e) {
    return {};
  }
})());
const visibleLimits = ref<Record<string, number>>({});
const isSearchOpen = ref(false);
const searchQuery = ref('');
const searchInput = ref<HTMLInputElement | null>(null);
const selectedSearchIndex = ref(0);
const readRunStamps = ref<Record<string, number>>(loadReadRunStamps());

const normalizedSearchQuery = computed(() => searchQuery.value.trim().toLowerCase());
const isSearching = computed(() => normalizedSearchQuery.value.length > 0);
const searchHeading = computed(() => isSearching.value ? 'Search results' : 'Recent chats');
const searchResults = computed(() => {
  const query = normalizedSearchQuery.value;
  return props.runs
    .filter(run => !query || runMatchesSearch(run))
    .slice()
    .sort((a, b) => Date.parse(b.lastActiveAt || b.updatedAt || b.createdAt) - Date.parse(a.lastActiveAt || a.updatedAt || a.createdAt))
    .slice(0, 9);
});

// Keep the active project expanded automatically
watch(
  () => props.activeProjectPath,
  (newPath) => {
    if (newPath) {
      expandedProjects.value[newPath] = true;
    }
  },
  { immediate: true }
);

watch(
  expandedProjects,
  (newVal) => {
    localStorage.setItem('expandedProjects', JSON.stringify(newVal));
  },
  { deep: true }
);

watch(searchQuery, () => {
  selectedSearchIndex.value = 0;
});

watch(searchResults, (results) => {
  if (selectedSearchIndex.value >= results.length) {
    selectedSearchIndex.value = Math.max(0, results.length - 1);
  }
});

watch(
  () => [props.activeRunId, props.runs] as const,
  () => markActiveRunRead(),
  { immediate: true, deep: true }
);

function handleProjectClick(path: string) {
  expandedProjects.value[path] = !expandedProjects.value[path];
}

function getBaseRunsForProject(projectPath: string): Run[] {
  return props.runs.filter(run => (run.projectPath || DEFAULT_PROJECT_PATH) === projectPath);
}

function getRunsForProject(projectPath: string): Run[] {
  return getBaseRunsForProject(projectPath);
}

function getLimitForProject(projectPath: string): number {
  return visibleLimits.value[projectPath] ?? 4;
}

function getDisplayedRunsForProject(projectPath: string): Run[] {
  const limit = getLimitForProject(projectPath);
  return getRunsForProject(projectPath).slice(0, limit);
}

function hasMoreRunsForProject(projectPath: string): boolean {
  const limit = getLimitForProject(projectPath);
  return getRunsForProject(projectPath).length > limit;
}

function loadMoreForProject(projectPath: string) {
  visibleLimits.value[projectPath] = getLimitForProject(projectPath) + 10;
}

function isProjectOpen(projectPath: string): boolean {
  return !!expandedProjects.value[projectPath];
}

function toggleSearch() {
  isSearchOpen.value = true;
  nextTick(() => {
    searchInput.value?.focus();
    searchInput.value?.select();
  });
}

function closeSearch() {
  searchQuery.value = '';
  isSearchOpen.value = false;
  selectedSearchIndex.value = 0;
}

function moveSearchSelection(delta: number) {
  if (searchResults.value.length === 0) return;
  const next = selectedSearchIndex.value + delta;
  selectedSearchIndex.value = (next + searchResults.value.length) % searchResults.value.length;
}

function selectHighlightedSearchRun() {
  const run = searchResults.value[selectedSearchIndex.value];
  if (run) selectSearchRun(run);
}

function selectSearchRun(run: Run) {
  closeSearch();
  emit('select-run', run);
}

function loadReadRunStamps(): Record<string, number> {
  try {
    const stored = localStorage.getItem('sidebarReadRunStamps');
    return stored ? JSON.parse(stored) : {};
  } catch {
    return {};
  }
}

function saveReadRunStamps() {
  localStorage.setItem('sidebarReadRunStamps', JSON.stringify(readRunStamps.value));
}

function runActivityTime(run: Run): number {
  const time = Date.parse(run.lastActiveAt || run.updatedAt || run.createdAt);
  return Number.isFinite(time) ? time : 0;
}

function markActiveRunRead() {
  if (!props.activeRunId) return;
  const run = props.runs.find(r => r.id === props.activeRunId);
  if (!run) return;
  const stamp = runActivityTime(run);
  if (!stamp) return;
  if (readRunStamps.value[run.id] === stamp) return;

  readRunStamps.value = {
    ...readRunStamps.value,
    [run.id]: stamp
  };
  saveReadRunStamps();
}

function isRunActive(run: Run): boolean {
  return ACTIVE_STATUSES.includes(run.status);
}

function isRunUnread(run: Run): boolean {
  if (run.id === props.activeRunId || isRunActive(run)) return false;
  const readStamp = readRunStamps.value[run.id];
  if (!readStamp) return false;
  return runActivityTime(run) > readStamp;
}

function runMatchesSearch(run: Run): boolean {
  const query = normalizedSearchQuery.value;
  if (!query) return true;
  return [run.title, run.task, run.projectName, run.model]
    .filter(Boolean)
    .some(value => String(value).toLowerCase().includes(query));
}

function projectNameForRun(run: Run): string {
  if (run.projectName) return run.projectName;
  const projectPath = run.projectPath || DEFAULT_PROJECT_PATH;
  const project = props.projectOptions.find(p => p.path === projectPath);
  return project?.name || projectPath.split(/[\\/]/).filter(Boolean).pop() || 'Project';
}

function searchRunTitle(run: Run): string {
  return run.title || run.task || 'Untitled chat';
}

function onNewChatForProject(path: string, event: Event) {
  event.stopPropagation();
  expandedProjects.value[path] = true;
  emit('select-project-and-new-chat', path);
}

function onDeleteProject(path: string, event: Event) {
  event.stopPropagation();
  emit('delete-project', path);
}

function formatRunAge(run: Run): string {
  const stamp = run.lastActiveAt || run.updatedAt || run.createdAt;
  const time = Date.parse(stamp);
  if (!Number.isFinite(time)) return '';

  const diffMs = Math.max(0, Date.now() - time);
  const minute = 60 * 1000;
  const hour = 60 * minute;
  const day = 24 * hour;
  const week = 7 * day;
  const month = 30 * day;
  const year = 365 * day;

  if (diffMs < minute) return 'now';
  if (diffMs < hour) return `${Math.floor(diffMs / minute)}m`;
  if (diffMs < day) return `${Math.floor(diffMs / hour)}h`;
  if (diffMs < week) return `${Math.floor(diffMs / day)}d`;
  if (diffMs < month) return `${Math.floor(diffMs / week)}w`;
  if (diffMs < year) return `${Math.floor(diffMs / month)}mo`;
  return `${Math.floor(diffMs / year)}y`;
}
</script>

<template>
  <aside class="sidebar" :class="{ collapsed: isSidebarCollapsed }">
    <div class="sidebar-header">
      <button class="collapse-btn" @click="emit('toggle-sidebar')" title="Collapse Sidebar">
        <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <rect width="18" height="18" x="3" y="3" rx="2" />
          <path d="M9 3v18" />
        </svg>
      </button>
    </div>

    <nav class="sidebar-nav" aria-label="Primary">
      <button class="nav-action sidebar-nav-item new-chat-action" @click="emit('new-chat')">
        <svg class="new-chat-icon" xmlns="http://www.w3.org/2000/svg" width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.25" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 20h9"/>
          <path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z"/>
        </svg>
        <span>New chat</span>
      </button>

      <button
        class="nav-action sidebar-nav-item search-action"
        :class="{ active: isSearchOpen }"
        type="button"
        @click="toggleSearch"
      >
        <svg xmlns="http://www.w3.org/2000/svg" width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.25" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="11" cy="11" r="8"/>
          <path d="m21 21-4.3-4.3"/>
        </svg>
        <span>Search</span>
      </button>

      <button class="nav-action sidebar-nav-item usage-logs-action" :class="{ active: showUsageLogsPage }" @click="emit('open-usage-logs')">
        <svg class="usage-logs-icon" xmlns="http://www.w3.org/2000/svg" width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.25" stroke-linecap="round" stroke-linejoin="round">
          <line x1="18" y1="20" x2="18" y2="10"/>
          <line x1="12" y1="20" x2="12" y2="4"/>
          <line x1="6" y1="20" x2="6" y2="14"/>
        </svg>
        <span>Usage logs</span>
      </button>
    </nav>

    <div class="sidebar-block projects-accordion">
      <div class="sidebar-label flex-between">
        <span>Projects</span>
        <button class="add-project-btn" title="Add Project" @click="emit('add-project')">
          <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>
            <line x1="12" y1="10" x2="12" y2="16"/>
            <line x1="9" y1="13" x2="15" y2="13"/>
          </svg>
        </button>
      </div>
      <div class="project-list">
        <div
          v-for="project in projectOptions"
          :key="project.path"
          class="project-accordion-item"
          :class="{ active: project.path === activeProjectPath }"
        >
          <div
            class="project-header"
            :class="{ active: project.path === activeProjectPath }"
            @click="handleProjectClick(project.path)"
          >
            <div class="project-header-left">
              <!-- Open Folder SVG when expanded -->
              <svg v-if="isProjectOpen(project.path)" class="folder-icon" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="m6 14 1.45-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.55 6a2 2 0 0 1-1.94 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2A2 2 0 0 0 12.07 6H20a2 2 0 0 1 2 2v2"/>
              </svg>
              <!-- Closed Folder SVG when collapsed -->
              <svg v-else class="folder-icon" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>
              </svg>
              <span class="project-name-text">{{ project.name }}</span>
            </div>
            <div class="project-header-actions">
              <button class="new-chat-project-btn" title="New Session" @click="onNewChatForProject(project.path, $event)">
                <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M12 20h9"/>
                  <path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z"/>
                </svg>
              </button>
              <button class="delete-project-btn" title="Remove Project" @click="onDeleteProject(project.path, $event)">
                <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M3 6h18"/>
                  <path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/>
                  <path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/>
                </svg>
              </button>
            </div>
          </div>

          <Transition name="expand">
            <div v-if="isProjectOpen(project.path)" class="project-chats-list">
              <div v-if="getRunsForProject(project.path).length === 0" class="empty-sidebar">No chats in this project.</div>
              <button
                v-for="run in getDisplayedRunsForProject(project.path)"
                :key="run.id"
                class="chat-history-item"
                :class="{ active: run.id === activeRunId }"
                @click="emit('select-run', run)"
              >
                <span v-if="isRunActive(run)" class="session-state session-spinner" aria-label="Running"></span>
                <span v-else-if="isRunUnread(run)" class="session-state session-unread-dot" aria-label="Unread"></span>
                <span class="chat-title">{{ run.title }}</span>
                <span class="chat-age">{{ formatRunAge(run) }}</span>
              </button>
              <button
                v-if="hasMoreRunsForProject(project.path)"
                type="button"
                class="load-more-btn"
                @click="loadMoreForProject(project.path)"
              >
                Show more
              </button>
            </div>
          </Transition>
        </div>
      </div>
    </div>

    <button class="nav-action muted settings-action" @click="emit('open-settings')">
      <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.1a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/>
        <circle cx="12" cy="12" r="3"/>
      </svg>
      <span>Settings</span>
    </button>
  </aside>

  <Teleport to="body">
    <div v-if="isSearchOpen" class="search-palette-backdrop" @click="closeSearch"></div>
    <section v-if="isSearchOpen" class="search-palette" @click.stop>
      <div class="search-palette-input-row">
        <input
          ref="searchInput"
          v-model="searchQuery"
          type="search"
          placeholder="Search chats"
          @keydown.esc="closeSearch"
          @keydown.down.prevent="moveSearchSelection(1)"
          @keydown.up.prevent="moveSearchSelection(-1)"
          @keydown.enter.prevent="selectHighlightedSearchRun"
        />
      </div>

      <div class="search-palette-section-label">{{ searchHeading }}</div>
      <div class="search-palette-results">
        <button
          v-for="(run, idx) in searchResults"
          :key="run.id"
          type="button"
          class="search-palette-result"
          :class="{ active: idx === selectedSearchIndex }"
          @mouseenter="selectedSearchIndex = idx"
          @click="selectSearchRun(run)"
        >
          <span class="search-result-title">{{ searchRunTitle(run) }}</span>
          <span class="search-result-project">{{ projectNameForRun(run) }}</span>
        </button>
        <div v-if="searchResults.length === 0" class="search-palette-empty">No matching chats.</div>
      </div>
    </section>
  </Teleport>
</template>


<style scoped>
.sidebar {
  gap: 0;
  padding: 12px 14px 14px !important;
  border-right: 1px solid var(--border);
  border-radius: 0 !important;
  border-top-left-radius: 0 !important;
  border-top-right-radius: 0 !important;
  border-bottom-left-radius: 0 !important;
  border-bottom-right-radius: 0 !important;
  background: var(--sidebar);
  outline: none;
  box-shadow: none;
}

.nav-action {
  padding: 0;
  font-size: 0.9rem;
}

.nav-action.active {
  color: var(--text);
  background: var(--sidebar-active);
  border-color: var(--border-soft);
}

.sidebar-header {
  display: flex;
  justify-content: flex-end;
  align-items: center;
  height: var(--top-bar-h);
  flex: 0 0 auto;
  padding: 0;
  background: transparent;
  border: none;
  margin: 0 0 12px;
}

.collapse-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border-radius: 6px;
  background: transparent;
  color: var(--faint);
  cursor: pointer;
  transition: all 0.2s ease;
  border: none;
}

.collapse-btn:hover {
  background: var(--control-bg-hover);
  color: var(--text);
}

.sidebar-nav {
  display: flex;
  flex-direction: column;
  gap: 3px;
  margin-bottom: 18px;
}

.sidebar-nav-item {
  display: grid;
  grid-template-columns: 24px minmax(0, 1fr);
  align-items: center;
  gap: 10px;
  min-height: 30px;
  padding: 3px 8px;
  border-radius: 7px;
  color: var(--muted);
}

.sidebar-nav-item svg {
  justify-self: center;
}

.sidebar-nav-item:hover {
  background: var(--control-bg-hover);
  color: var(--text);
}

.projects-accordion {
  flex: 1;
  overflow-y: auto;
  min-height: 0;
  margin-top: 0;
  scrollbar-gutter: stable;
}

.new-chat-action,
.usage-logs-action,
.search-action {
  display: grid;
  grid-template-columns: 24px minmax(0, 1fr);
  align-items: center;
  gap: 10px;
}

.new-chat-icon {
  flex-shrink: 0;
}

.settings-action {
  margin-top: 12px;
  flex-shrink: 0;
  display: grid;
  grid-template-columns: 24px minmax(0, 1fr);
  min-height: 32px;
  padding: 3px 8px;
  border-radius: 7px;
  color: var(--text);
  font-size: 0.9rem;
}

.settings-action svg {
  flex-shrink: 0;
  justify-self: center;
}

.settings-action:hover {
  background: var(--control-bg-hover);
}

.project-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-top: 12px;
}

.project-accordion-item {
  border-radius: 7px;
  overflow: hidden;
  transition: all 0.2s cubic-bezier(0.4, 0, 0.2, 1);
}

.project-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  min-height: 30px;
  padding: 3px 8px;
  border-radius: 7px;
  color: var(--faint);
  font-weight: 700;
  cursor: pointer;
  user-select: none;
  transition: all 0.2s ease;
  line-height: 1.35;
}

.project-header-left {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
  flex: 1;
}

.folder-icon {
  flex-shrink: 0;
  color: inherit;
  transition: color 0.2s ease;
  opacity: 0.85;
}

.project-header:hover .folder-icon {
  color: var(--text);
}

.project-header.active .folder-icon {
  color: inherit;
}

.project-header:hover {
  background: var(--control-bg-hover);
  color: var(--text);
}

.project-header.active {
  color: var(--faint);
  font-weight: 700;
}

.project-name-text {
  flex: 1 1 auto;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 0.9rem;
  line-height: 1.35;
  font-weight: inherit;
}

.project-chats-list {
  padding: 3px 0 1px 32px;
  margin-top: 0;
  border-left: none;
  margin-left: 0;
  display: flex;
  flex-direction: column;
  gap: 0;
}

.chat-history-item {
  position: relative;
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: baseline;
  gap: 10px;
  min-height: 0;
  padding: 2px 8px 3px 0;
  border-radius: 6px;
  color: var(--muted);
  font-size: 0.82rem;
  line-height: 1.45;
  overflow: visible;
}

.session-state {
  position: absolute;
  left: -18px;
  top: 50%;
  transform: translateY(-50%);
  flex-shrink: 0;
}

.session-unread-dot {
  width: 6px;
  height: 6px;
  border-radius: 999px;
  background: var(--text);
  box-shadow: 0 0 0 1px var(--control-border);
}

.session-spinner {
  width: 12px;
  height: 12px;
  border-radius: 999px;
  border: 2px solid var(--border-soft);
  border-top-color: var(--text);
  animation: sidebar-session-spin 0.75s linear infinite;
}

@keyframes sidebar-session-spin {
  to {
    transform: translateY(-50%) rotate(360deg);
  }
}

.chat-history-item:hover {
  background: transparent;
  color: var(--text);
}

.chat-history-item.active {
  background: transparent;
  border-color: transparent;
  color: var(--text);
  font-weight: 500;
}

.chat-title {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  line-height: 1.45;
  padding-bottom: 0;
}

.chat-age {
  color: var(--faint);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
  line-height: 1.45;
  padding-bottom: 0;
}

.project-header-actions {
  display: flex;
  align-items: center;
  gap: 2px;
}

.new-chat-project-btn,
.delete-project-btn {
  background: transparent;
  color: var(--faint);
  cursor: pointer;
  padding: 3px;
  border-radius: 4px;
  display: flex;
  align-items: center;
  justify-content: center;
  opacity: 0;
  transition: all 0.2s ease;
}

.project-header:hover .new-chat-project-btn,
.project-header:hover .delete-project-btn {
  opacity: 1;
}

.new-chat-project-btn:hover {
  color: var(--text);
  background: var(--control-bg-hover);
}

.delete-project-btn:hover {
  color: var(--danger);
  background: var(--danger-soft-strong);
}

/* Slide expand transition for the accordion */
.expand-enter-active,
.expand-leave-active {
  transition: max-height 0.22s cubic-bezier(0.4, 0, 0.2, 1), opacity 0.22s ease;
  max-height: 400px;
  overflow: hidden;
}

.expand-enter-from,
.expand-leave-to {
  max-height: 0;
  opacity: 0;
}

.load-more-btn {
  background: transparent;
  color: var(--faint);
  font-size: 0.82rem;
  padding: 2px 8px 2px 0;
  text-align: left;
  cursor: pointer;
  width: 100%;
  transition: color 0.2s ease;
  border: none;
  border-radius: 4px;
}

.load-more-btn:hover {
  color: var(--text);
  background: transparent;
}

.sidebar-label {
  padding: 0 8px;
  color: var(--faint);
  font-size: 0.9rem;
  line-height: 1.2;
}

.add-project-btn {
  opacity: 0;
  color: var(--faint);
}

.projects-accordion:hover .add-project-btn {
  opacity: 1;
}

.add-project-btn:hover {
  color: var(--text);
  background: var(--control-bg-hover);
}

.search-palette-backdrop {
  position: fixed;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background: rgba(0, 0, 0, 0.5);
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
  z-index: 99999;
}

.search-palette {
  position: fixed;
  top: 96px;
  left: 50%;
  width: min(600px, 90vw);
  max-height: min(560px, calc(100vh - 96px));
  transform: translateX(-50%);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  border: 1px solid var(--border);
  border-radius: 24px;
  background: var(--surface-elevated);
  box-shadow: 0 24px 80px rgba(0, 0, 0, 0.42);
  z-index: 100000;
}

.search-palette-input-row {
  padding: 18px 24px 12px;
}

.search-palette-input-row input {
  width: 100%;
  border: 0;
  outline: 0;
  background: transparent;
  color: var(--text);
  font: inherit;
  font-size: 1.02rem;
  line-height: 1.45;
}

.search-palette-input-row input::placeholder {
  color: var(--faint);
}

.search-palette-section-label {
  padding: 0 24px 8px;
  color: var(--faint);
  font-size: 0.9rem;
}

.search-palette-results {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-height: 0;
  overflow-y: auto;
  padding: 0 8px 12px;
}

.search-palette-result {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  gap: 12px;
  min-height: 40px;
  padding: 7px 8px;
  border: 0;
  border-radius: 16px;
  background: transparent;
  color: var(--muted);
  cursor: pointer;
  text-align: left;
  font: inherit;
}

.search-palette-result.active,
.search-palette-result:hover {
  background: var(--control-bg-hover);
  color: var(--text);
}

.search-result-title {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 0.98rem;
  line-height: 1.35;
}

.search-result-project {
  max-width: 180px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--faint);
  font-size: 0.9rem;
}

.search-palette-empty {
  padding: 18px 24px 24px;
  color: var(--faint);
  font-size: 0.92rem;
}
</style>
