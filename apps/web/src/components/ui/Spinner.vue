<script setup lang="ts">
withDefaults(
  defineProps<{
    /** Diameter in px. */
    size?: number;
    /** Border thickness in px. */
    thickness?: number;
    /** Track (ring) color; the moving segment uses currentColor. */
    track?: string;
  }>(),
  { size: 14, thickness: 2, track: 'var(--border-soft)' }
);
</script>

<template>
  <span
    class="ui-spinner"
    :style="{
      width: `${size}px`,
      height: `${size}px`,
      borderWidth: `${thickness}px`,
      borderColor: track,
      borderTopColor: 'currentColor'
    }"
    aria-hidden="true"
  ></span>
</template>

<style scoped>
.ui-spinner {
  display: inline-block;
  box-sizing: border-box;
  border-style: solid;
  border-radius: 50%;
  color: var(--text);
  animation: ui-spin 0.8s linear infinite;
}

@keyframes ui-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
