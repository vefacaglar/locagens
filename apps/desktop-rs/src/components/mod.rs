//! UI components of the desktop app.

pub mod appearance;
pub mod cards;
pub mod chat_header;
pub mod composer;
pub mod icons;
pub mod message_thread;
pub mod review;
pub mod right_panel;
pub mod settings;
pub mod sidebar;
pub mod slash;
pub mod trust_banner;
pub mod usage_logs;

use dioxus::prelude::*;
use locagens_core::db::Run;

/// i18n key of the human label for a run status.
pub fn status_key(status: &str) -> &'static str {
    match status {
        "created" => "status.created",
        "generating" => "status.generating",
        "awaiting_permission" => "status.awaiting_permission",
        "awaiting_input" => "status.awaiting_input",
        "done" => "status.done",
        "failed" => "status.failed",
        "cancelled" => "status.cancelled",
        _ => "status.unknown",
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
    let store: crate::state_ui::UiStore = use_context();
    rsx! {
        span { class: "status-dot {status}", title: "{store.t(status_key(status))}" }
    }
}

/// `onmousedown` handler of draggable header strips: a primary-button press
/// starts dragging the (undecorated) window, a second press within the
/// double-click interval toggles maximize.
///
/// `ondoubleclick` cannot be used: the first press hands the mouse to the
/// OS move loop (`drag()`), so the webview never sees the second click.
pub fn start_window_drag(event: Event<MouseData>) {
    if event.trigger_button() != Some(dioxus::html::input_data::MouseButton::Primary) {
        return;
    }
    let point = event.screen_coordinates();
    let window = dioxus::desktop::use_window();
    if is_double_press(std::time::Instant::now(), (point.x, point.y)) {
        window.toggle_maximized();
    } else {
        window.drag();
    }
}

/// Last primary press on a drag strip: time and screen position.
static LAST_DRAG_PRESS: std::sync::Mutex<Option<(std::time::Instant, (f64, f64))>> = std::sync::Mutex::new(None);

/// Windows' default double-click time and a few pixels of slack.
const DOUBLE_PRESS_MS: u128 = 500;
const DOUBLE_PRESS_SLOP: f64 = 6.0;

/// Records a press and says whether it completes a double press (the pair is
/// consumed, so a triple press does not toggle twice).
fn is_double_press(now: std::time::Instant, point: (f64, f64)) -> bool {
    let mut last = LAST_DRAG_PRESS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let double = last.is_some_and(|(at, (x, y))| {
        now.duration_since(at).as_millis() <= DOUBLE_PRESS_MS
            && (point.0 - x).abs() <= DOUBLE_PRESS_SLOP
            && (point.1 - y).abs() <= DOUBLE_PRESS_SLOP
    });
    *last = if double { None } else { Some((now, point)) };
    double
}

#[cfg(test)]
mod drag_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn second_press_nearby_and_soon_is_a_double_press() {
        let start = Instant::now();
        *LAST_DRAG_PRESS.lock().unwrap() = None;
        assert!(!is_double_press(start, (100.0, 10.0)));
        assert!(is_double_press(start + Duration::from_millis(200), (102.0, 11.0)));
        // Consumed: a third quick press starts a new pair.
        assert!(!is_double_press(start + Duration::from_millis(300), (102.0, 11.0)));
        // Too slow or too far is not a double press.
        assert!(!is_double_press(start + Duration::from_millis(1000), (102.0, 11.0)));
        assert!(!is_double_press(start + Duration::from_millis(1100), (300.0, 11.0)));
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
        div { class: "toast", role: "status", "{message}" }
    }
}

/// Windows-11-style minimize / maximize / close buttons fixed at the top
/// right of the window. Rendered on every platform except macOS, which keeps
/// its native traffic lights.
#[component]
pub fn WindowControls() -> Element {
    let store: crate::state_ui::UiStore = use_context();
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
                title: store.t("window.minimize"),
                onclick: {
                    let window = window.clone();
                    move |_| window.set_minimized(true)
                },
                {icons::window_minimize()}
            }
            button {
                class: "win-btn",
                title: if is_maximized { store.t("window.restore") } else { store.t("window.maximize") },
                onclick: {
                    let window = window.clone();
                    move |_| window.toggle_maximized()
                },
                if is_maximized { {icons::window_restore()} } else { {icons::window_maximize()} }
            }
            button {
                class: "win-btn win-close",
                title: store.t("window.close"),
                onclick: {
                    let window = window.clone();
                    move |_| window.close()
                },
                {icons::window_close()}
            }
        }
    }
}
