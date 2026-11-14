//! Permissions tab (UX-016): standing grants as sentences, grouped under
//! "Everywhere" and one heading per project, with `Revoke` and a five-second
//! `Undo`.

use crate::state_ui::{core, UiStore};
use dioxus::prelude::*;
use locagens_core::db::PermissionRule;

/// The sentence of one grant: `(i18n template key, argument)`.
pub fn grant_sentence(rule: &PermissionRule) -> (&'static str, String) {
    let deny = rule.status == "deny";
    let domains = rule.network_domains.join(", ");
    if !domains.is_empty() {
        return (if deny { "perms.deny_network" } else { "perms.allow_network" }, domains);
    }
    if !rule.command.trim().is_empty() {
        return (if deny { "perms.deny_command" } else { "perms.allow_command" }, rule.command.clone());
    }
    (if deny { "perms.deny_tool" } else { "perms.allow_tool" }, rule.tool.clone())
}

/// Grants grouped by project (`""` = everywhere first), in stored order.
pub fn group_rules(rules: &[PermissionRule]) -> Vec<(String, Vec<PermissionRule>)> {
    let mut groups: Vec<(String, Vec<PermissionRule>)> = Vec::new();
    let mut sorted: Vec<&PermissionRule> = rules.iter().collect();
    sorted.sort_by_key(|rule| !rule.project_path.is_empty());
    for rule in sorted {
        match groups.iter_mut().find(|(path, _)| *path == rule.project_path) {
            Some((_, list)) => list.push(rule.clone()),
            None => groups.push((rule.project_path.clone(), vec![rule.clone()])),
        }
    }
    groups
}

#[component]
pub(super) fn PermissionsTab() -> Element {
    let rules: Signal<Vec<PermissionRule>> = use_signal(Vec::new);
    // Rules revoked in the last five seconds (shown with Undo).
    let revoked: Signal<Vec<PermissionRule>> = use_signal(Vec::new);
    let store: UiStore = use_context();

    use_effect(move || {
        let mut rules = rules;
        spawn(async move {
            rules.set(locagens_core::api::permissions::list_permissions(&core()).await.unwrap_or_default());
        });
    });

    let mut shown = rules.read().clone();
    shown.extend(revoked.read().iter().cloned());
    if shown.is_empty() {
        return rsx! {
            div { class: "settings-empty", "{store.t(\"perms.none\")}" }
        };
    }
    rsx! {
        for (project, list) in group_rules(&shown) {
            div {
                key: "{project}",
                class: "panel-section-title",
                title: "{project}",
                if project.is_empty() { "{store.t(\"perms.everywhere\")}" } else { "{project}" }
            }
            for rule in list {
                GrantRow { key: "{rule.id}", rule: rule.clone(), rules, revoked }
            }
        }
    }
}

#[component]
fn GrantRow(rule: PermissionRule, rules: Signal<Vec<PermissionRule>>, revoked: Signal<Vec<PermissionRule>>) -> Element {
    let store: UiStore = use_context();
    let (template, argument) = grant_sentence(&rule);
    let is_revoked = revoked.read().iter().any(|r| r.id == rule.id);
    let rule_revoke = rule.clone();
    let rule_undo = rule.clone();
    let sentence = store.tf(template, &[&argument]);
    rsx! {
        div {
            class: if is_revoked { "settings-row revoked" } else { "settings-row" },
            div { class: "grow grant-sentence", "{sentence}" }
            if is_revoked {
                span { class: "muted", "{store.t(\"perms.revoked\")}" }
                button {
                    class: "subtle small",
                    onclick: move |_| {
                        let rule = rule_undo.clone();
                        let mut rules = rules;
                        let mut revoked = revoked;
                        spawn(async move {
                            let state = core();
                            if let Err(err) = locagens_core::api::permissions::restore_permission(&state, &rule).await {
                                store.toast(store.tf("toast.delete_rule_failed", &[&err.to_string()]));
                            }
                            revoked.with_mut(|list| list.retain(|r| r.id != rule.id));
                            rules.set(locagens_core::api::permissions::list_permissions(&state).await.unwrap_or_default());
                        });
                    },
                    "{store.t(\"perms.undo\")}"
                }
            } else {
                button {
                    class: "danger subtle small",
                    onclick: move |_| {
                        let rule = rule_revoke.clone();
                        let mut rules = rules;
                        let mut revoked = revoked;
                        spawn(async move {
                            let state = core();
                            if let Err(err) = locagens_core::api::permissions::delete_permission(&state, rule.id).await {
                                store.toast(store.tf("toast.delete_rule_failed", &[&err.to_string()]));
                                return;
                            }
                            revoked.with_mut(|list| list.push(rule.clone()));
                            rules.set(locagens_core::api::permissions::list_permissions(&state).await.unwrap_or_default());
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                            revoked.with_mut(|list| list.retain(|r| r.id != rule.id));
                        });
                    },
                    "{store.t(\"perms.revoke\")}"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: i64, project: &str, tool: &str, command: &str, domains: &[&str], status: &str) -> PermissionRule {
        PermissionRule {
            id,
            scope: if project.is_empty() { "global".into() } else { "project".into() },
            project_path: project.into(),
            tool: tool.into(),
            command: command.into(),
            network_domains: domains.iter().map(|d| d.to_string()).collect(),
            status: status.into(),
        }
    }

    #[test]
    fn grants_read_as_sentences() {
        assert_eq!(grant_sentence(&rule(1, "", "run_command", "cargo test *", &[], "allow")), ("perms.allow_command", "cargo test *".into()));
        assert_eq!(grant_sentence(&rule(2, "", "fetch_url", "", &["docs.rs"], "allow")), ("perms.allow_network", "docs.rs".into()));
        assert_eq!(grant_sentence(&rule(3, "", "edit_file", "", &[], "deny")), ("perms.deny_tool", "edit_file".into()));
    }

    #[test]
    fn everywhere_comes_first_then_projects() {
        let rules = vec![rule(1, "/p", "t", "", &[], "allow"), rule(2, "", "t", "", &[], "allow"), rule(3, "/p", "t", "", &[], "allow")];
        let groups = group_rules(&rules);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "");
        assert_eq!(groups[1].1.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1, 3]);
    }
}
