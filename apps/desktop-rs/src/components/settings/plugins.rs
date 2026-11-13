//! Plugins tab: install, toggle and uninstall.

use crate::state_ui::{core, UiStore};
use dioxus::prelude::*;
use locagens_core::api::plugins::ToggleInput;
use locagens_core::services::plugins::InstallPayload;
use serde_json::Value;

/// Plugins: list, install from template / local folder, toggle and uninstall.
#[component]
pub(super) fn PluginsTab() -> Element {
    let store: UiStore = use_context();
    let data: Signal<(Vec<Value>, Vec<Value>)> = use_signal(|| (Vec::new(), Vec::new()));
    let mut selected_template = use_signal(String::new);
    let mut local_path = use_signal(String::new);

    use_effect(move || {
        let mut data = data;
        spawn(async move {
            let state = core();
            if let Ok(response) = locagens_core::api::plugins::list_plugins(&state, None).await {
                data.set((response.plugins, response.templates));
            }
        });
    });

    let (plugins, templates) = data.read().clone();
    let selected_template_value = selected_template.read().clone();
    rsx! {
        div {
            class: "panel-section-title", "{store.t(\"tab.plugins\")}"
        }
        div {
            class: "panel-card",
            div { class: "panel-section-title", "{store.t(\"plugins.install\")}" }
            div {
                class: "composer-row-top",
                select {
                    value: "{selected_template_value}",
                    onchange: move |event| selected_template.set(event.value()),
                    option { value: "", "{store.t(\"plugins.choose\")}" }
                    for template in templates.iter() {
                        {
                            let template_id = template.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                            let template_name = template.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                            rsx! {
                                option {
                                    key: "{template_id}",
                                    value: "{template_id}",
                                    "{template_name}"
                                }
                            }
                        }
                    }
                }
                button {
                    class: "primary",
                    disabled: selected_template_value.is_empty(),
                    onclick: move |_| {
                        let mut data = data;
                        let uri = selected_template.read().clone();
                        spawn(async move {
                            let state = core();
                            let result = locagens_core::api::plugins::install_plugin(
                                &state,
                                InstallPayload {
                                    source: "template".to_string(),
                                    uri,
                                    scope: Some("user".to_string()),
                                    project_path: None,
                                },
                            )
                            .await;
                            match result {
                                Ok(plugin) => store.toast(store.tf("toast.installed", &[plugin.get("name").and_then(Value::as_str).unwrap_or(store.t("plugin.fallback_name"))])),
                                Err(err) => store.toast(store.tf("toast.install_failed", &[&err.to_string()])),
                            }
                            if let Ok(response) = locagens_core::api::plugins::list_plugins(&state, None).await {
                                data.set((response.plugins, response.templates));
                            }
                        });
                    },
                    "{store.t(\"plugins.install_template\")}"
                }
            }
            div {
                class: "composer-row-top",
                input {
                    placeholder: store.t("plugins.local_path"),
                    value: "{local_path.read()}",
                    oninput: move |event| local_path.set(event.value()),
                }
                button {
                    disabled: local_path.read().trim().is_empty(),
                    onclick: move |_| {
                        let mut data = data;
                        let uri = local_path.read().trim().to_string();
                        spawn(async move {
                            let state = core();
                            let result = locagens_core::api::plugins::install_plugin(
                                &state,
                                InstallPayload {
                                    source: "local".to_string(),
                                    uri,
                                    scope: Some("user".to_string()),
                                    project_path: None,
                                },
                            )
                            .await;
                            match result {
                                Ok(plugin) => {
                                    store.toast(store.tf("toast.installed", &[plugin.get("name").and_then(Value::as_str).unwrap_or(store.t("plugin.fallback_name"))]));
                                    local_path.set(String::new());
                                }
                                Err(err) => store.toast(store.tf("toast.install_failed", &[&err.to_string()])),
                            }
                            if let Ok(response) = locagens_core::api::plugins::list_plugins(&state, None).await {
                                data.set((response.plugins, response.templates));
                            }
                        });
                    },
                    "{store.t(\"plugins.install_local\")}"
                }
            }
        }
        if plugins.is_empty() {
            div { class: "settings-empty", "{store.t(\"plugins.none\")}" }
        }
        for plugin in plugins.iter() {
            {
                let id = plugin.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                let name = plugin.get("name").and_then(Value::as_str).unwrap_or(&id).to_string();
                let scope = plugin.get("scope").and_then(Value::as_str).unwrap_or("user").to_string();
                let enabled = plugin.get("enabled").and_then(Value::as_bool).unwrap_or(true);
                let toggle_id = id.clone();
                let delete_id = id.clone();
                rsx! {
                    div {
                        key: "{id}",
                        class: "settings-row",
                        div {
                            class: "grow",
                            div {
                                style: "font-weight:600",
                                "{name} "
                                span { class: if enabled { "badge ok" } else { "badge" }, if enabled { "{store.t(\"common.enabled\")}" } else { "{store.t(\"common.disabled\")}" } }
                                span { class: "badge", "{scope}" }
                            }
                            div { class: "mono", style: "font-size:11px", "{id}" }
                        }
                        button {
                            onclick: move |_| {
                                let mut data = data;
                                let toggle_id = toggle_id.clone();
                                let toggle_enabled = !enabled;
                                spawn(async move {
                                    let state = core();
                                    if let Err(err) = locagens_core::api::plugins::toggle_plugin(
                                        &state,
                                        &toggle_id,
                                        ToggleInput {
                                            enabled: Some(toggle_enabled),
                                            scope: None,
                                            project_path: None,
                                        },
                                    )
                                    .await {
                                        store.toast(store.tf("toast.toggle_failed", &[&err.to_string()]));
                                    }
                                    if let Ok(response) = locagens_core::api::plugins::list_plugins(&state, None).await {
                                        data.set((response.plugins, response.templates));
                                    }
                                });
                            },
                            if enabled { "{store.t(\"common.disable\")}" } else { "{store.t(\"common.enable\")}" }
                        }
                        button {
                            class: "danger",
                            onclick: move |_| {
                                let mut data = data;
                                let delete_id = delete_id.clone();
                                spawn(async move {
                                    let state = core();
                                    if let Err(err) = locagens_core::api::plugins::delete_plugin(&state, &delete_id, Some("user"), None).await {
                                        store.toast(store.tf("toast.uninstall_failed", &[&err.to_string()]));
                                    }
                                    if let Ok(response) = locagens_core::api::plugins::list_plugins(&state, None).await {
                                        data.set((response.plugins, response.templates));
                                    }
                                });
                            },
                            "{store.t(\"plugins.uninstall\")}"
                        }
                    }
                }
            }
        }
    }
}
