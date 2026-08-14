import type { Memory, MemoryCategory, MemoryScope } from '@locagens/shared';
import { api } from '../api/client';
import { useAsyncResource } from './useAsyncResource';
import { useCustomDialog } from './useCustomDialog';

/**
 * Owns the saved-memories list shown in Settings → Memory. Memories are the
 * durable facts the assistant recorded via the `remember` tool (plus any the
 * user adds by hand). Loaded lazily when the settings modal opens.
 */
export function useMemories() {
  const { showConfirm } = useCustomDialog();
  const { data: memories, isLoading, load: loadMemories } = useAsyncResource<Memory[]>(
    () => api.getMemories(),
    []
  );

  async function addMemory(payload: { scope: MemoryScope; category: MemoryCategory; content: string; projectPath?: string }) {
    const created = await api.createMemory(payload);
    memories.value = [created, ...memories.value];
  }

  async function updateMemory(id: number, content: string) {
    const updated = await api.updateMemory(id, content);
    memories.value = memories.value.map(m => (m.id === id ? updated : m));
  }

  async function deleteMemory(id: number) {
    await api.deleteMemory(id);
    memories.value = memories.value.filter(m => m.id !== id);
  }

  async function clearMemories() {
    if (memories.value.length === 0) return;
    if (!(await showConfirm('Are you sure you want to delete all saved memories?'))) return;
    await api.clearMemories();
    memories.value = [];
  }

  return {
    memories,
    isLoading,
    loadMemories,
    addMemory,
    updateMemory,
    deleteMemory,
    clearMemories
  };
}
