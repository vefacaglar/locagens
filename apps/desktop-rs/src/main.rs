//! Locagens native desktop app — Dioxus 0.7 window wired in-process to
//! `locagens_core::api` (no HTTP involved).
//!
//! `main` bootstraps the backend core (`api::bootstrap()`), stores the shared
//! `AppState` in a global for every component and spawned task, then launches
//! the desktop runtime with the root `App` component.

// FR-052: release builds are GUI programs on Windows (no console window).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard;
mod components;
#[cfg(test)]
mod e2e_tests;
mod i18n;
mod logo;
mod markdown;
mod prefs;
mod state_ui;
mod theme;

use dioxus::desktop::{Config, LogicalSize, WindowBuilder};
use dioxus::prelude::*;
use state_ui::{set_core, AppPage};

/// Smallest window that keeps the chat column usable (FR-061).
const MIN_WINDOW: (f64, f64) = (760.0, 520.0);

/// Scrollbars drawn over the content (Chromium's Fluent overlay scrollbars):
/// they take no layout width, so a scroller appearing never narrows its
/// content, and they show while scrolling and fade out on their own. Set
/// before the WebView2 environment is created; extra arguments already in
/// the variable (e.g. a debugging port) are kept.
fn enable_overlay_scrollbars() {
    const KEY: &str = "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS";
    const FLAG: &str = "--enable-features=FluentOverlayScrollbar,OverlayScrollbar";
    let args = match std::env::var(KEY) {
        Ok(existing) if !existing.trim().is_empty() => format!("{existing} {FLAG}"),
        _ => FLAG.to_string(),
    };
    std::env::set_var(KEY, args);
}

fn main() {
    enable_overlay_scrollbars();
    // 1. Bootstrap the core (opens the data store, runs startup tasks). A GUI
    //    build has no console, so a failure is shown in a dialog.
    let state = match locagens_core::api::bootstrap() {
        Ok(state) => state,
        Err(err) => {
            let _ = rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("Locagens")
                .set_description(i18n::tf(prefs::load().language, "startup.failed", &[&format!("{err:#}")]))
                .show();
            std::process::exit(1);
        }
    };
    set_core(std::sync::Arc::new(state));

    // 2. Launch the Dioxus desktop runtime.
    let mut window = WindowBuilder::new()
        .with_title("Locagens")
        .with_resizable(true)
        .with_inner_size(LogicalSize::new(1380.0, 860.0))
        .with_min_inner_size(LogicalSize::new(MIN_WINDOW.0, MIN_WINDOW.1))
        .with_window_icon(logo::window_icon());
    // Custom title bar: the header strips drag the window and `WindowControls`
    // draws minimize / maximize / close (macOS keeps its traffic lights).
    #[cfg(not(target_os = "macos"))]
    {
        window = window.with_decorations(false);
    }
    #[cfg(target_os = "windows")]
    {
        use dioxus::desktop::tao::platform::windows::WindowBuilderExtWindows;
        window = window.with_undecorated_shadow(true);
    }
    #[cfg(target_os = "macos")]
    {
        use dioxus::desktop::tao::dpi::LogicalPosition;
        use dioxus::desktop::tao::platform::macos::WindowBuilderExtMacOS;
        window = window
            .with_titlebar_transparent(true)
            .with_fullsize_content_view(true)
            .with_title_hidden(true)
            .with_traffic_light_inset(LogicalPosition::new(14.0, 14.0));
    }
    let config = Config::new()
        // The webview menu stays on so text fields keep copy / paste; the
        // context-menu script hides it everywhere else (FR-066).
        .with_disable_context_menu(false)
        .with_menu(None)
        // FR-034: the window never navigates away from the app; web links
        // (already filtered to http / https / mailto) open in the browser.
        .with_navigation_handler(|_| false)
        // FR-022: the event loop ends (window closed, quit): stop the exec
        // sessions of every run before the process exits.
        .with_custom_event_handler(|event, _| {
            if matches!(event, dioxus::desktop::tao::event::Event::LoopDestroyed) {
                shutdown_core();
            }
        })
        .with_window(window);
    dioxus::LaunchBuilder::desktop().with_cfg(config).launch(App);
}

/// Runs `api::runs::shutdown` on its own thread and runtime (the event loop
/// thread may already be inside the app's runtime) and waits for it.
fn shutdown_core() {
    let worker = std::thread::spawn(|| {
        if let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
            runtime.block_on(locagens_core::api::runs::shutdown(&state_ui::core()));
        }
    });
    let _ = worker.join();
}

