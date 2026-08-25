<script setup lang="ts">
import { computed, ref } from 'vue';
import type { SkillSummary } from '@locagens/shared';
import ThemedButton from '../ui/ThemedButton.vue';
import ThemedSelect from '../ui/ThemedSelect.vue';

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
  (e: 'create-skill', payload: { target: 'user' | 'project'; name: string; description: string; body: string }): void;
  (e: 'delete-skill', payload: { target: 'user' | 'project'; name: string }): void;
  (e: 'open-folder', target: 'user' | 'project'): void;
}>();

const userSkills = computed(() => props.skills.filter(s => s.source === 'user'));
const projectSkills = computed(() => props.skills.filter(s => s.source === 'project'));
const canInstallProject = computed(() => !!props.activeProjectPath);

const userInput = ref<HTMLInputElement | null>(null);
const projectInput = ref<HTMLInputElement | null>(null);

// Creation Form state
const showCreateForm = ref(false);
const newTarget = ref<'user' | 'project'>('user');
const newName = ref('');
const newDescription = ref('');
const newBody = ref('');

const targetOptions = computed(() => [
  { value: 'user', label: 'User Skill (Global)' },
  {
    value: 'project',
    label: `Project Skill${canInstallProject.value ? ` (${props.activeProjectName || 'Active Project'})` : ' (Select project first)'}`,
    disabled: !canInstallProject.value
  }
]);

// Expanded view of skill instructions
const expandedSkills = ref<Set<string>>(new Set());

function toggleExpand(key: string) {
  if (expandedSkills.value.has(key)) {
    expandedSkills.value.delete(key);
  } else {
    expandedSkills.value.add(key);
  }
}

function isExpanded(key: string): boolean {
  return expandedSkills.value.has(key);
}

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

function startCreate(target?: 'user' | 'project') {
  if (target === 'project' && canInstallProject.value) {
    newTarget.value = 'project';
  } else {
    newTarget.value = 'user';
  }
  showCreateForm.value = true;
}

function cancelCreate() {
  showCreateForm.value = false;
  newName.value = '';
  newDescription.value = '';
  newBody.value = '';
}

function submitCreate() {
  const name = newName.value.trim().toLowerCase().replace(/[^a-z0-9_-]/g, '-');
  const description = newDescription.value.trim();
  const body = newBody.value.trim() || '# Instructions\n\nProvide clear instructions for the assistant.';

  if (!name || !description) return;

  emit('create-skill', {
    target: newTarget.value === 'project' && canInstallProject.value ? 'project' : 'user',
    name,
    description,
    body
  });

  cancelCreate();
}

function confirmDelete(target: 'user' | 'project', name: string) {
  emit('delete-skill', { target, name });
}
</script>

