//! UI components of the desktop app.

pub mod cards;
pub mod chat_header;
pub mod composer;
pub mod message_thread;
pub mod right_panel;
pub mod settings;
pub mod sidebar;
pub mod usage_logs;

use dioxus::prelude::*;
use locagens_core::db::Run;

/// Human label for a run status.
pub fn status_label(status: &str) -> &'static str {
    match status {
        "created" => "Starting",
        "generating" => "Running",
        "awaiting_permission" => "Awaiting approval",
        "awaiting_input" => "Awaiting answer",
        "done" => "Done",
        "failed" => "Failed",
        "cancelled" => "Cancelled",
        _ => "Unknown",
    }
}

/// A run is "active" while its status is one of the live states.
pub fn run_active(run: Option<&Run>) -> bool {
    run.map(|run| {
        matches!(
            run.status.as_str(),
            "created" | "generating" | "awaiting_permission" | "awaiting_input"
        )
    })
    .unwrap_or(false)
}

/// Shared bottom toast (`UiStore.status_toast`).
#[component]
pub fn Toast() -> Element {
    let store: crate::state_ui::UiStore = use_context();
    let Some(message) = store.status_toast.read().clone() else {
        return rsx! {};
    };
    rsx! {
        div { class: "toast", "{message}" }
    }
}
