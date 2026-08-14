import { computed, ref, watch, type Ref } from 'vue';
import type { Run, Project } from '@locagens/shared';
import { api } from '../api/client';
import { STORAGE_KEYS } from '../lib/storageKeys';
import { DEFAULT_PROJECT_PATH } from '../lib/format';
import { useCustomDialog } from './useCustomDialog';

/**
 * Owns the project list, the active project selection and the add/remove
 * project flow. Run counts are derived from the passed-in runs ref.
 */
export function useProjects(runs: Ref<Run[]>) {
  const { showAlert, showConfirm } = useCustomDialog();
  const projects = ref<Project[]>([]);
  const activeProjectPath = ref(localStorage.getItem(STORAGE_KEYS.activeProjectPath) || DEFAULT_PROJECT_PATH);

  watch(activeProjectPath, (newVal) => {
    localStorage.setItem(STORAGE_KEYS.activeProjectPath, newVal);
  });

  const showAddProjectModal = ref(false);
  const newProjectName = ref('');
  const newProjectPath = ref('');
  const isSubmittingProject = ref(false);

  const projectOptions = computed(() => {
    return projects.value
      .map(p => ({
        path: p.path,
        name: p.name,
        count: runs.value.filter(run => (run.projectPath || DEFAULT_PROJECT_PATH) === p.path).length
      }))
      .sort((a, b) => a.name.localeCompare(b.name));
  });

  const activeProject = computed(() => {
    return projectOptions.value.find(p => p.path === activeProjectPath.value) || projectOptions.value[0];
  });

  async function loadProjects() {
    const data = await api.getProjects();
    if (data) projects.value = data;
  }

  async function browseFolder() {
    try {
      const data = await api.browseFolder();
      newProjectPath.value = data.path;
      newProjectName.value = data.name;
    } catch (err: any) {
      await showAlert(err.message || 'Failed to open folder selection dialog.');
    }
  }

  /** Creates a project and returns its path, or null if validation/submit fails. */
  async function submitNewProject(): Promise<string | null> {
    if (!newProjectPath.value.trim()) return null;
    isSubmittingProject.value = true;
    try {
      const added = await api.createProject(newProjectPath.value.trim(), newProjectName.value.trim());
      await loadProjects();
      closeAddProjectModal();
      return added.path;
    } catch (err: any) {
      await showAlert(err.message || 'An error occurred while saving the project.');
      return null;
    } finally {
      isSubmittingProject.value = false;
    }
  }

  /** Removes a project; returns the fallback path to select if the active one was deleted. */
  async function deleteProject(projectPath: string): Promise<string | null> {
    if (projectOptions.value.length <= 1) {
      await showAlert('At least one project must be defined.');
      return null;
    }
    if (!(await showConfirm('Are you sure you want to remove this project from the list? (Existing chat history will not be deleted)'))) {
      return null;
    }

    try {
      await api.deleteProject(projectPath);
      await loadProjects();
      if (activeProjectPath.value === projectPath) {
        return projectOptions.value[0]?.path || '';
      }
    } catch (err) {
      console.error(err);
    }
    return null;
  }

  function openAddProjectModal() {
    newProjectName.value = '';
    newProjectPath.value = '';
    showAddProjectModal.value = true;
  }

  function closeAddProjectModal() {
    showAddProjectModal.value = false;
  }

  return {
    projects,
    activeProjectPath,
    projectOptions,
    activeProject,
    showAddProjectModal,
    newProjectName,
    newProjectPath,
    isSubmittingProject,
    loadProjects,
    browseFolder,
    submitNewProject,
    deleteProject,
    openAddProjectModal,
    closeAddProjectModal
  };
}
