//! Chat header: run title, project pill, mode & status badges, token/cost
//! summary, theme toggle, Stop Run and Inspector (right panel) toggle.

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
    let mode = run
        .as_ref()
        .map(|run| run.mode.clone())
        .unwrap_or_else(|| store.selected_mode.read().clone());
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
                span { class: "{mode_badge_class(&mode)}", "{mode_label(&mode)}" }
                span { class: "{status_badge_class(&status)}", "{status_label(&status)}" }
                if !project_path.is_empty() {
                    span {
                        class: "pill",
                        title: "{project_path}",
                        "📁 {project_path}"
                    }
                }
                if let Some(usage_text) = usage_text {
                    span { class: "badge", "{usage_text}" }
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
                "{theme_glyph(*store.dark_mode.read())}"
            }
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
                    "■ Stop Run"
                }
            }
            button {
                class: "subtle icon-btn",
                title: "Toggle inspector panel",
                onclick: move |_| {
                    let mut store = store;
                    let open = *store.right_panel_open.read();
                    store.right_panel_open.set(!open);
                },
                "Inspector"
            }
        }
    }
}

fn theme_glyph(dark: bool) -> &'static str {
    if dark {
        "☾ Dark"
    } else {
        "☀ Light"
    }
}
