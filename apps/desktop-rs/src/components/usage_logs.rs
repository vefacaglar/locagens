//! Usage logs page: metrics cards and a paged table of token/cost entries.

use super::icons::{icon_arrow_left, icon_arrow_right};
use crate::state_ui::{core, fmt_cost, fmt_tokens, UiStore};
use dioxus::prelude::*;
use locagens_core::api::runs::{usage_logs, UsageLogQuery, UsageLogsResponse};
use locagens_core::db::UsageLogEntry;

#[component]
pub fn UsageLogsPage() -> Element {
    let store: UiStore = use_context();
    let data: Signal<Option<UsageLogsResponse>> = use_signal(|| None);
    let mut search = use_signal(String::new);
    let mut offset = use_signal(|| 0i64);
    const PAGE_SIZE: i64 = 50;

    use_effect(move || {
        // Re-query whenever the search text or page offset changes.
        let search_value = search.read().clone();
        let offset_value = *offset.read();
        let mut data = data;
        spawn(async move {
            let state = core();
            let query = UsageLogQuery {
                limit: Some(PAGE_SIZE),
                offset: Some(offset_value),
                search: if search_value.is_empty() { None } else { Some(search_value) },
                ..Default::default()
            };
            data.set(usage_logs(&state, &query).await.ok());
        });
    });

    let response = data.read().clone();
    let metrics = response.as_ref().map(|response| response.metrics.clone());
    let cache_label = metrics
        .as_ref()
        .map(|metrics| format!("{:.0}%", metrics.avg_cache_hit_rate))
        .unwrap_or_default();
    let logs = response
        .as_ref()
        .map(|response| response.logs.clone())
        .unwrap_or_default();
    let total = response.as_ref().map(|response| response.total).unwrap_or(0);

    rsx! {
        div {
            class: "page-header",
            // Draggable window titlebar strip (double-click to maximize).
            onmousedown: super::start_window_drag,
            ondoubleclick: move |_| {
                dioxus::desktop::use_window().toggle_maximized();
            },
            h2 { "Usage Logs" }
            input {
                r#type: "search",
                placeholder: "Search provider / model / run…",
                value: "{search.read()}",
                oninput: move |event| {
                    search.set(event.value());
                    offset.set(0);
                },
            }
            button {
                class: "subtle icon-btn",
                // Clicks on the button must not start a window drag.
                onmousedown: move |e| e.stop_propagation(),
                onclick: move |_| {
                    let mut store = store;
                    store.page.set(crate::state_ui::AppPage::Chat);
                },
                {icon_arrow_left()} "Back to Chat"
            }
        }
        div {
            class: "usage-layout",
            div {
                class: "usage-metrics",
                div {
                    class: "metric-card",
                    span { class: "metric-value", "{metrics.as_ref().map(|m| fmt_cost(m.total_cost)).unwrap_or_default()}" }
                    span { class: "metric-label", "Total cost" }
                }
                div {
                    class: "metric-card",
                    span { class: "metric-value", "{metrics.as_ref().map(|m| m.total_calls.to_string()).unwrap_or_default()}" }
                    span { class: "metric-label", "Model calls" }
                }
                div {
                    class: "metric-card",
                    span { class: "metric-value", "{metrics.as_ref().map(|m| fmt_tokens(m.total_tokens)).unwrap_or_default()}" }
                    span { class: "metric-label", "Tokens" }
                }
                div {
                    class: "metric-card",
                    span { class: "metric-value", "{cache_label}" }
                    span { class: "metric-label", "Avg cache hit" }
                }
            }
            table {
                class: "data-table",
                thead {
                    tr {
                        th { "When" }
                        th { "Run" }
                        th { "Role" }
                        th { "Provider" }
                        th { "Model" }
                        th { "In" }
                        th { "Out" }
                        th { "Cache" }
                        th { "Cost" }
                    }
                }
                tbody {
                    for entry in logs.iter() {
                        UsageRow { entry: entry.clone() }
                    }
                    if logs.is_empty() {
                        tr {
                            td { class: "empty-cell", colspan: "9", "No usage entries yet." }
                        }
                    }
                }
            }
            div {
                class: "usage-toolbar",
                span { class: "composer-hint", "{total} entries" }
                button {
                    disabled: *offset.read() <= 0,
                    onclick: move |_| {
                        let mut offset = offset;
                        let current = *offset.read();
                        offset.set((current - PAGE_SIZE).max(0));
                    },
                    {icon_arrow_left()} "Prev"
                }
                button {
                    disabled: *offset.read() + PAGE_SIZE >= total as i64,
                    onclick: move |_| {
                        let mut offset = offset;
                        let current = *offset.read();
                        offset.set(current + PAGE_SIZE);
                    },
                    "Next" {icon_arrow_right()}
                }
            }
        }
    }
}

#[component]
fn UsageRow(entry: UsageLogEntry) -> Element {
    // Show the run title if the list has it, else the raw id prefix.
    let store: UiStore = use_context();
    let runs = store.runs.read().clone();
    let run_title = runs
        .iter()
        .find(|run| run.id == entry.run_id)
        .map(|run| run.title.clone())
        .unwrap_or_else(|| entry.run_id.chars().take(14).collect());
    let when = entry.created_at.chars().take(19).collect::<String>();
    let role = entry
        .agent_role
        .clone()
        .unwrap_or_else(|| "planner".to_string());
    rsx! {
        tr {
            key: "{entry.id}",
            td { "{when}" }
            td { title: "{entry.run_id}", "{run_title}" }
            td { "{role}" }
            td { "{entry.provider_id}" }
            td { "{entry.model}" }
            td { "{fmt_tokens(entry.input_tokens)}" }
            td { "{fmt_tokens(entry.output_tokens)}" }
            td { "{fmt_tokens(entry.cache_read_tokens)}" }
            td { "{fmt_cost(entry.cost)}" }
        }
    }
}
