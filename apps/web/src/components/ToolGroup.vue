<script setup lang="ts">
import { ref, computed } from 'vue';
import type { RunMessage } from '@locagens/shared';
import { isToolSuccess } from '../lib/messageGroups';
import { cleanMessageContent, renderMarkdown } from '../lib/markdown';
import CopyButton from './ui/CopyButton.vue';

const props = defineProps<{
  id?: string;
  thought?: string;
  toolCalls: any[];
  toolResponses: RunMessage[];
  agentRole?: RunMessage['agentRole'];
  model?: string;
}>();

const isCoder = computed(() => props.agentRole === 'coder');

const emit = defineEmits<{
  (e: 'open-plan'): void;
  (e: 'view-file-in-review', path: string): void;
}>();

const detailsExpanded = ref<Record<number, boolean>>({});

// Local instance cache to prevent redundant JSON.parse & string manipulation on updates
const parsedArgsCache = new Map<string, Record<string, any>>();
const toolPathCache = new Map<string, string>();
const stepLabelCache = new Map<string, string>();
const planTitleCache = new Map<string, string>();
const planTaskCountCache = new Map<string, number>();
const planSummaryCache = new Map<string, string>();
const toolParamsCache = new Map<string, string>();
const isToolSuccessCache = new Map<string, boolean>();
const askAnswersCache = new Map<string, any>();
const delegatedResultCache = new Map<string, any>();
const singleBoxHeaderLabelCache = new Map<string, string>();
const singleBoxContentCache = new Map<string, string>();

function parseArgsCached(argumentsJson: string): Record<string, any> {
  let cached = parsedArgsCache.get(argumentsJson);
  if (cached === undefined) {
    cached = parseArgs(argumentsJson);
    parsedArgsCache.set(argumentsJson, cached);
  }
  return cached;
}

function getToolPathCached(argumentsJson: string): string {
  let cached = toolPathCache.get(argumentsJson);
  if (cached === undefined) {
    cached = getToolPath(argumentsJson);
    toolPathCache.set(argumentsJson, cached);
  }
  return cached;
}

function getStepLabelCached(name: string, args: string, idx: number): string {
  const resContent = props.toolResponses[idx]?.content ?? '';
  const key = `${idx}|${name}|${args}|${resContent}`;
  let cached = stepLabelCache.get(key);
  if (cached === undefined) {
    cached = getStepLabel(name, args, idx);
    stepLabelCache.set(key, cached);
  }
  return cached;
}

function getPlanTitleCached(argumentsJson: string): string {
  let cached = planTitleCache.get(argumentsJson);
  if (cached === undefined) {
    cached = getPlanTitle(argumentsJson);
    planTitleCache.set(argumentsJson, cached);
  }
  return cached;
}

function getPlanTaskCountCached(argumentsJson: string): number {
  let cached = planTaskCountCache.get(argumentsJson);
  if (cached === undefined) {
    cached = getPlanTaskCount(argumentsJson);
    planTaskCountCache.set(argumentsJson, cached);
  }
  return cached;
}

function getPlanSummaryCached(argumentsJson: string): string {
  let cached = planSummaryCache.get(argumentsJson);
  if (cached === undefined) {
    cached = getPlanSummary(argumentsJson);
    planSummaryCache.set(argumentsJson, cached);
  }
  return cached;
}

function formatToolParamsCached(name: string, argumentsJson: string): string {
  const key = `${name}|${argumentsJson}`;
  let cached = toolParamsCache.get(key);
  if (cached === undefined) {
    cached = formatToolParams(name, argumentsJson);
    toolParamsCache.set(key, cached);
  }
  return cached;
}

function hasToolParamsCached(name: string, argumentsJson: string): boolean {
  return !!formatToolParamsCached(name, argumentsJson).trim();
}

function isToolSuccessCached(content: string): boolean {
  let cached = isToolSuccessCache.get(content);
  if (cached === undefined) {
    cached = isToolSuccess(content);
    isToolSuccessCache.set(content, cached);
  }
  return cached;
}

function parseAskAnswersCached(contentJson: string): any {
  let cached = askAnswersCache.get(contentJson);
  if (cached === undefined) {
    cached = parseAskAnswers(contentJson);
    askAnswersCache.set(contentJson, cached);
  }
  return cached;
}

function parseDelegatedResultCached(contentJson: string): any {
  let cached = delegatedResultCache.get(contentJson);
  if (cached === undefined) {
    cached = parseDelegatedResult(contentJson);
    delegatedResultCache.set(contentJson, cached);
  }
  return cached;
}

function shouldRenderAskAnswersCached(name: string, contentJson: string): boolean {
  if (name !== 'ask_user_question') return false;
  return !!parseAskAnswersCached(contentJson)?.length;
}

