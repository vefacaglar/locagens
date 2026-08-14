import type { PermissionRule } from '@locagens/shared';
import { api } from '../api/client';
import { useAsyncResource } from './useAsyncResource';
import { useCustomDialog } from './useCustomDialog';

/**
 * Owns the standing-permissions list shown in Settings → Permissions.
 * Loaded lazily when the settings screen opens (see useAppShell.openSettings).
 */
export function usePermissions() {
  const { showConfirm } = useCustomDialog();
  const { data: permissions, isLoading, load: loadPermissions } = useAsyncResource<PermissionRule[]>(
    () => api.getPermissions(),
    []
  );

  async function revokePermission(id: number) {
    await api.revokePermission(id);
    permissions.value = permissions.value.filter(p => p.id !== id);
  }

  async function clearPermissions() {
    if (permissions.value.length === 0) return;
    if (!(await showConfirm('Are you sure you want to remove all saved permissions?'))) return;
    await api.clearPermissions();
    permissions.value = [];
  }

  return {
    permissions,
    isLoading,
    loadPermissions,
    revokePermission,
    clearPermissions
  };
}
