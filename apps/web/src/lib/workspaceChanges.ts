import type { RunMessage } from '@locagens/shared';
import { lineDiff, type DiffRow } from './diff';
import { parseToolCalls } from './messageGroups';

export type WorkspaceChangeKind = 'created' | 'edited' | 'deleted' | 'moved';

export interface WorkspaceChange {
  id: string;
  path: string;
  fullPath: string;
  sourceFullPath?: string;
  displayPath: string;
  kind: WorkspaceChangeKind;
  tool: string;
  oldText: string;
  newText: string;
  added: number;
  deleted: number;
  summary: string;
  transient?: boolean;
}

const CHANGE_TOOLS = new Set(['write_file', 'edit_file', 'delete_file', 'move_file']);

function parseJson(value?: string): any | null {
  if (!value) return null;
  try {
    return JSON.parse(value);
  } catch {
    return null;
  }
}

function normalizeChangePath(path: string): string {
  return path
    .replace(/\\/g, '/')
    .replace(/^\.\//, '')
    .replace(/\/+/g, '/')
    .replace(/\/$/, '');
}

function resolveFullPath(projectPath: string | undefined, relativePath: string): string {
  const normalized = normalizeChangePath(relativePath);
  if (normalized.startsWith('/') || /^[A-Za-z]:\//.test(normalized)) return normalized;
  if (!projectPath) return normalized;
  const base = projectPath.replace(/\\/g, '/').replace(/\/$/, '');
  return normalizeChangePath(`${base}/${normalized}`);
}

function commandCwd(command: string): string | undefined {
  const match = command.match(/(?:^|&&)\s*cd\s+(.+?)\s*&&/);
  return match?.[1]?.trim().replace(/^["']|["']$/g, '');
}

function commandOutputPath(command: string): string | undefined {
  const match = command.match(/(?:^|\s)(?:-o|--output)\s+("[^"]+"|'[^']+'|\S+)/);
  return match?.[1]?.trim().replace(/^["']|["']$/g, '');
}

function resolveCommandOutputPath(projectPath: string | undefined, command: string): string | undefined {
  if (!/\bdotnet\s+new\b/.test(command)) return undefined;
  const outputPath = commandOutputPath(command);
  if (!outputPath) return undefined;
  const cwd = commandCwd(command);
  if (outputPath.startsWith('/') || /^[A-Za-z]:[\\/]/.test(outputPath)) {
    return resolveFullPath(undefined, outputPath);
  }
  if (cwd) return resolveFullPath(cwd, outputPath);
  return resolveFullPath(projectPath, outputPath);
}

function isUnderRoot(fullPath: string, root: string): boolean {
  const path = normalizeChangePath(fullPath);
  const normalizedRoot = normalizeChangePath(root);
  return path === normalizedRoot || path.startsWith(`${normalizedRoot}/`);
}

function isSuccessfulToolResult(message?: RunMessage): boolean {
  if (!message) return false;
  const parsed = parseJson(message.content);
  return parsed?.success === true;
}

function diffStats(oldText: string, newText: string): { added: number; deleted: number } {
  const rows = lineDiff(oldText, newText);
  return {
    added: rows.filter(r => r.type === 'add').length,
    deleted: rows.filter(r => r.type === 'del').length
  };
}

function buildChange(id: string, tool: string, args: any, projectPath?: string): WorkspaceChange | null {
  if (tool === 'write_file') {
    const path = String(args.path || '');
    const newText = String(args.content ?? '');
    if (!path) return null;
    const stats = diffStats('', newText);
    return {
      id,
      path,
      fullPath: resolveFullPath(projectPath, path),
      displayPath: path,
      kind: 'created',
      tool,
      oldText: '',
      newText,
      ...stats,
      summary: 'Written'
    };
  }

  if (tool === 'edit_file') {
    const path = String(args.path || '');
    const oldText = String(args.old_string ?? '');
    const newText = String(args.new_string ?? '');
    if (!path) return null;
    const stats = diffStats(oldText, newText);
    return {
      id,
      path,
      fullPath: resolveFullPath(projectPath, path),
      displayPath: path,
      kind: 'edited',
      tool,
      oldText,
      newText,
      ...stats,
      summary: args.replace_all ? 'Edited all matches' : 'Edited'
    };
  }

  if (tool === 'delete_file') {
    const path = String(args.path || '');
    if (!path) return null;
    return {
      id,
      path,
      fullPath: resolveFullPath(projectPath, path),
      displayPath: path,
      kind: 'deleted',
      tool,
      oldText: '',
      newText: '',
      added: 0,
      deleted: 0,
      summary: 'Deleted'
    };
  }

  if (tool === 'move_file') {
    const source = String(args.source_path || '');
    const destination = String(args.destination_path || '');
    if (!source || !destination) return null;
    return {
      id,
      path: destination,
      fullPath: resolveFullPath(projectPath, destination),
      sourceFullPath: resolveFullPath(projectPath, source),
      displayPath: `${source} -> ${destination}`,
      kind: 'moved',
      tool,
      oldText: source,
      newText: destination,
      added: 0,
      deleted: 0,
      summary: 'Moved'
    };
  }

  return null;
}

export function collectWorkspaceChanges(messages: RunMessage[], projectPath?: string): WorkspaceChange[] {
  const changes: WorkspaceChange[] = [];
  const commandCreatedRoots: string[] = [];

  for (let i = 0; i < messages.length; i++) {
    const message = messages[i];
    if (message.role !== 'assistant') continue;

    const toolCalls = parseToolCalls(message);
    if (toolCalls.length === 0) continue;

    const toolResponses: RunMessage[] = [];
    let j = i + 1;
    while (j < messages.length && messages[j].role === 'tool') {
      toolResponses.push(messages[j]);
      j++;
    }

    toolCalls.forEach((toolCall: any, index: number) => {
      const tool = toolCall?.function?.name;
      if (!isSuccessfulToolResult(toolResponses[index])) return;

      const args = parseJson(toolCall?.function?.arguments) ?? {};
      if (tool === 'run_command') {
        const root = resolveCommandOutputPath(projectPath, String(args.command ?? ''));
        if (root) commandCreatedRoots.push(root);
        return;
      }

      if (!CHANGE_TOOLS.has(tool)) return;
      const change = buildChange(`${message.id}:${index}`, tool, args, projectPath);
      if (change?.kind === 'deleted' && commandCreatedRoots.some(root => isUnderRoot(change.fullPath, root))) {
        change.transient = true;
      }
      if (change) changes.push(change);
    });
  }

  return collapseTransientCreates(changes)
    .sort((a, b) => normalizeChangePath(a.fullPath).localeCompare(normalizeChangePath(b.fullPath)));
}

export function hasWorkspaceChangeSignals(messages: RunMessage[]): boolean {
  for (const message of messages) {
    if (message.role !== 'assistant') continue;
    // parseToolCalls is memoized per message, so this scan re-parses only the
    // message that changed since the previous side-panel snapshot.
    const toolCalls = parseToolCalls(message);
    if (toolCalls.some((toolCall: any) => CHANGE_TOOLS.has(toolCall?.function?.name))) {
      return true;
    }
  }
  return false;
}

export function changeDiffRows(change: WorkspaceChange): DiffRow[] {
  if (change.kind === 'edited' || change.kind === 'created') {
    return lineDiff(change.oldText, change.newText);
  }
  return [];
}

function collapseTransientCreates(changes: WorkspaceChange[]): WorkspaceChange[] {
  const createdIndexByPath = new Map<string, number>();
  const transientPaths = new Set<string>();

  changes.forEach((change, index) => {
    if (change.transient) {
      transientPaths.add(normalizeChangePath(change.fullPath));
      return;
    }
    const path = normalizeChangePath(change.fullPath);
    if (change.kind === 'created' && !createdIndexByPath.has(path)) {
      createdIndexByPath.set(path, index);
    }
    if (change.kind === 'deleted') {
      const createdIndex = createdIndexByPath.get(path);
      if (createdIndex !== undefined && createdIndex < index) {
        transientPaths.add(path);
      }
    }
  });

  if (transientPaths.size === 0) return changes;

  return changes.filter(change => !transientPaths.has(normalizeChangePath(change.fullPath)));
}
