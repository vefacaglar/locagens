//! Chat header: the top strip of the main column (window drag area) with a
//! status dot, `project / conversation` breadcrumb and
//! icon buttons for stop, theme and the inspector panel.

use super::icons::{icon_folder, icon_moon, icon_panel_right, icon_stop, icon_sun};
use super::*;
use crate::state_ui::UiStore;
use dioxus::prelude::*;

#[component]
pub fn ChatHeader() -> Element {
    let store: UiStore = use_context();

    let run = store.active_run.read().clone();
    let title = run
        .as_ref()
        .map(|run| run.title.clone())
        .unwrap_or_else(|| store.t("conversation.new").to_string());
    let project_path = run
        .as_ref()
        .map(|run| run.project_path.clone())
        .or_else(|| store.selected_project.read().clone())
        .unwrap_or_default();
    let status = run
        .as_ref()
        .map(|run| run.status.clone())
        .unwrap_or_else(|| "done".to_string());
    let is_active = run_active(run.as_ref());
    let dark = store.is_dark();
    let inspector_open = *store.right_panel_open.read();
    let files_changed = store
        .turn_diff
        .read()
        .as_ref()
        .map(|(_, files)| *files)
        .unwrap_or(0);

    let folder = std::path::Path::new(&project_path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| project_path.clone());

    rsx! {
        header {
            class: "chat-header",
            // Draggable window titlebar strip (double-click maximizes, see start_window_drag).
            onmousedown: start_window_drag,
            div {
                class: "chat-header-meta",
                {status_dot(&status)}
                if !project_path.is_empty() {
                    span {
                        class: "crumb",
                        title: "{project_path}",
                        {icon_folder()}
                        span { "{folder}" }
                        span { class: "crumb-sep", "/" }
                    }
                }
                div {
                    class: "chat-title",
                    title: "{title}",
                    "{title}"
                }
            }
            div {
                class: "chat-header-actions",
                // Clicks on the buttons must neither start a drag nor maximize.
                onmousedown: move |e| e.stop_propagation(),
                ondoubleclick: move |e| e.stop_propagation(),
                if is_active {
                    button {
                        class: "icon-btn danger",
                        title: store.t("header.stop"),
                        onclick: move |_| {
                            let store = store;
                            let run_id = run.as_ref().map(|run| run.id.clone());
                            spawn(async move {
                                if let Some(run_id) = run_id {
                                    let state = crate::state_ui::core();
                                    if let Err(err) = locagens_core::api::runs::cancel_run(&state, &run_id).await {
                                        store.toast(store.tf("toast.stop_failed", &[&err.to_string()]));
                                    }
                                    store.refresh_runs();
                                }
                            });
                        },
                        {icon_stop()}
                    }
                }
                if !is_active {
                    super::review::ReviewButton {}
                }
                button {
                    class: "icon-btn",
                    title: if dark { store.t("header.to_light") } else { store.t("header.to_dark") },
                    onclick: move |_| {
                        store.toggle_theme();
                    },
                    if dark { {icon_sun()} } else { {icon_moon()} }
                }
                button {
                    class: if inspector_open { "icon-btn on" } else { "icon-btn" },
                    title: store.t("header.inspector"),
                    onclick: move |_| {
                        let mut store = store;
                        let open = *store.right_panel_open.read();
                        store.right_panel_open.set(!open);
                    },
                    {icon_panel_right()}
                    if files_changed > 0 {
                        span { class: "count-chip", "{files_changed}" }
                    }
                }
            }
        }
    }
}
