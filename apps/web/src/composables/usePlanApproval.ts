import { computed, nextTick, ref, watch, type ComputedRef, type Ref } from 'vue';
import type { Plan } from '@locagens/shared';
import type { MessageGroup } from '../lib/messageGroups';
import type { ChatMode } from './useComposerSettings';

interface PlanApprovalOptions {
  currentMode: Ref<ChatMode>;
  currentPlan: Ref<Plan | null>;
  groupedMessages: ComputedRef<MessageGroup[]>;
  activeRunId: Ref<string | null>;
  focusSignal: Ref<number>;
  sendQuickReply: (message: string) => void;
}

/**
 * Plan approval actions (Start / Revise / Reject). While in plan mode, the
 * panel offers three choices once a plan is presented. The decision is tracked
 * per plan version so the buttons reappear if the assistant produces a revised
 * plan.
 */
export function usePlanApproval(options: PlanApprovalOptions) {
  const { currentMode, currentPlan, groupedMessages, activeRunId, focusSignal, sendQuickReply } = options;

  const isPlanMode = computed(() => currentMode.value === 'plan');
  const planKey = computed(() =>
    currentPlan.value ? `${currentPlan.value.id}:${currentPlan.value.version}` : ''
  );
  const decidedPlanKey = ref('');

  const latestPlanProposalMessageIndex = computed(() => {
    const list = groupedMessages.value;
    if (!list) return -1;
    for (let i = list.length - 1; i >= 0; i--) {
      const group = list[i];
      if (group.type === 'tool_group') {
        const hasUpdatePlan = group.toolCalls.some(tc => tc.function?.name === 'update_plan');
        if (hasUpdatePlan) return i;
      }
    }
    return -1;
  });

  const showPlanActions = computed(() => {
    if (!isPlanMode.value || !currentPlan.value) return false;

    const planIndex = latestPlanProposalMessageIndex.value;
    if (planIndex === -1) return false;

    // If there is any user message after the latest plan proposal, the user has already interacted with it
    for (let i = planIndex + 1; i < groupedMessages.value.length; i++) {
      if (groupedMessages.value[i].type === 'user') {
        return false;
      }
    }

    return decidedPlanKey.value !== planKey.value;
  });

  watch(activeRunId, () => { decidedPlanKey.value = ''; });

  // Start: approve the plan, switch to build (accept edits) mode, and kick off
  // implementation with a follow-up message. We flip the mode FIRST and wait a
  // tick so the continue request (and the system prompt it rebuilds) is already
  // in Build mode before the approval message is sent — otherwise the model can
  // reply as if it were still in Plan mode and ask to switch again.
  async function startPlan() {
    decidedPlanKey.value = planKey.value;
    currentMode.value = 'accept_edits';
    await nextTick();
    sendQuickReply(
      "I approve this plan. We are now in Build mode — implement it step by step right away. Do not ask me to switch modes; you are already in Build mode."
    );
  }

  // Revise: stay in plan mode and focus the composer so the user can describe
  // the changes they want to the plan.
  function revisePlan() {
    decidedPlanKey.value = planKey.value;
    focusSignal.value++;
  }

  // Reject: tell the model the plan is turned down so it does NOT implement it,
  // and stay in plan mode. Without sending this message the model never learns
  // it was rejected and may go ahead and build the plan anyway.
  async function rejectPlan() {
    decidedPlanKey.value = planKey.value;
    if (currentMode.value !== 'plan') {
      currentMode.value = 'plan';
      await nextTick();
    }
    sendQuickReply(
      "I reject this plan. Do NOT implement it or make any changes. Stay in Plan mode and wait for my further instructions."
    );
  }

  return { showPlanActions, startPlan, revisePlan, rejectPlan };
}
