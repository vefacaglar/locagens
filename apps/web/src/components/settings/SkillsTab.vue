<script setup lang="ts">
import { computed, ref } from 'vue';
import type { SkillSummary } from '@locagens/shared';
import ThemedButton from '../ui/ThemedButton.vue';

const props = defineProps<{
  skills: SkillSummary[];
  isLoading: boolean;
  isInstalling?: boolean;
  userRoot: string;
  projectRoot: string | null;
  activeProjectPath: string;
  activeProjectName: string;
  error?: string | null;
  statusMessage?: string | null;
}>();

const emit = defineEmits<{
  (e: 'refresh'): void;
  (e: 'install-file', payload: { target: 'user' | 'project'; file: File }): void;
}>();

const userSkills = computed(() => props.skills.filter(s => s.source === 'user'));
const projectSkills = computed(() => props.skills.filter(s => s.source === 'project'));
const canInstallProject = computed(() => !!props.activeProjectPath);

const userInput = ref<HTMLInputElement | null>(null);
const projectInput = ref<HTMLInputElement | null>(null);

function pickUser() {
  userInput.value?.click();
}
function pickProject() {
  if (!canInstallProject.value) return;
  projectInput.value?.click();
}

function onUserFiles(ev: Event) {
  const input = ev.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = '';
  if (file) emit('install-file', { target: 'user', file });
}

function onProjectFiles(ev: Event) {
  const input = ev.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = '';
  if (file) emit('install-file', { target: 'project', file });
}
</script>

<template>
  <div class="settings-tab-panel">
    <header class="settings-section-head">
      <div>
        <h3 class="settings-section-title">Skills</h3>
        <p class="settings-section-desc">
          Specialized instruction packs the assistant loads with
          <code>load_skill</code>. Use <strong>Add skill</strong> to pick a
          <code>SKILL.md</code> from disk (works in the browser and desktop app);
          it is copied into the app skills folder.
        </p>
      </div>
      <ThemedButton variant="secondary" size="sm" :disabled="isLoading || isInstalling" @click="emit('refresh')">
        Refresh
      </ThemedButton>
    </header>

    <p v-if="error" class="skills-banner skills-banner-error">{{ error }}</p>
    <p v-else-if="statusMessage" class="skills-banner">{{ statusMessage }}</p>

    <input
      ref="userInput"
      type="file"
      class="skills-file-input"
      accept=".md,text/markdown,text/plain"
      @change="onUserFiles"
    />
    <input
      ref="projectInput"
      type="file"
      class="skills-file-input"
      accept=".md,text/markdown,text/plain"
      @change="onProjectFiles"
    />

    <div v-if="isLoading" class="settings-empty">Loading…</div>

    <template v-else>
      <section class="skills-group">
        <div class="skills-group-head">
          <h4 class="skills-group-title">User skills</h4>
          <ThemedButton
            variant="primary"
            size="sm"
            :disabled="isInstalling"
            @click="pickUser"
          >
            {{ isInstalling ? 'Installing…' : 'Add skill' }}
          </ThemedButton>
        </div>
        <p v-if="userRoot" class="skills-path" :title="userRoot">{{ userRoot }}</p>
        <ul v-if="userSkills.length > 0" class="skills-list">
          <li v-for="skill in userSkills" :key="`user-${skill.name}`" class="skills-item">
            <span class="skills-name">{{ skill.name }}</span>
            <span class="skills-desc">{{ skill.description }}</span>
          </li>
        </ul>
        <p v-else class="skills-empty-hint">
          No user skills yet. Click <strong>Add skill</strong> and choose a
          <code>SKILL.md</code> file.
        </p>
      </section>

      <section class="skills-group">
        <div class="skills-group-head">
          <h4 class="skills-group-title">
            Project skills
            <span v-if="activeProjectName" class="skills-project-label">· {{ activeProjectName }}</span>
          </h4>
          <ThemedButton
            variant="primary"
            size="sm"
            :disabled="!canInstallProject || isInstalling"
            :title="canInstallProject ? 'Install SKILL.md into this project' : 'Select a project first'"
            @click="pickProject"
          >
            {{ isInstalling ? 'Installing…' : 'Add skill' }}
          </ThemedButton>
        </div>
        <p v-if="projectRoot" class="skills-path" :title="projectRoot">{{ projectRoot }}</p>
        <p v-else-if="!canInstallProject" class="skills-empty-hint">
          Select a project to manage project-scoped skills
          (<code>.locagens/skills</code>).
        </p>
        <ul v-if="projectSkills.length > 0" class="skills-list">
          <li v-for="skill in projectSkills" :key="`project-${skill.name}`" class="skills-item">
            <span class="skills-name">{{ skill.name }}</span>
            <span class="skills-desc">{{ skill.description }}</span>
          </li>
        </ul>
        <p v-else-if="canInstallProject" class="skills-empty-hint">
          No project skills yet. Click <strong>Add skill</strong> and choose a
          <code>SKILL.md</code> file.
        </p>
      </section>

      <section class="skills-format">
        <h4 class="skills-group-title">SKILL.md format</h4>
        <pre class="skills-format-code">---
name: my-skill
description: "USE FOR: … DO NOT USE FOR: …"
---
# Instructions
…</pre>
      </section>
    </template>
  </div>
</template>

<style scoped>
.skills-file-input {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}

.skills-banner {
  margin: 0 0 14px;
  padding: 10px 12px;
  border-radius: 8px;
  font-size: 12px;
  line-height: 1.4;
  background: var(--surface);
  border: 1px solid var(--border);
  color: var(--text-secondary);
  word-break: break-all;
}

.skills-banner-error {
  border-color: color-mix(in srgb, var(--danger, #c44) 40%, var(--border));
  color: var(--danger, #c44);
}

.skills-group {
  margin-bottom: 22px;
}

.skills-group-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 6px;
}

.skills-group-title {
  margin: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
}

.skills-project-label {
  font-weight: 500;
  color: var(--text-secondary);
}

.skills-path {
  margin: 0 0 10px;
  font-size: 11px;
  line-height: 1.35;
  color: var(--text-tertiary, var(--text-secondary));
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  word-break: break-all;
}

.skills-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.skills-item {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 12px 14px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--surface);
}

.skills-name {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
}

.skills-desc {
  font-size: 12px;
  line-height: 1.45;
  color: var(--text-secondary);
}

.skills-empty-hint {
  margin: 0;
  font-size: 12px;
  line-height: 1.45;
  color: var(--text-secondary);
}

.skills-empty-hint code,
.skills-format-code {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
}

.skills-format {
  margin-top: 8px;
  padding-top: 16px;
  border-top: 1px solid var(--border);
}

.skills-format-code {
  margin: 10px 0 0;
  padding: 12px 14px;
  border-radius: 8px;
  background: var(--surface);
  border: 1px solid var(--border);
  font-size: 11px;
  line-height: 1.5;
  color: var(--text-secondary);
  overflow-x: auto;
  white-space: pre;
}
</style>
