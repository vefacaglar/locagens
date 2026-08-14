<script setup lang="ts">
import { computed, ref } from 'vue';
import type { Memory, MemoryCategory, MemoryScope } from '@locagens/shared';
import { basename } from '../../lib/format';
import ThemedSelect from '../ui/ThemedSelect.vue';
import ThemedButton from '../ui/ThemedButton.vue';

const props = defineProps<{
  memories: Memory[];
  isLoading: boolean;
  activeProjectPath: string;
  activeProjectName: string;
}>();

const emit = defineEmits<{
  (e: 'add', payload: { scope: MemoryScope; category: MemoryCategory; content: string; projectPath?: string }): void;
  (e: 'update', payload: { id: number; content: string }): void;
  (e: 'delete', id: number): void;
  (e: 'clear-all'): void;
}>();

const CATEGORIES: MemoryCategory[] = ['user', 'feedback', 'project', 'reference'];

const globalMemories = computed(() => props.memories.filter(m => m.scope === 'global'));

// Project memories grouped by their project path, so multiple projects stay
// visually separated. Each group carries a readable label (the folder name).
const projectGroups = computed(() => {
  const byPath = new Map<string, Memory[]>();
  for (const m of props.memories) {
    if (m.scope !== 'project') continue;
    const list = byPath.get(m.projectPath) ?? [];
    list.push(m);
    byPath.set(m.projectPath, list);
  }
  return [...byPath.entries()].map(([path, items]) => ({ path, label: basename(path) || path, items }));
});

// --- Inline edit ---
const editingId = ref<number | null>(null);
const editDraft = ref('');

function startEdit(memory: Memory) {
  editingId.value = memory.id;
  editDraft.value = memory.content;
}
function cancelEdit() {
  editingId.value = null;
  editDraft.value = '';
}
function saveEdit(id: number) {
  const content = editDraft.value.trim();
  if (content) emit('update', { id, content });
  cancelEdit();
}

// --- Add form ---
const showAdd = ref(false);
const newScope = ref<MemoryScope>('global');
const newCategory = ref<MemoryCategory>('user');
const newContent = ref('');

const canAddProject = computed(() => !!props.activeProjectPath);
const scopeOptions = computed(() => [
  { value: 'global', label: 'Global (all projects)' },
  {
    value: 'project',
    label: `Project${canAddProject.value ? ` - ${props.activeProjectName}` : ' (select a project first)'}`,
    disabled: !canAddProject.value
  }
]);
const categoryOptions = computed(() => CATEGORIES.map(category => ({ value: category, label: category })));

function submitAdd() {
  const content = newContent.value.trim();
  if (!content) return;
  const scope = newScope.value === 'project' && canAddProject.value ? 'project' : 'global';
  emit('add', {
    scope,
    category: newCategory.value,
    content,
    projectPath: scope === 'project' ? props.activeProjectPath : undefined
  });
  newContent.value = '';
  showAdd.value = false;
}
</script>

<template>
  <div class="settings-tab-panel">
    <header class="settings-section-head">
      <div>
        <h3 class="settings-section-title">Memory</h3>
        <p class="settings-section-desc">
          Durable facts the assistant remembers across sessions. It saves these
          itself as it learns your preferences; global memories apply everywhere,
          project memories only to that project.
        </p>
      </div>
      <div class="mem-head-actions">
        <ThemedButton :variant="showAdd ? 'secondary' : 'primary'" size="sm" @click="showAdd = !showAdd">{{ showAdd ? 'Cancel' : 'Add memory' }}</ThemedButton>
        <ThemedButton v-if="memories.length > 0" variant="danger" size="sm" @click="emit('clear-all')">Clear all</ThemedButton>
      </div>
    </header>

    <div v-if="showAdd" class="mem-add-form">
      <div class="mem-add-row">
        <label class="mem-field">
          <span class="mem-field-label">Scope</span>
          <ThemedSelect v-model="newScope" :options="scopeOptions" />
        </label>
        <label class="mem-field">
          <span class="mem-field-label">Category</span>
          <ThemedSelect v-model="newCategory" :options="categoryOptions" />
        </label>
      </div>
      <input
        v-model="newContent"
        type="text"
        class="mem-input text-input"
        placeholder="A durable fact to remember (e.g. 'Always use pnpm build')"
        @keyup.enter="submitAdd"
      />
      <div class="mem-add-actions">
        <ThemedButton variant="primary" :disabled="!newContent.trim()" @click="submitAdd">Save memory</ThemedButton>
      </div>
    </div>

    <div v-if="isLoading" class="settings-empty">Loading…</div>

    <div v-else-if="memories.length === 0" class="settings-empty">
      No saved memories yet. The assistant will add them as it learns what you prefer.
    </div>

    <template v-else>
      <section v-if="globalMemories.length > 0" class="mem-group">
        <h4 class="mem-group-title">Global</h4>
        <ul class="mem-list">
          <li v-for="m in globalMemories" :key="m.id" class="mem-item" :class="{ editing: editingId === m.id }">
            <span class="mem-category">{{ m.category }}</span>
            <div class="mem-body">
              <template v-if="editingId === m.id">
                <div class="mem-edit-row">
                  <input
                    v-model="editDraft"
                    type="text"
                    class="mem-edit-input text-input"
                    @keyup.enter="saveEdit(m.id)"
                    @keyup.esc="cancelEdit"
                  />
                  <div class="mem-edit-actions">
                    <button class="mem-action-text-btn save" :disabled="!editDraft.trim()" @click="saveEdit(m.id)">Save</button>
                    <button class="mem-action-text-btn" @click="cancelEdit">Cancel</button>
                  </div>
                </div>
              </template>
              <span v-else class="mem-content">{{ m.content }}</span>
            </div>
            <div v-if="editingId !== m.id" class="mem-actions">
              <button class="mem-action-text-btn" title="Edit" @click="startEdit(m)">Edit</button>
              <button class="mem-action-text-btn delete" title="Delete" @click="emit('delete', m.id)">Delete</button>
            </div>
          </li>
        </ul>
      </section>

      <section v-for="group in projectGroups" :key="group.path" class="mem-group">
        <h4 class="mem-group-title">
          Project · <span class="mem-group-path">{{ group.label }}</span>
        </h4>
        <ul class="mem-list">
          <li v-for="m in group.items" :key="m.id" class="mem-item" :class="{ editing: editingId === m.id }">
            <span class="mem-category">{{ m.category }}</span>
            <div class="mem-body">
              <template v-if="editingId === m.id">
                <div class="mem-edit-row">
                  <input
                    v-model="editDraft"
                    type="text"
                    class="mem-edit-input text-input"
                    @keyup.enter="saveEdit(m.id)"
                    @keyup.esc="cancelEdit"
                  />
                  <div class="mem-edit-actions">
                    <button class="mem-action-text-btn save" :disabled="!editDraft.trim()" @click="saveEdit(m.id)">Save</button>
                    <button class="mem-action-text-btn" @click="cancelEdit">Cancel</button>
                  </div>
                </div>
              </template>
              <span v-else class="mem-content">{{ m.content }}</span>
            </div>
            <div v-if="editingId !== m.id" class="mem-actions">
              <button class="mem-action-text-btn" title="Edit" @click="startEdit(m)">Edit</button>
              <button class="mem-action-text-btn delete" title="Delete" @click="emit('delete', m.id)">Delete</button>
            </div>
          </li>
        </ul>
      </section>
    </template>
  </div>
