<script setup lang="ts">
import { computed, ref } from 'vue';
import { onClickOutside } from '@vueuse/core';

type OptionValue = string | number;

const props = defineProps<{
  modelValue: OptionValue;
  options: { value: OptionValue; label: string; disabled?: boolean }[];
}>();

const emit = defineEmits<{
  (e: 'update:modelValue', value: OptionValue): void;
}>();

const root = ref<HTMLElement | null>(null);
const isOpen = ref(false);

const selectedLabel = computed(() => (
  props.options.find(option => option.value === props.modelValue)?.label ?? 'Select'
));

function selectOption(value: OptionValue, disabled?: boolean) {
  if (disabled) return;
  emit('update:modelValue', value);
  isOpen.value = false;
}

onClickOutside(root, () => {
  isOpen.value = false;
});
</script>

<template>
  <div ref="root" class="themed-select">
    <button
      type="button"
      class="themed-select-button"
      :class="{ open: isOpen }"
      @click.stop="isOpen = !isOpen"
    >
      <span class="themed-select-label truncate">{{ selectedLabel }}</span>
      <span class="themed-select-caret">⌄</span>
    </button>
    <div v-if="isOpen" class="themed-select-menu">
      <button
        v-for="option in options"
        :key="String(option.value)"
        type="button"
        class="themed-select-option"
        :class="{ active: option.value === modelValue, disabled: option.disabled }"
        :disabled="option.disabled"
        @click.stop="selectOption(option.value, option.disabled)"
      >
        {{ option.label }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.themed-select {
  position: relative;
  min-width: 0;
}

.themed-select-button {
  width: 100%;
  height: 36px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  background: var(--control-bg);
  border: 1px solid var(--control-border);
  border-radius: 7px;
  color: var(--text);
  padding: 0 10px 0 11px;
  font: inherit;
  font-size: 0.86rem;
  cursor: pointer;
  transition: border-color 0.15s ease, background 0.15s ease;
}

.themed-select-button:hover,
.themed-select-button.open {
  background: var(--control-bg-hover);
  border-color: var(--control-border-focus);
}

.themed-select-label {
  min-width: 0;
}

.themed-select-caret {
  flex: 0 0 auto;
  color: var(--faint);
  font-size: 0.9rem;
  line-height: 1;
}

.themed-select-menu {
  position: absolute;
  top: calc(100% + 6px);
  left: 0;
  width: 100%;
  min-width: 220px;
  max-height: 260px;
  overflow-y: auto;
  z-index: 1200;
  display: flex;
  flex-direction: column;
  gap: 3px;
  padding: 6px;
  background: var(--control-bg);
  border: 1px solid var(--control-border);
  border-radius: 10px;
  box-shadow: 0 12px 30px var(--composer-dropdown-shadow);
}

.themed-select-option {
  min-height: 36px;
  display: flex;
  align-items: center;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--muted);
  padding: 0 10px;
  font: inherit;
  font-size: 0.84rem;
  text-align: left;
  cursor: pointer;
}

.themed-select-option:hover {
  background: var(--control-bg-hover);
  color: var(--text);
}

.themed-select-option.active {
  color: var(--text);
  background: var(--control-bg-active);
  font-weight: 600;
}

.themed-select-option.disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
</style>
