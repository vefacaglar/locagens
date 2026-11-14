//! One searchable model picker (UX-013): a `model · effort` button that opens
//! a popover with a focused search field, models grouped by provider, `↑↓`
//! to move, `Enter` to pick, `Esc` to close, an effort row for the
//! highlighted model and "Manage models…". `Ctrl+M` / `/model` open it.

use super::icons::{icon_check, icon_chevron_down};
use crate::state_ui::{AppPage, SettingsTab, UiStore};
use dioxus::prelude::*;
use locagens_core::pi_providers::ProviderMetadata;

/// i18n key of the display label for a reasoning-effort value.
pub fn effort_key(effort: &str) -> &'static str {
    match effort {
        "minimal" => "effort.minimal",
        "low" => "effort.low",
        "medium" => "effort.medium",
        "high" => "effort.high",
        "xhigh" => "effort.xhigh",
        "max" => "effort.max",
        _ => "effort.medium",
    }
}

/// Supported reasoning-effort levels for a model: the provider's catalog
/// `modelSettings[model].reasoningEfforts` when present, capability fallbacks
/// otherwise. An empty list means the model has no effort control.
pub fn model_efforts(provider: Option<&ProviderMetadata>, model_id: &str) -> Vec<String> {
    if let Some(efforts) = provider
        .and_then(|p| p.model_settings.as_ref())
        .and_then(|ms| ms.get(model_id))
        .and_then(|entry| entry.get("reasoningEfforts"))
        .and_then(|v| v.as_array())
    {
        return efforts.iter().filter_map(|value| value.as_str().map(str::to_string)).collect();
    }
    locagens_core::pi_providers::model_reasoning_efforts(model_id, None, None)
}

/// The effective effort for a model: the selected one when supported,
/// otherwise the model's default ("medium" or the first supported level).
/// `None` means the model has no effort control at all.
pub fn effective_effort<'a>(selected: &'a str, efforts: &'a [String]) -> Option<&'a str> {
    if efforts.is_empty() {
        return None;
    }
    if efforts.iter().any(|e| e == selected) {
        return Some(selected);
    }
    if efforts.iter().any(|e| e == "medium") {
        return Some("medium");
    }
    efforts.first().map(String::as_str)
}

/// One pickable row: `(provider id, provider name, model id)`.
pub type ModelRow = (String, String, String);
/// `(provider id, provider name, rows with their flat position)`.
type ModelGroup = (String, String, Vec<(usize, ModelRow)>);

/// Visible models matching `query` (case-insensitive, on `provider/model`
/// and the provider's name), in catalog order.
pub fn filter_models(catalog: &[ProviderMetadata], query: &str) -> Vec<ModelRow> {
    let query = query.trim().to_lowercase();
    catalog
        .iter()
        .flat_map(|provider| {
            provider.models.iter().map(move |model| (provider.id.clone(), provider.display_name.clone(), model.clone()))
        })
        .filter(|(id, name, model)| {
            query.is_empty()
                || format!("{id}/{model}").to_lowercase().contains(&query)
                || name.to_lowercase().contains(&query)
        })
        .collect()
}

