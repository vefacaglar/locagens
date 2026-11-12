//! Right inspector panel: Plan, Agents, Git and Terminal tabs. Functional
//! initial versions (Phase 7a); deeper tooling lands in Phase 7b.

use crate::markdown::render_markdown;
use crate::state_ui::{core, UiStore};
use dioxus::prelude::*;
use locagens_core::api::projects::GitStatus;
use serde_json::Value;

#[component]
pub fn RightPanel() -> Element {
    let store: UiStore = use_context();
    let tab = *store.right_panel_tab.read();

    rsx! {
        div {
            class: "right-panel-tabs",
            for tab_item in crate::state_ui::RightPanelTab::ALL.iter() {
                button {
                    key: "{tab_item.label()}",
                    class: if *tab_item == tab { "active" } else { "" },
                    onclick: move |_| {
                        let mut store = store;
                        store.right_panel_tab.set(*tab_item);
                    },
                    "{tab_item.label()}"
                }
            }
        }
        div {
            class: "right-panel-body",
            match tab {
                crate::state_ui::RightPanelTab::Plan => rsx! { PlanTab {} },
                crate::state_ui::RightPanelTab::Agents => rsx! { AgentsTab {} },
                crate::state_ui::RightPanelTab::Git => rsx! { GitTab {} },
                crate::state_ui::RightPanelTab::Terminal => rsx! { TerminalTab {} },
            }
        }
    }
}

