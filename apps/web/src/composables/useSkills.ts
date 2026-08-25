import { ref } from 'vue';
import type { SkillSummary } from '@locagens/shared';
import { api } from '../api/client';

/**
 * Owns the skills list shown in Settings → Skills. Skills live as SKILL.md
 * folders on disk; this composable only lists them and opens the folders.
 */
export function useSkills() {
  const skills = ref<SkillSummary[]>([]);
  const userRoot = ref('');
  const projectRoot = ref<string | null>(null);
  const isLoading = ref(false);
  const error = ref<string | null>(null);
  const statusMessage = ref<string | null>(null);

  async function loadSkills(projectPath?: string) {
    isLoading.value = true;
    error.value = null;
    try {
      const data = await api.getSkills(projectPath);
      skills.value = data.skills ?? [];
      userRoot.value = data.roots?.user ?? '';
      projectRoot.value = data.roots?.project ?? null;
    } catch (err: any) {
      error.value = err?.message ?? 'Failed to load skills.';
      skills.value = [];
    } finally {
      isLoading.value = false;
    }
  }

  async function openFolder(target: 'user' | 'project', projectPath?: string) {
    statusMessage.value = null;
    error.value = null;
    try {
      const result = await api.openSkillsFolder(target, projectPath);
      if (result.opened) {
        statusMessage.value = `Opened ${result.path}`;
      } else if (result.copied) {
        statusMessage.value = `Path copied: ${result.path}`;
      } else {
        statusMessage.value = result.path;
      }
      return result.path;
    } catch (err: any) {
      error.value = err?.message ?? 'Failed to open skills folder.';
      return null;
    }
  }

  return {
    skills,
    userRoot,
    projectRoot,
    isLoading,
    error,
    statusMessage,
    loadSkills,
    openFolder
  };
}
