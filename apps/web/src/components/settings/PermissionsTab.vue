<script setup lang="ts">
import { computed } from 'vue';
import type { PermissionRule } from '@locagens/shared';
import { basename } from '../../lib/format';
import { useExpandedMap } from '../../composables/useExpandedMap';
import ThemedButton from '../ui/ThemedButton.vue';

const props = defineProps<{
  permissions: PermissionRule[];
  isLoading: boolean;
}>();

const emit = defineEmits<{
  (e: 'revoke', id: number): void;
  (e: 'clear-all'): void;
}>();

const { map: expandedIds, toggle: toggleExpand } = useExpandedMap<number>();
const { map: expandedGroups, toggle: toggleGroup } = useExpandedMap();

function ruleScopeText(rule: PermissionRule): string {
  return rule.scope === 'global'
    ? 'Allowed in all projects'
    : `Allowed in ${basename(rule.projectPath)}`;
}

function truncateCommand(cmd: string): string {
  if (!cmd) return '';
  return cmd.length > 50 ? cmd.slice(0, 50) + '...' : cmd;
}

const globalPermissions = computed(() => props.permissions.filter(p => p.scope === 'global'));

const projectPermissionsGroups = computed(() => {
  const byPath = new Map<string, PermissionRule[]>();
  for (const p of props.permissions) {
    if (p.scope === 'global') continue;
    const path = p.projectPath || 'Unknown Project';
    const list = byPath.get(path) ?? [];
    list.push(p);
    byPath.set(path, list);
  }
  return [...byPath.entries()].map(([path, items]) => ({ path, label: basename(path) || path, items }));
});
</script>

<template>
  <div class="settings-tab-panel">
    <header class="settings-section-head">
      <div>
        <h3 class="settings-section-title">Permissions</h3>
        <p class="settings-section-desc">
          Saved approvals let tool calls run without asking. Remove one to be
          prompted again next time.
        </p>
      </div>
      <ThemedButton
        v-if="permissions.length > 0"
        variant="danger"
        size="sm"
        @click="emit('clear-all')"
      >
        Clear all
      </ThemedButton>
    </header>

    <div v-if="isLoading" class="settings-empty">Loading…</div>

    <div v-else-if="permissions.length === 0" class="settings-empty">
      No saved permissions. Every tool call will ask for approval.
    </div>

    <template v-else>
      <!-- Global Permissions -->
      <section v-if="globalPermissions.length > 0" class="perm-group">
        <h4 class="perm-group-title" @click="toggleGroup('global')">
          <span>Global</span>
          <span class="group-arrow" :class="{ rotated: expandedGroups['global'] }">
            <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
              <path d="m6 9 6 6 6-6"/>
            </svg>
          </span>
        </h4>
        <ul v-if="expandedGroups['global']" class="perm-accordion-list">
          <li
            v-for="rule in globalPermissions"
            :key="rule.id"
            class="perm-accordion-item"
          >
            <div class="perm-accordion-header" @click="toggleExpand(rule.id)">
              <span class="perm-accordion-arrow" :class="{ rotated: expandedIds[rule.id] }">
                <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                  <path d="m9 18 6-6-6-6"/>
                </svg>
              </span>
              <span class="perm-accordion-title">{{ rule.tool }}</span>
              <span v-if="rule.command" class="perm-accordion-desc truncate">— {{ truncateCommand(rule.command) }}</span>
              <ThemedButton
                variant="danger"
                size="sm"
                class="perm-revoke-btn"
                title="Revoke"
                @click.stop="emit('revoke', rule.id)"
              >
                Revoke
              </ThemedButton>
            </div>
            
            <div v-if="expandedIds[rule.id]" class="perm-accordion-content">
              <div class="perm-details-grid">
                <div v-if="rule.command" class="perm-detail-row">
                  <span class="perm-detail-label">Command:</span>
                  <code class="perm-detail-code">{{ rule.command }}</code>
                </div>
                <div class="perm-detail-row">
                  <span class="perm-detail-label">Scope:</span>
                  <span class="perm-detail-val">{{ ruleScopeText(rule) }}</span>
                </div>
              </div>
            </div>
          </li>
        </ul>
      </section>

      <!-- Project Permissions -->
      <section v-for="group in projectPermissionsGroups" :key="group.path" class="perm-group">
        <h4 class="perm-group-title" @click="toggleGroup(group.path)">
          <span>Project · <span class="perm-group-path">{{ group.label }}</span></span>
          <span class="group-arrow" :class="{ rotated: expandedGroups[group.path] }">
            <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
              <path d="m6 9 6 6 6-6"/>
            </svg>
          </span>
        </h4>
        <ul v-if="expandedGroups[group.path]" class="perm-accordion-list">
          <li
            v-for="rule in group.items"
            :key="rule.id"
            class="perm-accordion-item"
          >
            <div class="perm-accordion-header" @click="toggleExpand(rule.id)">
              <span class="perm-accordion-arrow" :class="{ rotated: expandedIds[rule.id] }">
                <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                  <path d="m9 18 6-6-6-6"/>
                </svg>
              </span>
              <span class="perm-accordion-title">{{ rule.tool }}</span>
              <span v-if="rule.command" class="perm-accordion-desc truncate">— {{ truncateCommand(rule.command) }}</span>
              <ThemedButton
                variant="danger"
                size="sm"
                class="perm-revoke-btn"
                title="Revoke"
                @click.stop="emit('revoke', rule.id)"
              >
                Revoke
              </ThemedButton>
            </div>
            
            <div v-if="expandedIds[rule.id]" class="perm-accordion-content">
              <div class="perm-details-grid">
                <div v-if="rule.command" class="perm-detail-row">
                  <span class="perm-detail-label">Command:</span>
                  <code class="perm-detail-code">{{ rule.command }}</code>
                </div>
                <div class="perm-detail-row">
                  <span class="perm-detail-label">Path:</span>
                  <span class="perm-detail-val">{{ rule.projectPath }}</span>
                </div>
              </div>
            </div>
          </li>
        </ul>
      </section>
    </template>
  </div>
