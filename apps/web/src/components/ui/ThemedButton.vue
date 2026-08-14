<script setup lang="ts">
import { computed } from 'vue';

type ButtonVariant = 'primary' | 'secondary' | 'danger' | 'chip';
type ButtonSize = 'sm' | 'md' | 'lg';

const props = withDefaults(
  defineProps<{
    variant?: ButtonVariant;
    size?: ButtonSize;
    disabled?: boolean;
    type?: 'button' | 'submit' | 'reset';
    active?: boolean;
  }>(),
  {
    variant: 'secondary',
    size: 'md',
    disabled: false,
    type: 'button',
    active: false,
  }
);

const emit = defineEmits<{
  (e: 'click', event: MouseEvent): void;
}>();

const buttonClasses = computed(() => {
  return [
    'themed-btn',
    `btn-${props.variant}`,
    `btn-${props.size}`,
    { 'btn-active': props.active, 'btn-disabled': props.disabled }
  ];
});

function handleClick(event: MouseEvent) {
  if (props.disabled) {
    event.preventDefault();
    event.stopPropagation();
    return;
  }
  emit('click', event);
}
</script>

<template>
  <button
    :type="type"
    :class="buttonClasses"
    :disabled="disabled"
    @click="handleClick"
  >
    <span class="btn-content">
      <slot></slot>
    </span>
  </button>
</template>

<style scoped>
.themed-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-family: inherit;
  font-weight: 550;
  border-radius: 8px !important;
  border: none;
  cursor: pointer;
  outline: none;
  white-space: nowrap;
  user-select: none;
  transition: all 0.2s cubic-bezier(0.4, 0, 0.2, 1);
  position: relative;
  overflow: hidden;
}

/* SIZES */
.btn-sm {
  min-height: 28px;
  padding: 4px 10px;
  font-size: 0.76rem;
  border-radius: 6px !important;
}

.btn-md {
  min-height: 36px;
  padding: 6px 14px;
  font-size: 0.86rem;
}

.btn-lg {
  min-height: 44px;
  padding: 10px 20px;
  font-size: 0.94rem;
}

/* VARIANTS - All filled, no transparent/outlined buttons */

/* Primary: Solid White High Contrast */
.btn-primary {
  background: var(--btn-primary-bg);
  color: var(--btn-primary-color);
  box-shadow: 0 4px 12px var(--btn-primary-shadow);
}

.btn-primary:hover:not(:disabled) {
  background: var(--btn-primary-hover-bg);
  transform: translateY(-1px);
  box-shadow: 0 6px 16px var(--btn-primary-hover-shadow);
}

.btn-primary:active:not(:disabled) {
  background: var(--btn-primary-active-bg);
  transform: translateY(0);
}

/* Secondary: Premium Sleek Dark Gray */
.btn-secondary {
  background: var(--btn-secondary-bg);
  color: var(--btn-secondary-color);
  border: 1px solid var(--btn-secondary-border) !important;
}

.btn-secondary:hover:not(:disabled) {
  background: var(--btn-secondary-hover-bg);
  color: var(--btn-secondary-hover-color);
  transform: translateY(-1px);
  box-shadow: 0 4px 12px var(--btn-secondary-shadow);
  border-color: var(--btn-secondary-hover-border) !important;
}

.btn-secondary:active:not(:disabled) {
  background: var(--btn-secondary-active-bg);
  transform: translateY(0);
}

/* Danger: Soft Red Glow */
.btn-danger {
  background: var(--btn-danger-bg);
  color: var(--btn-danger-color);
  border: 1px solid var(--btn-danger-border) !important;
}

.btn-danger:hover:not(:disabled) {
  background: var(--btn-danger-hover-bg);
  color: var(--btn-danger-hover-color);
  transform: translateY(-1px);
  box-shadow: 0 4px 12px var(--btn-danger-shadow);
  border-color: var(--btn-danger-hover-border) !important;
}

.btn-danger:active:not(:disabled) {
  background: var(--btn-danger-active-bg);
  transform: translateY(0);
}

/* Chip: Small Rounded Option Pill */
.btn-chip {
  background: var(--btn-chip-bg);
  color: var(--btn-chip-color);
  border-radius: 999px !important;
  border: 1px solid var(--btn-chip-border) !important;
}

.btn-chip:hover:not(:disabled) {
  background: var(--btn-chip-hover-bg);
  color: var(--btn-chip-hover-color);
  border-color: var(--btn-chip-hover-border) !important;
}

.btn-chip.btn-active {
  background: var(--btn-primary-bg);
  color: var(--btn-primary-color);
  font-weight: 600;
  border-color: var(--btn-primary-bg) !important;
  box-shadow: 0 4px 12px var(--btn-primary-shadow);
}

.btn-chip.btn-active:hover:not(:disabled) {
  background: var(--btn-primary-hover-bg);
  color: var(--btn-primary-color);
  border-color: var(--btn-primary-hover-bg) !important;
}

/* DISABLED STATE */
.themed-btn:disabled,
.themed-btn.btn-disabled {
  opacity: 0.45;
  cursor: not-allowed;
  transform: none !important;
  box-shadow: none !important;
}

.btn-content {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  width: 100%;
}
</style>
