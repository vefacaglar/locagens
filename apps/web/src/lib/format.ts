import type { RunStatus } from '@locagens/shared';

/** Run statuses that mean a run is still actively processing. */
export const ACTIVE_STATUSES: RunStatus[] = ['created', 'generating', 'awaiting_permission', 'awaiting_input'];

/** Default workspace path used when a run has no explicit project path. */
// TODO: hardcoded developer path; should come from config or the projects list.
export const DEFAULT_PROJECT_PATH = '/Users/vefa/Projects/agent-bridge';

export function formatTime(value: string): string {
  return new Date(value).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

/** Last path segment; trailing slashes ignored. Returns the input if empty. */
export function basename(path: string): string {
  const parts = path.replace(/\/+$/, '').split('/');
  return parts[parts.length - 1] || path;
}

/** Pretty-prints a JSON string; returns the original text if it is not JSON. */
export function formatJson(content: string): string {
  try {
    return JSON.stringify(JSON.parse(content), null, 2);
  } catch (e) {
    return content;
  }
}

/** Splits a "providerId:model" combined value into its parts. */
export function splitCombined(value: string): { providerId: string; model: string } {
  const [providerId, ...modelParts] = value.split(':');
  return { providerId, model: modelParts.join(':') };
}
