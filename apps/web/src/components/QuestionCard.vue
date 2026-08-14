<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import type { UserQuestion } from '@locagens/shared';
import ThemedButton from './ui/ThemedButton.vue';
import StepperFooter from './ui/StepperFooter.vue';

const props = defineProps<{
  // The pending question request ({ questions: UserQuestion[] }) or null.
  request: { questions: UserQuestion[] } | null;
}>();

const emit = defineEmits<{
  (e: 'submit', payload: { selections: string[][]; notes: string[] }): void;
}>();

const questions = computed<UserQuestion[]>(() => props.request?.questions ?? []);

// Local selection state: one array of chosen labels per question (by index).
const selections = ref<string[][]>([]);
// Free-text comment per question (the user's own input), by index.
const notes = ref<string[]>([]);
// Step through questions one at a time (previous / next).
const currentIndex = ref(0);

// Reset state + step whenever a new request arrives.
watch(
  () => props.request,
  () => {
    selections.value = questions.value.map(() => []);
    notes.value = questions.value.map(() => '');
    currentIndex.value = 0;
  },
  { immediate: true }
);

const currentQuestion = computed<UserQuestion | null>(() => questions.value[currentIndex.value] ?? null);
const isFirst = computed(() => currentIndex.value === 0);
const isLast = computed(() => currentIndex.value === questions.value.length - 1);

// A question counts as answered if the user picked an option OR wrote a note.
function answered(i: number): boolean {
  return (selections.value[i]?.length ?? 0) > 0 || (notes.value[i]?.trim().length ?? 0) > 0;
}
const currentAnswered = computed(() => answered(currentIndex.value));

function goPrev() {
  if (!isFirst.value) currentIndex.value--;
}

function goNext() {
  if (!isLast.value && currentAnswered.value) currentIndex.value++;
}

function isSelected(qIdx: number, label: string): boolean {
  return selections.value[qIdx]?.includes(label) ?? false;
}

function toggle(qIdx: number, label: string) {
  const q = questions.value[qIdx];
  const current = selections.value[qIdx] ?? [];
  if (q.multiSelect) {
    selections.value[qIdx] = current.includes(label)
      ? current.filter(l => l !== label)
      : [...current, label];
  } else {
    // Single-select: replace (allow toggling off by clicking the chosen one).
    selections.value[qIdx] = current.includes(label) ? [] : [label];
  }
}

// Every question must be answered (option or note) before submitting.
const canSubmit = computed(() =>
  questions.value.length > 0 && questions.value.every((_, i) => answered(i))
);

function submit() {
  if (!canSubmit.value) return;
  emit('submit', {
    selections: selections.value.map(s => [...s]),
    notes: notes.value.map(n => n.trim())
  });
}
</script>

<template>
  <transition name="card-slide-up">
    <div v-if="request && currentQuestion" class="prompt-card prompt-card--floating composer-question-card">
      <div class="question-card-header">
        <strong>The assistant needs your input</strong>
        <span v-if="questions.length > 1" class="question-step">{{ currentIndex + 1 }} / {{ questions.length }}</span>
      </div>

      <div class="question-block">
        <div class="question-head">
          <span v-if="currentQuestion.header" class="question-chip">{{ currentQuestion.header }}</span>
          <span class="question-text">{{ currentQuestion.question }}</span>
          <span v-if="currentQuestion.multiSelect" class="question-multi">choose any</span>
        </div>
        <div class="question-options">
          <ThemedButton
            v-for="opt in currentQuestion.options"
            :key="opt.label"
            :variant="isSelected(currentIndex, opt.label) ? 'primary' : 'secondary'"
            class="question-option"
            style="display: flex; flex-direction: column; align-items: flex-start; text-align: left;"
            @click="toggle(currentIndex, opt.label)"
          >
            <span class="option-label" style="display: block; font-weight: 600; width: 100%;">{{ opt.label }}</span>
            <span v-if="opt.description" class="option-desc" style="display: block; font-size: 0.76rem; opacity: 0.8; width: 100%; margin-top: 2px;">{{ opt.description }}</span>
          </ThemedButton>
        </div>

        <textarea
          v-model="notes[currentIndex]"
          class="question-note prompt-note"
          rows="2"
          placeholder="Your own comment (optional)…"
        ></textarea>
      </div>

      <StepperFooter
        :is-first="isFirst"
        :is-last="isLast"
        :can-advance="currentAnswered"
        :can-submit="canSubmit"
        @prev="goPrev"
        @next="goNext"
        @submit="submit"
      />
    </div>
  </transition>
</template>

<style scoped>
/* Shell + positioning come from the global .prompt-card / .prompt-card--floating. */
.composer-question-card {
  padding: 16px;
  gap: 14px;
  max-height: 60vh;
  overflow-y: auto;
}

.question-card-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  font-size: 0.9rem;
  color: var(--text);
}

.question-step {
  flex: 0 0 auto;
  font-size: 0.72rem;
  color: var(--muted);
  font-variant-numeric: tabular-nums;
}

.question-block {
  display: flex;
  flex-direction: column;
  gap: 9px;
}

.question-head {
  display: flex;
  align-items: baseline;
  flex-wrap: wrap;
  gap: 8px;
}

.question-chip {
  flex: 0 0 auto;
  font-size: 0.66rem;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--muted);
  border: 1px solid var(--border);
  border-radius: 5px;
  padding: 2px 6px;
}

.question-text {
  font-size: 0.88rem;
  color: var(--text);
}

.question-multi {
  font-size: 0.7rem;
  color: var(--faint);
}

.question-options {
  display: flex;
  flex-direction: column;
  gap: 7px;
}

.question-options .question-option {
  width: 100%;
}

/* The label/description live inside ThemedButton's centered .btn-content span,
   so pierce the child scope with :deep() to stack them left-aligned. */
.question-options .question-option :deep(.btn-content) {
  flex-direction: column;
  align-items: flex-start;
  justify-content: flex-start;
  text-align: left;
  white-space: normal;
}

.option-label {
  font-size: 0.85rem;
  color: inherit;
  font-weight: 550;
}

.option-desc {
  font-size: 0.76rem;
  color: inherit;
  opacity: 0.7;
}

/* Base input styling comes from the global .prompt-note. */
.question-note {
  margin-top: 2px;
  border-radius: 9px;
  padding: 8px 11px;
  resize: vertical;
}
</style>