<template>
  <div class="settings-tab-panel">
    <header class="settings-section-head">
      <div>
        <h3 class="settings-section-title">Skills</h3>
        <p class="settings-section-desc">
          Specialized instruction packs the assistant loads on demand with
          <code>load_skill</code>. You can install existing <code>SKILL.md</code> files or create new custom skills.
        </p>
      </div>
      <div class="skills-head-actions">
        <ThemedButton
          :variant="showCreateForm ? 'secondary' : 'primary'"
          size="sm"
          :disabled="isInstalling"
          @click="showCreateForm ? cancelCreate() : startCreate()"
        >
          {{ showCreateForm ? 'Cancel' : '+ New skill' }}
        </ThemedButton>
        <ThemedButton
          variant="secondary"
          size="sm"
          :disabled="isLoading || isInstalling"
          @click="emit('refresh')"
        >
          Refresh
        </ThemedButton>
      </div>
    </header>

    <p v-if="error" class="skills-banner skills-banner-error">{{ error }}</p>
    <p v-else-if="statusMessage" class="skills-banner">{{ statusMessage }}</p>

    <!-- Hidden file pickers for user and project skills -->
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

    <!-- New Skill Inline Form -->
    <div v-if="showCreateForm" class="skills-form-wrap">
      <h4 class="skills-form-title">Create New Skill</h4>
      <div class="skills-form-row">
        <label class="skills-field">
          <span class="skills-field-label">Target Scope</span>
          <ThemedSelect v-model="newTarget" :options="targetOptions" />
        </label>
        <label class="skills-field flex-2">
          <span class="skills-field-label">Skill Name (slug)</span>
          <input
            v-model="newName"
            type="text"
            class="text-input skills-input"
            placeholder="e.g. deploy-azure, prisma-migrate"
            maxlength="64"
          />
        </label>
      </div>
      <label class="skills-field">
        <span class="skills-field-label">Description (for model discovery)</span>
        <input
          v-model="newDescription"
          type="text"
          class="text-input skills-input"
          placeholder='e.g. "USE FOR: deploying Azure OpenAI models. DO NOT USE FOR: local deployments."'
        />
      </label>
      <label class="skills-field">
        <span class="skills-field-label">Skill Instructions (Markdown Body)</span>
        <textarea
          v-model="newBody"
          class="text-input skills-textarea"
          rows="6"
          placeholder="# Steps&#10;1. Check existing configuration&#10;2. Run the deployment script"
        ></textarea>
      </label>
      <div class="skills-form-actions">
        <ThemedButton
          variant="primary"
          size="sm"
          :disabled="!newName.trim() || !newDescription.trim() || isInstalling"
          @click="submitCreate"
        >
          {{ isInstalling ? 'Saving…' : 'Save Skill' }}
        </ThemedButton>
        <ThemedButton variant="secondary" size="sm" @click="cancelCreate">Cancel</ThemedButton>
      </div>
    </div>

    <div v-if="isLoading" class="settings-empty">Loading…</div>

    <template v-else>
      <!-- User Skills Section -->
      <section class="skills-group">
        <div class="skills-group-head">
          <div>
            <h4 class="skills-group-title">User skills (Global)</h4>
          </div>
          <div class="skills-group-actions">
            <ThemedButton
              variant="secondary"
              size="sm"
              :disabled="isInstalling"
              title="Import a SKILL.md file from disk"
              @click="pickUser"
            >
              Import .md
            </ThemedButton>
            <ThemedButton
              variant="secondary"
              size="sm"
              title="Open the skills folder in your file manager"
              @click="emit('open-folder', 'user')"
            >
              Open folder
            </ThemedButton>
          </div>
        </div>
        <p v-if="userRoot" class="skills-path" :title="userRoot">{{ userRoot }}</p>
        <ul v-if="userSkills.length > 0" class="skills-list">
          <li
            v-for="skill in userSkills"
            :key="`user-${skill.name}`"
            class="skills-item"
          >
            <div class="skills-item-head">
              <div class="skills-item-title-wrap">
                <span class="skills-name">{{ skill.name }}</span>
                <span class="skills-badge user">user</span>
              </div>
              <div class="skills-item-actions">
                <button
                  v-if="skill.body"
                  class="skills-btn-link"
                  @click="toggleExpand(`user-${skill.name}`)"
                >
                  {{ isExpanded(`user-${skill.name}`) ? 'Hide details' : 'View instructions' }}
                </button>
                <button
                  class="skills-btn-link delete"
                  title="Delete this skill"
                  @click="confirmDelete('user', skill.name)"
                >
                  Delete
                </button>
              </div>
            </div>
            <p class="skills-desc">{{ skill.description }}</p>
            <div v-if="isExpanded(`user-${skill.name}`) && skill.body" class="skills-body-preview">
              <pre>{{ skill.body }}</pre>
            </div>
          </li>
        </ul>
        <p v-else class="skills-empty-hint">
          No user skills yet. Click <strong>+ New skill</strong> or <strong>Import .md</strong> to add one.
        </p>
      </section>

      <!-- Project Skills Section -->
      <section class="skills-group">
        <div class="skills-group-head">
          <div>
            <h4 class="skills-group-title">
              Project skills
              <span v-if="activeProjectName" class="skills-project-label">· {{ activeProjectName }}</span>
            </h4>
          </div>
          <div class="skills-group-actions">
            <ThemedButton
              variant="secondary"
              size="sm"
              :disabled="!canInstallProject || isInstalling"
              :title="canInstallProject ? 'Import a SKILL.md into this project' : 'Select a project first'"
              @click="pickProject"
            >
              Import .md
            </ThemedButton>
            <ThemedButton
              variant="secondary"
              size="sm"
              :disabled="!canInstallProject"
              :title="canInstallProject ? 'Open project skills folder' : 'Select a project first'"
              @click="emit('open-folder', 'project')"
            >
              Open folder
            </ThemedButton>
          </div>
        </div>
        <p v-if="projectRoot" class="skills-path" :title="projectRoot">{{ projectRoot }}</p>
        <p v-else-if="!canInstallProject" class="skills-empty-hint">
          Select a project to manage project-scoped skills (<code>.locagens/skills</code>).
        </p>
        <ul v-if="projectSkills.length > 0" class="skills-list">
          <li
            v-for="skill in projectSkills"
            :key="`project-${skill.name}`"
            class="skills-item"
          >
            <div class="skills-item-head">
              <div class="skills-item-title-wrap">
                <span class="skills-name">{{ skill.name }}</span>
                <span class="skills-badge project">project</span>
              </div>
              <div class="skills-item-actions">
                <button
                  v-if="skill.body"
                  class="skills-btn-link"
                  @click="toggleExpand(`project-${skill.name}`)"
                >
                  {{ isExpanded(`project-${skill.name}`) ? 'Hide details' : 'View instructions' }}
                </button>
                <button
                  class="skills-btn-link delete"
                  title="Delete this skill"
                  @click="confirmDelete('project', skill.name)"
                >
                  Delete
                </button>
              </div>
            </div>
            <p class="skills-desc">{{ skill.description }}</p>
            <div v-if="isExpanded(`project-${skill.name}`) && skill.body" class="skills-body-preview">
              <pre>{{ skill.body }}</pre>
            </div>
          </li>
        </ul>
        <p v-else-if="canInstallProject" class="skills-empty-hint">
          No project skills yet. Click <strong>+ New skill</strong> or <strong>Import .md</strong> to add one for this repository.
        </p>
      </section>

      <!-- SKILL.md Format Guide -->
      <section class="skills-format">
        <h4 class="skills-group-title">SKILL.md format specification</h4>
        <pre class="skills-format-code">---
