//! Locagens native desktop app — Dioxus 0.7 window wired in-process to
//! `locagens_core::api` (no HTTP involved).
//!
//! `main` bootstraps the backend core (`api::bootstrap()`), stores the shared
//! `AppState` in a global for every component and spawned task, then launches
//! the desktop runtime with the root `App` component.

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

fn main() {
    // 1. Bootstrap the core (opens the data store, runs startup tasks).
    let state = locagens_core::api::bootstrap().expect("Failed to bootstrap the Locagens core");
    set_core(std::sync::Arc::new(state));

    // 2. Launch the Dioxus desktop runtime.
    let mut window = WindowBuilder::new()
        .with_title("Locagens")
        .with_resizable(true)
        .with_inner_size(LogicalSize::new(1380.0, 860.0))
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
        .with_disable_context_menu(true)
        .with_menu(None)
        .with_window(window);
    dioxus::LaunchBuilder::desktop().with_cfg(config).launch(App);
}

/// Root component: builds the `UiStore` signals, provides it via context and
/// renders the 3-column shell (sidebar / main column / inspector panel).
#[component]
fn App() -> Element {
    let store = state_ui::use_ui_store();
    let appearance = store.appearance;
    let system_dark = store.system_dark;
    use_context_provider(|| store);

    // Clipboard handler for code-block copy buttons, and chat scroll helpers.
    use_effect(|| {
        dioxus::document::eval(markdown::COPY_SCRIPT);
        dioxus::document::eval(state_ui::CHAT_SCROLL_SCRIPT);
        dioxus::document::eval(state_ui::A11Y_SCRIPT);
    });

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
    // The main column clears the window controls only while it is rightmost.
    let shell_inspector_class = if show_right_panel { "with-inspector" } else { "no-inspector" };

    rsx! {
        style { dangerous_inner_html: stylesheet }
        div {
            class: "app-shell {theme::theme_class(store.is_dark())} {shell_inspector_class}",
            components::sidebar::Sidebar {}
            div {
                class: "main-column",
                match store.page.read().clone() {
                    AppPage::Chat => {
                        rsx! {
                            components::chat_header::ChatHeader {}
                            div {
                                class: "chat-area",
                                components::message_thread::MessageThread {}
                                components::trust_banner::TrustBanner {}
                                components::cards::PendingCards {}
                                components::composer::Composer {}
                            }
                        }
                    }
                    AppPage::Settings(tab) => rsx! {
                        components::settings::SettingsPage { initial_tab: tab }
                    },
                    AppPage::UsageLogs => rsx! {
                        components::usage_logs::UsageLogsPage {}
                    },
                }
            }
            if show_right_panel {
                div {
                    class: "right-panel",
                    components::right_panel::RightPanel {}
                }
            }
            components::Toast {}
            if !cfg!(target_os = "macos") {
                components::WindowControls {}
            }
        }
    }
}