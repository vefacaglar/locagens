//! MCP tab: servers list, add / save, toggle and delete.

use crate::state_ui::{core, UiStore};
use dioxus::prelude::*;
use locagens_core::services::mcp::McpServerInput;

/// The project whose `.locagens/mcp.json` servers the MCP tab should include.
fn selected_project(store: &UiStore) -> Option<String> {
    store.selected_project.read().clone().filter(|path| !path.is_empty())
}

/// MCP servers: list, add / save, toggle and delete (user scope).
#[component]
pub(super) fn McpTab() -> Element {
    let store: UiStore = use_context();
    let servers: Signal<Vec<locagens_core::services::mcp::McpServerInfo>> = use_signal(Vec::new);
    let mut new_name = use_signal(String::new);
    let mut new_transport = use_signal(|| "stdio".to_string());
    let mut new_command = use_signal(String::new);
    let mut new_url = use_signal(String::new);
    let mut error = use_signal(String::new);

    use_effect(move || {
        let mut servers = servers;
        let project = selected_project(&store);
        spawn(async move {
            let state = core();
            if let Ok(response) = locagens_core::api::mcp::list_mcp_servers(&state, project.as_deref()).await {
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
                    option { value: "stdio", "{store.t(\"mcp.transport_stdio\")}" }
                    option { value: "sse", "{store.t(\"mcp.transport_sse\")}" }
                    option { value: "streamable-http", "{store.t(\"mcp.transport_http\")}" }
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
                            if let Ok(response) = locagens_core::api::mcp::list_mcp_servers(&state, selected_project(&store).as_deref()).await {
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
                                            locagens_core::api::mcp::logout_mcp_server(&state, &auth_name, selected_project(&store).as_deref())
                                                .await
                                                .map(|_| ())
                                        } else {
                                            store.toast(store.t("mcp.signin_opening").to_string());
                                            let on_url = |url: &str| {
                                                let _ = locagens_core::services::mcp::oauth::open_browser(url);
                                            };
                                            locagens_core::api::mcp::login_mcp_server(&state, &auth_name, selected_project(&store).as_deref(), &on_url)
                                                .await
                                                .map(|_| ())
                                        };
                                        if let Err(err) = result {
                                            store.toast(store.tf("mcp.signin_failed", &[&err.to_string()]));
                                        }
                                        if let Ok(response) = locagens_core::api::mcp::list_mcp_servers(&state, selected_project(&store).as_deref()).await {
                                            servers.set(response.servers);
                                        }
                                    });
                                },
                                if signed_in { "{store.t(\"mcp.sign_out\")}" } else { "{store.t(\"mcp.sign_in\")}" }
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
                                        locagens_core::api::mcp::toggle_mcp_server(&state, &toggle_name, toggle_enabled, selected_project(&store).as_deref()).await
                                    {
                                        store.toast(store.tf("toast.toggle_failed", &[&err.to_string()]));
                                    }
                                    if let Ok(response) = locagens_core::api::mcp::list_mcp_servers(&state, selected_project(&store).as_deref()).await {
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
                                        locagens_core::api::mcp::delete_mcp_server(&state, &delete_name, selected_project(&store).as_deref()).await
                                    {
                                        store.toast(store.tf("toast.delete_failed", &[&err.to_string()]));
                                    }
                                    if let Ok(response) = locagens_core::api::mcp::list_mcp_servers(&state, selected_project(&store).as_deref()).await {
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
