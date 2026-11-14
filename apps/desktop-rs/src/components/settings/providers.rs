//! Providers tab: signed-in providers, their models and a per-model on/off
//! switch that controls what the composer's model picker shows.

use crate::state_ui::UiStore;
use dioxus::prelude::*;
use locagens_core::pi_providers::ProviderMetadata;

/// Configured providers with their models and key state.
#[component]
pub(super) fn ProvidersTab() -> Element {
    let store: UiStore = use_context();
    let providers = store.providers_catalog.read().clone();

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
        div { class: "settings-hint", "{store.t(\"providers.subtitle\")}" }
        for provider in providers.iter() {
            ProviderRow { key: "{provider.id}", provider: provider.clone() }
        }
    }
}

#[component]
pub(super) fn ProviderRow(provider: ProviderMetadata) -> Element {
    let store: UiStore = use_context();
    let mut expanded = use_signal(|| false);
    let open = *expanded.read();
    let shown = {
        let hidden = store.hidden_models.read();
        provider.models.iter().filter(|model| !hidden.contains(&format!("{}/{model}", provider.id))).count()
    };
    rsx! {
        button {
            class: "settings-row provider-head",
            "aria-expanded": "{open}",
            onclick: move |_| expanded.set(!open),
            div {
                class: "grow",
                div { style: "font-weight:600", "{provider.display_name}" }
                div { class: "mono", style: "font-size:11px", "{provider.id} · {provider.kind}" }
            }
            div {
                class: "badge",
                "{shown}/{provider.models.len()} {store.t(\"providers.models\")}"
            }
            span { class: "provider-chevron", if open { "▾" } else { "▸" } }
        }
        if open {
            div {
                class: "provider-models",
                for model in provider.models.iter() {
                    ModelSwitch { key: "{model}", provider_id: provider.id.clone(), model: model.clone() }
                }
            }
        }
    }
}

/// One model with an accessible on/off switch (a native button, so Space and
/// Enter toggle it).
#[component]
fn ModelSwitch(provider_id: String, model: String) -> Element {
    let store: UiStore = use_context();
    let hidden = store.hidden_models.read().contains(&format!("{provider_id}/{model}"));
    let on = !hidden;
    let label = format!("{} — {}", store.t("providers.show_model"), model);
    rsx! {
        div {
            class: "settings-row model-row",
            div { class: "grow mono", style: "font-size:12px", "{model}" }
            button {
                class: if on { "switch on" } else { "switch" },
                role: "switch",
                "aria-checked": "{on}",
                "aria-label": "{label}",
                onclick: move |_| store.set_model_hidden(&provider_id, &model, on),
                span { class: "switch-knob" }
            }
        }
    }
}
