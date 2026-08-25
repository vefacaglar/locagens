import { ref } from 'vue';
import type { SkillSummary } from '@locagens/shared';
import { api } from '../api/client';

/**
 * Owns the skills list shown in Settings → Skills. Skills are installed by
 * picking a SKILL.md file; the API writes it under the user/project skills root.
 */
export function useSkills() {
  const skills = ref<SkillSummary[]>([]);
  const userRoot = ref('');
  const projectRoot = ref<string | null>(null);
  const isLoading = ref(false);
  const isInstalling = ref(false);
  const error = ref<string | null>(null);
  const statusMessage = ref<string | null>(null);

  async function loadSkills(projectPath?: string) {
    isLoading.value = true;
    error.value = null;
    try {
      const data = await api.getSkills(projectPath);
      // getJson returns null on network/auth failure — surface that in the UI.
      if (!data) {
        error.value = 'Failed to load skills. Is the API running (pnpm dev)?';
        skills.value = [];
        userRoot.value = '';
        projectRoot.value = null;
        return;
      }
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

  async function installSkill(
    target: 'user' | 'project',
    content: string,
    projectPath?: string
  ): Promise<boolean> {
    isInstalling.value = true;
    statusMessage.value = null;
    error.value = null;
    try {
      const result = await api.installSkill({
        target,
        content,
        projectPath: target === 'project' ? projectPath : undefined
      });
      statusMessage.value = `Installed skill “${result.skill.name}”.`;
      await loadSkills(projectPath);
      return true;
    } catch (err: any) {
      error.value = err?.message ?? 'Failed to install skill.';
      return false;
    } finally {
      isInstalling.value = false;
    }
  }

  async function installSkillFile(
    target: 'user' | 'project',
    file: File,
    projectPath?: string
  ): Promise<boolean> {
    const name = file.name || '';
    if (!name.toLowerCase().endsWith('.md') && name.toLowerCase() !== 'skill.md') {
      // Still allow any text file named SKILL.md variants; reject obvious binaries.
      if (!/\.md$/i.test(name) && name !== '') {
        error.value = 'Choose a SKILL.md markdown file.';
        return false;
      }
    }
    if (file.size > 120_000) {
      error.value = 'File is too large (max ~120KB).';
      return false;
    }
    const content = await file.text();
    return installSkill(target, content, projectPath);
  }

  return {
    skills,
    userRoot,
    projectRoot,
    isLoading,
    isInstalling,
    error,
    statusMessage,
    loadSkills,
    installSkill,
    installSkillFile
  };
}
