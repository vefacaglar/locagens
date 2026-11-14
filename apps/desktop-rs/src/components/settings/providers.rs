//! Models & accounts tab (UX-001): connect a provider in place (`Sign in` for
//! OAuth providers, `Add API key` for the others, results shown under the
//! row, never a toast), then every connected provider's models with an
//! on/off switch that controls what the model picker shows.

use super::auth::{begin_oauth_login, reload_accounts, submit_pasted_redirect};
use crate::components::ConfirmButton;
use crate::state_ui::{core, UiStore};
use dioxus::prelude::*;
use locagens_core::api::providers::OAuthLoginStart;
use locagens_core::pi_providers::{LoginStatus, ProviderMetadata, SetPiAuthInput, TestProviderInput};
use serde_json::Value;

/// Connect rows plus the model switches of connected providers.
#[component]
pub(super) fn ProvidersTab() -> Element {
    let store: UiStore = use_context();
    let accounts: Signal<Vec<Value>> = use_signal(Vec::new);
    let pending: Signal<Option<OAuthLoginStart>> = use_signal(|| None);
    let login_task: Signal<Option<dioxus::core::Task>> = use_signal(|| None);
    let refresh = use_signal(|| 0u32);
    use_effect(move || {
        let _ = refresh.read();
        reload_accounts(accounts);
    });
    let logins = {
        let _ = refresh.read();
        let _ = accounts.read();
        locagens_core::api::providers::login_providers(&core())
    };
    let providers = store.providers_catalog.read().clone();
    let waiting = pending.read().clone();

    rsx! {
        div { class: "panel-section-title", "{store.t(\"providers.connect\")}" }
        div { class: "settings-hint", "{store.t(\"providers.connect_hint\")}" }
        for login in logins.iter() {
            {
                let (status, preview) = match &login.status {
                    LoginStatus::OAuth => ("oauth".to_string(), String::new()),
                    LoginStatus::ApiKey => {
                        let preview = accounts
                            .read()
                            .iter()
                            .find(|account| account["providerId"] == login.id.as_str())
                            .and_then(|account| account["keyPreview"].as_str().map(str::to_string))
                            .unwrap_or_default();
                        ("api_key".to_string(), preview)
                    }
                    LoginStatus::Env(var) => ("env".to_string(), var.clone()),
                    LoginStatus::SignedOut => ("signed_out".to_string(), String::new()),
                };
                let oauth_id = locagens_core::api::providers::oauth_providers()
                    .into_iter()
                    .find(|info| info.id == login.id)
                    .map(|info| info.id);
                rsx! {
                    ConnectRow {
                        key: "{login.id}",
                        provider_id: login.id.clone(),
                        name: login.name.clone(),
                        status,
                        preview,
                        oauth_id,
                        oauth_label: login.oauth_label.clone(),
                        takes_key: login.api_key_env.is_some() || login.oauth_label.is_none(),
                        busy: waiting.is_some(),
                        on_sign_in: move |id: &'static str| begin_oauth_login(id, accounts, pending, login_task, store),
                        refresh,
                    }
                }
            }
        }
        if waiting.is_some() {
            OAuthPending { accounts, pending, login_task }
        }
        div { class: "panel-section-title", "{store.t(\"providers.models_title\")} ({providers.len()})" }
        if providers.is_empty() {
            div { class: "settings-empty", "{store.t(\"providers.none\")}" }
        } else {
            div { class: "settings-hint", "{store.t(\"providers.subtitle\")}" }
        }
        for provider in providers.iter() {
            ProviderRow { key: "{provider.id}", provider: provider.clone() }
        }
    }
}

