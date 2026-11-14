//! Settings page: every tab is wired in-process to `locagens_core::api` —
//! models & accounts (API keys, sign-in, model switches), permissions,
//! memory, skills, MCP servers, plugins, security and appearance.

use crate::state_ui::{AppPage, SettingsTab, UiStore};
use dioxus::prelude::*;

mod auth;
mod mcp;
mod memory;
mod permissions;
mod plugins;
mod providers;
mod security;
mod skills;

use self::mcp::McpTab;
use self::memory::MemoryTab;
use self::permissions::PermissionsTab;
use self::plugins::PluginsTab;
use self::providers::ProvidersTab;
use self::security::SecurityTab;
use self::skills::SkillsTab;

/// The page shows the tab named by `AppPage::Settings(tab)`, so opening
/// Settings on another tab while it is open switches tabs (FR-058).
#[component]
pub fn SettingsPage(tab: SettingsTab) -> Element {
    let mut store: UiStore = use_context();

    rsx! {
        div {
            class: "page-header",
            // Draggable window titlebar strip (double-click maximizes, see start_window_drag).
            onmousedown: super::start_window_drag,
            h2 { "{store.t(\"settings.title\")}" }
        }
        div {
            class: "settings-layout",
            nav {
                class: "settings-nav",
                role: "tablist",
                for tab_item in SettingsTab::ALL {
                    button {
                        key: "{tab_item.label_key()}",
                        class: if tab_item == tab { "active" } else { "" },
                        role: "tab",
                        "aria-selected": "{tab_item == tab}",
                        onclick: move |_| store.page.set(AppPage::Settings(tab_item)),
                        "{store.t(tab_item.label_key())}"
                    }
                }
            }
            div {
                class: "settings-content",
                role: "tabpanel",
                match tab {
                    SettingsTab::Providers => rsx! { ProvidersTab {} },
                    SettingsTab::Permissions => rsx! { PermissionsTab {} },
                    SettingsTab::Memory => rsx! { MemoryTab {} },
                    SettingsTab::Skills => rsx! { SkillsTab {} },
                    SettingsTab::Mcp => rsx! { McpTab {} },
                    SettingsTab::Plugins => rsx! { PluginsTab {} },
                    SettingsTab::Security => rsx! { SecurityTab {} },
                    SettingsTab::Appearance => rsx! { super::appearance::AppearanceTab {} },
                }
            }
        }
    }
}
