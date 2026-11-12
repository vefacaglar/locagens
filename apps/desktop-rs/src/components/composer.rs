//! Composer: prompt textarea (Enter to send, Shift+Enter newline), file
//! attachments, @file / @symbol quick-inserts, mode / provider / model /
//! effort selectors and the Send / Continue / Queue action.

use crate::state_ui::{core, UiStore};
use dioxus::prelude::*;
use locagens_core::api::runs::{ContinueRunPayload, CreateRunPayload};
use locagens_core::pi_providers::ProviderMetadata;

#[component]
pub fn Composer() -> Element {
    let store: UiStore = use_context();
    let busy = use_signal(|| false);
    let mut project_files: Signal<Vec<String>> = use_signal(Vec::new);

    let providers = store.providers_catalog.read().clone();
    let selected_provider = store.selected_provider.read().clone();
    let selected_model = store.selected_model.read().clone();
    let selected_effort = store.selected_effort.read().clone();
    let selected_mode = store.selected_mode.read().clone();
    let prompt_text = store.prompt.read().clone();

    let provider_models: Vec<String> = providers
        .iter()
        .find(|provider: &&ProviderMetadata| provider.id == selected_provider)
        .map(|provider| provider.models.clone())
        .unwrap_or_default();

    let run = store.active_run.read().clone();
    let run_status = run.as_ref().map(|run| run.status.clone());
    let is_generating = run_status.as_deref() == Some("generating");
    let is_awaiting =
        matches!(run_status.as_deref(), Some("awaiting_permission") | Some("awaiting_input"));
    let has_run = run.is_some();
    let send_label = if is_generating {
        "Queue"
    } else if has_run {
        "Continue"
    } else {
        "Send"
    };

    rsx! {
        div {
            class: "composer",
            // Quick-insert bar.
            div {
                class: "composer-quickbar",
                span { class: "composer-hint", "Insert:" }
                button {
                    class: "chip",
                    onclick: move |_| {
                        let store = store;
                        let path = store.selected_project.read().clone().unwrap_or_default();
                        spawn(async move {
                            let state = core();
                            let files = locagens_core::api::projects::list_files(&state, &path, None)
                                .await
                                .unwrap_or_default();
                            project_files.set(files);
                        });
                    },
                    "@file"
                }
                button {
                    class: "chip",
                    onclick: move |_| {
                        let mut store = store;
                        store.prompt.with_mut(|prompt| prompt.push('@'));
                    },
                    "@symbol"
                }
                if !project_files.read().is_empty() {
                    span { class: "composer-hint", "Files:" }
                    for (index, file) in project_files.read().iter().take(12).enumerate() {
                        {
                            let file = file.clone();
                            rsx! {
                                button {
                                    key: "{index}",
                                    class: "chip",
                                    title: "{file}",
                                    onclick: move |_| {
                                        let mut store = store;
                                        let insert = format!("@{} ", file);
                                        store.prompt.with_mut(|prompt| prompt.push_str(&insert));
                                    },
                                    "{file}"
                                }
                            }
                        }
                    }
                }
            }
            div {
                class: "composer-row-top",
                textarea {
                    placeholder: "Describe a task… (Enter to send, Shift+Enter for newline)",
                    value: "{prompt_text}",
                    rows: "2",
                    oninput: move |event| {
                        let mut store = store;
                        store.prompt.set(event.value());
                    },
                    onkeydown: move |event: KeyboardEvent| {
                        if event.key() == Key::Enter && !event.modifiers().shift() {
                            event.prevent_default();
                            submit(&store, busy);
                        }
                    },
                }
                button {
                    class: "icon-btn",
                    title: "Attach files (@path references)",
                    onclick: move |_| {
                        let mut store = store;
                        spawn(async move {
                            if let Some(files) = rfd::AsyncFileDialog::new().pick_files().await {
                                let references: Vec<String> = files
                                    .iter()
                                    .map(|file| format!("@{} ", file.path().to_string_lossy()))
                                    .collect();
                                store.prompt.with_mut(|prompt| {
                                    prompt.push_str(&references.join(""));
                                });
                            }
                        });
                    },
                    "📎"
                }
                button {
                    class: "primary",
                    disabled: *busy.read() || is_awaiting || prompt_text.trim().is_empty(),
                    title: if is_awaiting { "Resolve the pending card above first" } else { "" },
                    onclick: move |_| submit(&store, busy),
                    "{send_label}"
                }
            }
            div {
                class: "composer-row-bottom",
                select {
                    title: "Mode",
                    value: "{selected_mode}",
                    onchange: move |event| {
                        let mut store = store;
                        store.selected_mode.set(event.value());
                    },
                    option { value: "accept_edits", selected: selected_mode == "accept_edits", "Build (accept_edits)" }
                    option { value: "plan", selected: selected_mode == "plan", "Plan (plan)" }
                    option { value: "auto", selected: selected_mode == "auto", "Full Access (auto)" }
                }
                select {
                    title: "Provider",
                    value: "{selected_provider}",
                    onchange: move |event| {
                        let mut store = store;
                        let provider_id = event.value();
                        store.selected_provider.set(provider_id.clone());
                        // Reset the model to the provider's first model.
                        let first = store.providers_catalog.read().iter()
                            .find(|provider| provider.id == provider_id)
                            .and_then(|provider| provider.models.first().cloned());
                        if let Some(first) = first {
                            store.selected_model.set(first);
                        }
                    },
                    if providers.is_empty() {
                        option { value: "", "No providers configured" }
                    }
                    for provider in providers.iter() {
                        option {
                            key: "{provider.id}",
                            value: "{provider.id}",
                            selected: provider.id == selected_provider,
                            "{provider.display_name}"
                        }
                    }
                }
                select {
                    title: "Model",
                    value: "{selected_model}",
                    onchange: move |event| {
                        let mut store = store;
                        store.selected_model.set(event.value());
                    },
                    if provider_models.is_empty() {
                        option { value: "{selected_model}", "{selected_model}" }
                    }
                    for model in provider_models.iter() {
                        option {
                            key: "{model}",
                            value: "{model}",
                            selected: *model == selected_model,
                            "{model}"
                        }
                    }
                }
                select {
                    title: "Reasoning effort",
                    value: "{selected_effort}",
                    onchange: move |event| {
                        let mut store = store;
                        store.selected_effort.set(event.value());
                    },
                    option { value: "low", selected: selected_effort == "low", "Effort: low" }
                    option { value: "medium", selected: selected_effort == "medium", "Effort: medium" }
                    option { value: "high", selected: selected_effort == "high", "Effort: high" }
                    option { value: "xhigh", selected: selected_effort == "xhigh", "Effort: xhigh" }
                }
                span { class: "composer-spacer" }
                span {
                    class: "composer-hint",
                    "{selected_provider} / {selected_model}"
                }
            }
        }
    }
}

