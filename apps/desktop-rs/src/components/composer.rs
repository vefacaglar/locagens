//! Composer: prompt textarea (Enter to send, Shift+Enter newline), file
//! attachments, @file / @symbol quick-inserts, mode / provider / model /
//! effort selectors and the Send / Continue / Queue action.

use super::icons::*;
use super::slash::{self, SlashAction};
use crate::state_ui::{core, UiStore};
use dioxus::prelude::*;
use locagens_core::api::runs::{ContinueRunPayload, CreateRunPayload};
use locagens_core::pi_providers::ProviderMetadata;

/// i18n key of the display label for a reasoning-effort value.
fn effort_key(effort: &str) -> &'static str {
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
fn model_efforts(provider: Option<&ProviderMetadata>, model_id: &str) -> Vec<String> {
    if let Some(efforts) = provider
        .and_then(|p| p.model_settings.as_ref())
        .and_then(|ms| ms.get(model_id))
        .and_then(|entry| entry.get("reasoningEfforts"))
        .and_then(|v| v.as_array())
    {
        return efforts
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();
    }
    locagens_core::pi_providers::model_reasoning_efforts(model_id, None, None)
}

/// The effective effort for a model: the selected one when supported,
/// otherwise the model's default ("medium" or the first supported level).
/// `None` means the model has no effort control at all.
fn effective_effort<'a>(selected: &'a str, efforts: &'a [String]) -> Option<&'a str> {
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

/// Right-side hover flyout for choosing a model's reasoning effort.
fn render_effort_flyout(
    mut store: UiStore,
    model_id: String,
    efforts: Vec<String>,
    active_effort: &str,
    flyout_pos: (f64, f64),
    mut hovered_model: Signal<Option<String>>,
    mut context_menu: Signal<Option<&'static str>>,
) -> Element {
    rsx! {
        div {
            class: "composer-flyout-right",
            style: "left: {flyout_pos.0}px; top: min({flyout_pos.1}px, calc(100vh - 190px));",
            onclick: move |e| e.stop_propagation(),
            div {
                class: "composer-flyout-card",
                for effort in efforts.iter() {
                    {
                        let val = effort.as_str();
                        let label = store.t(effort_key(val));
                        let is_active = val == active_effort;
                        let m_pick = model_id.clone();
                        let val_pick = val.to_string();
                        rsx! {
                            button {
                                key: "effort-{val}",
                                class: if is_active { "ccm-item active" } else { "ccm-item" },
                                onclick: move |e| {
                                    e.stop_propagation();
                                    store.selected_model.set(m_pick.clone());
                                    store.selected_effort.set(val_pick.clone());
                                    hovered_model.set(None);
                                    context_menu.set(None);
                                },
                                span { class: "ccm-item-label", "{label}" }
                                if is_active { {icon_check()} }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn Composer() -> Element {
    let store: UiStore = use_context();
    let busy = use_signal(|| false);
    let project_files: Signal<Vec<String>> = use_signal(Vec::new);
    let mut file_query = use_signal(String::new);
    let mut context_menu: Signal<Option<&'static str>> = use_signal(|| None);
    let mut hovered_model: Signal<Option<String>> = use_signal(|| None);
    let mut hovered_flyout_pos: Signal<(f64, f64)> = use_signal(|| (0.0, 0.0));
    let skills: Signal<Vec<locagens_core::services::skills::SkillSummary>> = use_signal(Vec::new);

    // Live fuzzy filter: whenever the @file picker is open, re-fetch the file
    // list with the current query attached (fuzzy-ranked by the core API).
    use_effect(move || {
        if *context_menu.read() != Some("mentions") {
            return;
        }
        let query = file_query.read().trim().to_string();
        let path = store.selected_project.read().clone().unwrap_or_default();
        let mut project_files = project_files;
        spawn(async move {
            let state = core();
            let files = locagens_core::api::projects::list_files(&state, &path, Some(&query))
                .await
                .unwrap_or_default();
            project_files.set(files);
        });
    });

    let providers = store.providers_catalog.read().clone();
    let selected_provider = store.selected_provider.read().clone();
    let current_provider = providers.iter().find(|p| p.id == selected_provider);
    let selected_model = store.selected_model.read().clone();
    let selected_effort = store.selected_effort.read().clone();
    let selected_mode = store.selected_mode.read().clone();
    let prompt_text = store.prompt.read().clone();
    // `/` suggestions from the shared command table while typing a command.
    let slash_suggestions = slash::suggestions(&prompt_text);

    let provider_models: Vec<String> = providers
        .iter()
        .find(|provider: &&ProviderMetadata| provider.id == selected_provider)
        .map(|provider| provider.models.clone())
        .unwrap_or_default();

    let provider_label = providers
        .iter()
        .find(|p| p.id == selected_provider)
        .map(|p| p.display_name.clone())
        .unwrap_or_else(|| {
            if selected_provider.is_empty() {
                store.t("common.provider").to_string()
            } else {
                selected_provider.clone()
            }
        });
    let model_label = if selected_model.is_empty() {
        store.t("common.model").to_string()
    } else {
        selected_model.clone()
    };
    // Reasoning effort shown muted next to the model when it supports one.
    let effort_hint: Option<String> = {
        let efforts = model_efforts(current_provider, &selected_model);
        if efforts.is_empty() {
            None
        } else {
            let active = effective_effort(&selected_effort, &efforts).unwrap_or("medium");
            Some(store.t(effort_key(active)).to_string())
        }
    };
    let mode_label = match selected_mode.as_str() {
        "plan" => store.t("mode.plan"),
        "auto" => store.t("mode.auto"),
        _ => store.t("mode.build"),
    };

    let run = store.active_run.read().clone();
    let run_status = run.as_ref().map(|run| run.status.clone());
    let is_generating = run_status.as_deref() == Some("generating");
    let is_awaiting =
        matches!(run_status.as_deref(), Some("awaiting_permission") | Some("awaiting_input"));
    let has_run = run.is_some();
    let send_label = if is_generating {
        store.t("composer.queue")
    } else if has_run {
        store.t("composer.continue")
    } else {
        store.t("composer.send")
    };

    // Hovered-model effort flyout data: looked up once per render so the
    // floating effort submenu can be rendered as a sibling of the scrollable
    // model list (it must not scroll or clip with the list).
    let hovered_flyout: Option<(String, Vec<String>, String)> = hovered_model
        .read()
        .as_ref()
        .and_then(|hovered_id| {
            let efforts = model_efforts(current_provider, hovered_id);
            if efforts.is_empty() {
                None
            } else {
                let active = effective_effort(&selected_effort, &efforts)
                    .unwrap_or("medium")
                    .to_string();
                Some((hovered_id.clone(), efforts, active))
            }
        });

    rsx! {
        div {
            class: "composer",
            // Click-outside-to-close backdrop for open menus / flyouts.
            if context_menu.read().is_some() {
                div {
                    class: "popover-backdrop",
                    onclick: move |e| {
                        e.stop_propagation();
                        context_menu.set(None);
                        hovered_model.set(None);
                    },
                }
            }
            // Slash-command suggestions (shared table, translated descriptions).
            if context_menu.read().is_none() && !slash_suggestions.is_empty() {
                div {
                    class: "composer-context-menu slash-menu",
                    div { class: "ccm-header", "{store.t(\"composer.commands\")}" }
                    div {
                        class: "ccm-list",
                        for spec in slash_suggestions.iter() {
                            {
                                let insert = format!("/{} ", spec.name);
                                let usage = spec.usage();
                                let description = crate::i18n::command_description(store.lang(), spec);
                                rsx! {
                                    button {
                                        key: "cmd-{spec.name}",
                                        class: "ccm-item",
                                        title: "{description}",
                                        onclick: move |_| {
                                            let mut store = store;
                                            store.prompt.set(insert.clone());
                                        },
                                        span { class: "ccm-cmd-usage", "{usage}" }
                                        span { class: "ccm-cmd-desc", "{description}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // Full-width borderless prompt input.
            textarea {
                placeholder: store.t("composer.placeholder"),
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
                onclick: move |_| {
                    context_menu.set(None);
                    hovered_model.set(None);
                },
            }
            div {
                class: "composer-row-bottom",
                // "Add Context" (+) button with popover menu.
                div {
                    class: "composer-dropdown",
                    button {
                        class: "composer-plus-btn",
                        title: store.t("composer.add_context"),
                        onclick: move |_| {
                            let next = if *context_menu.read() == Some("root") { None } else { Some("root") };
                            hovered_model.set(None);
                            context_menu.set(next);
                        },
                        {icon_plus()}
                    }
                    if let Some(menu_kind) = *context_menu.read() {
                        {match menu_kind {
                        "root" => rsx! {
                            div {
                                class: "composer-context-menu",
                                onclick: move |e| e.stop_propagation(),
                                div { class: "ccm-header", "{store.t(\"composer.add_context\")}" }
                                button {
                                    class: "ccm-item",
                                    onclick: move |_| {
                                        context_menu.set(None);
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
                                    {icon_paperclip()}
                                    span { "{store.t(\"composer.files\")}" }
                                }
                                button {
                                    class: "ccm-item",
                                    onclick: move |_| {
                                        context_menu.set(None);
                                        let mut store = store;
                                        spawn(async move {
                                            if let Some(folder) = rfd::AsyncFileDialog::new().pick_folder().await {
                                                let insert = format!("@{} ", folder.path().to_string_lossy());
                                                store.prompt.with_mut(|prompt| prompt.push_str(&insert));
                                            }
                                        });
                                    },
                                    {icon_folder()}
                                    span { "{store.t(\"composer.folder\")}" }
                                }
                                button {
                                    class: "ccm-item",
                                    onclick: move |_| {
                                        file_query.set(String::new());
                                        context_menu.set(Some("mentions"));
                                    },
                                    {icon_at()}
                                    span { "{store.t(\"composer.mentions\")}" }
                                }
                                button {
                                    class: "ccm-item",
                                    onclick: move |_| {
                                        let path = store.selected_project.read().clone().unwrap_or_default();
                                        let mut skills = skills;
                                        spawn(async move {
                                            let state = core();
                                            let list = locagens_core::api::skills::list_skills(&state, Some(&path))
                                                .await
                                                .map(|response| response.skills)
                                                .unwrap_or_default();
                                            skills.set(list);
                                        });
                                        context_menu.set(Some("actions"));
                                    },
                                    {icon_actions()}
                                    span { "{store.t(\"composer.actions\")}" }
                                }
                            }
                        },
                        "mentions" => rsx! {
                            div {
                                class: "composer-context-menu",
                                onclick: move |e| e.stop_propagation(),
                                div {
                                    class: "ccm-header",
                                    button {
                                        class: "subtle icon-btn",
                                        onclick: move |_| context_menu.set(Some("root")),
                                        "← {store.t(\"composer.add_context\")}"
                                    }
                                }
                                input {
                                    class: "ccm-search",
                                    r#type: "text",
                                    placeholder: store.t("composer.filter_files"),
                                    value: "{file_query.read()}",
                                    oninput: move |event| file_query.set(event.value()),
                                }
                                div {
                                    class: "ccm-list",
                                    if project_files.read().is_empty() {
                                        div { class: "ccm-empty", "{store.t(\"composer.no_files\")}" }
                                    }
                                    for (index, file) in project_files.read().iter().take(10).enumerate() {
                                        {
                                            let file = file.clone();
                                            rsx! {
                                                button {
                                                    key: "file-{index}",
                                                    class: "ccm-item",
                                                    title: "{file}",
                                                    onclick: move |_| {
                                                        let mut store = store;
                                                        let insert = format!("@{file} ");
                                                        store.prompt.with_mut(|prompt| prompt.push_str(&insert));
                                                        context_menu.set(None);
                                                    },
                                                    "{file}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        "actions" => rsx! {
                            div {
                                class: "composer-context-menu",
                                onclick: move |e| e.stop_propagation(),
                                div {
                                    class: "ccm-header",
                                    button {
                                        class: "subtle icon-btn",
                                        onclick: move |_| context_menu.set(Some("root")),
                                        "← {store.t(\"composer.add_context\")}"
                                    }
                                }
                                div {
                                    class: "ccm-list",
                                    if skills.read().is_empty() {
                                        div { class: "ccm-empty", "{store.t(\"composer.no_skills\")}" }
                                    }
                                    for (index, skill) in skills.read().iter().take(12).enumerate() {
                                        {
                                            let skill_name = skill.name.clone();
                                            let skill_title = format!("{} — {}", skill.name, skill.description);
                                            rsx! {
                                                button {
                                                    key: "skill-{index}",
                                                    class: "ccm-item",
                                                    title: "{skill_title}",
                                                    onclick: move |_| {
                                                        let mut store = store;
                                                        let insert = format!("${skill_name} ");
                                                        store.prompt.with_mut(|prompt| prompt.push_str(&insert));
                                                        context_menu.set(None);
                                                    },
                                                    "{skill_name}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        _ => rsx! {},
                        }}
                    }
                }
                // Provider selector.
                div {
                    class: "composer-dropdown",
                    button {
                        class: if *context_menu.read() == Some("provider") { "composer-select-btn open" } else { "composer-select-btn" },
                        title: store.t("common.provider"),
                        onclick: move |_| {
                            let next = if *context_menu.read() == Some("provider") { None } else { Some("provider") };
                            hovered_model.set(None);
                            context_menu.set(next);
                        },
                        span { "{provider_label}" }
                        {icon_chevron_down()}
                    }
                    if *context_menu.read() == Some("provider") {
                        div {
                            class: "composer-context-menu",
                            onclick: move |e| e.stop_propagation(),
                            div { class: "ccm-header", "{store.t(\"common.provider\")}" }
                            div {
                                class: "ccm-list",
                                if providers.is_empty() {
                                    div { class: "ccm-empty", "{store.t(\"composer.no_providers\")}" }
                                }
                                for provider in providers.iter() {
                                    {
                                        let pid = provider.id.clone();
                                        let pname = provider.display_name.clone();
                                        let first_model = provider.models.first().cloned();
                                        let is_active = pid == selected_provider;
                                        rsx! {
                                            button {
                                                key: "prov-{pid}",
                                                class: if is_active { "ccm-item active" } else { "ccm-item" },
                                                onclick: move |_| {
                                                    let mut store = store;
                                                    store.selected_provider.set(pid.clone());
                                                    // Reset the model to the provider's first model.
                                                    if let Some(first) = first_model.clone() {
                                                        store.selected_model.set(first);
                                                    }
                                                    context_menu.set(None);
                                                },
                                                span { class: "ccm-item-label", "{pname}" }
                                                if is_active { {icon_check()} }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // Model selector.
                div {
                    class: "composer-dropdown",
                    button {
                        class: if *context_menu.read() == Some("model") { "composer-select-btn open" } else { "composer-select-btn" },
                        title: store.t("common.model"),
                        onclick: move |_| {
                            let next = if *context_menu.read() == Some("model") { None } else { Some("model") };
                            if next.is_some() {
                                // Pick up credentials added elsewhere (CLI, pi, Settings).
                                store.refresh_providers();
                            }
                            hovered_model.set(None);
                            context_menu.set(next);
                        },
                        span { class: "ccm-model-name", "{model_label}" }
                        if let Some(effort) = effort_hint.clone() {
                            span { class: "ccm-model-effort", "{effort}" }
                        }
                        {icon_chevron_down()}
                    }
                    if *context_menu.read() == Some("model") {
                        div {
                            class: "composer-context-menu model-menu",
                            onclick: move |e| e.stop_propagation(),
                            onmouseleave: move |_| hovered_model.set(None),
                            div { class: "ccm-header", "{store.t(\"common.model\")}" }
                            div {
                                class: "ccm-list model-list",
                                onscroll: move |_| hovered_model.set(None),
                                if provider_models.is_empty() {
                                    if selected_model.is_empty() {
                                        div { class: "ccm-empty", "{store.t(\"composer.no_models\")}" }
                                    } else {
                                        div { class: "ccm-empty", "{selected_model}" }
                                    }
                                }
                                for model in provider_models.iter() {
                                    {
                                        let m = model.clone();
                                        let m_enter = m.clone();
                                        let m_click = m.clone();
                                        let is_active = *model == selected_model;
                                        let is_hovered = hovered_model.read().as_deref() == Some(m.as_str());
                                        let efforts = model_efforts(current_provider, &m);
                                        let active_eff =
                                            effective_effort(&selected_effort, &efforts).map(str::to_string);
                                        let m_eff_click = active_eff.clone();
                                        let show_effort = active_eff.is_some();
                                        let effort_label = show_effort
                                            .then(|| {
                                                effective_effort(&selected_effort, &efforts)
                                                    .unwrap_or("medium")
                                            })
                                            .map(|effort| store.t(effort_key(effort)))
                                            .unwrap_or("");
                                        rsx! {
                                            div {
                                                key: "model-{m}",
                                                class: "ccm-submenu-row",
                                                onmouseenter: move |event: MouseEvent| {
                                                    if show_effort {
                                                        let elem = event.element_coordinates();
                                                        let client = event.client_coordinates();
                                                        let x = (client.x - elem.x + 271.0).max(0.0);
                                                        let y = (client.y - elem.y.clamp(0.0, 30.0) - 6.0).max(12.0);
                                                        hovered_flyout_pos.set((x, y));
                                                        hovered_model.set(Some(m_enter.clone()));
                                                    } else {
                                                        hovered_model.set(None);
                                                    }
                                                },
                                                button {
                                                    class: if is_active || is_hovered { "ccm-item active" } else { "ccm-item" },
                                                    onclick: move |_| {
                                                        let mut store = store;
                                                        store.selected_model.set(m_click.clone());
                                                        if let Some(eff) = m_eff_click.clone() {
                                                            store.selected_effort.set(eff);
                                                        }
                                                        hovered_model.set(None);
                                                        context_menu.set(None);
                                                    },
                                                    span {
                                                        class: "ccm-item-label",
                                                        span { class: "ccm-model-name", "{m}" }
                                                        if show_effort {
                                                            span { class: "ccm-model-effort", "{effort_label}" }
                                                        }
                                                    }
                                                    if is_active { {icon_check()} }
                                                    if show_effort {
                                                        span { class: "ccm-row-chevron", {icon_chevron_right()} }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            if let Some((flyout_model, flyout_efforts, flyout_active)) = hovered_flyout {
                                {render_effort_flyout(store, flyout_model, flyout_efforts, &flyout_active, *hovered_flyout_pos.read(), hovered_model, context_menu)}
                            }
                        }
                    }
                }
                span { class: "composer-spacer" }
                // Execution mode selector.
                div {
                    class: "composer-dropdown",
                    button {
                        class: if *context_menu.read() == Some("mode") { "composer-select-btn open" } else { "composer-select-btn" },
                        title: store.t("composer.mode_title"),
                        onclick: move |_| {
                            let next = if *context_menu.read() == Some("mode") { None } else { Some("mode") };
                            hovered_model.set(None);
                            context_menu.set(next);
                        },
                        {icon_shield()}
                        span { "{mode_label}" }
                        {icon_chevron_down()}
                    }
                    if *context_menu.read() == Some("mode") {
                        div {
                            class: "composer-context-menu align-right",
                            onclick: move |e| e.stop_propagation(),
                            div { class: "ccm-header", "{store.t(\"composer.mode\")}" }
                            div {
                                class: "ccm-list",
                                for (val, label) in [("accept_edits", store.t("mode.build")), ("plan", store.t("mode.plan")), ("auto", store.t("mode.auto"))] {
                                    {
                                        let is_active = val == selected_mode;
                                        rsx! {
                                            button {
                                                key: "mode-{val}",
                                                class: if is_active { "ccm-item active" } else { "ccm-item" },
                                                onclick: move |_| {
                                                    let mut store = store;
                                                    store.selected_mode.set(val.to_string());
                                                    context_menu.set(None);
                                                },
                                                span { class: "ccm-item-label", "{label}" }
                                                if is_active { {icon_check()} }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if is_generating {
                    button {
                        class: "icon-btn danger",
                        title: store.t("header.stop"),
                        onclick: move |_| {
                            let Some(run_id) = run.as_ref().map(|run| run.id.clone()) else {
                                return;
                            };
                            spawn(async move {
                                let state = core();
                                let _ = locagens_core::api::runs::cancel_run(&state, &run_id).await;
                            });
                        },
                        {icon_stop()}
                    }
                }
                button {
                    class: "composer-send-btn",
                    disabled: *busy.read() || is_awaiting || prompt_text.trim().is_empty(),
                    title: if is_awaiting {
                        store.t("composer.resolve_first").to_string()
                    } else {
                        send_label.to_string()
                    },
                    onclick: move |_| submit(&store, busy),
                    {icon_arrow_up()}
                }
            }
        }
    }
}

/// Sends the prompt: a slash command, or create_run (no active run),
/// continue_run (finished run) or queue_message (a generating run). Attaches
/// the event stream that was subscribed before the drive task started.
pub(crate) fn submit(store: &UiStore, busy: Signal<bool>) {
    let store = *store;
    let prompt = store.prompt.read().trim().to_string();
    if prompt.is_empty() || *busy.read() {
        return;
    }

    // Client-side slash commands (shared table and busy rule) run before any
    // run is started.
    let generating = store.active_run.read().as_ref().is_some_and(|run| run.status == "generating");
    if let Some(action) = slash::resolve(&prompt, generating) {
        run_slash(store, action);
        return;
    }
    // A pending card must be resolved first.
    if store.pending_permission.read().is_some() || store.pending_question.read().is_some() {
        store.toast(store.t("toast.resolve_first").to_string());
        return;
    }
    start_turn(store, prompt, busy);
}

/// Executes a slash command on the desktop.
fn run_slash(mut store: UiStore, action: SlashAction) {
    match action {
        SlashAction::Reset => {
            store.active_run_id.set(None);
            store.active_run.set(None);
            store.messages.set(Vec::new());
            store.plan.set(None);
            store.run_usage.set(None);
            store.pending_permission.set(None);
            store.pending_question.set(None);
            store.turn_diff.set(None);
            store.spawned_agents.set(Vec::new());
            store.prompt.set(String::new());
        }
        SlashAction::Fork => {
            if let Some(run_id) = store.active_run_id.read().clone() {
                store.prompt.set(String::new());
                spawn(async move {
                    let state = core();
                    match locagens_core::api::runs::fork_run(&state, &run_id, None).await {
                        Ok(forked) => {
                            store.refresh_runs();
                            store.select_run(forked.id);
                            store.toast(store.t("toast.forked").to_string());
                        }
                        Err(err) => store.toast(store.tf("toast.fork_failed", &[&err.to_string()])),
                    }
                });
            } else {
                store.toast(store.t("toast.no_fork").to_string());
            }
        }
        SlashAction::Undo => {
            let run_id = store.active_run_id.read().clone();
            let project = store
                .selected_project
                .read()
                .clone()
                .or_else(|| store.active_run.read().as_ref().map(|run| run.project_path.clone()));
            if let Some(project) = project {
                store.prompt.set(String::new());
                spawn(async move {
                    let state = core();
                    match locagens_core::api::projects::undo_last_turn(&state, &project, run_id.as_deref()).await {
                        Ok(snapshot) => store.toast(store.tf("toast.restored", &[&snapshot.id])),
                        Err(err) => store.toast(store.tf("toast.undo_failed", &[&err.to_string()])),
                    }
                });
            } else {
                store.toast(store.t("toast.select_project").to_string());
            }
        }
        SlashAction::Review(argument) => super::review::start_from_command(&store, &argument),
        SlashAction::SetMode(mode) => {
            store.selected_mode.set(mode.to_string());
            store.prompt.set(String::new());
            store.toast(store.tf("toast.mode_set", &[mode]));
        }
        SlashAction::Help => {
            store.prompt.set(String::new());
            store.toast(store.tf("toast.commands", &[&slash::command_help(store.lang())]));
        }
        SlashAction::Busy(name) => store.toast(store.tf("toast.command_busy", &[&name])),
        SlashAction::Unavailable(name) => store.toast(store.tf("toast.command_cli_only", &[&name])),
        SlashAction::Unknown(head) => {
            store.toast(store.tf("toast.unknown_command", &[&head, &slash::command_names()]));
        }
    }
}

/// Sends a (non-command) prompt: queue on a generating run, continue a
/// finished one, or create a new run.
fn start_turn(mut store: UiStore, prompt: String, busy: Signal<bool>) {
    let provider_id = store.selected_provider.read().clone();
    let model = store.selected_model.read().clone();
    if provider_id.is_empty() || model.is_empty() {
        store.toast(store.t("toast.select_provider").to_string());
        return;
    }
    let mode = store.selected_mode.read().clone();
    let effort = store.selected_effort.read().clone();
    let project_path = store.selected_project.read().clone();

    // Clamp the effort to the selected model's supported levels.
    let providers_catalog = store.providers_catalog.read().clone();
    let current_provider = providers_catalog.iter().find(|p| p.id == provider_id);
    let efforts = model_efforts(current_provider, &model);
    let effort = effective_effort(&effort, &efforts).map(str::to_string);

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
                Err(err) => store.toast(store.tf("toast.queue_failed", &[&err.to_string()])),
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
                reasoning_effort: effort.clone(),
                mode: Some(mode.clone()),
                bypass_permissions: mode == "auto",
                ..Default::default()
            };
            match locagens_core::api::runs::continue_run(&state, &run_id, &payload).await {
                Ok(continued) => {
                    store.prompt.set(String::new());
                    store.active_run_id.set(Some(run_id.clone()));
                    store.turn_diff.set(None);
                    store.spawned_agents.set(Vec::new());
                    // Show the appended user message at once instead of
                    // waiting for the first streamed event.
                    let msgs = locagens_core::api::runs::list_messages(&state, &run_id)
                        .await
                        .unwrap_or_default();
                    crate::state_ui::pin_sent_message(&msgs);
                    store.messages.set(msgs);
                    if let Ok(updated_run) = locagens_core::api::runs::get_run(&state, &run_id).await {
                        store.active_run.set(Some(updated_run));
                    }
                    store.attach_event_stream(run_id, continued.events);
                    store.refresh_runs();
                }
                Err(err) => store.toast(store.tf("toast.continue_failed", &[&err.to_string()])),
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
            reasoning_effort: effort.clone(),
            mode: Some(mode.clone()),
            bypass_permissions: mode == "auto",
            system_prompt: None,
            append_system_prompt: None,
            ..Default::default()
        };
        match locagens_core::api::runs::create_run(&state, &payload).await {
            Ok(started) => {
                let run_id = started.run.id.clone();
                store.prompt.set(String::new());
                store.active_run_id.set(Some(run_id.clone()));
                store.active_run.set(Some(started.run));
                // Load the initial user message so it appears immediately.
                let initial_msgs = locagens_core::api::runs::list_messages(&state, &run_id)
                    .await
                    .unwrap_or_default();
                crate::state_ui::pin_sent_message(&initial_msgs);
                store.messages.set(initial_msgs);
                store.plan.set(None);
                store.run_usage.set(None);
                store.pending_permission.set(None);
                store.pending_question.set(None);
                store.turn_diff.set(None);
                store.spawned_agents.set(Vec::new());
                store.attach_event_stream(run_id, started.events);
                store.refresh_runs();
            }
            Err(err) => store.toast(store.tf("toast.start_failed", &[&err.to_string()])),
        }
        busy.set(false);
    });
}
