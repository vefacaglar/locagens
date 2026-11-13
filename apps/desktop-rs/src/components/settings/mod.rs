//! Settings page: every tab is wired in-process to `locagens_core::api` —
//! Codex / Locagens auth & provider accounts, permissions, memory, skills, MCP servers,
//! plugins and security.


use crate::state_ui::UiStore;
use dioxus::prelude::*;

mod auth;
mod mcp;
mod memory;
mod permissions;
mod plugins;
mod providers;
mod security;
mod skills;

use self::auth::AuthTab;
use self::mcp::McpTab;
use self::memory::MemoryTab;
use self::permissions::PermissionsTab;
use self::plugins::PluginsTab;
use self::providers::ProvidersTab;
use self::security::SecurityTab;
use self::skills::SkillsTab;

#[component]
pub fn SettingsPage(initial_tab: crate::state_ui::SettingsTab) -> Element {
    let mut tab = use_signal(|| initial_tab);
    let store: UiStore = use_context();

    rsx! {
        div {
            class: "page-header",
            // Draggable window titlebar strip (double-click to maximize).
            onmousedown: move |_| {
                dioxus::desktop::use_window().drag();
            },
            ondoubleclick: move |_| {
                dioxus::desktop::use_window().toggle_maximized();
            },
            h2 { "{store.t(\"settings.title\")}" }
            button {
                class: "subtle icon-btn",
                // Clicks on the button must not start a window drag.
                onmousedown: move |e| e.stop_propagation(),
                onclick: move |_| {
                    let mut store: UiStore = use_context();
                    store.page.set(crate::state_ui::AppPage::Chat);
                },
                "← {store.t(\"common.back_to_chat\")}"
            }
        }
        div {
            class: "settings-layout",
            nav {
                class: "settings-nav",
                for tab_item in crate::state_ui::SettingsTab::ALL.iter() {
                    button {
                        key: "{tab_item.label_key()}",
                        class: if *tab_item == *tab.read() { "active" } else { "" },
                        onclick: move |_| tab.set(*tab_item),
                        "{store.t(tab_item.label_key())}"
                    }
                }
            }
            div {
                class: "settings-content",
                match *tab.read() {
                    crate::state_ui::SettingsTab::Providers => rsx! { ProvidersTab {} },
                    crate::state_ui::SettingsTab::Auth => rsx! { AuthTab {} },
                    crate::state_ui::SettingsTab::Permissions => rsx! { PermissionsTab {} },
                    crate::state_ui::SettingsTab::Memory => rsx! { MemoryTab {} },
                    crate::state_ui::SettingsTab::Skills => rsx! { SkillsTab {} },
                    crate::state_ui::SettingsTab::Mcp => rsx! { McpTab {} },
                    crate::state_ui::SettingsTab::Plugins => rsx! { PluginsTab {} },
                    crate::state_ui::SettingsTab::Security => rsx! { SecurityTab {} },
                    crate::state_ui::SettingsTab::Appearance => rsx! { super::appearance::AppearanceTab {} },
                }
            }
        }
    }
}
