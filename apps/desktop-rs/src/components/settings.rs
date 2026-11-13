//! Settings page: every tab is wired in-process to `locagens_core::api` —
//! Codex / Locagens auth & provider accounts, permissions, memory, skills, MCP servers,
//! plugins and security.

use crate::state_ui::{core, UiStore};
use dioxus::prelude::*;
use locagens_core::api::plugins::ToggleInput;
use locagens_core::api::providers::OAuthLoginStart;
use locagens_core::db::{Memory, PermissionRule};
use locagens_core::pi_providers::ProviderMetadata;
use locagens_core::services::mcp::McpServerInput;
use locagens_core::services::plugins::InstallPayload;
use locagens_core::services::skills::SkillSummary;
use serde_json::Value;

const NEW_SKILL_TEMPLATE: &str = "---\nname: my-skill\ndescription: One-line description shown to the agent.\n---\n\n# My Skill\n\nInstructions the agent follows when this skill is loaded.\n";

/// State of the SKILL.md editor card (`name == None` means "creating new").
#[derive(Clone, Debug)]
struct SkillEditor {
    name: Option<String>,
    target: String, // "user" | "project"
    content: String,
}

#[component]
pub fn SettingsPage(initial_tab: crate::state_ui::SettingsTab) -> Element {
    let mut tab = use_signal(|| initial_tab);
    let store: UiStore = use_context();

    rsx! {
        div {
            class: "page-header",
            // Draggable window titlebar strip (double-click to maximize).
            onmousedown: move |_| {
                dioxus::desktop::use_window().drag();
            },
            ondoubleclick: move |_| {
                dioxus::desktop::use_window().toggle_maximized();
            },
            h2 { "{store.t(\"settings.title\")}" }
            button {
                class: "subtle icon-btn",
                // Clicks on the button must not start a window drag.
                onmousedown: move |e| e.stop_propagation(),
                onclick: move |_| {
                    let mut store: UiStore = use_context();
                    store.page.set(crate::state_ui::AppPage::Chat);
                },
                "← {store.t(\"common.back_to_chat\")}"
            }
        }
        div {
            class: "settings-layout",
            nav {
                class: "settings-nav",
                for tab_item in crate::state_ui::SettingsTab::ALL.iter() {
                    button {
                        key: "{tab_item.label_key()}",
                        class: if *tab_item == *tab.read() { "active" } else { "" },
                        onclick: move |_| tab.set(*tab_item),
                        "{store.t(tab_item.label_key())}"
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
                    crate::state_ui::SettingsTab::Skills => rsx! { SkillsTab {} },
                    crate::state_ui::SettingsTab::Mcp => rsx! { McpTab {} },
                    crate::state_ui::SettingsTab::Plugins => rsx! { PluginsTab {} },
                    crate::state_ui::SettingsTab::Security => rsx! { SecurityTab {} },
                    crate::state_ui::SettingsTab::Appearance => rsx! { super::appearance::AppearanceTab {} },
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

/// Native auth.json accounts (~/.locagens, Codex / pi fallbacks) plus OAuth
/// sign-in for providers that support it.
#[component]
fn AuthTab() -> Element {
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

fn reload_accounts(accounts: Signal<Vec<Value>>) {
    let mut accounts = accounts;
    spawn(async move {
        let state = core();
        accounts.set(locagens_core::api::providers::get_pi_auth(&state).await.unwrap_or_default());
    });
}

/// Manual fallback of an authorization-URL login: completes it with the
/// redirect URL the user pasted and stops the background wait.
fn submit_pasted_redirect(
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
fn begin_oauth_login(
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
fn account_detail(store: &UiStore, account: &Value) -> String {
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
            div { class: "settings-empty", "{store.t(\"perms.none\")}" }
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
                                        store.toast(store.tf("toast.delete_rule_failed", &[&err.to_string()]));
                                    }
                                    rules.set(locagens_core::api::permissions::list_permissions(&state).await.unwrap_or_default());
                                });
                            },
                            "{store.t(\"common.delete\")}"
                        }
                    }
                }
            }
        }
    }
}