#[component]
pub fn ModelPicker() -> Element {
    let mut store: UiStore = use_context();
    let mut query = use_signal(String::new);
    let mut highlighted = use_signal(|| 0usize);
    let open = *store.model_picker_open.read();
    let catalog = store.visible_catalog();
    let selected_provider = store.selected_provider.read().clone();
    let selected_model = store.selected_model.read().clone();
    let selected_effort = store.selected_effort.read().clone();
    let current_provider = catalog.iter().find(|p| p.id == selected_provider).cloned();

    let effort_label = {
        let efforts = model_efforts(current_provider.as_ref(), &selected_model);
        effective_effort(&selected_effort, &efforts).map(|effort| store.t(effort_key(effort)))
    };
    let button_label = if selected_model.is_empty() { store.t("common.model").to_string() } else { selected_model.clone() };

    let rows = filter_models(&catalog, &query.read());
    let index = (*highlighted.read()).min(rows.len().saturating_sub(1));
    let highlighted_row = rows.get(index).cloned();
    let highlighted_efforts: Vec<String> = highlighted_row
        .as_ref()
        .map(|(provider_id, _, model)| model_efforts(catalog.iter().find(|p| &p.id == provider_id), model))
        .unwrap_or_default();
    let rows_for_keys = rows.clone();
    // Rows grouped under their provider, each keeping its flat position.
    let mut groups: Vec<ModelGroup> = Vec::new();
    for (position, row) in rows.iter().enumerate() {
        match groups.last_mut() {
            Some((id, _, entries)) if *id == row.0 => entries.push((position, row.clone())),
            _ => groups.push((row.0.clone(), row.1.clone(), vec![(position, row.clone())])),
        }
    }

    let mut close = move || {
        let mut store = store;
        store.model_picker_open.set(false);
        query.set(String::new());
        highlighted.set(0);
    };
    // Picking a model keeps the selected effort when the model supports it.
    let mut pick = move |(provider_id, _, model): ModelRow| {
        let efforts = model_efforts(store.visible_catalog().iter().find(|p| p.id == provider_id), &model);
        let effort = effective_effort(&store.selected_effort.peek(), &efforts).map(str::to_string);
        store.pick_model(&provider_id, &model, effort.as_deref());
        close();
        crate::state_ui::focus_composer();
    };

    rsx! {
        div {
            class: "composer-dropdown",
            button {
                class: if open { "composer-select-btn open" } else { "composer-select-btn" },
                title: store.tf("picker.title", &[&super::shortcut_label("M")]),
                "aria-haspopup": "listbox",
                "aria-expanded": "{open}",
                onclick: move |_| {
                    if open {
                        close();
                    } else {
                        // Pick up credentials added elsewhere (CLI, pi, Settings).
                        store.refresh_providers();
                        store.model_picker_open.set(true);
                    }
                },
                span { class: "ccm-model-name", "{button_label}" }
                if let Some(effort) = effort_label {
                    span { class: "ccm-model-effort", "· {effort}" }
                }
                {icon_chevron_down()}
            }
            if open {
                div {
                    class: "popover-backdrop",
                    onclick: move |_| close(),
                }
                div {
                    class: "composer-context-menu model-menu",
                    onclick: move |event| event.stop_propagation(),
                    input {
                        class: "ccm-search",
                        r#type: "text",
                        placeholder: store.t("picker.search"),
                        "aria-label": store.t("picker.search"),
                        role: "combobox",
                        "aria-controls": "model-list",
                        value: "{query.read()}",
                        onmounted: move |event| async move {
                            let _ = event.set_focus(true).await;
                        },
                        oninput: move |event| {
                            query.set(event.value());
                            highlighted.set(0);
                        },
                        onkeydown: move |event: KeyboardEvent| {
                            let last = rows_for_keys.len().saturating_sub(1);
                            match event.key() {
                                Key::ArrowDown => {
                                    event.prevent_default();
                                    let next = (*highlighted.peek() + 1).min(last);
                                    highlighted.set(next);
                                }
                                Key::ArrowUp => {
                                    event.prevent_default();
                                    let next = highlighted.peek().saturating_sub(1);
                                    highlighted.set(next);
                                }
                                Key::Enter => {
                                    event.prevent_default();
                                    let at = (*highlighted.peek()).min(last);
                                    if let Some(row) = rows_for_keys.get(at).cloned() {
                                        pick(row);
                                    }
                                }
                                Key::Escape => {
                                    event.prevent_default();
                                    event.stop_propagation();
                                    close();
                                }
                                _ => {}
                            }
                        },
                    }
                    div {
                        class: "ccm-list model-list",
                        id: "model-list",
                        role: "listbox",
                        if rows.is_empty() {
                            div { class: "ccm-empty", "{store.t(\"composer.no_models\")}" }
                        }
                        for (provider_id, provider_name, entries) in groups.iter() {
                            div {
                                key: "{provider_id}",
                                role: "group",
                                "aria-label": "{provider_name}",
                                div { class: "ccm-group", "{provider_name}" }
                                for (position, row) in entries.iter() {
                                    {
                                        let position = *position;
                                        let (row_provider, _, model) = row.clone();
                                        let is_current = row_provider == selected_provider && model == selected_model;
                                        let is_highlighted = position == index;
                                        let row_pick = row.clone();
                                        let class = match (is_current, is_highlighted) {
                                            (_, true) => "ccm-item active",
                                            (true, false) => "ccm-item current",
                                            _ => "ccm-item",
                                        };
                                        rsx! {
                                            button {
                                                key: "{model}",
                                                class: "{class}",
                                                role: "option",
                                                "aria-selected": "{is_current}",
                                                onmouseenter: move |_| highlighted.set(position),
                                                onclick: move |_| pick(row_pick.clone()),
                                                span { class: "ccm-item-label ccm-model-name", "{model}" }
                                                if is_current { {icon_check()} }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Some((provider_id, _, model)) = highlighted_row.clone().filter(|_| !highlighted_efforts.is_empty()) {
                        div {
                            class: "picker-effort",
                            span { class: "picker-effort-label", "{store.t(\"picker.effort\")}" }
                            div {
                                class: "seg",
                                role: "radiogroup",
                                "aria-label": store.t("picker.effort"),
                                for effort in highlighted_efforts.iter() {
                                    {
                                        let is_model = provider_id == selected_provider && model == selected_model;
                                        let active = is_model && effective_effort(&selected_effort, &highlighted_efforts) == Some(effort.as_str());
                                        let effort = effort.clone();
                                        let provider_id = provider_id.clone();
                                        let model = model.clone();
                                        rsx! {
                                            button {
                                                key: "{effort}",
                                                class: if active { "active" } else { "" },
                                                role: "radio",
                                                "aria-checked": "{active}",
                                                onclick: move |_| store.pick_model(&provider_id, &model, Some(&effort)),
                                                "{store.t(effort_key(&effort))}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    button {
                        class: "ccm-item ccm-footer",
                        onclick: move |_| {
                            close();
                            store.page.set(AppPage::Settings(SettingsTab::Providers));
                        },
                        span { class: "ccm-item-label", "{store.t(\"picker.manage\")}" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str, name: &str, models: &[&str]) -> ProviderMetadata {
        ProviderMetadata {
            id: id.into(),
            display_name: name.into(),
            kind: "openai".into(),
            models: models.iter().map(|m| m.to_string()).collect(),
            model_settings: None,
        }
    }

    #[test]
    fn search_matches_provider_model_and_provider_name() {
        let catalog = vec![provider("openai", "OpenAI", &["gpt-5", "o4-mini"]), provider("anthropic", "Anthropic", &["claude-x"])];
        assert_eq!(filter_models(&catalog, "").len(), 3);
        assert_eq!(filter_models(&catalog, "GPT"), vec![("openai".into(), "OpenAI".into(), "gpt-5".into())]);
        assert_eq!(filter_models(&catalog, "anthropic").len(), 1);
        assert_eq!(filter_models(&catalog, "openai/o4").len(), 1);
        assert!(filter_models(&catalog, "zzz").is_empty());
    }

    #[test]
    fn effort_falls_back_to_medium_or_first() {
        let efforts: Vec<String> = ["low", "medium", "high"].iter().map(|s| s.to_string()).collect();
        assert_eq!(effective_effort("high", &efforts), Some("high"));
        assert_eq!(effective_effort("max", &efforts), Some("medium"));
        let only: Vec<String> = vec!["high".into()];
        assert_eq!(effective_effort("low", &only), Some("high"));
        assert_eq!(effective_effort("low", &[]), None);
    }
}