</template>

<style scoped>
.mem-head-actions {
  display: flex;
  gap: 8px;
  flex: 0 0 auto;
}

.mem-add-form {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 14px;
  margin-bottom: 18px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 8px;
}

.mem-add-row {
  display: flex;
  gap: 12px;
}

.mem-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  flex: 1 1 0;
}

.mem-field-label {
  font-size: 0.7rem;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--muted);
}

.mem-input {
  width: 100%;
  height: 36px;
  box-sizing: border-box;
  border-radius: 6px;
  padding: 0 12px;
}

.mem-add-actions {
  display: flex;
  gap: 8px;
  margin-top: 6px;
}

.mem-edit-row {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
}

.mem-edit-input {
  flex: 1;
  min-width: 0;
  border-radius: 6px;
  height: 32px;
  box-sizing: border-box;
  padding: 0 10px;
}

.mem-edit-actions {
  display: flex;
  gap: 8px;
  flex-shrink: 0;
}

.mem-group {
  margin-bottom: 20px;
}

.mem-group-title {
  margin: 0 0 8px;
  font-size: 0.74rem;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: var(--muted);
  font-weight: 400;
}

.mem-group-path {
  font-family: monospace;
  text-transform: none;
  letter-spacing: 0;
  color: var(--faint);
}

.mem-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 0;
}

.mem-item {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  padding: 10px 0;
  background: transparent;
  border-bottom: 1px solid var(--memory-item-border);
}

.mem-item.editing {
  align-items: center;
}

.mem-item.editing .mem-category {
  height: 28px;
  box-sizing: border-box;
  display: inline-flex;
  align-items: center;
  padding: 0 8px;
  transform: none;
}

.mem-item:last-child {
  border-bottom: none;
}

.mem-category {
  flex: 0 0 auto;
  font-size: 0.68rem;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--muted);
  background: var(--surface-strong);
  border: 1px solid var(--border);
  padding: 5px 8px;
  border-radius: 5px;
  transform: translateY(-2px);
}

.mem-body {
  flex: 1 1 auto;
  min-width: 0;
}

.mem-content {
  font-size: 0.9rem;
  color: var(--text);
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
}

.mem-actions {
  flex: 0 0 auto;
  display: flex;
  gap: 12px;
  align-items: center;
  opacity: 0;
  transition: opacity 0.2s ease;
}

.mem-item:hover .mem-actions {
  opacity: 1;
}

@media (max-width: 768px) {
  .mem-actions {
    opacity: 1;
  }
}

.mem-action-text-btn {
  background: transparent;
  border: none;
  padding: 0;
  font-size: 0.8rem;
  font-weight: 500;
  color: var(--muted);
  cursor: pointer;
  transition: color 0.2s ease;
}

.mem-action-text-btn:hover {
  color: var(--text);
  text-decoration: underline;
}

.mem-action-text-btn.save {
  color: var(--text);
  font-weight: 600;
}

.mem-action-text-btn.save:disabled {
  opacity: 0.4;
  cursor: not-allowed;
  text-decoration: none;
}

.mem-action-text-btn.delete {
  color: var(--memory-delete-hover);
}

.mem-action-text-btn.delete:hover {
  color: var(--memory-delete-hover-solid);
}
</style>