/// The active plan: title, Markdown body and tracked tasks.
#[component]
fn PlanTab() -> Element {
    let store: UiStore = use_context();
    let Some(plan) = store.plan.read().clone() else {
        return rsx! {
            div { class: "settings-empty", "No plan for this conversation yet. Switch the mode to Plan and ask for one." }
        };
    };
    let tasks: Vec<(String, String)> = plan
        .tasks
        .as_array()
        .map(|tasks| {
            tasks
                .iter()
                .filter_map(|task| {
                    let text = task.get("text").and_then(Value::as_str)?;
                    let status = task.get("status").and_then(Value::as_str).unwrap_or("pending");
                    Some((text.to_string(), status.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();

    rsx! {
        div {
            class: "panel-section-title",
            "{plan.title} · v{plan.version}"
        }
        if let Some(body) = plan.body.clone() {
            div {
                class: "panel-card md",
                dangerous_inner_html: render_markdown(&body),
            }
        }
        if !tasks.is_empty() {
            div {
                class: "panel-card",
                for (index, (text, status)) in tasks.iter().enumerate() {
                    div {
                        key: "{index}",
                        class: "plan-task task-{status}",
                        span { class: "task-status", "{status}" }
                        span { class: "task-text", "{text}" }
                    }
                }
            }
        }
    }
}

/// Sub-agent summary for the active run.
#[component]
fn AgentsTab() -> Element {
    let store: UiStore = use_context();
    let messages = store.messages.read().clone();
    let agents: Vec<(String, String, usize)> = {
        // Group by (role, name).
        let mut map: std::collections::BTreeMap<(String, String), usize> =
            std::collections::BTreeMap::new();
        for message in messages.iter() {
            if let Some(role) = message.agent_role.clone() {
                let name = message.agent_name.clone().unwrap_or_else(|| role.clone());
                *map.entry((role, name)).or_insert(0) += 1;
            }
        }
        map.into_iter().map(|((role, name), count)| (role, name, count)).collect()
    };

    if agents.is_empty() {
        return rsx! {
            div { class: "settings-empty", "No sub-agents ran in this conversation." }
        };
    }
    rsx! {
        for (index, (role, name, count)) in agents.iter().enumerate() {
            div {
                key: "{index}",
                class: "panel-card",
                div {
                    style: "display:flex;align-items:center;gap:8px",
                    span { class: "badge accent", "{role}" }
                    span { style: "flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap", "{name}" }
                    span { class: "badge", "{count} msgs" }
                }
            }
        }
    }
}

/// Git status of the selected project plus commit / push actions.
#[component]
fn GitTab() -> Element {
    let store: UiStore = use_context();
    let status: Signal<Option<GitStatus>> = use_signal(|| None);
    let mut message = use_signal(String::new);
    let busy = use_signal(|| false);

    let project_path = store
        .selected_project
        .read()
        .clone()
        .or_else(|| store.active_run.read().as_ref().map(|run| run.project_path.clone()));

    // Load the status once when the tab mounts / project changes.
    let path_for_status = project_path.clone();
    use_effect(move || {
        let mut status = status;
        let path = path_for_status.clone();
        spawn(async move {
            let Some(path) = path else { return };
            let state = core();
            status.set(locagens_core::api::projects::git_status(&state, &path).await.ok());
        });
    });

    let path_for_commit = project_path.clone().unwrap_or_default();
    let path_for_push = project_path.clone().unwrap_or_default();
    let current_status = status.read().clone();

    rsx! {
        div {
            class: "panel-section-title", "Git"
        }
        match current_status {
            Some(git_status) if git_status.is_git => {
                rsx! {
                    div {
                        class: "panel-card",
                        div { "Branch: " span { class: "mono", "{git_status.branch.clone().unwrap_or_default()}" } }
                        if git_status.has_changes.unwrap_or(false) {
                            div { style: "color:#e6b455", "Working tree has changes." }
                        } else {
                            div { style: "color:#4ecf7a", "Working tree clean." }
                        }
                    }
                    div {
                        class: "composer-row-top",
                        input {
                            placeholder: "Commit message…",
                            value: "{message.read()}",
                            oninput: move |event| message.set(event.value()),
                        }
                    }
                    div {
                        style: "display:flex;gap:8px",
                        button {
                            class: "primary",
                            disabled: *busy.read() || message.read().trim().is_empty(),
                            onclick: move |_| {
                                let mut busy = busy;
                                let path = path_for_commit.clone();
                                let commit_message = message.read().trim().to_string();
                                busy.set(true);
                                spawn(async move {
                                    let state = core();
                                    let result = locagens_core::api::projects::git_commit(
                                        &state,
                                        locagens_core::api::projects::GitCommitInput {
                                            path: Some(path),
                                            message: Some(commit_message),
                                            action: Some("commit".to_string()),
                                        },
                                    )
                                    .await;
                                    if let Err(err) = result {
                                        store.toast(format!("Commit failed: {err}"));
                                    } else {
                                        store.toast("Committed.".to_string());
                                        message.set(String::new());
                                    }
                                    busy.set(false);
                                });
                            },
                            "Commit"
                        }
                        button {
                            disabled: *busy.read(),
                            onclick: move |_| {
                                let mut busy = busy;
                                let path = path_for_push.clone();
                                busy.set(true);
                                spawn(async move {
                                    let state = core();
                                    let result = locagens_core::api::projects::git_commit(
                                        &state,
                                        locagens_core::api::projects::GitCommitInput {
                                            path: Some(path),
                                            message: None,
                                            action: Some("push".to_string()),
                                        },
                                    )
                                    .await;
                                    if let Err(err) = result {
                                        store.toast(format!("Push failed: {err}"));
                                    } else {
                                        store.toast("Pushed.".to_string());
                                    }
                                    busy.set(false);
                                });
                            },
                            "Push"
                        }
                    }
                }
            }
            Some(_) => rsx! {
                div { class: "settings-empty", "The selected folder is not a git repository." }
            },
            None => rsx! {
                div { class: "settings-empty", "Select a project to inspect its git status." }
            },
        }
    }
}

/// Terminal placeholder — a full PTY lands in Phase 7b.
#[component]
fn TerminalTab() -> Element {
    rsx! {
        div {
            class: "panel-card mono",
            "Terminal output will stream here in Phase 7b. Use the agent's run_command tool in chat for now."
        }
    }
}