name: my-skill
description: "USE FOR: &lt;tasks&gt;. DO NOT USE FOR: &lt;non-tasks&gt;."
---
# Instructions
Step-by-step guidance the model receives when executing load_skill.</pre>
      </section>
    </template>
  </div>
</template>

<style scoped>
.skills-head-actions,
.skills-group-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

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

.skills-form-wrap {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px;
  margin-bottom: 20px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 10px;
}

.skills-form-title {
  margin: 0 0 4px;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
}

.skills-form-row {
  display: flex;
  gap: 12px;
}

.skills-field {
  display: flex;
  flex-direction: column;
  gap: 6px;
  flex: 1;
}

.skills-field.flex-2 {
  flex: 2;
}

.skills-field-label {
  font-size: 11px;
  font-weight: 500;
  color: var(--text-secondary);
}

.skills-input,
.skills-textarea {
  width: 100%;
  box-sizing: border-box;
  padding: 8px 10px;
  font-size: 12px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--surface-input, var(--surface));
  color: var(--text-primary);
  outline: none;
}

.skills-textarea {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  resize: vertical;
}

.skills-form-actions {
  display: flex;
  gap: 8px;
  margin-top: 4px;
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
  gap: 6px;
  padding: 12px 14px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--surface);
  transition: border-color 0.15s ease;
}

.skills-item:hover {
  border-color: var(--border-hover, var(--border));
}

.skills-item-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.skills-item-title-wrap {
  display: flex;
  align-items: center;
  gap: 8px;
}

.skills-name {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
}

.skills-badge {
  font-size: 10px;
  font-weight: 600;
  padding: 2px 6px;
  border-radius: 4px;
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.skills-badge.user {
  background: rgba(100, 140, 255, 0.15);
  color: #648cff;
}

.skills-badge.project {
  background: rgba(80, 200, 120, 0.15);
  color: #50c878;
}

.skills-item-actions {
  display: flex;
  align-items: center;
  gap: 10px;
}

.skills-btn-link {
  background: none;
  border: none;
  padding: 0;
  font-size: 11px;
  color: var(--text-secondary);
  cursor: pointer;
  text-decoration: underline;
  transition: color 0.15s ease;
}

.skills-btn-link:hover {
  color: var(--text-primary);
}

.skills-btn-link.delete {
  color: var(--danger, #c44);
}

.skills-btn-link.delete:hover {
  color: color-mix(in srgb, var(--danger, #c44) 80%, black);
}

.skills-desc {
  margin: 0;
  font-size: 12px;
  line-height: 1.45;
  color: var(--text-secondary);
}

.skills-body-preview {
  margin-top: 6px;
  padding: 10px 12px;
  background: var(--surface-strong, rgba(0, 0, 0, 0.04));
  border: 1px solid var(--border);
  border-radius: 6px;
  overflow-x: auto;
}

.skills-body-preview pre {
  margin: 0;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 11px;
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
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
  margin-top: 10px;
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
