//! OAuth sign-in helpers of the Models & accounts tab.

use crate::state_ui::{core, UiStore};
use dioxus::prelude::*;
use locagens_core::api::providers::OAuthLoginStart;
use serde_json::Value;

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
