//! Permissions tab: standing grants.

use crate::state_ui::{core, UiStore};
use dioxus::prelude::*;
use locagens_core::db::PermissionRule;

/// Stored permission rules (standing grants) with delete.
#[component]
pub(super) fn PermissionsTab() -> Element {
    let rules: Signal<Vec<PermissionRule>> = use_signal(Vec::new);
    let store: UiStore = use_context();

    use_effect(move || {
        let mut rules = rules;
        spawn(async move {
            let state = core();
            rules.set(locagens_core::api::permissions::list_permissions(&state).await.unwrap_or_default());
        });
    });

    let data = rules.read().clone();
    if data.is_empty() {
        return rsx! {
            div { class: "settings-empty", "{store.t(\"perms.none\")}" }
        };
    }
    rsx! {
        for rule in data.iter() {
            {
                let id = rule.id;
                rsx! {
                    div {
                        key: "{id}",
                        class: "settings-row",
                        div {
                            class: "grow",
                            div {
                                style: "font-weight:600",
                                "{rule.tool} "
                                span { class: "badge", "{rule.status}" }
                            }
                            div { class: "mono", style: "font-size:11px", "{rule.command} · {rule.project_path}" }
                        }
                        button {
                            class: "danger",
                            onclick: move |_| {
                                let mut rules = rules;
                                spawn(async move {
                                    let state = core();
                                    if let Err(err) =
                                        locagens_core::api::permissions::delete_permission(&state, id).await
                                    {
                                        store.toast(store.tf("toast.delete_rule_failed", &[&err.to_string()]));
                                    }
                                    rules.set(locagens_core::api::permissions::list_permissions(&state).await.unwrap_or_default());
                                });
                            },
                            "{store.t(\"common.delete\")}"
                        }
                    }
                }
            }
        }
    }
}