/// Memory entries with create, edit and delete.
#[component]
fn MemoryTab() -> Element {
    let memories: Signal<Vec<Memory>> = use_signal(Vec::new);
    let mut new_memory = use_signal(String::new);
    let mut editing: Signal<Option<i64>> = use_signal(|| None);
    let mut edit_text = use_signal(String::new);
    let store: UiStore = use_context();

    use_effect(move || {
        let mut memories = memories;
        spawn(async move {
            let state = core();
            memories.set(locagens_core::api::memory::list_memories(&state).await.unwrap_or_default());
        });
    });

    let data = memories.read().clone();
    let editing_id = *editing.read();
    rsx! {
        div {
            class: "composer-row-top",
            input {
                placeholder: store.t("memory.placeholder"),
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
                                    store.toast(store.tf("toast.save_memory_failed", &[&err.to_string()]));
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
            div { class: "settings-empty", "{store.t(\"memory.none\")}" }
        }
        for memory in data.iter() {
            {
                let id = memory.id;
                let is_editing = editing_id == Some(id);
                let memory_content = memory.content.clone();
                rsx! {
                    div {
                        key: "{id}",
                        class: "settings-row",
                        if is_editing {
                            textarea {
                                class: "grow",
                                style: "min-height:60px;font-size:12.5px",
                                value: "{edit_text.read()}",
                                oninput: move |event| edit_text.set(event.value()),
                            }
                            button {
                                class: "primary",
                                onclick: move |_| {
                                    let mut memories = memories;
                                    let content = edit_text.read().trim().to_string();
                                    spawn(async move {
                                        let state = core();
                                        if let Err(err) =
                                            locagens_core::api::memory::update_memory(&state, id, &content).await
                                        {
                                            store.toast(store.tf("toast.update_memory_failed", &[&err.to_string()]));
                                        }
                                        editing.set(None);
                                        memories.set(locagens_core::api::memory::list_memories(&state).await.unwrap_or_default());
                                    });
                                },
                                "{store.t(\"common.save\")}"
                            }
                            button {
                                onclick: move |_| editing.set(None),
                                "{store.t(\"common.cancel\")}"
                            }
                        } else {
                            div {
                                class: "grow",
                                style: "white-space:pre-wrap",
                                "{memory.content}"
                            }
                            button {
                                onclick: move |_| {
                                    edit_text.set(memory_content.clone());
                                    editing.set(Some(id));
                                },
                                "{store.t(\"common.edit\")}"
                            }
                            button {
                                class: "danger",
                                onclick: move |_| {
                                    let mut memories = memories;
                                    spawn(async move {
                                        let state = core();
                                        if let Err(err) = locagens_core::api::memory::delete_memory(&state, id).await {
                                            store.toast(store.tf("toast.delete_memory_failed", &[&err.to_string()]));
                                        }
                                        memories.set(locagens_core::api::memory::list_memories(&state).await.unwrap_or_default());
                                    });
                                },
                                "{store.t(\"common.delete\")}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Skills: list, create / edit `SKILL.md` and delete (user + project scope).
#[component]
fn SkillsTab() -> Element {
    let store: UiStore = use_context();
    let skills: Signal<Vec<SkillSummary>> = use_signal(Vec::new);
    let mut editor: Signal<Option<SkillEditor>> = use_signal(|| None);
    let project_filter = store.selected_project.read().clone();

    use_effect(move || {
        let mut skills = skills;
        let project = project_filter.clone();
        spawn(async move {
            let state = core();
            if let Ok(response) = locagens_core::api::skills::list_skills(&state, project.as_deref()).await {
                skills.set(response.skills);
            }
        });
    });

    let data = skills.read().clone();
    let editor_state = editor.read().clone();
    let selected_project = store.selected_project.read().clone();
    let project_label = selected_project
        .clone()
        .unwrap_or_else(|| store.t("skills.no_project").to_string());

    rsx! {
        div {
            class: "panel-section-title", "{store.t(\"tab.skills\")}"
            span { style: "flex:1" }
            button {
                class: "subtle",
                onclick: move |_| {
                    editor.set(Some(SkillEditor {
                        name: None,
                        target: "user".to_string(),
                        content: NEW_SKILL_TEMPLATE.to_string(),
                    }));
                },
                "+ {store.t(\"skills.new\")}"
            }
        }
        if let Some(edit_state) = editor_state {
            div {
                class: "panel-card",
                div {
                    class: "panel-section-title",
                    if let Some(name) = &edit_state.name { "{store.t(\"skills.edit\")} · {name}" } else { "{store.t(\"skills.new_md\")}" }
                }
                div {
                    class: "composer-row-top",
                    select {
                        value: "{edit_state.target}",
                        onchange: move |event| {
                            let value = event.value();
                            editor.with_mut(|option| {
                                if let Some(state) = option {
                                    state.target = value;
                                }
                            });
                        },
                        option { value: "user", "{store.t(\"skills.user_scope\")}" }
                        option { value: "project", "{store.t(\"skills.project_scope\")}" }
                    }
                    if edit_state.target == "project" {
                        span {
                            class: "mono",
                            style: "font-size:11px",
                            "→ {project_label}"
                        }
                    }
                }
                textarea {
                    style: "width:100%;min-height:180px;font-size:12px",
                    value: "{edit_state.content}",
                    oninput: move |event| {
                        let value = event.value();
                        editor.with_mut(|option| {
                            if let Some(state) = option {
                                state.content = value;
                            }
                        });
                    },
                }
                div {
                    style: "display:flex;gap:8px",
                    button {
                        class: "primary",
                        onclick: move |_| {
                            let mut skills = skills;
                            let Some(edit_state) = editor.read().clone() else { return };
                            let project = store.selected_project.read().clone();
                            spawn(async move {
                                let state = core();
                                if edit_state.target == "project" && project.is_none() {
                                    store.toast(store.t("skills.need_project").to_string());
                                    return;
                                }
                                let result = locagens_core::api::skills::install_skill(
                                    &state,
                                    locagens_core::api::skills::InstallSkillInput {
                                        target: Some(edit_state.target.clone()),
                                        content: Some(edit_state.content.clone()),
                                        project_path: project.clone(),
                                    },
                                )
                                .await;
                                match result {
                                    Ok(skill) => {
                                        store.toast(store.tf("toast.skill_saved", &[&skill.name]));
                                        editor.set(None);
                                    }
                                    Err(err) => store.toast(store.tf("toast.save_skill_failed", &[&err.to_string()])),
                                }
                                if let Ok(response) = locagens_core::api::skills::list_skills(&state, project.as_deref()).await {
                                    skills.set(response.skills);
                                }
                            });
                        },
                        "{store.t(\"common.save\")}"
                    }
                    button {
                        onclick: move |_| editor.set(None),
                        "{store.t(\"common.cancel\")}"
                    }
                }
            }
        }
        if data.is_empty() {
            div { class: "settings-empty", "{store.t(\"skills.none\")}" }
        }
        for skill in data.iter() {
            {
                let name = skill.name.clone();
                let source = skill.source.clone();
                let description = skill.description.clone();
                let body = skill.body.clone().unwrap_or_default();
                let edit_name = name.clone();
                let edit_source = source.clone();
                let edit_body = body.clone();
                let delete_name = name.clone();
                let delete_source = source.clone();
                rsx! {
                    div {
                        key: "{name}",
                        class: "settings-row",
                        div {
                            class: "grow",
                            div {
                                style: "font-weight:600",
                                "{name} "
                                span { class: "badge", "{source}" }
                            }
                            div {
                                style: "font-size:11.5px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap",
                                "{description}"
                            }
                        }
                        button {
                            onclick: move |_| {
                                editor.set(Some(SkillEditor {
                                    name: Some(edit_name.clone()),
                                    target: edit_source.clone(),
                                    content: edit_body.clone(),
                                }));
                            },
                            "{store.t(\"common.edit\")}"
                        }
                        button {
                            class: "danger",
                            onclick: move |_| {
                                let mut skills = skills;
                                let delete_name = delete_name.clone();
                                let target = delete_source.clone();
                                let project = store.selected_project.read().clone();
                                spawn(async move {
                                    let state = core();
                                    if let Err(err) = locagens_core::api::skills::delete_skill(
                                        &state,
                                        &delete_name,
                                        Some(target.as_str()),
                                        project.as_deref(),
                                    )
                                    .await {
                                        store.toast(store.tf("toast.delete_skill_failed", &[&err.to_string()]));
                                    }
                                    if let Ok(response) = locagens_core::api::skills::list_skills(&state, project.as_deref()).await {
                                        skills.set(response.skills);
                                    }
                                });
                            },
                            "{store.t(\"common.delete\")}"
                        }
                    }
                }
            }
        }
    }
}

/// MCP servers: list, add / save, toggle and delete (user scope).
#[component]
fn McpTab() -> Element {
    let store: UiStore = use_context();
    let servers: Signal<Vec<locagens_core::services::mcp::McpServerInfo>> = use_signal(Vec::new);
    let mut new_name = use_signal(String::new);
    let mut new_transport = use_signal(|| "stdio".to_string());
    let mut new_command = use_signal(String::new);
    let mut new_url = use_signal(String::new);
    let mut error = use_signal(String::new);

    use_effect(move || {
        let mut servers = servers;
        spawn(async move {
            let state = core();
            if let Ok(response) = locagens_core::api::mcp::list_mcp_servers(&state, None).await {
                servers.set(response.servers);
            }
        });
    });

    let data = servers.read().clone();
    let transport_value = new_transport.read().clone();
    rsx! {
        div {
            class: "panel-section-title", "{store.t(\"mcp.servers\")}"
        }
        div {
            class: "panel-card",
            div { class: "panel-section-title", "{store.t(\"mcp.add\")}" }
            div {
                class: "composer-row-top",
                input {
                    placeholder: store.t("mcp.name"),
                    value: "{new_name.read()}",
                    oninput: move |event| new_name.set(event.value()),
                }
                select {
                    value: "{transport_value}",
                    onchange: move |event| new_transport.set(event.value()),
                    option { value: "stdio", "stdio" }
                    option { value: "sse", "sse" }
                    option { value: "streamable-http", "streamable-http" }
                }
            }
            div {
                class: "composer-row-top",
                if transport_value == "stdio" {
                    input {
                        placeholder: store.t("mcp.command"),
                        value: "{new_command.read()}",
                        oninput: move |event| new_command.set(event.value()),
                    }
                } else {
                    input {
                        placeholder: store.t("mcp.url"),
                        value: "{new_url.read()}",
                        oninput: move |event| new_url.set(event.value()),
                    }
                }
                button {
                    class: "primary",
                    disabled: new_name.read().trim().is_empty(),
                    onclick: move |_| {
                        let mut servers = servers;
                        let name = new_name.read().trim().to_string();
                        let transport = new_transport.read().clone();
                        let command = new_command.read().trim().to_string();
                        let url = new_url.read().trim().to_string();
                        spawn(async move {
                            let state = core();
                            let result = locagens_core::api::mcp::upsert_mcp_server(
                                &state,
                                McpServerInput {
                                    name: Some(name),
                                    scope: Some("user".to_string()),
                                    transport: Some(transport),
                                    command: if command.is_empty() { None } else { Some(command) },
                                    args: None,
                                    env: None,
                                    url: if url.is_empty() { None } else { Some(url) },
                                    enabled: Some(true),
                                    project_path: None,
                                },
                            )
                            .await;
                            match result {
                                Ok(_) => {
                                    new_name.set(String::new());
                                    new_command.set(String::new());
                                    new_url.set(String::new());
                                    error.set(String::new());
                                }
                                Err(err) => error.set(err.message),
                            }
                            if let Ok(response) = locagens_core::api::mcp::list_mcp_servers(&state, None).await {
                                servers.set(response.servers);
                            }
                        });
                    },
                    "{store.t(\"mcp.save\")}"
                }
            }
            if !error.read().is_empty() {
                div { class: "badge err", "{error.read()}" }
            }
        }
        if data.is_empty() {
            div { class: "settings-empty", "{store.t(\"mcp.none\")}" }
        }
        for server in data.iter() {
            {
                let name = server.config.name.clone();
                let enabled = server.config.enabled;
                let transport = server.config.transport.clone();
                let scope = server.config.scope.clone();
                let detail = server.config.command.clone()
                    .or_else(|| server.config.url.clone())
                    .unwrap_or_default();
                let tool_count = server.tools.len();
                let toggle_name = name.clone();
                let delete_name = name.clone();
                let auth_name = name.clone();
                let needs_login = server.needs_login;
                let signed_in = server.signed_in;
                rsx! {
                    div {
                        key: "{name}",
                        class: "settings-row",
                        div {
                            class: "grow",
                            div {
                                style: "font-weight:600",
                                "{name} "
                                span { class: if enabled { "badge ok" } else { "badge" }, if enabled { "{store.t(\"common.enabled\")}" } else { "{store.t(\"common.disabled\")}" } }
                                span { class: "badge", "{transport} · {scope}" }
                            }
                            div { class: "mono", style: "font-size:11px", "{detail} · {tool_count} {store.t(\"mcp.tools\")}" }
                            if let Some(err) = server.error.clone() {
                                div { class: "badge err", "{err}" }
                            }
                        }
                        if needs_login || signed_in {
                            button {
                                onclick: move |_| {
                                    let mut servers = servers;
                                    let auth_name = auth_name.clone();
                                    spawn(async move {
                                        let state = core();
                                        let result = if signed_in {
                                            locagens_core::api::mcp::logout_mcp_server(&state, &auth_name, None)
                                                .await
                                                .map(|_| ())
                                        } else {
                                            store.toast("Opening the browser to sign in…".to_string());
                                            let on_url = |url: &str| {
                                                let _ = locagens_core::services::mcp::oauth::open_browser(url);
                                            };
                                            locagens_core::api::mcp::login_mcp_server(&state, &auth_name, None, &on_url)
                                                .await
                                                .map(|_| ())
                                        };
                                        if let Err(err) = result {
                                            store.toast(format!("MCP sign-in failed: {err}"));
                                        }
                                        if let Ok(response) = locagens_core::api::mcp::list_mcp_servers(&state, None).await {
                                            servers.set(response.servers);
                                        }
                                    });
                                },
                                if signed_in { "Sign out" } else { "Sign in" }
                            }
                        }
                        button {
                            onclick: move |_| {
                                let mut servers = servers;
                                let toggle_name = toggle_name.clone();
                                let toggle_enabled = !enabled;
                                spawn(async move {
                                    let state = core();
                                    if let Err(err) =
                                        locagens_core::api::mcp::toggle_mcp_server(&state, &toggle_name, toggle_enabled, None).await
                                    {
                                        store.toast(store.tf("toast.toggle_failed", &[&err.to_string()]));
                                    }
                                    if let Ok(response) = locagens_core::api::mcp::list_mcp_servers(&state, None).await {
                                        servers.set(response.servers);
                                    }
                                });
                            },
                            if enabled { "{store.t(\"common.disable\")}" } else { "{store.t(\"common.enable\")}" }
                        }
                        button {
                            class: "danger",
                            onclick: move |_| {
                                let mut servers = servers;
                                let delete_name = delete_name.clone();
                                spawn(async move {
                                    let state = core();
                                    if let Err(err) =
                                        locagens_core::api::mcp::delete_mcp_server(&state, &delete_name, None).await
                                    {
                                        store.toast(store.tf("toast.delete_failed", &[&err.to_string()]));
                                    }
                                    if let Ok(response) = locagens_core::api::mcp::list_mcp_servers(&state, None).await {
                                        servers.set(response.servers);
                                    }
                                });
                            },
                            "{store.t(\"common.delete\")}"
                        }
                    }
                }
            }
        }
    }
}

/// Plugins: list, install from template / local folder, toggle and uninstall.
#[component]
fn PluginsTab() -> Element {
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
                                Ok(plugin) => store.toast(store.tf("toast.installed", &[plugin.get("name").and_then(Value::as_str).unwrap_or("plugin")])),
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
                                    store.toast(store.tf("toast.installed", &[plugin.get("name").and_then(Value::as_str).unwrap_or("plugin")]));
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

/// Security: workspace policy and command sandbox status, with the Windows
/// sandbox setup action when it is available.
#[component]
fn SecurityTab() -> Element {
    let store: UiStore = use_context();
    let status: Signal<Option<locagens_core::api::security::SecurityStatus>> = use_signal(|| None);

    use_effect(move || {
        let mut status = status;
        spawn(async move {
            status.set(locagens_core::api::security::security_status().await.ok());
        });
    });

    let data = status.read().clone();
    rsx! {
        div {
            class: "panel-section-title", "{store.t(\"tab.security\")}"
        }
        match data {
            Some(security) => rsx! {
                div {
                    class: "panel-card",
                    div {
                        class: "panel-section-title", "{store.t(\"security.sandbox\")}"
                    }
                    div {
                        "{store.t(\"security.platform\")}: " span { class: "mono", "{security.sandbox.platform}" }
                        " · {store.t(\"security.status\")}: "
                        span {
                            class: if security.sandbox.status == "ready" { "badge ok" } else { "badge warn" },
                            "{security.sandbox.status}"
                        }
                    }
                    for (index, warning) in security.sandbox.warnings.iter().enumerate() {
                        div {
                            key: "warn-{index}",
                            class: "badge warn",
                            "{warning}"
                        }
                    }
                    for (index, sandbox_error) in security.sandbox.errors.iter().enumerate() {
                        div {
                            key: "err-{index}",
                            class: "badge err",
                            "{sandbox_error}"
                        }
                    }
                }
                div {
                    class: "panel-card",
                    div {
                        class: "panel-section-title", "{store.t(\"security.policy\")}"
                    }
                    div { "{store.t(\"security.network\")}: " span { class: "mono", "{security.policy.command_network}" } }
                    div { "{store.t(\"security.writes_only\")}: " span { class: "mono", "{security.policy.workspace_writes_only}" } }
                    div {
                        "{store.t(\"security.bind\")}: " span { class: "mono", "{security.api.bind_address}" }
                        " · {store.t(\"security.authenticated\")}: " span { class: "mono", "{security.api.authenticated}" }
                    }
                }
                if security.sandbox.can_install {
                    button {
                        class: "primary",
                        onclick: move |_| {
                            let mut status = status;
                            spawn(async move {
                                match locagens_core::api::security::setup_sandbox(true).await {
                                    Ok(updated) => status.set(Some(updated)),
                                    Err(err) => store.toast(store.tf("toast.sandbox_failed", &[&err.to_string()])),
                                }
                            });
                        },
                        "{store.t(\"security.setup\")}"
                    }
                }
            },
            None => rsx! {
                div { class: "settings-empty", "{store.t(\"security.unavailable\")}" }
            },
        }
    }
}
