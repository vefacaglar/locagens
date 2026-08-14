import type { PermissionPreview } from '@locagens/shared';
import type { PermissionDecision } from '../api/client';
import { lineDiff, type DiffRow } from './diff';

export interface PermissionOption {
  decision: PermissionDecision;
  label: string;
  hint?: string;
}

/** Approval options, in Claude Code's numbered order. */
export const PERMISSION_OPTIONS: PermissionOption[] = [
  { decision: 'allow_once', label: 'Yes' },
  { decision: 'allow_project', label: "Yes, don't ask again in this project" },
  { decision: 'allow_always', label: 'Yes, allow always (global)' },
  { decision: 'allow_run', label: 'Yes, and allow this exact action for this run' },
  { decision: 'deny', label: 'No, tell Locagens what to do differently', hint: 'esc' }
];

export function actionLabel(action?: string): string {
  switch (action) {
    case 'create': return 'Create';
    case 'edit': return 'Edit';
    case 'delete': return 'Delete';
    case 'read': return 'Read';
    case 'list': return 'List';
    case 'mkdir': return 'New folder';
    case 'move': return 'Move';
    case 'search': return 'Search';
    case 'command': return 'Run command';
    case 'fetch': return 'Fetch URL';
    default: return 'Run';
  }
}

export function actionQuestion(preview: PermissionPreview | null | undefined, toolName?: string): string {
  if (!preview) return `Do you want to run ${toolName ?? 'this tool'}?`;
  const target = preview.path || preview.absolutePath;
  switch (preview.action) {
    case 'create': return `Do you want to create ${target}?`;
    case 'edit': return `Do you want to make this edit to ${target}?`;
    case 'delete': return `Do you want to delete ${target}?`;
    case 'read': return `Do you want to read ${target}?`;
    case 'list': return `Do you want to list ${target}?`;
    case 'mkdir': return `Do you want to create the folder ${target}?`;
    case 'move': return `Do you want to move ${target} to ${preview.destPath ?? ''}?`;
    case 'search': return toolName === 'search_web'
      ? `Do you want to search the web for "${preview.query ?? ''}"?`
      : `Do you want to search for "${preview.query ?? ''}"?`;
    case 'command': return 'Do you want to run this command?';
    case 'fetch': return `Do you want to fetch ${preview.url ?? ''}?`;
    default: return 'Do you want to proceed?';
  }
}

/** The diff rows to render for a preview (empty for read/list). */
export function previewDiff(preview: PermissionPreview | null | undefined): DiffRow[] {
  if (!preview) return [];
  switch (preview.action) {
    case 'edit': return lineDiff(preview.oldContent ?? '', preview.newContent ?? '');
    case 'create': return lineDiff('', preview.newContent ?? '');
    case 'delete': return lineDiff(preview.oldContent ?? '', '');
    default: return [];
  }
}
