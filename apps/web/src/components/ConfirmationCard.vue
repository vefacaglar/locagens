<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import type { MessageGroup } from '../lib/messageGroups';
import { getConfirmations } from '../lib/confirmation';
import ThemedButton from './ui/ThemedButton.vue';
import StepperFooter from './ui/StepperFooter.vue';

const props = defineProps<{
  group: MessageGroup | null;
}>();

const emit = defineEmits<{
  (e: 'reply', option: string): void;
}>();

const confirmations = computed(() => getConfirmations(props.group?.message?.content || ''));
const currentQuestion = computed(() => confirmations.value[currentIndex.value] || null);

const currentIndex = ref(0);
const selections = ref<string[]>([]);
const notes = ref<string[]>([]);

const isFirst = computed(() => currentIndex.value === 0);
const isLast = computed(() => currentIndex.value === confirmations.value.length - 1);

watch(
  () => props.group,
  () => {
    currentIndex.value = 0;
    selections.value = [];
    notes.value = [];
  },
  { immediate: true }
);

const commentPlaceholder = computed(() => 'Your own comment (optional)...');


function handleOptionSelect(opt: string) {
  selections.value[currentIndex.value] = opt;
  if (confirmations.value.length === 1) {
    submit();
  }
}

function goPrev() {
  if (!isFirst.value) currentIndex.value--;
}

function goNext() {
  if (!isLast.value && selections.value[currentIndex.value]) currentIndex.value++;
}

function submit() {
  const parts: string[] = [];
  confirmations.value.forEach((q, idx) => {
    const sel = selections.value[idx];
    const note = notes.value[idx]?.trim();
    if (!sel) {
      // Free-form answer typed in the note field (no Yes/No picked).
      if (note) parts.push(note);
      return;
    }
    let line = `${q.question ? q.question + ': ' : ''}${sel}`;
    if (note) {
      line += ` (Note: ${note})`;
    }
    parts.push(line);
  });
  
  // Nothing selected and nothing typed — don't send an empty reply.
  if (parts.length === 0) return;

  emit('reply', parts.join('\n\n'));
  currentIndex.value = 0;
  selections.value = [];
  notes.value = [];
}
</script>

<template>
  <transition name="card-slide-up">
    <div v-if="group && currentQuestion" class="prompt-card prompt-card--inline composer-confirmation-card">
      <div class="confirm-card-header">
        <strong>{{ currentQuestion.question || 'Confirm prompt action?' }}</strong>
        <span v-if="confirmations.length > 1" class="confirm-step">{{ currentIndex + 1 }} / {{ confirmations.length }}</span>
      </div>
      <div class="confirm-options">
        <ThemedButton
          v-for="opt in currentQuestion.options"
          :key="opt"
          :variant="selections[currentIndex] === opt ? 'primary' : 'secondary'"
          class="confirm-option-btn"
          @click="handleOptionSelect(opt)"
        >
          {{ opt }}
        </ThemedButton>
      </div>
      <input
        v-model="notes[currentIndex]"
        type="text"
        class="confirm-note prompt-note"
        :placeholder="commentPlaceholder"
        @keydown.enter.prevent="submit"
      />

      <StepperFooter
        v-if="confirmations.length > 1"
        class="confirm-card-footer"
        :is-first="isFirst"
        :is-last="isLast"
        :can-advance="!!selections[currentIndex]"
        :can-submit="!!selections[currentIndex]"
        @prev="goPrev"
        @next="goNext"
        @submit="submit"
      />
    </div>
  </transition>
</template>

<style scoped>
/* Shell + positioning come from the global .prompt-card / .prompt-card--inline. */
.composer-confirmation-card {
  padding: 14px 16px;
  gap: 12px;
}

.confirm-card-header {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 0.9rem;
  color: var(--text);
}

.confirm-options {
  display: flex;
  flex-direction: column;
  gap: 8px;
  width: 100%;
}

/* Left-align the Yes/No labels. The centering lives on ThemedButton's inner
   .btn-content span, so we must pierce the child scope with :deep(). */
.confirm-options .confirm-option-btn :deep(.btn-content) {
  justify-content: flex-start;
  text-align: left;
}

/* Base input styling comes from the global .prompt-note. */
.confirm-note {
  margin-top: 4px;
  border-radius: 8px;
  padding: 8px 12px;
  resize: none;
}

.confirm-step {
  margin-left: auto;
  font-size: 0.75rem;
  color: var(--muted);
}

.stepper-footer.confirm-card-footer {
  gap: 8px;
  align-items: center;
  margin-top: 8px;
  border-top: 1px solid var(--border);
  padding-top: 12px;
  width: 100%;
}
</style>
