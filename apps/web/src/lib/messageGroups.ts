import type { RunMessage } from '@locagens/shared';
import { formatJson } from './format';
import { messageTokenEstimate } from './messageDerived';

export interface MessageGroup {
  type: 'user' | 'assistant' | 'tool_group' | 'system' | 'coder_group';
  id: string;
  message: RunMessage;
  toolCalls: any[];
  toolResponses: RunMessage[];
  /** Only set for `coder_group`: the sub-agent's own groups, rendered nested. */
  children?: MessageGroup[];
  /** Only set for `coder_group`: the sub-agent's name (its delegated task title). */
  title?: string;
}

export interface AgentSummary {
  id: string;
  title: string;
  role: 'coder' | 'utility';
  roleLabel: string;
  model: string;
  status: 'running' | 'done';
  tokenEstimate: number;
  toolUseCount: number;
  children?: MessageGroup[];
}

// groupMessages re-runs over the FULL message list on every streaming flush, so
// without memoization every assistant message's rawResponse would be JSON.parsed
// once per frame. Message objects are replaced immutably on update, so a WeakMap
// keyed by the message object is both correct and self-cleaning.
const parsedToolCallsCache = new WeakMap<RunMessage, any[]>();

export function parseToolCalls(message: RunMessage): any[] {
  if (!message.rawResponse) return [];
  const cached = parsedToolCallsCache.get(message);
  if (cached) return cached;

  let toolCalls: any[] = [];
  try {
    const parsed = JSON.parse(message.rawResponse);
    if (Array.isArray(parsed)) toolCalls = parsed;
  } catch (e) {
    // Malformed rawResponse: treat as no tool calls
  }
  parsedToolCallsCache.set(message, toolCalls);
  return toolCalls;
}

/** Whether an assistant message carries serialized tool calls in rawResponse. */
export function hasToolCalls(message: RunMessage): boolean {
  return parseToolCalls(message).length > 0;
}

/** Treats a tool response as successful unless it explicitly reports failure. */
export function isToolSuccess(content: string): boolean {
  try {
    return JSON.parse(content).success !== false;
  } catch (e) {
    return true;
  }
}

/** Collapses structural tool data classes for styling. */
export function getToolStatusClass(response?: RunMessage): string {
  if (!response) return 'pending';
  return isToolSuccess(response.content) ? 'success' : 'failed';
}

export function formatToolArgs(args?: string): string {
  if (!args) return '';
  return formatJson(args);
}

/**
 * Collapses the flat message list into renderable groups, pairing each
 * tool-calling assistant message with the tool responses that follow it.
 */
export function groupMessages(messages: RunMessage[]): MessageGroup[] {
  return foldCoderGroups(buildFlatGroups(messages));
}

export function collectAgentSummaries(groups: MessageGroup[], isRunning: boolean): AgentSummary[] {
  return collectAgentSummariesInternal(groups, isRunning, true);
}

export function collectAgentSummaryLinks(groups: MessageGroup[], isRunning: boolean): AgentSummary[] {
  return collectAgentSummariesInternal(groups, isRunning, false);
}

function collectAgentSummariesInternal(groups: MessageGroup[], isRunning: boolean, includeChildren: boolean): AgentSummary[] {
  const lastNonCoderIdx = lastNonCoderIndex(groups);
  return groups
    .map((group, index) => ({ group, index }))
    .filter(({ group }) => group.type === 'coder_group' && group.children?.length)
    .map(({ group, index }) => {
      const children = group.children ?? [];
      const role = children[0]?.message.agentRole === 'utility' ? 'utility' : 'coder';
      const model = children.find(c => c.message.model)?.message.model ?? '';
      return {
        id: group.id,
        title: group.title || (role === 'utility' ? 'Utility task' : 'Coder task'),
        role,
        roleLabel: role === 'utility' ? 'Utility' : 'Coder',
        model,
        status: isRunning && index > lastNonCoderIdx ? 'running' : 'done',
        tokenEstimate: children.reduce((sum, child) => sum + messageTokenEstimate(child.message), 0),
        toolUseCount: children.reduce((sum, child) => {
          if (child.type !== 'tool_group') return sum;
          return sum + Math.max(child.toolCalls.length, 1);
        }, 0),
        children: includeChildren ? children : undefined
      };
    });
}

export function lastNonCoderIndex(groups: MessageGroup[]): number {
  for (let i = groups.length - 1; i >= 0; i--) {
    if (groups[i].type !== 'coder_group') return i;
  }
  return -1;
}