/// One provider in the Connect section, with its inline API-key form.
#[component]
fn ConnectRow(
    provider_id: String,
    name: String,
    status: String,
    preview: String,
    oauth_id: Option<&'static str>,
    oauth_label: Option<String>,
    takes_key: bool,
    busy: bool,
    on_sign_in: EventHandler<&'static str>,
    refresh: Signal<u32>,
) -> Element {
    let store: UiStore = use_context();
    let mut editing = use_signal(|| false);
    let mut key = use_signal(String::new);
    let mut saving = use_signal(|| false);
    // Inline result: (ok, text).
    let mut result: Signal<Option<(bool, String)>> = use_signal(|| None);
    let status_text = match status.as_str() {
        "oauth" => store.t("providers.signed_in").to_string(),
        "api_key" if preview.is_empty() => store.t("providers.connected_key").to_string(),
        "api_key" => store.tf("providers.connected_key_preview", &[&preview]),
        "env" => store.tf("providers.from_env", &[&preview]),
        _ => String::new(),
    };
    let connected = status != "signed_out";
    let removable = status == "oauth" || status == "api_key";
    let save_id = provider_id.clone();
    let remove_id = provider_id.clone();

    rsx! {
        div {
            class: "settings-row connect-row",
            div {
                class: "grow",
                div { class: "connect-name", "{name}" }
                if !status_text.is_empty() {
                    div { class: "connect-status ok-text", "{status_text}" }
                }
            }
            if removable {
                ConfirmButton {
                    label: store.t("providers.remove").to_string(),
                    class: "subtle small".to_string(),
                    on_confirm: move |_| {
                        let provider_id = remove_id.clone();
                        let mut refresh = refresh;
                        spawn(async move {
                            let input = SetPiAuthInput { provider_id, action: "delete".into(), api_key: None };
                            match locagens_core::api::providers::set_pi_auth(&core(), &input).await {
                                Ok(()) => result.set(None),
                                Err(err) => result.set(Some((false, err.to_string()))),
                            }
                            store.refresh_providers();
                            refresh += 1;
                        });
                    },
                }
            }
            if !connected || status == "env" {
                if let (Some(id), Some(label)) = (oauth_id, oauth_label.clone()) {
                    button {
                        disabled: busy,
                        onclick: move |_| on_sign_in.call(id),
                        "{label}"
                    }
                }
                if takes_key && !*editing.read() {
                    button {
                        onclick: move |_| {
                            editing.set(true);
                            result.set(None);
                        },
                        "{store.t(\"providers.add_key\")}"
                    }
                }
            }
        }
        if *editing.read() {
            div {
                class: "connect-form",
                input {
                    r#type: "password",
                    class: "grow",
                    autocomplete: "off",
                    placeholder: store.t("providers.key_placeholder"),
                    "aria-label": store.tf("providers.key_label", &[&name]),
                    value: "{key.read()}",
                    onmounted: move |event| async move {
                        let _ = event.set_focus(true).await;
                    },
                    oninput: move |event| key.set(event.value()),
                }
                button {
                    class: "primary",
                    disabled: key.read().trim().is_empty() || *saving.read(),
                    onclick: move |_| {
                        let provider_id = save_id.clone();
                        let api_key = key.peek().trim().to_string();
                        let mut refresh = refresh;
                        saving.set(true);
                        spawn(async move {
                            let state = core();
                            let input = SetPiAuthInput { provider_id: provider_id.clone(), action: "set".into(), api_key: Some(api_key) };
                            if let Err(err) = locagens_core::api::providers::set_pi_auth(&state, &input).await {
                                result.set(Some((false, err.to_string())));
                                saving.set(false);
                                return;
                            }
                            key.set(String::new());
                            editing.set(false);
                            let test = TestProviderInput { provider_id: provider_id.clone(), base_url: None, api_key: None, model: None };
                            let tested = locagens_core::api::providers::test_provider(&state, &test).await.ok();
                            let models = locagens_core::api::providers::list_providers(&state)
                                .await
                                .ok()
                                .and_then(|list| list.into_iter().find(|p| p.id == provider_id))
                                .map(|p| p.models.len())
                                .unwrap_or(0);
                            result.set(Some(match tested {
                                Some(test) if !test.success => (false, store.tf("providers.key_rejected", &[&test.error.unwrap_or_default()])),
                                _ => (true, store.tf("providers.key_ok", &[&models.to_string()])),
                            }));
                            store.refresh_providers();
                            saving.set(false);
                            refresh += 1;
                        });
                    },
                    "{store.t(\"common.save\")}"
                }
                button {
                    onclick: move |_| {
                        editing.set(false);
                        key.set(String::new());
                    },
                    "{store.t(\"common.cancel\")}"
                }
            }
            div { class: "settings-hint connect-hint", "{store.t(\"providers.key_hint\")}" }
        }
        if let Some((ok, text)) = result.read().clone() {
            div { class: if ok { "connect-result ok-text" } else { "connect-result err-text" }, role: "status", "{text}" }
        }
    }
}

/// A started OAuth sign-in: the code or browser hint, and the manual
/// redirect fallback.
#[component]
fn OAuthPending(
    accounts: Signal<Vec<Value>>,
    pending: Signal<Option<OAuthLoginStart>>,
    login_task: Signal<Option<dioxus::core::Task>>,
) -> Element {
    let store: UiStore = use_context();
    let mut pasted_redirect = use_signal(String::new);
    let Some(start) = pending.read().clone() else { return rsx! {} };
    let url = start.url_to_open().to_string();
    rsx! {
        div {
            class: "settings-row",
            div {
                class: "grow",
                if let Some(code) = start.user_code.clone() {
                    div { {store.t("auth.enter_code")} }
                    div { class: "mono device-code", "{code}" }
                } else {
                    div { {store.t("auth.finish_in_browser")} }
                }
                div { class: "mono path-line", "{url}" }
            }
            button {
                onclick: move |_| {
                    if let Err(err) = locagens_core::api::providers::open_verification_url(&url) {
                        store.toast(format!("{err}"));
                    }
                },
                {store.t("auth.open_browser")}
            }
            button {
                class: "danger",
                onclick: move |_| {
                    let mut pending = pending;
                    let mut login_task = login_task;
                    if let Some(task) = login_task.take() {
                        task.cancel();
                    }
                    pending.set(None);
                },
                {store.t("common.cancel")}
            }
        }
        if start.auth_url.is_some() {
            div {
                class: "settings-row",
                input {
                    class: "grow",
                    placeholder: store.t("auth.paste_placeholder"),
                    value: "{pasted_redirect.read()}",
                    oninput: move |event| pasted_redirect.set(event.value()),
                }
                button {
                    disabled: pasted_redirect.read().trim().is_empty(),
                    onclick: {
                        let start = start.clone();
                        move |_| submit_pasted_redirect(start.clone(), pasted_redirect, accounts, pending, login_task, store)
                    },
                    {store.t("auth.submit")}
                }
            }
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
                div { class: "mono path-line", "{provider.id} · {provider.kind}" }
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