function shouldRenderDelegatedResultCached(name: string, contentJson: string): boolean {
  if (name !== 'delegate_tasks' && name !== 'delegate_to_utility' && name !== 'Workspace Tool Output') return false;
  return !!parseDelegatedResultCached(contentJson)?.length;
}

function getSingleBoxHeaderLabelCached(name: string, argumentsJson: string): string {
  const key = `${name}|${argumentsJson}`;
  let cached = singleBoxHeaderLabelCache.get(key);
  if (cached === undefined) {
    cached = getSingleBoxHeaderLabel(name, argumentsJson);
    singleBoxHeaderLabelCache.set(key, cached);
  }
  return cached;
}

function formatSingleBoxContentCached(name: string, argumentsJson: string, response: any): string {
  const resContent = response?.content ?? '';
  const key = `${name}|${argumentsJson}|${resContent}`;
  let cached = singleBoxContentCache.get(key);
  if (cached === undefined) {
    cached = formatSingleBoxContent(name, argumentsJson, response);
    singleBoxContentCache.set(key, cached);
  }
  return cached;
}

// Consecutive read-only inspection calls in the same turn are visually noisy
// (a long stack of "File read: …" / "Directory listed: …" rows). We fold any
// run of >= CLUSTER_MIN such calls into a single collapsible summary row.
const CLUSTERABLE = new Set(['read_file', 'list_directory', 'search_files']);
const CLUSTER_MIN = 3;

interface ToolCluster {
  firstIdx: number;
  indices: number[];
}

const clusters = computed<ToolCluster[]>(() => {
  const out: ToolCluster[] = [];
  let run: number[] = [];
  const flush = () => {
    if (run.length >= CLUSTER_MIN) out.push({ firstIdx: run[0], indices: [...run] });
    run = [];
  };
  props.toolCalls.forEach((tc, idx) => {
    if (CLUSTERABLE.has(tc.function?.name)) run.push(idx);
    else flush();
  });
  flush();
  return out;
});

// idx -> the cluster it belongs to (if any), and idx -> cluster it starts.
const clusterByIndex = computed(() => {
  const m = new Map<number, ToolCluster>();
  for (const c of clusters.value) for (const i of c.indices) m.set(i, c);
  return m;
});
const clusterByFirst = computed(() => {
  const m = new Map<number, ToolCluster>();
  for (const c of clusters.value) m.set(c.firstIdx, c);
  return m;
});

const clustersExpanded = ref<Record<number, boolean>>({});

function toggleCluster(firstIdx: number) {
  clustersExpanded.value[firstIdx] = !clustersExpanded.value[firstIdx];
}
function isClusterExpanded(firstIdx: number): boolean {
  return !!clustersExpanded.value[firstIdx];
}

/** The cluster that STARTS at this index, for rendering the summary header. */
function clusterStart(idx: number): ToolCluster | undefined {
  return clusterByFirst.value.get(idx);
}
function isClustered(idx: number): boolean {
  return clusterByIndex.value.has(idx);
}
/** A clustered row is hidden until its summary header is expanded. */
function isRowVisible(idx: number): boolean {
  const c = clusterByIndex.value.get(idx);
  return c ? isClusterExpanded(c.firstIdx) : true;
}

function clusterLabel(c: ToolCluster): string {
  const names = c.indices.map(i => props.toolCalls[i].function?.name);
  const n = c.indices.length;
  const isPending = clusterStatus(c) === 'pending';
  if (isPending) {
    if (names.every(name => name === 'read_file')) return `Reading ${n} files...`;
    if (names.every(name => name === 'list_directory')) return `Listing ${n} directories...`;
    if (names.every(name => name === 'search_files')) return `Running ${n} searches...`;
    return `Inspecting ${n} items...`;
  } else {
    if (names.every(name => name === 'read_file')) return `Read ${n} files`;
    if (names.every(name => name === 'list_directory')) return `Listed ${n} directories`;
    if (names.every(name => name === 'search_files')) return `Ran ${n} searches`;
    return `Inspected ${n} items`;
  }
}

function clusterStatus(c: ToolCluster): 'success' | 'failed' | 'pending' {
  let anyPending = false;
  for (const i of c.indices) {
    const res = props.toolResponses[i];
    if (!res) anyPending = true;
    else if (!isToolSuccessCached(res.content)) return 'failed';
  }
  return anyPending ? 'pending' : 'success';
}

function getSingleBoxHeaderLabel(name: string, argumentsJson: string): string {
  if (name === 'run_command') return 'bash';
  if (name === 'read_file' || name === 'write_file' || name === 'edit_file') {
    const path = getToolPathCached(argumentsJson);
    return path.split('/').pop() || name;
  }
  return name;
}

