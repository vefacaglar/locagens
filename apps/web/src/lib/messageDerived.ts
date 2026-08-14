import type { RunMessage } from '@locagens/shared';
import { cleanMessageContent } from './markdown';

// Per-message derived values, memoized by message object identity. Message
// objects are replaced immutably on every update (see useChatSession), so a
// WeakMap keyed by the message is both correct and self-cleaning: only the one
// message that changed in a streaming flush pays the derivation cost, instead
// of every message in the conversation being re-derived on every frame.

const cleanedContentCache = new WeakMap<RunMessage, string>();

/** cleanMessageContent(message.content), computed once per message version. */
export function cleanedMessageContent(message: RunMessage): string {
  let cached = cleanedContentCache.get(message);
  if (cached === undefined) {
    cached = cleanMessageContent(message.content || '');
    cleanedContentCache.set(message, cached);
  }
  return cached;
}

const planCache = new WeakMap<RunMessage, string | null>();

/** The inline <plan> block of an assistant message, or null. */
export function extractPlanFromMessage(message: RunMessage): string | null {
  if (planCache.has(message)) return planCache.get(message) ?? null;
  const match = (message.content || '').match(/<plan>([\s\S]*?)<\/plan>/);
  const plan = match ? match[1].trim() : null;
  planCache.set(message, plan);
  return plan;
}

const taskListCache = new WeakMap<RunMessage, string | null>();

/** The inline <task_list> block of an assistant message, or null. */
export function extractTaskListFromMessage(message: RunMessage): string | null {
  if (taskListCache.has(message)) return taskListCache.get(message) ?? null;
  const match = (message.content || '').match(/<task_list>([\s\S]*?)<\/task_list>/);
  const taskList = match ? match[1].trim() : null;
  taskListCache.set(message, taskList);
  return taskList;
}

function estimateTokens(text: string): number {
  if (!text) return 0;
  const cleanText = text.includes('data:image')
    ? text.replace(/!\[([^\]]*)\]\(data:image\/[^)]+\)/g, '[Image]')
    : text;
  const charCount = cleanText.length;
  const wordCount = cleanText.trim().split(/\s+/).length;
  return Math.round(Math.max(charCount / 3.7, wordCount * 1.3));
}

const tokenEstimateCache = new WeakMap<RunMessage, number>();

/** Rough token estimate for content + reasoning, computed once per message version. */
export function messageTokenEstimate(message: RunMessage): number {
  let cached = tokenEstimateCache.get(message);
  if (cached === undefined) {
    cached = estimateTokens(message.content || '') + estimateTokens(message.reasoningContent || '');
    tokenEstimateCache.set(message, cached);
  }
  return cached;
}
