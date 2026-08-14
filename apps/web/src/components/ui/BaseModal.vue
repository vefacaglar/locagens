<script setup lang="ts">
import ThemedButton from './ThemedButton.vue';

defineProps<{
  title?: string;
  /** CSS width override for the card, e.g. 'min(580px, 95%)'. */
  width?: string;
}>();

const emit = defineEmits<{
  (e: 'close'): void;
}>();
</script>

<template>
  <div class="modal-overlay" @click.self="emit('close')">
    <div class="modal-card" :style="width ? { width } : undefined">
      <header class="modal-header">
        <slot name="header">
          <h3>{{ title }}</h3>
          <ThemedButton size="sm" @click="emit('close')">Close</ThemedButton>
        </slot>
      </header>

      <main class="modal-body">
        <slot />
      </main>

      <footer v-if="$slots.footer" class="modal-footer">
        <slot name="footer" />
      </footer>
    </div>
  </div>
</template>
