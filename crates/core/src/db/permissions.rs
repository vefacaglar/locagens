//! Standing permission grants (`permissions.json`).

use super::*;

impl Db {
    // ------------------------------------------------------------------
    // Permissions
    // ------------------------------------------------------------------

    pub fn list_permissions(&self) -> Vec<PermissionRule> {
        self.sync_shared_files(false, true, false);
        let mut list = self.read().permissions.clone();
        list.sort_by_key(|rule| rule.id);
        list
    }

    /// Inserts a standing approval, deduplicating on
    /// `(scope, project_path, tool, command, network_domains)`.
    pub fn upsert_permission(
        &self,
        scope: &str,
        project_path: &str,
        tool: &str,
        command: &str,
        network_domains: Vec<String>,
        status: &str,
    ) -> PermissionRule {
        self.with_shared_mutation(|state| {
            let mut sorted_domains = network_domains.clone();
            sorted_domains.sort();
            let mut updated: Option<PermissionRule> = None;
            for existing in state.permissions.iter_mut() {
                let mut domains = existing.network_domains.clone();
                domains.sort();
                if existing.scope == scope
                    && existing.project_path == project_path
                    && existing.tool == tool
                    && existing.command == command
                    && domains == sorted_domains
                {
                    existing.status = status.to_string();
                    updated = Some(existing.clone());
                    break;
                }
            }
            if let Some(existing) = updated {
                persist_permissions(&self.root, &state.permissions);
                return existing;
            }
            let rule = PermissionRule {
                id: state.next_permission_id,
                scope: scope.to_string(),
                project_path: project_path.to_string(),
                tool: tool.to_string(),
                command: command.to_string(),
                network_domains,
                status: status.to_string(),
            };
            state.next_permission_id += 1;
            state.permissions.push(rule.clone());
            persist_permissions(&self.root, &state.permissions);
            rule
        })
    }

    pub fn delete_permission(&self, id: i64) -> bool {
        self.with_shared_mutation(|state| {
            let before = state.permissions.len();
            state.permissions.retain(|rule| rule.id != id);
            if state.permissions.len() != before {
                persist_permissions(&self.root, &state.permissions);
                true
            } else {
                false
            }
        })
    }

    pub fn clear_permissions(&self) {
        self.with_shared_mutation(|state| {
            if !state.permissions.is_empty() {
                state.permissions.clear();
                persist_permissions(&self.root, &state.permissions);
            }
        })
    }
}

pub(super) fn persist_permissions(root: &Path, permissions: &[PermissionRule]) {
    write_json_atomic(&root.join("permissions.json"), &permissions);
}
