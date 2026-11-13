//! Chat header: integrated top bar (window drag strip) with run title,
//! project pill, token/cost summary, theme toggle, Stop Run and Inspector
//! (right panel) toggle. Completely boxless, like Antigravity.

use super::*;
use crate::state_ui::{fmt_cost, fmt_tokens, UiStore};
use dioxus::prelude::*;

#[component]
pub fn ChatHeader() -> Element {
    let store: UiStore = use_context();

    let run = store.active_run.read().clone();
    let title = run
        .as_ref()
        .map(|run| run.title.clone())
        .unwrap_or_else(|| "New Conversation".to_string());
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

    let usage = store.run_usage.read().clone();
    let usage_text = usage.as_ref().map(|usage| {
        format!(
            "{} tok · {}",
            fmt_tokens(usage.total_input_tokens + usage.total_output_tokens),
            fmt_cost(usage.total_cost),
        )
    });

    rsx! {
        header {
            class: "chat-header",
            // Draggable window titlebar strip (double-click to maximize).
            onmousedown: move |_| {
                dioxus::desktop::use_window().drag();
            },
            ondoubleclick: move |_| {
                dioxus::desktop::use_window().toggle_maximized();
            },
            div {
                class: "chat-header-meta",
                span {
                    class: "status-dot {status}",
                    title: "{status_label(&status)}",
                }
                div {
                    class: "chat-title",
                    title: "{title}",
                    "{title}"
                }
                if !project_path.is_empty() {
                    {
                        let folder = std::path::Path::new(&project_path)
                            .file_name()
                            .map(|name| name.to_string_lossy().to_string())
                            .unwrap_or_else(|| project_path.clone());
                        rsx! {
                            span {
                                class: "pill",
                                title: "{project_path}",
                                "📁 {folder}"
                            }
                        }
                    }
                }
                if let Some(usage_text) = usage_text {
                    span { class: "badge", "{usage_text}" }
                }
            }
            div {
                class: "chat-header-actions",
                // Clicks on the buttons must not start a window drag.
                onmousedown: move |e| e.stop_propagation(),
                if is_active {
                    button {
                        class: "danger",
                        onclick: move |_| {
                            let store = store;
                            let run_id = run.as_ref().map(|run| run.id.clone());
                            spawn(async move {
                                if let Some(run_id) = run_id {
                                    let state = crate::state_ui::core();
                                    if let Err(err) = locagens_core::api::runs::cancel_run(&state, &run_id).await {
                                        store.toast(format!("Failed to stop run: {err}"));
                                    }
                                    store.refresh_runs();
                                }
                            });
                        },
                        "■ Stop"
                    }
                }
                button {
                    class: "subtle icon-btn",
                    title: "Toggle dark / light theme",
                    onclick: move |_| {
                        let mut store = store;
                        let dark = *store.dark_mode.read();
                        store.dark_mode.set(!dark);
                    },
                    if *store.dark_mode.read() { "☾" } else { "☀" }
                }
                button {
                    // Highlighted while the inspector panel is open; appends the
                    // changed-file count of the current turn diff when present.
                    class: "subtle icon-btn",
                    title: "Toggle inspector panel",
                    style: if *store.right_panel_open.read() { "color: #0a84ff;" } else { "" },
                    onclick: move |_| {
                        let mut store = store;
                        let open = *store.right_panel_open.read();
                        store.right_panel_open.set(!open);
                    },
                    if let Some((_, files_changed)) = store.turn_diff.read().as_ref() {
                        if *files_changed > 0 {
                            "◫ {files_changed}"
                        } else {
                            "◫"
                        }
                    } else {
                        "◫"
                    }
                }
            }
        }
    }
}
