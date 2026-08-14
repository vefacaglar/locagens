/**
 * Central registry of localStorage keys (exact strings preserved from the
 * original call sites). The composer-settings namespace (bm_run_*, bm_draft_*,
 * bm_last_used_*) is intentionally NOT here — it is owned by
 * composables/useComposerSettings.ts and written only through its helpers.
 */
export const STORAGE_KEYS = {
  activeRunId: 'activeRunId',
  activeProjectPath: 'activeProjectPath',
  sidePanelWidth: 'sidePanelWidth',
  expandedProjects: 'expandedProjects',
  sidebarReadRunStamps: 'sidebarReadRunStamps',
  favoriteModels: 'bm_favorite_models'
} as const;

export const runStorageKeys = {
  sidePanelCollapsed: (runId: string) => `sidePanelCollapsed:${runId}`,
  panelState: (runId: string) => `runPanelState:${runId}`
};
