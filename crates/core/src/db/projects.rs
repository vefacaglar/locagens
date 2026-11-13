//! Projects (`projects.json`).

use super::*;

impl Db {
    // ------------------------------------------------------------------
    // Projects
    // ------------------------------------------------------------------

    pub fn list_projects(&self) -> Vec<Project> {
        self.sync_shared_files(true, false, false);
        self.read().projects.clone()
    }

    pub fn get_project(&self, path: &str) -> Option<Project> {
        self.sync_shared_files(true, false, false);
        self.read()
            .projects
            .iter()
            .find(|project| project.path == path)
            .cloned()
    }

    pub fn create_project(&self, path: &str, name: &str) -> Project {
        self.with_shared_mutation(|state| {
            let mut updated: Option<Project> = None;
            for existing in state.projects.iter_mut() {
                if existing.path == path {
                    existing.name = name.to_string();
                    existing.created_at = now_iso();
                    updated = Some(existing.clone());
                    break;
                }
            }
            if let Some(existing) = updated {
                persist_projects(&self.root, &state.projects);
                return existing;
            }
            let project = Project {
                path: path.to_string(),
                name: name.to_string(),
                created_at: now_iso(),
            };
            state.projects.push(project.clone());
            persist_projects(&self.root, &state.projects);
            project
        })
    }

    pub fn delete_project(&self, path: &str) {
        self.with_shared_mutation(|state| {
            let before = state.projects.len();
            state.projects.retain(|project| project.path != path);
            if state.projects.len() != before {
                persist_projects(&self.root, &state.projects);
            }
        })
    }
}

pub(super) fn persist_projects(root: &Path, projects: &[Project]) {
    write_json_atomic(&root.join("projects.json"), &projects);
}
