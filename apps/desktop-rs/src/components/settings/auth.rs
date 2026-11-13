//! Auth tab: auth.json accounts and OAuth sign-in.

use crate::state_ui::{core, UiStore};
use dioxus::prelude::*;
use locagens_core::api::providers::OAuthLoginStart;
use serde_json::Value;

/// Native auth.json accounts (~/.locagens, Codex / pi fallbacks) plus OAuth
/// sign-in for providers that support it.
#[component]
pub(super) fn AuthTab() -> Element {
    let accounts: Signal<Vec<Value>> = use_signal(Vec::new);
    let pending: Signal<Option<OAuthLoginStart>> = use_signal(|| None);
    let login_task: Signal<Option<dioxus::core::Task>> = use_signal(|| None);
    let mut pasted_redirect = use_signal(String::new);
    let store: UiStore = use_context();
    use_effect(move || reload_accounts(accounts));

    let data = accounts.read().clone();
    let waiting = pending.read().clone();
    rsx! {
        for info in locagens_core::api::providers::oauth_providers() {
            div {
                key: "{info.id}",
                class: "settings-row",
                div { class: "grow", "{info.name}" }
                button {
                    disabled: waiting.is_some(),
                    onclick: move |_| begin_oauth_login(info.id, accounts, pending, login_task, store),
                    "{info.login_label}"
                }
            }
        }
        if let Some(start) = waiting {
            div {
                class: "settings-row",
                div {
                    class: "grow",
                    if let Some(code) = start.user_code.clone() {
                        div { {store.t("auth.enter_code")} }
                        div { class: "mono", style: "font-size:20px;font-weight:600", "{code}" }
                    } else {
                        div { {store.t("auth.finish_in_browser")} }
                    }
                    div { class: "mono", style: "font-size:11px", "{start.url_to_open()}" }
                }
                button {
                    onclick: {
                        let url = start.url_to_open().to_string();
                        move |_| {
                            if let Err(err) = locagens_core::api::providers::open_verification_url(&url) {
                                store.toast(format!("{err}"));
                            }
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
        if data.is_empty() {
            div { class: "settings-empty", {store.t("auth.empty")} }
        }
        for (index, account) in data.iter().enumerate() {
            div {
                key: "{index}",
                class: "settings-row",
                div {
                    class: "grow",
                    span { style: "font-weight:600", "{account[\"providerId\"].as_str().unwrap_or(\"?\")} " }
                    span { class: "badge", "{account[\"type\"].as_str().unwrap_or(\"unknown\")}" }
                }
                div { class: "mono", style: "font-size:12px", "{account_detail(&store, account)}" }
            }
        }
    }
}

pub(super) fn reload_accounts(accounts: Signal<Vec<Value>>) {
    let mut accounts = accounts;
    spawn(async move {
        let state = core();
        accounts.set(locagens_core::api::providers::get_pi_auth(&state).await.unwrap_or_default());
    });
}

/// Manual fallback of an authorization-URL login: completes it with the
/// redirect URL the user pasted and stops the background wait.
pub(super) fn submit_pasted_redirect(
    start: OAuthLoginStart,
    pasted_redirect: Signal<String>,
    accounts: Signal<Vec<Value>>,
    pending: Signal<Option<OAuthLoginStart>>,
    login_task: Signal<Option<dioxus::core::Task>>,
    store: UiStore,
) {
    let mut pasted_redirect = pasted_redirect;
    let mut pending = pending;
    let mut login_task = login_task;
    let pasted = pasted_redirect.read().trim().to_string();
    spawn(async move {
        let state = core();
        match locagens_core::api::providers::submit_oauth_redirect(&state, &start, &pasted).await {
            Ok(()) => {
                if let Some(task) = login_task.take() {
                    task.cancel();
                }
                pending.set(None);
                pasted_redirect.set(String::new());
                store.toast(store.t("auth.signed_in").to_string());
                store.refresh_providers();
                reload_accounts(accounts);
            }
            Err(err) => store.toast(store.tf("auth.sign_in_failed", &[&err.to_string()])),
        }
    });
}

/// Starts a sign-in (device code or authorization URL), shows what the user
/// needs, then waits for the approval or redirect in the background.
pub(super) fn begin_oauth_login(
    provider_id: &'static str,
    accounts: Signal<Vec<Value>>,
    pending: Signal<Option<OAuthLoginStart>>,
    login_task: Signal<Option<dioxus::core::Task>>,
    store: UiStore,
) {
    let mut pending = pending;
    let mut login_task = login_task;
    spawn(async move {
        let state = core();
        let start = match locagens_core::api::providers::start_oauth_login(&state, provider_id).await {
            Ok(start) => start,
            Err(err) => {
                store.toast(store.tf("auth.sign_in_failed", &[&err.to_string()]));
                return;
            }
        };
        pending.set(Some(start.clone()));
        let task = spawn(async move {
            let state = core();
            match locagens_core::api::providers::finish_oauth_login(&state, &start).await {
                Ok(()) => {
                    store.toast(store.t("auth.signed_in").to_string());
                    store.refresh_providers();
                }
                Err(err) => store.toast(store.tf("auth.sign_in_failed", &[&err.to_string()])),
            }
            pending.set(None);
            login_task.set(None);
            reload_accounts(accounts);
        });
        login_task.set(Some(task));
    });
}

/// Human-readable credential detail: OAuth expiry or masked key preview.
pub(super) fn account_detail(store: &UiStore, account: &Value) -> String {
    if account["type"] == "oauth" {
        let expires_at = account["expiresAt"].as_i64().unwrap_or(0);
        if account["expired"] == true {
            return store.t("auth.token_expired").to_string();
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as i64)
            .unwrap_or(0);
        let minutes = ((expires_at - now_ms) / 60_000).max(0).to_string();
        return store.tf("auth.token_valid_for", &[&minutes]);
    }
    account["keyPreview"].as_str().unwrap_or("").to_string()
}
