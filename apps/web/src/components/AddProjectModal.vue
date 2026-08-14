<script setup lang="ts">
import BaseModal from './ui/BaseModal.vue';
import ThemedButton from './ui/ThemedButton.vue';

defineProps<{
  show: boolean;
  isMac: boolean;
  isSubmitting: boolean;
  name: string;
  path: string;
}>();

const emit = defineEmits<{
  (e: 'close'): void;
  (e: 'browse'): void;
  (e: 'submit'): void;
  (e: 'update:name', value: string): void;
  (e: 'update:path', value: string): void;
}>();
</script>

<template>
  <BaseModal v-if="show" title="Add Project Folder" @close="emit('close')">
    <div v-if="isMac" class="form-group">
      <ThemedButton variant="primary" class="browse-btn" @click="emit('browse')">
        Select Folder (macOS Finder)
      </ThemedButton>
      <div class="separator-text">or enter path manually</div>
    </div>

    <div class="form-group">
      <label for="project-path">Absolute Folder Path</label>
      <input
        id="project-path"
        type="text"
        :value="path"
        placeholder="/Users/username/Projects/my-app"
        @input="emit('update:path', ($event.target as HTMLInputElement).value)"
      />
    </div>

    <div class="form-group">
      <label for="project-name">Display Name (Optional)</label>
      <input
        id="project-name"
        type="text"
        :value="name"
        placeholder="my-app"
        @input="emit('update:name', ($event.target as HTMLInputElement).value)"
      />
    </div>

    <template #footer>
      <ThemedButton variant="secondary" @click="emit('close')">Cancel</ThemedButton>
      <ThemedButton variant="primary" :disabled="!path.trim() || isSubmitting" @click="emit('submit')">
        {{ isSubmitting ? 'Adding...' : 'Add Project' }}
      </ThemedButton>
    </template>
  </BaseModal>
</template>
