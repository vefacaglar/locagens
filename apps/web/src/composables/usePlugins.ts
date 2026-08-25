import { ref } from 'vue';
import type {
  PluginManifest,
  PluginTemplate,
  InstallPluginPayload,
  PluginScope
} from '@locagens/shared';
import { api } from '../api/client';

export function usePlugins() {
  const plugins = ref<PluginManifest[]>([]);
  const templates = ref<PluginTemplate[]>([]);
  const userPluginsDir = ref<string>('');
  const projectPluginsDir = ref<string | null>(null);
  const isLoading = ref(false);
  const isInstalling = ref(false);
  const error = ref<string | null>(null);
  const statusMessage = ref<string | null>(null);

  function clearMessages() {
    error.value = null;
    statusMessage.value = null;
  }

  async function loadPlugins(projectPath?: string) {
    isLoading.value = true;
    clearMessages();
    try {
      const res = await api.getPlugins(projectPath);
      plugins.value = res?.plugins ?? [];
      templates.value = res?.templates ?? [];
      userPluginsDir.value = res?.userPluginsDir ?? '';
      projectPluginsDir.value = res?.projectPluginsDir ?? null;
    } catch (err: any) {
      error.value = err?.message || 'Failed to load plugins.';
    } finally {
      isLoading.value = false;
    }
  }

  async function installPlugin(payload: InstallPluginPayload) {
    isInstalling.value = true;
    clearMessages();
    try {
      const res = await api.installPlugin(payload);
      statusMessage.value = `Plugin "${res.plugin.name}" installed successfully.`;
      await loadPlugins(payload.projectPath);
      return res.plugin;
    } catch (err: any) {
      error.value = err?.message || 'Failed to install plugin.';
      throw err;
    } finally {
      isInstalling.value = false;
    }
  }

  async function togglePlugin(id: string, enabled: boolean, scope?: PluginScope, projectPath?: string) {
    clearMessages();
    try {
      await api.togglePlugin(id, enabled, scope, projectPath);
      statusMessage.value = `Plugin "${id}" ${enabled ? 'enabled' : 'disabled'}.`;
      await loadPlugins(projectPath);
    } catch (err: any) {
      error.value = err?.message || 'Failed to toggle plugin.';
      throw err;
    }
  }

  async function deletePlugin(id: string, scope?: PluginScope, projectPath?: string) {
    clearMessages();
    try {
      await api.deletePlugin(id, scope, projectPath);
      statusMessage.value = `Plugin "${id}" deleted.`;
      await loadPlugins(projectPath);
    } catch (err: any) {
      error.value = err?.message || 'Failed to delete plugin.';
      throw err;
    }
  }

  return {
    plugins,
    templates,
    userPluginsDir,
    projectPluginsDir,
    isLoading,
    isInstalling,
    error,
    statusMessage,
    loadPlugins,
    installPlugin,
    togglePlugin,
    deletePlugin,
    clearMessages
  };
}