</template>

<style scoped>
.perm-accordion-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.perm-accordion-item {
  border-bottom: 1px solid var(--perm-group-border);
  padding: 8px 0;
}

.perm-accordion-item:last-child {
  border-bottom: none;
}

.perm-accordion-header {
  display: flex;
  align-items: center;
  gap: 8px;
  cursor: pointer;
  padding: 6px 8px;
  user-select: none;
  border-radius: 6px;
  transition: background 0.15s ease, color 0.15s ease;
}

.perm-accordion-header:hover {
  background: var(--perm-header-hover-bg);
}

.perm-accordion-arrow {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--faint);
  transition: transform 0.2s ease;
  flex-shrink: 0;
}

.perm-accordion-arrow.rotated {
  transform: rotate(90deg);
}

.perm-accordion-title {
  font-size: 0.88rem;
  font-weight: 500;
  color: var(--text);
  flex-shrink: 0;
}

.perm-accordion-desc {
  font-size: 0.82rem;
  color: var(--faint);
  flex: 1;
  min-width: 0;
}

.perm-revoke-btn {
  margin-left: auto;
  opacity: 0;
  transition: opacity 0.2s ease;
  flex-shrink: 0;
}

.perm-accordion-header:hover .perm-revoke-btn {
  opacity: 1;
}

@media (max-width: 768px) {
  .perm-revoke-btn {
    opacity: 1;
  }
}

.perm-accordion-content {
  padding: 10px 12px 10px 20px;
  background: var(--perm-content-bg);
  border-radius: 6px;
  margin-top: 4px;
}

.perm-details-grid {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.perm-detail-row {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  font-size: 0.8rem;
}

.perm-detail-label {
  color: var(--muted);
  font-weight: 500;
  width: 70px;
  flex-shrink: 0;
}

.perm-detail-val {
  color: var(--text);
  word-break: break-all;
}

.perm-detail-code {
  font-family: monospace;
  background: var(--perm-code-bg);
  padding: 2px 6px;
  border-radius: 4px;
  color: var(--perm-code-color);
  word-break: break-all;
}

.perm-group {
  margin-bottom: 20px;
}

.perm-group-title {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin: 0 0 10px;
  font-size: 0.74rem;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: var(--muted);
  font-weight: 400;
  cursor: pointer;
  user-select: none;
  transition: color 0.15s ease;
}

.perm-group-title:hover {
  color: var(--text);
}

.perm-group-title:hover .group-arrow {
  color: var(--text);
}

.group-arrow {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--faint);
  transition: transform 0.2s ease, color 0.15s ease;
}

.group-arrow.rotated {
  transform: rotate(180deg);
}

.perm-group-path {
  font-family: monospace;
  text-transform: none;
  letter-spacing: 0;
  color: var(--faint);
}
</style>