/// Hides the webview's own context menu except in text fields and on a text
/// selection, where copy and paste are needed.
const CONTEXT_MENU_SCRIPT: &str = r#"(() => {
  if (window.lcgnContextMenu) return;
  window.lcgnContextMenu = true;
  document.addEventListener('contextmenu', (e) => {
    const t = e.target;
    const editable = t && t.closest && t.closest('input, textarea, [contenteditable="true"]');
    const selected = String(window.getSelection ? window.getSelection() : '').length > 0;
    if (!editable && !selected) e.preventDefault();
  });
})();"#;

/// Root component: builds the `UiStore` signals, provides it via context and
/// renders the 3-column shell (sidebar / main column / inspector panel).
#[component]
fn App() -> Element {
    let store = state_ui::use_ui_store();
    let appearance = store.appearance;
    let system_dark = store.system_dark;
    use_context_provider(|| store);

    // Chat scroll helpers, a11y basics and the context-menu rule.
    use_effect(|| {
        dioxus::document::eval(state_ui::CHAT_SCROLL_SCRIPT);
        dioxus::document::eval(state_ui::A11Y_SCRIPT);
        dioxus::document::eval(CONTEXT_MENU_SCRIPT);
    });
    // Copy buttons of rendered code blocks (FR-060).
    use_future(clipboard::serve_code_copy_buttons);
    // Keyboard layer: Esc, Ctrl+N/K/L/M/B/./,// (UX-019).
    components::keyboard::use_keyboard_layer(store);

    // Load projects, runs and the provider catalog once at startup.
    use_hook(move || {
        let store = store;
        store.refresh_initial();
    });

    // Track the OS colour scheme so the header toggle / theme class know
    // whether `System` currently resolves to dark. (The stylesheet itself
    // follows the OS through a `prefers-color-scheme` rule.)
    use_future(move || {
        let mut system_dark = system_dark;
        async move {
            let mut watcher = dioxus::document::eval(
                r#"const m = window.matchMedia('(prefers-color-scheme: dark)');
                   dioxus.send(m.matches);
                   m.addEventListener('change', e => dioxus.send(e.matches));"#,
            );
            while let Ok(dark) = watcher.recv::<bool>().await {
                system_dark.set(dark);
            }
        }
    });

    // The platform-native stylesheet (re-rendered when appearance changes).
    i18n::set_current(appearance.read().language);
    let stylesheet = theme::build_stylesheet_appearance(&appearance.read());

    let show_right_panel = *store.right_panel_open.read();
    let show_sidebar = *store.sidebar_open.read();
    // The main column clears the window controls only while it is rightmost.
    let shell_inspector_class = if show_right_panel { "with-inspector" } else { "no-inspector" };
    let shell_sidebar_class = if show_sidebar { "" } else { "no-sidebar" };

    rsx! {
        style { dangerous_inner_html: stylesheet }
        div {
            class: "app-shell {theme::theme_class(store.is_dark())} {shell_inspector_class} {shell_sidebar_class}",
            style: "--sidebar-w: {store.layout.read().sidebar_width}px; --panel-w: {store.layout.read().right_panel_width}px;",
            if show_sidebar {
                components::sidebar::Sidebar {}
                components::resize::ResizeHandle { edge: components::resize::Edge::Sidebar }
            }
            div {
                class: "main-column",
                match *store.page.read() {
                    AppPage::Chat => {
                        rsx! {
                            components::chat_header::ChatHeader {}
                            div {
                                class: "chat-area",
                                components::message_thread::MessageThread {}
                                // Floats over the thread's bottom, so the thread
                                // (and its scrollbar) runs to the window edge.
                                div {
                                    id: "chat-dock",
                                    class: "chat-dock",
                                    components::trust_banner::TrustBanner {}
                                    components::cards::PendingCards {}
                                    components::composer::Composer {}
                                }
                            }
                        }
                    }
                    AppPage::Settings(tab) => rsx! {
                        components::settings::SettingsPage { tab }
                    },
                    AppPage::UsageLogs => rsx! {
                        components::usage_logs::UsageLogsPage {}
                    },
                }
            }
            if show_right_panel {
                components::resize::ResizeHandle { edge: components::resize::Edge::Panel }
                div {
                    class: "right-panel",
                    components::right_panel::RightPanel {}
                }
            }
            components::palette::Overlays {}
            components::Toast {}
            if !cfg!(target_os = "macos") {
                components::WindowControls {}
            }
        }
    }
}