/**
 * Collapses runs of consecutive coder (sub-agent) groups into per-sub-agent
 * `coder_group` boxes — one window per delegated sub-agent — so each coder gets
 * its own contained, scrollable box instead of all coders merging into one.
 *
 * Sub-agents are identified by `agentName` (the delegated task title). Parallel
 * sub-agents interleave their messages by timestamp, so we partition each
 * contiguous coder block by `agentName` (first-appearance order) rather than
 * relying on adjacency. Two separate delegate_tasks calls are split anyway by
 * the architect's own messages sitting between them.
 */
function isSubAgent(group: MessageGroup): boolean {
  return group.message.agentRole === 'coder' || group.message.agentRole === 'utility';
}

// Group objects are memoized so that re-grouping (which happens on every
// streaming flush) hands unchanged groups back with the SAME object identity.
// That lets Vue's props comparison bail out, so only the group containing the
// streaming message actually re-renders instead of every ToolGroup in the
// thread. Keys are the immutably-replaced message/group objects, so the caches
// are self-cleaning (same invariant as parsedToolCallsCache above).
const simpleGroupCache = new WeakMap<RunMessage, MessageGroup>();
const toolGroupCache = new WeakMap<RunMessage, { responses: RunMessage[]; group: MessageGroup }>();
const coderGroupCache = new WeakMap<MessageGroup, { children: MessageGroup[]; group: MessageGroup }>();

function sameItems<T>(a: T[], b: T[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

function makeSimpleGroup(type: MessageGroup['type'], msg: RunMessage, toolCalls: any[] = []): MessageGroup {
  const cached = simpleGroupCache.get(msg);
  if (cached && cached.type === type) return cached;
  const group: MessageGroup = { type, id: msg.id, message: msg, toolCalls, toolResponses: [] };
  simpleGroupCache.set(msg, group);
  return group;
}

function makeToolGroup(msg: RunMessage, toolCalls: any[], toolResponses: RunMessage[]): MessageGroup {
  const cached = toolGroupCache.get(msg);
  if (cached && sameItems(cached.responses, toolResponses)) return cached.group;
  const group: MessageGroup = { type: 'tool_group', id: msg.id, message: msg, toolCalls, toolResponses };
  toolGroupCache.set(msg, { responses: toolResponses, group });
  return group;
}

function makeCoderGroup(children: MessageGroup[]): MessageGroup {
  const lead = children[0];
  const cached = coderGroupCache.get(lead);
  if (cached && sameItems(cached.children, children)) return cached.group;
  const group: MessageGroup = {
    type: 'coder_group',
    id: `coder-${lead.id}`,
    message: lead.message,
    toolCalls: [],
    toolResponses: [],
    children,
    title: lead.message.agentName || undefined
  };
  coderGroupCache.set(lead, { children, group });
  return group;
}

function foldCoderGroups(flat: MessageGroup[]): MessageGroup[] {
  const result: MessageGroup[] = [];
  let i = 0;
  while (i < flat.length) {
    if (isSubAgent(flat[i])) {
      const block: MessageGroup[] = [];
      while (i < flat.length && isSubAgent(flat[i])) {
        block.push(flat[i]);
        i++;
      }

      // Partition the block into one bucket per sub-agent, keyed by role+name so
      // coder and utility sub-agents never merge, keeping each in order and
      // preserving first-appearance order.
      const buckets = new Map<string, MessageGroup[]>();
      for (const g of block) {
        const key = `${g.message.agentRole}:${g.message.agentName || ''}`;
        const bucket = buckets.get(key);
        if (bucket) bucket.push(g);
        else buckets.set(key, [g]);
      }

      for (const children of buckets.values()) {
        result.push(makeCoderGroup(children));
      }
    } else {
      result.push(flat[i]);
      i++;
    }
  }
  return result;
}

function buildFlatGroups(messages: RunMessage[]): MessageGroup[] {
  const result: MessageGroup[] = [];
  let i = 0;

  while (i < messages.length) {
    const msg = messages[i];

    if (msg.role === 'user') {
      result.push(makeSimpleGroup('user', msg));
      i++;
    } else if (msg.role === 'system') {
      result.push(makeSimpleGroup('system', msg));
      i++;
    } else if (msg.role === 'assistant') {
      const toolCalls = parseToolCalls(msg);
      if (toolCalls.length > 0) {
        const toolResponses: RunMessage[] = [];
        let j = i + 1;
        while (j < messages.length && messages[j].role === 'tool') {
          toolResponses.push(messages[j]);
          j++;
        }

        result.push(makeToolGroup(msg, toolCalls, toolResponses));
        i = j;
      } else {
        result.push(makeSimpleGroup('assistant', msg));
        i++;
      }
    } else if (msg.role === 'tool') {
      result.push(makeToolGroup(msg, [{ function: { name: 'Workspace Tool Output', arguments: '{}' } }], [msg]));
      i++;
    } else {
      i++;
    }
  }

  return result;
}