/// Sends the prompt: create_run (no active run), continue_run (finished run)
/// or queue_message (a generating run). Attaches the event stream that was
/// subscribed before the drive task started.
fn submit(store: &UiStore, busy: Signal<bool>) {
    let mut store = *store;
    let prompt = store.prompt.read().trim().to_string();
    if prompt.is_empty() || *busy.read() {
        return;
    }
    // A pending card must be resolved first.
    if store.pending_permission.read().is_some() || store.pending_question.read().is_some() {
        store.toast("Resolve the pending permission or question first.".to_string());
        return;
    }

    let provider_id = store.selected_provider.read().clone();
    let model = store.selected_model.read().clone();
    if provider_id.is_empty() || model.is_empty() {
        store.toast("Select a provider and model first (Settings → Providers).".to_string());
        return;
    }
    let mode = store.selected_mode.read().clone();
    let effort = store.selected_effort.read().clone();
    let project_path = store.selected_project.read().clone();

    let run = store.active_run.read().clone();
    let status = run.as_ref().map(|run| run.status.clone());

    let mut busy = busy;
    busy.set(true);

    if status.as_deref() == Some("generating") {
        // Mid-run: queue the message on the active run's inbox.
        let run_id = run.as_ref().map(|run| run.id.clone()).unwrap_or_default();
        spawn(async move {
            let state = core();
            match locagens_core::api::runs::queue_message(&state, &run_id, &prompt).await {
                Ok(()) => {
                    store.prompt.set(String::new());
                }
                Err(err) => store.toast(format!("Failed to queue message: {err}")),
            }
            busy.set(false);
        });
        return;
    }

    if let Some(run) = run {
        // Finished run → continue it.
        let run_id = run.id.clone();
        spawn(async move {
            let state = core();
            let payload = ContinueRunPayload {
                task: prompt.clone(),
                provider_id: Some(provider_id.clone()),
                model: Some(model.clone()),
                reasoning_effort: Some(effort.clone()),
                mode: Some(mode.clone()),
                bypass_permissions: mode == "auto",
                ..Default::default()
            };
            match locagens_core::api::runs::continue_run(&state, &run_id, &payload).await {
                Ok(continued) => {
                    store.prompt.set(String::new());
                    store.active_run_id.set(Some(run_id.clone()));
                    store.attach_event_stream(run_id, continued.events);
                    store.refresh_runs();
                }
                Err(err) => store.toast(format!("Failed to continue run: {err}")),
            }
            busy.set(false);
        });
        return;
    }

    // No active run → create one.
    spawn(async move {
        let state = core();
        let payload = CreateRunPayload {
            task: prompt.clone(),
            project_path: project_path.clone(),
            project_name: None,
            provider_id: provider_id.clone(),
            model: model.clone(),
            reasoning_effort: Some(effort.clone()),
            mode: Some(mode.clone()),
            bypass_permissions: mode == "auto",
            system_prompt: None,
            append_system_prompt: None,
        };
        match locagens_core::api::runs::create_run(&state, &payload).await {
            Ok(started) => {
                let run_id = started.run.id.clone();
                store.prompt.set(String::new());
                store.active_run_id.set(Some(run_id.clone()));
                store.active_run.set(Some(started.run));
                store.messages.set(Vec::new());
                store.plan.set(None);
                store.run_usage.set(None);
                store.pending_permission.set(None);
                store.pending_question.set(None);
                store.attach_event_stream(run_id, started.events);
                store.refresh_runs();
            }
            Err(err) => store.toast(format!("Failed to start run: {err}")),
        }
        busy.set(false);
    });
}
