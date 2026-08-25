import { ref } from 'vue';
import type { McpServerConfig, McpServerInfo } from '@locagens/shared';
import { api } from '../api/client';

export function useMcpServers() {
  const servers = ref<McpServerInfo[]>([]);
  const isLoading = ref(false);
  const isSaving = ref(false);
  const error = ref<string | null>(null);
  const statusMessage = ref<string | null>(null);

  function clearMessages() {
    error.value = null;
    statusMessage.value = null;
  }

  async function loadServers(projectPath?: string) {
    isLoading.value = true;
    clearMessages();
    try {
      const res = await api.getMcpServers(projectPath);
      servers.value = res?.servers ?? [];
    } catch (err: any) {
      error.value = err?.message || 'Failed to load MCP servers.';
    } finally {
      isLoading.value = false;
    }
  }

  async function saveServer(config: McpServerConfig, projectPath?: string) {
    isSaving.value = true;
    clearMessages();
    try {
      const res = await api.saveMcpServer({
        ...config,
        projectPath: config.scope === 'project' ? projectPath : undefined
      });
      statusMessage.value = `Server "${config.name}" saved successfully.`;
      await loadServers(projectPath);
      return res.server;
    } catch (err: any) {
      error.value = err?.message || 'Failed to save MCP server.';
      throw err;
    } finally {
      isSaving.value = false;
    }
  }

  async function deleteServer(name: string, projectPath?: string) {
    clearMessages();
    try {
      await api.deleteMcpServer(name, projectPath);
      statusMessage.value = `Server "${name}" deleted.`;
      await loadServers(projectPath);
    } catch (err: any) {
      error.value = err?.message || 'Failed to delete MCP server.';
      throw err;
    }
  }

  async function restartServer(name: string, projectPath?: string) {
    clearMessages();
    try {
      await api.restartMcpServer(name, projectPath);
      statusMessage.value = `Server "${name}" reconnected.`;
      await loadServers(projectPath);
    } catch (err: any) {
      error.value = err?.message || 'Failed to restart MCP server.';
      throw err;
    }
  }

  async function toggleServer(name: string, enabled: boolean, projectPath?: string) {
    clearMessages();
    try {
      await api.toggleMcpServer(name, enabled, projectPath);
      statusMessage.value = `Server "${name}" ${enabled ? 'enabled' : 'disabled'}.`;
      await loadServers(projectPath);
    } catch (err: any) {
      error.value = err?.message || 'Failed to toggle MCP server.';
      throw err;
    }
  }

  return {
    servers,
    isLoading,
    isSaving,
    error,
    statusMessage,
    loadServers,
    saveServer,
    deleteServer,
    restartServer,
    toggleServer,
    clearMessages
  };
}
