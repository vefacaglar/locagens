import { ref, computed, watch, onBeforeUnmount } from 'vue';
import type { ProcessInfo, ProcessLogEntry } from '@locagens/shared';
import { api } from '../api/client';

export function useProcesses(activeProjectPath: () => string | null | undefined) {
  const processes = ref<ProcessInfo[]>([]);
  const activeProcessId = ref<string | null>(null);
  const logs = ref<ProcessLogEntry[]>([]);
  const isLoading = ref(false);
  const isSpawning = ref(false);
  const error = ref<string | null>(null);

  let pollInterval: ReturnType<typeof setInterval> | null = null;

  const activeProcess = computed(() => {
    return processes.value.find((p) => p.id === activeProcessId.value) || null;
  });

  const runningCount = computed(() => {
    return processes.value.filter((p) => p.status === 'running').length;
  });

  async function loadProcesses() {
    try {
      const path = activeProjectPath() || undefined;
      const res = await api.getProcesses(path);
      processes.value = res?.processes || [];

      // Auto-select first process if none selected or selected was deleted
      if (processes.value.length > 0) {
        if (!activeProcessId.value || !processes.value.some((p) => p.id === activeProcessId.value)) {
          activeProcessId.value = processes.value[0].id;
          void loadLogs(activeProcessId.value);
        }
      } else {
        activeProcessId.value = null;
        logs.value = [];
      }
    } catch (err: any) {
      error.value = err?.message || 'Failed to load processes.';
    }
  }

  async function loadLogs(processId?: string | null) {
    const targetId = processId || activeProcessId.value;
    if (!targetId) return;

    try {
      const res = await api.getProcessLogs(targetId);
      if (res?.logs) {
        logs.value = res.logs;
      }
      if (res?.process) {
        const idx = processes.value.findIndex((p) => p.id === targetId);
        if (idx >= 0) {
          processes.value[idx] = res.process;
        }
      }
    } catch {
      /* ignore poll errors */
    }
  }

  function selectProcess(id: string) {
    activeProcessId.value = id;
    logs.value = [];
    void loadLogs(id);
  }

  async function spawn(command: string) {
    const trimmed = command.trim();
    if (!trimmed) return;

    isSpawning.value = true;
    error.value = null;

    try {
      const path = activeProjectPath() || undefined;
      const res = await api.spawnProcess(trimmed, path);
      if (res?.process) {
        processes.value.unshift(res.process);
        activeProcessId.value = res.process.id;
        void loadLogs(res.process.id);
      }
    } catch (err: any) {
      error.value = err?.message || 'Failed to start process.';
    } finally {
      isSpawning.value = false;
    }
  }

  async function kill(id: string) {
    try {
      await api.killProcess(id);
      const proc = processes.value.find((p) => p.id === id);
      if (proc) {
        proc.status = 'stopped';
      }
      void loadLogs(id);
    } catch (err: any) {
      error.value = err?.message || 'Failed to stop process.';
    }
  }

  async function restart(id: string) {
    try {
      const res = await api.restartProcess(id);
      if (res?.process) {
        const idx = processes.value.findIndex((p) => p.id === id);
        if (idx >= 0) processes.value[idx] = res.process;
        if (activeProcessId.value === id) {
          void loadLogs(id);
        }
      }
    } catch (err: any) {
      error.value = err?.message || 'Failed to restart process.';
    }
  }

  function startPolling() {
    stopPolling();
    void loadProcesses();
    if (activeProcessId.value) void loadLogs(activeProcessId.value);
    pollInterval = setInterval(() => {
      void loadProcesses();
      if (activeProcessId.value) void loadLogs(activeProcessId.value);
    }, 1500);
  }

  function stopPolling() {
    if (pollInterval) {
      clearInterval(pollInterval);
      pollInterval = null;
    }
  }

  watch(
    () => activeProjectPath(),
    () => {
      void loadProcesses();
    }
  );

  onBeforeUnmount(() => {
    stopPolling();
  });

  return {
    processes,
    activeProcessId,
    activeProcess,
    logs,
    runningCount,
    isLoading,
    isSpawning,
    error,
    loadProcesses,
    loadLogs,
    selectProcess,
    spawn,
    kill,
    restart,
    startPolling,
    stopPolling
  };
}
