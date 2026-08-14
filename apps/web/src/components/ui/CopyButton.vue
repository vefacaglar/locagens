<script setup lang="ts">
import { ref } from 'vue';

const props = withDefaults(
  defineProps<{
    /** Text to copy, or a getter evaluated at click time. */
    text: string | (() => string);
    label?: string;
  }>(),
  { label: 'Copy' }
);

const copied = ref(false);
let resetTimer: ReturnType<typeof setTimeout> | null = null;

function onClick() {
  const value = typeof props.text === 'function' ? props.text() : props.text;
  navigator.clipboard.writeText(value);
  copied.value = true;
  if (resetTimer) clearTimeout(resetTimer);
  resetTimer = setTimeout(() => {
    copied.value = false;
    resetTimer = null;
  }, 2000);
}
</script>

<template>
  <button :title="label" :class="{ copied }" @click.stop="onClick">
    <svg v-if="copied" class="check-icon" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
      <path d="M20 6 9 17l-5-5"/>
    </svg>
    <svg v-else class="copy-icon" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <rect width="14" height="14" x="8" y="8" rx="2" ry="2"/>
      <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>
    </svg>
  </button>
</template>
