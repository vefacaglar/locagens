//! Banner shown above the chat when the open project folder is not trusted
//! but ships executable project configuration (`.locagens` hooks, MCP
//! servers, plugins, rules, agent roles). That configuration stays ignored
//! until the user trusts the folder here (or with `lcgn trust`).

use crate::state_ui::UiStore;
use dioxus::prelude::*;
use locagens_core::api::projects::{project_trust, trust_project, ProjectTrust};

/// The folder the chat works in: the open run's, else the selected project.
fn current_project(store: &UiStore) -> String {
    store
        .active_run
        .read()
        .as_ref()
        .map(|run| run.project_path.clone())
        .or_else(|| store.selected_project.read().clone())
        .unwrap_or_default()
}

#[component]
pub fn TrustBanner() -> Element {
    let store: UiStore = use_context();
    let mut dismissed: Signal<Vec<String>> = use_signal(Vec::new);
    let mut refresh = use_signal(|| 0u32);
    let status = use_resource(move || async move {
        let _ = *refresh.read();
        let path = current_project(&store);
        if path.trim().is_empty() {
            return None;
        }
        project_trust(&path).await.ok().map(|trust: ProjectTrust| (path, trust))
    });

    let Some(Some((path, trust))) = status.read().clone() else {
        return rsx! {};
    };
    if trust.trusted || trust.ignored_files.is_empty() || dismissed.read().contains(&path) {
        return rsx! {};
    }
    let files = trust.ignored_files.join(", ");
    let trust_path = path.clone();

    rsx! {
        div {
            class: "permission-card",
            div {
                class: "pc-title",
                span { class: "badge warn", "{store.t(\"trust.title\")}" }
            }
            div { class: "pc-detail", "{store.t(\"trust.body\")}" }
            div { class: "pc-pathbox", "{files}" }
            div {
                class: "pc-buttons",
                button {
                    class: "primary",
                    onclick: move |_| {
                        let path = trust_path.clone();
                        spawn(async move {
                            match trust_project(&path, true).await {
                                Ok(_) => store.toast(store.t("trust.done").to_string()),
                                Err(err) => store.toast(store.tf("trust.failed", &[&err.to_string()])),
                            }
                            refresh += 1;
                        });
                    },
                    "{store.t(\"trust.trust\")}"
                }
                button {
                    onclick: move |_| dismissed.write().push(path.clone()),
                    "{store.t(\"trust.not_now\")}"
                }
            }
        }
    }
}
