//! UI components of the desktop app.

pub mod cards;
pub mod chat_header;
pub mod composer;
pub mod icons;
pub mod message_thread;
pub mod review;
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

/// Small status dot with the human status as tooltip; the colour comes from
/// the `.status-dot.<status>` theme classes.
pub fn status_dot(status: &str) -> Element {
    rsx! {
        span { class: "status-dot {status}", title: "{status_label(status)}" }
    }
}

/// `onmousedown` handler of draggable header strips: a primary-button press
/// starts dragging the (undecorated) window.
pub fn start_window_drag(event: Event<MouseData>) {
    if event.trigger_button() == Some(dioxus::html::input_data::MouseButton::Primary) {
        dioxus::desktop::use_window().drag();
    }
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

/// Windows-11-style minimize / maximize / close buttons fixed at the top
/// right of the window. Rendered on every platform except macOS, which keeps
/// its native traffic lights.
#[component]
pub fn WindowControls() -> Element {
    let window = dioxus::desktop::use_window();
    let mut maximized = use_signal(|| false);
    // The window can also be maximized by double-clicking a header strip or
    // through the OS, so mirror its real state for the restore glyph.
    use_future({
        let window = window.clone();
        move || {
            let window = window.clone();
            async move {
                loop {
                    let now = window.is_maximized();
                    if *maximized.peek() != now {
                        maximized.set(now);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
            }
        }
    });
    let is_maximized = *maximized.read();
    rsx! {
        div {
            class: "window-controls",
            button {
                class: "win-btn",
                title: "Minimize",
                onclick: {
                    let window = window.clone();
                    move |_| window.set_minimized(true)
                },
                {icons::window_minimize()}
            }
            button {
                class: "win-btn",
                title: if is_maximized { "Restore" } else { "Maximize" },
                onclick: {
                    let window = window.clone();
                    move |_| window.toggle_maximized()
                },
                if is_maximized { {icons::window_restore()} } else { {icons::window_maximize()} }
            }
            button {
                class: "win-btn win-close",
                title: "Close",
                onclick: {
                    let window = window.clone();
                    move |_| window.close()
                },
                {icons::window_close()}
            }
        }
    }
}