function formatSingleBoxContent(name: string, argumentsJson: string, response: any): string {
  const params = hasToolParamsCached(name, argumentsJson) ? formatToolParamsCached(name, argumentsJson) : '';
  const result = response ? formatToolResult(name, response.content) : 'Running...';
  
  if (params && result) {
    return `Parameters:\n${params}\n\nResult:\n${result}`;
  }
  if (params) {
    return `Parameters:\n${params}`;
  }
  if (result) {
    return result;
  }
  return '';
}

/** Title of an update_plan call, for the clickable plan card. */
function getPlanTitle(argumentsJson: string): string {
  const parsed = parseArgsCached(argumentsJson);
  return typeof parsed.title === 'string' && parsed.title.trim() ? parsed.title.trim() : 'Plan';
}

/** Number of steps in an update_plan call's tasks array. */
function getPlanTaskCount(argumentsJson: string): number {
  const parsed = parseArgsCached(argumentsJson);
  return Array.isArray(parsed.tasks) ? parsed.tasks.length : 0;
}

/** A short plain-text summary for the plan card: first real line of the body,
 *  falling back to the first few step titles. */
function getPlanSummary(argumentsJson: string): string {
  const parsed = parseArgsCached(argumentsJson);
  const clip = (s: string) => (s.length > 150 ? s.slice(0, 150).trim() + '…' : s);

  const body = typeof parsed.body === 'string' ? parsed.body : '';
  for (const raw of body.split('\n')) {
    let line = raw.trim();
    if (!line || line.startsWith('#') || line.startsWith('```') || line.startsWith('|') || line.startsWith('---')) continue;
    line = line
      .replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1') // links/images -> text
      .replace(/[*_`>#]/g, '')                    // emphasis/markers
      .trim();
    if (line) return clip(line);
  }

  if (Array.isArray(parsed.tasks) && parsed.tasks.length) {
    const joined = parsed.tasks
      .slice(0, 3)
      .map((t: any) => (typeof t?.text === 'string' ? t.text : ''))
      .filter(Boolean)
      .join(' · ');
    if (joined) return clip(joined);
  }
  return '';
}

function toggleDetails(index: number) {
  detailsExpanded.value[index] = !detailsExpanded.value[index];
}

const cleanedThought = computed(() => {
  if (!props.thought) return '';
  return cleanMessageContent(props.thought);
});

function getToolPath(argumentsJson: string): string {
  if (!argumentsJson) return '';
  try {
    const parsed = JSON.parse(argumentsJson);
    return parsed.path || '';
  } catch (e) {
    return '';
  }
}

function isFileLinkTool(name: string): boolean {
  return name === 'edit_file' || name === 'write_file' || name === 'delete_file';
}

function handleRowClick(name: string, args: string, idx: number) {
  if (isFileLinkTool(name)) {
    const path = getToolPathCached(args);
    emit('view-file-in-review', path);
  } else {
    toggleDetails(idx);
  }
}

function parseArgs(argumentsJson: string): Record<string, any> {
  if (!argumentsJson) return {};
  try {
    return JSON.parse(argumentsJson) || {};
  } catch (e) {
    return {};
  }
}

/** Label for a step; for orphaned/standalone tool outputs (no known tool name)
 *  we surface the trimmed result message instead of a generic placeholder. */
function getStepLabel(name: string, args: string, idx: number): string {
  if (name === 'Workspace Tool Output') {
    const res = props.toolResponses[idx];
    if (res) {
      const delegated = parseDelegatedResultCached(res.content);
      if (delegated?.length) {
        return delegated.length === 1 ? `Utility result: ${delegated[0].title}` : `${delegated.length} utility results`;
      }
      try {
        const parsed = parseArgsCached(res.content);
        if (parsed.error) return `Error: ${parsed.error}`;
        if (typeof parsed.message === 'string' && parsed.message) return parsed.message;
      } catch (e) { /* fall through */ }
    }
    return 'Workspace tool output';
  }
  const isFinished = !!props.toolResponses[idx];
  return getToolLabel(name, args, isFinished);
}

function getToolLabel(name: string, args: string, isFinished: boolean): string {
  const path = getToolPathCached(args);
  const parsed = parseArgsCached(args);
  switch (name) {
    case 'read_file':
      return isFinished ? `Read: ${path}` : `Reading: ${path}`;
    case 'write_file':
      return isFinished ? `Written: ${path}` : `Writing: ${path}`;
    case 'edit_file':
      return isFinished ? `Edited: ${path}` : `Editing: ${path}`;
    case 'delete_file':
      return isFinished ? `Deleted: ${path}` : `Deleting: ${path}`;
    case 'list_directory':
      return isFinished ? `Directory listed: ${path || 'root'}` : `Listing directory: ${path || 'root'}`;
    case 'create_directory':
      return isFinished ? `Folder created: ${path}` : `Creating folder: ${path}`;
    case 'move_file':
      return isFinished 
        ? `Moved: ${parsed.source_path ?? ''} → ${parsed.destination_path ?? ''}`
        : `Moving: ${parsed.source_path ?? ''} → ${parsed.destination_path ?? ''}`;
    case 'search_files':
      return isFinished ? `Searched: ${parsed.query ?? ''}` : `Searching: ${parsed.query ?? ''}`;
    case 'search_web':
      return isFinished ? `Searched web: ${parsed.query ?? ''}` : `Searching web: ${parsed.query ?? ''}`;
    case 'run_command':
      return isFinished ? `Ran command: ${parsed.command ?? ''}` : `Running command: ${parsed.command ?? ''}`;
    case 'fetch_url':
      return isFinished ? `Fetched: ${parsed.url ?? ''}` : `Fetching: ${parsed.url ?? ''}`;
    case 'delegate_tasks': {
      const count = Array.isArray(parsed.tasks) ? parsed.tasks.length : 0;
      return isFinished
        ? `Delegated ${count} task${count === 1 ? '' : 's'} to coder${parsed.parallel ? ' (parallel)' : ''}`
        : `Delegating ${count} task${count === 1 ? '' : 's'} to coder...`;
    }
    case 'delegate_to_utility': {
      const count = Array.isArray(parsed.tasks) ? parsed.tasks.length : 0;
      return isFinished
        ? `Delegated ${count} task${count === 1 ? '' : 's'} to utility${parsed.parallel ? ' (parallel)' : ''}`
        : `Delegating ${count} task${count === 1 ? '' : 's'} to utility...`;
    }
    default:
      return isFinished ? `Tool ${name} executed` : `Executing tool ${name}...`;
  }
}

function formatObjectToLines(obj: any): string {
  if (obj === null || obj === undefined) return '';
  if (typeof obj !== 'object') return String(obj);
  
  if (Array.isArray(obj)) {
    return obj.map(item => formatObjectToLines(item)).filter(Boolean).join('\n');
  }
  
  return Object.entries(obj)
    .map(([key, val]) => {
      if (typeof val === 'boolean' || val === null || val === undefined) {
        return '';
      }
      
      if (key === 'exitCode') {
        return '';
      }
      
      if (typeof val === 'object') {
        return formatObjectToLines(val);
      }
      
      return `${key}: ${String(val).trim()}`;
    })
    .filter(Boolean)
    .join('\n');
}

function truncateText(str: string, maxLen: number = 1000): string {
  if (!str) return '';
  if (str.length <= maxLen) return str;
  return str.slice(0, maxLen) + '\n... [truncated]';
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
}

function formatToolParams(name: string, argumentsJson: string): string {
  if (!argumentsJson) return '';
  try {
    const args = JSON.parse(argumentsJson);
    switch (name) {
      case 'Workspace Tool Output':
        return '';
      case 'run_command':
        return args.command || '';
      case 'read_file':
      case 'delete_file':
      case 'list_directory':
      case 'create_directory':
        return `Path: ${args.path || '.'}`;
      case 'write_file':
        return `Path: ${args.path || '.'}\n\nContent:\n${truncateText(args.content || '', 800)}`;
      case 'edit_file':
        return `Path: ${args.path || '.'}\n\nFind:\n${truncateText(args.old_string || '', 300)}\n\nReplace:\n${truncateText(args.new_string || '', 300)}`;
      case 'move_file':
        return `Source: ${args.source_path || ''}\nDestination: ${args.destination_path || ''}`;
      case 'search_files':
        return `Query: "${args.query || ''}"${args.path ? `\nIn path: ${args.path}` : ''}`;
      case 'search_web':
        return `Query: "${args.query || ''}"${args.max_results ? `\nMax results: ${args.max_results}` : ''}`;
      case 'fetch_url':
        return `URL: ${args.url || ''}`;
      case 'set_chat_title':
        return `Title: ${args.title || ''}`;
      case 'ask_user_question': {
        if (!Array.isArray(args.questions) || args.questions.length === 0) return '';
        return args.questions
          .map((q: any, idx: number) => {
            const head = typeof q?.header === 'string' && q.header.trim() ? `${q.header.trim()} — ` : '';
            const text = typeof q?.question === 'string' ? q.question.trim() : '';
            const opts = Array.isArray(q?.options)
              ? q.options
                  .map((o: any) => {
                    const label = typeof o?.label === 'string' ? o.label : '';
                    const desc = typeof o?.description === 'string' && o.description.trim() ? ` — ${o.description.trim()}` : '';
                    return `   • ${label}${desc}`;
                  })
                  .join('\n')
              : '';
            return `${idx + 1}. ${head}${text}${opts ? `\n${opts}` : ''}`;
          })
          .join('\n\n');
      }
      case 'delegate_tasks':
      case 'delegate_to_utility': {
        if (!Array.isArray(args.tasks) || args.tasks.length === 0) return '';
        return args.tasks
          .map((task: any, idx: number) => {
            const title = typeof task?.title === 'string' && task.title.trim() ? task.title.trim() : `Task ${idx + 1}`;
            const body = typeof task?.task === 'string' && task.task.trim() ? `\n${task.task.trim()}` : '';
            return `${idx + 1}. ${title}${body}`;
          })
          .join('\n\n');
      }
      default:
        return formatObjectToLines(args);
    }
  } catch (e) {
    return argumentsJson;
  }
}


function parseDelegatedResult(contentJson: string): { title: string; summary: string }[] | null {
  if (!contentJson) return null;
  try {
    const res = JSON.parse(contentJson);
    if (!Array.isArray(res.results)) return null;
    const sections = res.results
      .map((item: any, idx: number) => ({
        title: typeof item?.title === 'string' && item.title.trim() ? item.title.trim() : `Result ${idx + 1}`,
        summary: typeof item?.summary === 'string' ? item.summary.trim() : ''
      }))
      .filter((item: { title: string; summary: string }) => item.summary);
    return sections.length ? sections : null;
  } catch (e) {
    return null;
  }
}

/** Parsed answers for an ask_user_question result, ready for the Q&A card. */
function parseAskAnswers(contentJson: string): { header: string; question: string; selected: string[]; note: string }[] | null {
  if (!contentJson) return null;
  try {
    const res = JSON.parse(contentJson);
    if (!Array.isArray(res.answers)) return null;
    return res.answers.map((a: any) => ({
      header: typeof a?.header === 'string' ? a.header.trim() : '',
      question: typeof a?.question === 'string' ? a.question.trim() : '',
      selected: Array.isArray(a?.selected) ? a.selected.filter((s: any) => typeof s === 'string') : [],
      note: typeof a?.note === 'string' ? a.note.trim() : ''
    }));
  } catch (e) {
    return null;
  }
}


function formatToolResult(name: string, contentJson: string): string {
  if (!contentJson) return '';
  try {
    const res = JSON.parse(contentJson);
    
    if (res.success === false) {
      const codeStr = res.exitCode !== undefined && res.exitCode !== null ? ` (Exit code: ${res.exitCode})` : '';
      let errStr = `Error: ${res.error || res.message || `Operation failed${codeStr}`}`;
      if (res.stdout) {
        errStr += `\n\nstdout:\n${res.stdout}`;
      }
      if (res.stderr) {
        errStr += `\n\nstderr:\n${res.stderr}`;
      }
      return errStr;
    }

    switch (name) {
      case 'run_command': {
        const codeStr = res.exitCode !== undefined && res.exitCode !== null ? `Exit code: ${res.exitCode}\n\n` : '';
        let out = codeStr;
        if (res.stdout) {
          out += res.stdout;
        }
        if (res.stderr) {
          if (res.stdout) out += '\n';
          out += res.stderr;
        }
        if (res.error) {
          if (out) out += '\n\n';
          out += `Error: ${res.error}`;
        }
        return out || 'Command completed successfully with no output.';
      }
      case 'read_file':
        return res.content !== undefined ? res.content : JSON.stringify(res, null, 2);
      case 'list_directory': {
        if (Array.isArray(res.files)) {
          if (res.files.length === 0) return 'Directory is empty.';
          return res.files
            .map((f: any) => {
              const icon = f.isDirectory ? '📁' : '📄';
              const size = f.isDirectory ? '' : ` (${formatBytes(f.size)})`;
              return `${icon} ${f.name}${size}`;
            })
            .join('\n');
        }
        return JSON.stringify(res, null, 2);
      }
      case 'search_files': {
        if (Array.isArray(res.matches)) {
          const count = res.matchCount ?? res.matches.length;
          if (count === 0) return 'No matches found.';
          const list = res.matches
            .map((m: any) => {
              const lineInfo = m.line ? `:${m.line}` : '';
              const previewInfo = m.preview ? ` - ${m.preview}` : '';
              return `- ${m.path}${lineInfo}${previewInfo}`;
            })
            .join('\n');
          return `Found ${count} match(es):\n${list}`;
        }
        return JSON.stringify(res, null, 2);
      }
      case 'write_file':
      case 'edit_file':
      case 'delete_file':
      case 'create_directory':
      case 'move_file':
        return res.message || 'Success';
      case 'set_chat_title':
        return `Chat title updated to: ${res.title || 'Success'}`;
      default:
        if (res.content !== undefined) return res.content;
        if (res.message !== undefined) return res.message;
        return formatObjectToLines(res);
    }
  } catch (e) {
    return contentJson;
  }
}
</script>

<template>
  <div class="tool-group-wrap" :class="{ 'coder-tool-group': isCoder }">
    <!-- Coder sub-agent label -->
    <div v-if="isCoder" class="coder-tool-meta">
      <span class="agent-badge coder-badge">Coder</span>
      <span v-if="model" class="coder-tool-model">{{ model }}</span>
    </div>

    <!-- Thought Step -->
    <div
      v-if="cleanedThought"
      class="tool-thought-body markdown-body"
      v-html="renderMarkdown(cleanedThought, props.id ? 'thought-' + props.id : undefined)"
    ></div>

    <!-- Tool Call Accordions -->
    <div class="tool-calls-list">
      <template v-for="(tc, idx) in toolCalls" :key="idx">
      <!-- Summary header for a folded run of consecutive inspection calls. -->
      <header
        v-if="clusterStart(idx)"
        class="step-row tool-cluster-header"
        :class="{ 'is-expanded': isClusterExpanded(idx) }"
        @click="toggleCluster(idx)"
      >
        <svg class="step-row-toggle" :class="{ rotated: isClusterExpanded(idx) }" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="m9 18 6-6-6-6"></path>
        </svg>
        <span
          class="step-row-label"
          :style="{ color: clusterStatus(clusterStart(idx)!) === 'failed' ? 'var(--danger)' : undefined }"
        >
          {{ clusterLabel(clusterStart(idx)!) }}
        </span>
        <span v-if="clusterStatus(clusterStart(idx)!) === 'pending'" class="status-dot pending" title="Running..."></span>
      </header>

      <!-- update_plan renders as a clickable card that opens the plan panel -->
      <button
        v-if="tc.function?.name === 'update_plan'"
        type="button"
        class="plan-tool-card"
        @click="emit('open-plan')"
      >
        <div class="plan-tool-head">
          <span class="plan-tool-eyebrow">Plan</span>
          <span v-if="getPlanTaskCountCached(tc.function?.arguments)" class="plan-tool-count">
            {{ getPlanTaskCountCached(tc.function?.arguments) }} steps
          </span>
        </div>
        <span class="plan-tool-title">{{ getPlanTitleCached(tc.function?.arguments) }}</span>
        <span v-if="getPlanSummaryCached(tc.function?.arguments)" class="plan-tool-summary">
          {{ getPlanSummaryCached(tc.function?.arguments) }}
        </span>
        <span class="plan-tool-action">Open in panel</span>
      </button>

      <div
        v-else-if="isRowVisible(idx)"
        class="tool-call-accordion"
        :class="{ 'is-expanded': detailsExpanded[idx], 'clustered-row': isClustered(idx) }"
      >
        <header class="step-row" @click="handleRowClick(tc.function?.name, tc.function?.arguments, idx)">
          <svg v-if="!isFileLinkTool(tc.function?.name)" class="step-row-toggle" :class="{ rotated: detailsExpanded[idx] }" xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="m9 18 6-6-6-6"></path>
          </svg>
          
          <span
            class="step-row-label"
            :style="{ color: (toolResponses[idx] && !isToolSuccessCached(toolResponses[idx].content)) ? 'var(--danger)' : undefined }"
          >
            {{ getStepLabelCached(tc.function?.name, tc.function?.arguments, idx) }}
          </span>
          
          <span v-if="!toolResponses[idx]" class="status-dot pending" title="Running..."></span>
        </header>

        <div v-if="detailsExpanded[idx]" class="tool-call-details">
          <!-- If it has custom result cards (Q&A or Delegated sub-agents) -->
          <template v-if="toolResponses[idx] && (shouldRenderAskAnswersCached(tc.function?.name, toolResponses[idx].content) || shouldRenderDelegatedResultCached(tc.function?.name, toolResponses[idx].content))">
            <!-- Parameters (inside standard wrapper) -->
            <div v-if="hasToolParamsCached(tc.function?.name, tc.function?.arguments)" class="code-block-wrapper">
              <div class="code-block-header">
                <span class="code-block-lang">{{ getSingleBoxHeaderLabelCached(tc.function?.name, tc.function?.arguments) }} (params)</span>
                <CopyButton
                  class="code-block-copy-btn"
                  label="Copy parameters"
                  :text="() => formatToolParamsCached(tc.function?.name, tc.function?.arguments)"
                />
              </div>
              <pre class="faint-code"><code>{{ formatToolParamsCached(tc.function?.name, tc.function?.arguments) }}</code></pre>

              <!-- Answer rendered inside the same params box. -->
              <div
                v-if="shouldRenderAskAnswersCached(tc.function?.name, toolResponses[idx].content)"
                class="ask-answer-list"
              >
              <section
                v-for="(ans, aIdx) in parseAskAnswersCached(toolResponses[idx].content)"
                :key="aIdx"
                class="ask-answer-card"
              >
                <div v-if="ans.header" class="ask-answer-header">{{ ans.header }}</div>
                <div class="ask-answer-question">{{ ans.question }}</div>

                <ul v-if="ans.selected.length" class="ask-answer-choices">
                  <li v-for="(choice, cIdx) in ans.selected" :key="cIdx">{{ choice }}</li>
                </ul>

                <!-- Free-text answer: when nothing was picked from the list it IS
                     the answer (checkmark); otherwise it's an added note. -->
                <ul v-if="ans.note && !ans.selected.length" class="ask-answer-choices">
                  <li>{{ ans.note }}</li>
                </ul>
                <div v-else-if="ans.note" class="ask-answer-note">{{ ans.note }}</div>

                <div v-if="!ans.selected.length && !ans.note" class="ask-answer-empty">Seçim yapılmadı</div>
              </section>
              </div>
            </div>

            <div
              v-if="shouldRenderDelegatedResultCached(tc.function?.name, toolResponses[idx].content)"
              class="delegated-result-list"
            >
              <section
                v-for="section in parseDelegatedResultCached(toolResponses[idx].content)"
                :key="section.title"
                class="delegated-result-card"
              >
                <div class="delegated-result-title">{{ section.title }}</div>
                <div class="delegated-result-body markdown-body" v-html="renderMarkdown(section.summary, 'delegated-' + toolResponses[idx].id + '-' + section.title)"></div>
              </section>
            </div>
          </template>

          <!-- Standard Tool Case (single wrapper for both params and results) -->
          <template v-else>
            <div class="code-block-wrapper">
              <div class="code-block-header">
                <span class="code-block-lang">{{ getSingleBoxHeaderLabelCached(tc.function?.name, tc.function?.arguments) }}</span>
                <CopyButton
                  class="code-block-copy-btn"
                  label="Copy results"
                  :text="() => formatSingleBoxContentCached(tc.function?.name, tc.function?.arguments, toolResponses[idx])"
                />
              </div>
              <pre class="faint-code"><code>{{ formatSingleBoxContentCached(tc.function?.name, tc.function?.arguments, toolResponses[idx]) }}</code></pre>
            </div>
          </template>
        </div>
      </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.plan-tool-card {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 7px;
  width: 100%;
  text-align: left;
  padding: 14px 16px;
  border: 1px solid var(--border);
  border-radius: 10px;
  background: var(--surface-strong);
  color: var(--text);
  cursor: pointer;
  transition: border-color 0.15s ease, background 0.15s ease;
}

.plan-tool-card:hover {
  border-color: transparent;
  background: var(--plan-card-hover-bg);
}

.plan-tool-head {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
}

.plan-tool-eyebrow {
  font-size: 0.68rem;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--faint);
}

.plan-tool-count {
  margin-left: auto;
  font-family: monospace;
  font-size: 0.72rem;
  color: var(--faint);
}

.plan-tool-title {
  font-weight: 650;
  font-size: 1rem;
  line-height: 1.3;
  color: var(--text);
}

.plan-tool-summary {
  font-size: 0.82rem;
  line-height: 1.5;
  color: var(--muted);
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.plan-tool-action {
  margin-top: 3px;
  font-size: 0.74rem;
  color: var(--muted);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 3px 10px;
}

.tool-group-wrap {
  margin: 1px 0;
  width: 100%;
}

/* Coder sub-agent tool group: subtle left accent matching coder messages. */
.tool-group-wrap.coder-tool-group {
  border-left: 2px solid var(--border);
  padding-left: 14px;
  margin-left: 2px;
}

.coder-tool-meta {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
}

.coder-tool-meta .agent-badge.coder-badge {
  font-size: 0.66rem;
  font-weight: 600;
  letter-spacing: 0.04em;
  text-transform: uppercase;
  color: var(--text);
  background: var(--surface-strong);
  border: 1px solid var(--border);
  border-radius: 5px;
  padding: 1px 6px;
}

.coder-tool-model {
  font-size: 0.72rem;
  color: var(--faint);
  font-family: monospace;
}

.tool-thought-body {
  margin-bottom: 12px;
  color: var(--msg-thought-text);
  line-height: 1.6;
}

.tool-calls-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

/* Layout only — the row look + label + toggle come from the shared .step-row
   classes in style.css. */
.tool-call-accordion {
  width: 100%;
  overflow: hidden;
}

/* Folded inspection-call summary header (shares the .step-row look). */
.tool-cluster-header {
  cursor: pointer;
  user-select: none;
}

/* Individual rows revealed under an expanded cluster sit indented beneath it. */
.tool-call-accordion.clustered-row {
  margin-left: 18px;
  border-left: 1px solid var(--border-soft);
  padding-left: 8px;
}

.status-icon {
  flex-shrink: 0;
  display: inline-block;
  vertical-align: middle;
}

.status-icon.success {
  color: var(--success);
}

.status-icon.error {
  color: var(--danger);
}

.status-dot.pending {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--warning);
  animation: shimmerPulse 1.5s infinite ease-in-out;
  flex-shrink: 0;
}

@keyframes shimmerPulse {
  0%, 100% { opacity: 0.6; }
  50% { opacity: 1; }
}

.tool-call-details {
  padding: 8px 0 12px;
  background: transparent;
  border-top: 1px solid var(--border-soft);
  display: flex;
  flex-direction: column;
  gap: 10px;
  font-style: normal;
}

.detail-section {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.response-section {
  border-top: 1px solid var(--border-soft);
  padding-top: 8px;
}

.detail-label {
  font-size: 0.75rem;
  color: var(--muted);
  font-weight: 500;
}

.tool-call-details .code-block-wrapper {
  margin: 6px 0 0;
}

.faint-code {
  font-family: monospace;
  font-size: 0.75rem;
  color: var(--muted);
  white-space: pre-wrap;
  word-break: break-all;
  margin: 0;
  background: transparent;
  padding: 14px;
  border-radius: 0;
  border: none;
}

.delegated-result-list {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.delegated-result-card {
  border: none;
  background: transparent;
  padding: 0;
}

.delegated-result-card + .delegated-result-card {
  border-top: 1px solid var(--border-soft);
  padding-top: 10px;
}

.delegated-result-title {
  margin-bottom: 8px;
  color: var(--text);
  font-size: 0.86rem;
  font-weight: 600;
  font-style: normal;
}

.delegated-result-body {
  color: var(--muted);
  font-size: 0.84rem;
  line-height: 1.55;
}

:deep(.delegated-result-body pre) {
  white-space: pre-wrap;
}

:deep(.delegated-result-body table) {
  display: block;
  max-width: 100%;
  overflow-x: auto;
}

.ask-answer-list {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

/* The answer lives inside the params box: a divider + padding separate it from
   the params above, sharing one container. */
.code-block-wrapper .ask-answer-list {
  padding: 14px;
  border-top: 1px solid var(--border);
  /* Match the params (faint-code) above — same monospace font. */
  font-family: monospace;
}

.ask-answer-card {
  border: none;
  background: transparent;
  padding: 0;
}

.ask-answer-header {
  margin-bottom: 4px;
  color: var(--muted);
  font-size: 0.7rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.ask-answer-question {
  margin-bottom: 8px;
  color: var(--text);
  font-size: 0.86rem;
  font-weight: 600;
}

.ask-answer-choices {
  margin: 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.ask-answer-choices li {
  position: relative;
  padding-left: 18px;
  color: var(--text);
  font-size: 0.84rem;
  line-height: 1.45;
}

.ask-answer-choices li::before {
  content: '✓';
  position: absolute;
  left: 0;
  color: var(--success);
  font-weight: 700;
}

.ask-answer-empty {
  color: var(--muted);
  font-size: 0.82rem;
  font-style: italic;
}

.ask-answer-note {
  margin-top: 8px;
  color: var(--muted);
  font-size: 0.82rem;
  line-height: 1.5;
  font-style: italic;
}

.tool-running-shimmer {
  height: 24px;
  background: var(--plan-task-hover-bg);
  border-radius: 4px;
  overflow: hidden;
  position: relative;
}

.tool-running-shimmer::after {
  content: '';
  display: block;
  width: 100%;
  height: 100%;
  transform: translateX(-100%);
  background: linear-gradient(90deg, transparent, var(--shimmer-glow), transparent);
  animation: toolShimmer 1.5s infinite;
}

@keyframes toolShimmer {
  100% {
    transform: translateX(100%);
  }
}
</style>
