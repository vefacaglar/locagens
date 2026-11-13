//! Providers tab: configured providers with their models and key state.

use crate::state_ui::UiStore;
use dioxus::prelude::*;
use locagens_core::pi_providers::ProviderMetadata;

/// Configured providers with their models and key state.
#[component]
pub(super) fn ProvidersTab() -> Element {
    let store: UiStore = use_context();
    let providers = store.providers_catalog.read().clone();
    let selected_provider = store.selected_provider.read().clone();

    if providers.is_empty() {
        return rsx! {
            div { class: "settings-empty", "{store.t(\"providers.none\")}" }
        };
    }
    rsx! {
        div {
            class: "panel-section-title",
            "{store.t(\"tab.providers\")} ({providers.len()})"
        }
        for provider in providers.iter() {
            ProviderRow { provider: provider.clone(), selected: provider.id == selected_provider }
        }
    }
}

#[component]
pub(super) fn ProviderRow(provider: ProviderMetadata, selected: bool) -> Element {
    let store: UiStore = use_context();
    rsx! {
        div {
            class: "settings-row",
            div {
                class: "grow",
                div {
                    style: "font-weight:600",
                    "{provider.display_name} "
                    if selected { span { class: "badge accent", "{store.t(\"providers.default\")}" } }
                }
                div { class: "mono", style: "font-size:11px", "{provider.id} · {provider.kind}" }
            }
            div {
                class: "badge",
                "{provider.models.len()} {store.t(\"providers.models\")}"
            }
            button {
                onclick: move |_| {
                    let mut store = store;
                    store.selected_provider.set(provider.id.clone());
                    if let Some(model) = provider.models.first() {
                        store.selected_model.set(model.clone());
                    }
                },
                "{store.t(\"providers.use\")}"
            }
        }
    }
}
