//! Settings page: Providers (list + auth state), pi auth accounts,
//! permissions, memory and pointers to the core-managed skills / MCP /
//! plugins / security stores.

use crate::state_ui::{core, UiStore};
use dioxus::prelude::*;
use locagens_core::db::PermissionRule;
use locagens_core::db::Memory;
use locagens_core::pi_providers::ProviderMetadata;
use serde_json::Value;

#[component]
pub fn SettingsPage(initial_tab: crate::state_ui::SettingsTab) -> Element {
    let mut tab = use_signal(|| initial_tab);

    rsx! {
        div {
            class: "page-header",
            h2 { "Settings" }
            button {
                class: "subtle icon-btn",
                onclick: move |_| {
                    let mut store: UiStore = use_context();
                    store.page.set(crate::state_ui::AppPage::Chat);
                },
                "← Back to Chat"
            }
        }
        div {
            class: "settings-layout",
            nav {
                class: "settings-nav",
                for tab_item in crate::state_ui::SettingsTab::ALL.iter() {
                    button {
                        key: "{tab_item.label()}",
                        class: if *tab_item == *tab.read() { "active" } else { "" },
                        onclick: move |_| tab.set(*tab_item),
                        "{tab_item.label()}"
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
                    crate::state_ui::SettingsTab::Skills => rsx! { InfoTab {
                        title: "Skills".to_string(),
                        hint: "Skills are read from the user and project skills directories (Settings → Skills in the legacy UI).".to_string(),
                    } },
                    crate::state_ui::SettingsTab::Mcp => rsx! { InfoTab {
                        title: "MCP servers".to_string(),
                        hint: "MCP servers are configured in the core's mcp.json; they are listed here in Phase 7b.".to_string(),
                    } },
                    crate::state_ui::SettingsTab::Plugins => rsx! { InfoTab {
                        title: "Plugins".to_string(),
                        hint: "Plugins (custom tools and lifecycle hooks) are configured in the core's plugins store.".to_string(),
                    } },
                    crate::state_ui::SettingsTab::Security => rsx! { InfoTab {
                        title: "Security".to_string(),
                        hint: "The workspace sandbox is deny-by-default with explicit network grants per command.".to_string(),
                    } },
                }
            }
        }
    }
}

/// Configured providers with their models and key state.
#[component]
fn ProvidersTab() -> Element {
    let store: UiStore = use_context();
    let providers = store.providers_catalog.read().clone();
    let selected_provider = store.selected_provider.read().clone();

    if providers.is_empty() {
        return rsx! {
            div { class: "settings-empty", "No providers configured. Add one via `pi auth login` or the provider config file." }
        };
    }
    rsx! {
        div {
            class: "panel-section-title",
            "Providers ({providers.len()})"
        }
        for provider in providers.iter() {
            ProviderRow { provider: provider.clone(), selected: provider.id == selected_provider }
        }
    }
}

#[component]
fn ProviderRow(provider: ProviderMetadata, selected: bool) -> Element {
    let store: UiStore = use_context();
    rsx! {
        div {
            class: "settings-row",
            div {
                class: "grow",
                div {
                    style: "font-weight:600",
                    "{provider.display_name} "
                    if selected { span { class: "badge accent", "default" } }
                }
                div { class: "mono", style: "font-size:11px", "{provider.id} · {provider.kind}" }
            }
            div {
                class: "badge",
                "{provider.models.len()} models"
            }
            button {
                onclick: move |_| {
                    let mut store = store;
                    store.selected_provider.set(provider.id.clone());
                    if let Some(model) = provider.models.first() {
                        store.selected_model.set(model.clone());
                    }
                },
                "Use"
            }
        }
    }
}

/// Native pi auth.json accounts.
#[component]
fn AuthTab() -> Element {
    let accounts: Signal<Vec<Value>> = use_signal(Vec::new);
    use_effect(move || {
        let mut accounts = accounts;
        spawn(async move {
            let state = core();
            accounts.set(locagens_core::api::providers::get_pi_auth(&state).await.unwrap_or_default());
        });
    });
    let data = accounts.read().clone();
    if data.is_empty() {
        return rsx! {
            div { class: "settings-empty", "No pi auth accounts found. Run `pi auth login` in a terminal." }
        };
    }
    rsx! {
        for (index, account) in data.iter().enumerate() {
            div {
                key: "{index}",
                class: "settings-row",
                div {
                    class: "grow mono",
                    style: "font-size:12px",
                    "{serde_json::to_string_pretty(account).unwrap_or_default()}"
                }
            }
        }
    }
}

/// Stored permission rules (standing grants) with delete.
#[component]
fn PermissionsTab() -> Element {
    let rules: Signal<Vec<PermissionRule>> = use_signal(Vec::new);
    let store: UiStore = use_context();

    use_effect(move || {
        let mut rules = rules;
        spawn(async move {
            let state = core();
            rules.set(locagens_core::api::permissions::list_permissions(&state).await.unwrap_or_default());
        });
    });

    let data = rules.read().clone();
    if data.is_empty() {
        return rsx! {
            div { class: "settings-empty", "No standing permission grants yet. Approve with \"Always Allow\" / \"Allow in Project\" to create them." }
        };
    }
    rsx! {
        for rule in data.iter() {
            {
                let id = rule.id;
                rsx! {
                    div {
                        key: "{id}",
                        class: "settings-row",
                        div {
                            class: "grow",
                            div {
                                style: "font-weight:600",
                                "{rule.tool} "
                                span { class: "badge", "{rule.status}" }
                            }
                            div { class: "mono", style: "font-size:11px", "{rule.command} · {rule.project_path}" }
                        }
                        button {
                            class: "danger",
                            onclick: move |_| {
                                let mut rules = rules;
                                spawn(async move {
                                    let state = core();
                                    if let Err(err) =
                                        locagens_core::api::permissions::delete_permission(&state, id).await
                                    {
                                        store.toast(format!("Failed to delete rule: {err}"));
                                    }
                                    rules.set(locagens_core::api::permissions::list_permissions(&state).await.unwrap_or_default());
                                });
                            },
                            "Delete"
                        }
                    }
                }
            }
        }
    }
}

/// Memory entries with create + delete.
#[component]
fn MemoryTab() -> Element {
    let memories: Signal<Vec<Memory>> = use_signal(Vec::new);
    let mut new_memory = use_signal(String::new);
    let store: UiStore = use_context();

    use_effect(move || {
        let mut memories = memories;
        spawn(async move {
            let state = core();
            memories.set(locagens_core::api::memory::list_memories(&state).await.unwrap_or_default());
        });
    });

    let data = memories.read().clone();
    rsx! {
        div {
            class: "composer-row-top",
            input {
                placeholder: "Remember… (a fact the agent should always know)",
                value: "{new_memory.read()}",
                oninput: move |event| new_memory.set(event.value()),
                onkeydown: move |event: KeyboardEvent| {
                    if event.key() == Key::Enter {
                        let content = new_memory.read().trim().to_string();
                        if !content.is_empty() {
                            let mut memories = memories;
                            spawn(async move {
                                let state = core();
                                if let Err(err) = locagens_core::api::memory::create_memory(
                                    &state,
                                    locagens_core::api::memory::CreateMemoryInput {
                                        scope: Some("global".to_string()),
                                        category: None,
                                        content: Some(content),
                                        project_path: None,
                                    },
                                )
                                .await {
                                    store.toast(format!("Failed to save memory: {err}"));
                                }
                                new_memory.set(String::new());
                                memories.set(locagens_core::api::memory::list_memories(&state).await.unwrap_or_default());
                            });
                        }
                    }
                },
            }
        }
        if data.is_empty() {
            div { class: "settings-empty", "No memories stored yet." }
        }
        for memory in data.iter() {
            {
                let id = memory.id;
                rsx! {
                    div {
                        key: "{id}",
                        class: "settings-row",
                        div {
                            class: "grow",
                            "{memory.content}"
                        }
                        button {
                            class: "danger",
                            onclick: move |_| {
                                let mut memories = memories;
                                spawn(async move {
                                    let state = core();
                                    if let Err(err) = locagens_core::api::memory::delete_memory(&state, id).await {
                                        store.toast(format!("Failed to delete memory: {err}"));
                                    }
                                    memories.set(locagens_core::api::memory::list_memories(&state).await.unwrap_or_default());
                                });
                            },
                            "Delete"
                        }
                    }
                }
            }
        }
    }
}

/// Static info tab for core-managed feature stores.
#[component]
fn InfoTab(title: String, hint: String) -> Element {
    rsx! {
        div {
            class: "panel-section-title", "{title}"
        }
        div { class: "settings-empty", "{hint}" }
    }
}
