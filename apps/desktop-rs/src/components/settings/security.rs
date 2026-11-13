//! Security tab: workspace policy and sandbox status.

use crate::state_ui::UiStore;
use dioxus::prelude::*;

/// Security: workspace policy and command sandbox status, with the Windows
/// sandbox setup action when it is available.
#[component]
pub(super) fn SecurityTab() -> Element {
    let store: UiStore = use_context();
    let status: Signal<Option<locagens_core::api::security::SecurityStatus>> = use_signal(|| None);

    use_effect(move || {
        let mut status = status;
        spawn(async move {
            status.set(locagens_core::api::security::security_status().await.ok());
        });
    });

    let data = status.read().clone();
    rsx! {
        div {
            class: "panel-section-title", "{store.t(\"tab.security\")}"
        }
        match data {
            Some(security) => rsx! {
                div {
                    class: "panel-card",
                    div {
                        class: "panel-section-title", "{store.t(\"security.sandbox\")}"
                    }
                    div {
                        "{store.t(\"security.platform\")}: " span { class: "mono", "{security.sandbox.platform}" }
                        " · {store.t(\"security.status\")}: "
                        span {
                            class: if security.sandbox.status == "ready" { "badge ok" } else { "badge warn" },
                            "{security.sandbox.status}"
                        }
                    }
                    for (index, warning) in security.sandbox.warnings.iter().enumerate() {
                        div {
                            key: "warn-{index}",
                            class: "badge warn",
                            "{warning}"
                        }
                    }
                    for (index, sandbox_error) in security.sandbox.errors.iter().enumerate() {
                        div {
                            key: "err-{index}",
                            class: "badge err",
                            "{sandbox_error}"
                        }
                    }
                }
                div {
                    class: "panel-card",
                    div {
                        class: "panel-section-title", "{store.t(\"security.policy\")}"
                    }
                    div { "{store.t(\"security.network\")}: " span { class: "mono", "{security.policy.command_network}" } }
                    div { "{store.t(\"security.writes_only\")}: " span { class: "mono", "{security.policy.workspace_writes_only}" } }
                    div {
                        "{store.t(\"security.bind\")}: " span { class: "mono", "{security.api.bind_address}" }
                        " · {store.t(\"security.authenticated\")}: " span { class: "mono", "{security.api.authenticated}" }
                    }
                }
                if security.sandbox.can_install {
                    button {
                        class: "primary",
                        onclick: move |_| {
                            let mut status = status;
                            spawn(async move {
                                match locagens_core::api::security::setup_sandbox(true).await {
                                    Ok(updated) => status.set(Some(updated)),
                                    Err(err) => store.toast(store.tf("toast.sandbox_failed", &[&err.to_string()])),
                                }
                            });
                        },
                        "{store.t(\"security.setup\")}"
                    }
                }
            },
            None => rsx! {
                div { class: "settings-empty", "{store.t(\"security.unavailable\")}" }
            },
        }
    }
}
