//! Sidebar: brand header, run search, project tree with runs and the footer
//! navigation between Chat / Usage Logs / Settings.

use super::*;
use crate::state_ui::UiStore;
use dioxus::prelude::*;

#[component]
pub fn Sidebar() -> Element {
    let store: UiStore = use_context();
    let mut show_add_modal = use_signal(|| false);

    // Filtered runs by the search query (title/task match).
    let query = store.search_query.read().to_lowercase();
    let all_runs = store.runs.read().clone();
    let filtered: Vec<&Run> = all_runs
        .iter()
        .filter(|run| {
            query.is_empty()
                || run.title.to_lowercase().contains(&query)
                || run.task.to_lowercase().contains(&query)
        })
        .collect();

    // Group runs by project.
    let projects = store.projects.read().clone();
    let selected_project = store.selected_project.read().clone();
    let active_run_id = store.active_run_id.read().clone();

    rsx! {
        aside {
            class: "sidebar",
            div {
                class: "sidebar-brand",
                div {
                    class: "brand-row",
                    span { class: "brand-title", "Locagens" }
                    span { class: "badge os-badge", "{os_label()}" }
                }
                div {
                    class: "sidebar-actions",
                    button {
                        class: "primary",
                        onclick: move |_| new_chat(&store),
                        "+ New Chat"
                    }
                    button {
                        onclick: move |_| show_add_modal.set(true),
                        "Add Project"
                    }
                }
            }
            div {
                class: "sidebar-search",
                input {
                    r#type: "search",
                    placeholder: "Search conversations…",
                    value: "{store.search_query.read()}",
                    oninput: move |event| {
                        let mut store = store;
                        store.search_query.set(event.value());
                    },
                }
            }
            div {
                class: "sidebar-tree",
                for project in projects.iter() {
                    {
                        let runs: Vec<&Run> = filtered
                            .iter()
                            .copied()
                            .filter(|run| run.project_path == project.path)
                            .collect();
                        let is_selected = selected_project.as_deref() == Some(project.path.as_str());
                        let project_row_class = if is_selected { "project-row selected" } else { "project-row" };
                        let path = project.path.clone();
                        let path_select = path.clone();
                        let path_delete = path.clone();
                        rsx! {
                            div { class: "project-group",
                                div {
                                    class: "{project_row_class}",
                                    onclick: move |_| {
                                        let mut store = store;
                                        store.selected_project.set(Some(path_select.clone()));
                                    },
                                    span { class: "project-name", title: "{project.path}", "{project.name}" }
                                    button {
                                        class: "subtle icon-btn",
                                        title: "Remove project from Locagens",
                                        onclick: move |_| {
                                            delete_project(&store, &path_delete.clone());
                                        },
                                        "✕"
                                    }
                                }
                                for run in runs.iter() {
                                    { run_item(&store, run, active_run_id.as_deref()) }
                                }
                            }
                        }
                    }
                }
                // Runs whose project is not (or no longer) registered.
                {
                    let known: Vec<String> = projects.iter().map(|project| project.path.clone()).collect();
                    let orphaned: Vec<&Run> = filtered
                        .iter()
                        .copied()
                        .filter(|run| !known.contains(&run.project_path))
                        .collect();
                    if !orphaned.is_empty() {
                        rsx! {
                            div { class: "project-group",
                                div { class: "project-row", span { class: "project-name", "Other conversations" } }
                                for run in orphaned.iter() {
                                    { run_item(&store, run, active_run_id.as_deref()) }
                                }
                            }
                        }
                    } else {
                        rsx! {}
                    }
                }
                if filtered.is_empty() {
                    div { class: "settings-empty", "No conversations yet — start a New Chat." }
                }
            }
            div {
                class: "sidebar-footer",
                button {
                    class: if *store.page.read() == crate::state_ui::AppPage::Chat { "active" } else { "" },
                    onclick: move |_| {
                        let mut store = store;
                        store.page.set(crate::state_ui::AppPage::Chat);
                    },
                    "Chat"
                }
                button {
                    class: if *store.page.read() == crate::state_ui::AppPage::UsageLogs { "active" } else { "" },
                    onclick: move |_| {
                        let mut store = store;
                        store.page.set(crate::state_ui::AppPage::UsageLogs);
                    },
                    "Usage Logs"
                }
                button {
                    class: if matches!(*store.page.read(), crate::state_ui::AppPage::Settings(_)) { "active" } else { "" },
                    onclick: move |_| {
                        let mut store = store;
                        store.page.set(crate::state_ui::AppPage::Settings(crate::state_ui::SettingsTab::Providers));
                    },
                    "Settings"
                }
            }
        }
        if *show_add_modal.read() {
            AddProjectModal { on_close: move || show_add_modal.set(false) }
        }
    }
}

