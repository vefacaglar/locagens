<script setup lang="ts">
import ThemedButton from './ThemedButton.vue';

defineProps<{
  isFirst: boolean;
  isLast: boolean;
  /** Enables the Next button. */
  canAdvance: boolean;
  /** Enables the Submit button (shown on the last step). */
  canSubmit: boolean;
}>();

const emit = defineEmits<{
  (e: 'prev'): void;
  (e: 'next'): void;
  (e: 'submit'): void;
}>();
</script>

<template>
  <div class="stepper-footer">
    <ThemedButton v-if="!isFirst" variant="secondary" @click="emit('prev')">Previous</ThemedButton>
    <ThemedButton v-if="!isLast" variant="primary" :disabled="!canAdvance" @click="emit('next')">Next</ThemedButton>
    <ThemedButton v-else variant="primary" :disabled="!canSubmit" @click="emit('submit')">Submit</ThemedButton>
  </div>
</template>

<style scoped>
.stepper-footer {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
}
</style>