/// One run row: status dot, title, model badge and a hover delete button.
fn run_item(store: &UiStore, run: &Run, active_run_id: Option<&str>) -> Element {
    let store = *store;
    let run_id = run.id.clone();
    let run_id_select = run_id.clone();
    let run_title = run.title.clone();
    let model = run.model.clone();
    let status = run.status.clone();
    let is_active = active_run_id == Some(run.id.as_str());
    let item_class = if is_active { "run-item active" } else { "run-item" };
    rsx! {
        div {
            class: "{item_class}",
            onclick: move |_| store.select_run(run_id_select.clone()),
            span { class: "status-dot {status}", title: "{status_label(&status)}" }
            span { class: "run-title", "{run_title}" }
            span { class: "run-model", "{model}" }
            button {
                class: "subtle icon-btn run-delete",
                title: "Delete conversation",
                onclick: move |event| {
                    event.stop_propagation();
                    let run_id_delete = run_id.clone();
                    delete_run(&store, &run_id_delete);
                },
                "🗑"
            }
        }
    }
}

/// Modal to register a project: manual path input or native folder picker.
#[component]
fn AddProjectModal(on_close: EventHandler) -> Element {
    let store: UiStore = use_context();
    let mut path = use_signal(String::new);
    let busy = use_signal(|| false);

    rsx! {
        div {
            class: "modal-backdrop",
            onclick: move |_| on_close.call(()),
            div {
                class: "modal",
                onclick: move |event| event.stop_propagation(),
                h3 { "Add Project" }
                label { "Project folder" }
                div {
                    class: "composer-row-top",
                    input {
                        placeholder: r"C:\path\to\project",
                        value: "{path.read()}",
                        oninput: move |event| path.set(event.value()),
                    }
                    button {
                        disabled: *busy.read(),
                        onclick: move |_| {
                            let mut path = path;
                            spawn(async move {
                                if let Some(folder) = rfd::AsyncFileDialog::new().pick_folder().await {
                                    path.set(folder.path().to_string_lossy().to_string());
                                }
                            });
                        },
                        "Browse…"
                    }
                }
                div {
                    class: "modal-actions",
                    button { onclick: move |_| on_close.call(()), "Cancel" }
                    button {
                        class: "primary",
                        disabled: *busy.read() || path.read().trim().is_empty(),
                        onclick: move |_| {
                            let store = store;
                            let mut busy = busy;
                            let raw = path.read().trim().to_string();
                            busy.set(true);
                            spawn(async move {
                                let state = crate::state_ui::core();
                                let result = locagens_core::api::projects::create_project(
                                    &state,
                                    locagens_core::api::projects::CreateProjectInput {
                                        path: Some(raw.clone()),
                                        name: None,
                                    },
                                )
                                .await;
                                match result {
                                    Ok(_project) => {
                                        store.refresh_projects();
                                        on_close.call(());
                                    }
                                    Err(err) => store.toast(format!("Failed to add project: {err}")),
                                }
                                busy.set(false);
                            });
                        },
                        "Add Project"
                    }
                }
            }
        }
    }
}

/// Clears the active conversation so the composer starts a fresh run.
fn new_chat(store: &UiStore) {
    let mut store = *store;
    store.active_run_id.set(None);
    store.active_run.set(None);
    store.messages.set(Vec::new());
    store.plan.set(None);
    store.run_usage.set(None);
    store.pending_permission.set(None);
    store.pending_question.set(None);
    store.page.set(crate::state_ui::AppPage::Chat);
}

/// Registers nothing — removes a project registration.
fn delete_project(store: &UiStore, path: &str) {
    let mut store = *store;
    let path = path.to_string();
    spawn(async move {
        let state = crate::state_ui::core();
        if let Err(err) = locagens_core::api::projects::delete_project(&state, &path).await {
            store.toast(format!("Failed to remove project: {err}"));
        }
        let projects = locagens_core::api::projects::list_projects(&state).await.unwrap_or_default();
        store.projects.set(projects);
        let still_registered = store
            .projects
            .read()
            .iter()
            .any(|project| project.path == path);
        if !still_registered && store.selected_project.read().as_deref() == Some(path.as_str()) {
            store.selected_project.set(store.projects.read().first().map(|project| project.path.clone()));
        }
    });
}

/// Deletes a run (direct DB call; the facade does not expose a wrapper).
fn delete_run(store: &UiStore, run_id: &str) {
    let mut store = *store;
    let run_id = run_id.to_string();
    spawn(async move {
        let state = crate::state_ui::core();
        state.db.delete_run(&run_id);
        if store.active_run_id.read().as_deref() == Some(run_id.as_str()) {
            store.active_run_id.set(None);
            store.active_run.set(None);
            store.messages.set(Vec::new());
            store.plan.set(None);
            store.run_usage.set(None);
            store.pending_permission.set(None);
            store.pending_question.set(None);
        }
        store.refresh_runs();
    });
}

/// OS badge text shown next to the brand.
pub fn os_label() -> &'static str {
    if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        "Desktop"
    }
}
