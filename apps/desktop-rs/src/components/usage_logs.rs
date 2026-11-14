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
    // Epoch when the page opened: that first load is immediate, later ones debounced.
    let opened_at = use_hook(|| *store.usage_epoch.peek());
    // Newest query generation: older responses are dropped (FR-066).
    let generation = use_hook(|| std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)));
    let mut last_search = use_signal(String::new);

    use_effect(move || {
        // Re-query when the search text or page changes, and (debounced)
        // whenever new usage lands. The page only exists while visible, so
        // this costs nothing on other pages.
        let search_value = search.read().clone();
        let offset_value = *offset.read();
        let epoch = *store.usage_epoch.read();
        let mut data = data;
        let generation = generation.clone();
        let mine = generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let typed = *last_search.peek() != search_value;
        if typed {
            last_search.set(search_value.clone());
        }
        spawn(async move {
            let current = || generation.load(std::sync::atomic::Ordering::SeqCst) == mine;
            if typed {
                // Debounce typing: one scan after a pause, not per keystroke.
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                if !current() {
                    return;
                }
            }
            if epoch != opened_at {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if *store.usage_epoch.peek() != epoch {
                    return; // newer usage arrived; its own effect run reloads
                }
            }
            let state = core();
            let query = UsageLogQuery {
                limit: Some(PAGE_SIZE),
                offset: Some(offset_value),
                search: if search_value.is_empty() { None } else { Some(search_value) },
                ..Default::default()
            };
            let response = usage_logs(&state, &query).await.ok();
            if current() {
                data.set(response);
            }
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
            // Draggable window titlebar strip (double-click maximizes, see start_window_drag).
            onmousedown: super::start_window_drag,
            h2 { "{store.t(\"usage.title\")}" }
            input {
                r#type: "search",
                placeholder: store.t("usage.search"),
                value: "{search.read()}",
                oninput: move |event| {
                    search.set(event.value());
                    offset.set(0);
                },
            }
        }
        div {
            class: "usage-layout",
            div {
                class: "usage-metrics",
                div {
                    class: "metric-card",
                    span { class: "metric-value", "{metrics.as_ref().map(|m| fmt_cost(m.total_cost)).unwrap_or_default()}" }
                    span { class: "metric-label", "{store.t(\"usage.total_cost\")}" }
                }
                div {
                    class: "metric-card",
                    span { class: "metric-value", "{metrics.as_ref().map(|m| m.total_calls.to_string()).unwrap_or_default()}" }
                    span { class: "metric-label", "{store.t(\"usage.model_calls\")}" }
                }
                div {
                    class: "metric-card",
                    span { class: "metric-value", "{metrics.as_ref().map(|m| fmt_tokens(m.total_tokens)).unwrap_or_default()}" }
                    span { class: "metric-label", "{store.t(\"usage.tokens\")}" }
                }
                div {
                    class: "metric-card",
                    span { class: "metric-value", "{cache_label}" }
                    span { class: "metric-label", "{store.t(\"usage.avg_cache\")}" }
                }
            }
            table {
                class: "data-table",
                thead {
                    tr {
                        th { "{store.t(\"usage.when\")}" }
                        th { "{store.t(\"usage.run\")}" }
                        th { "{store.t(\"usage.role\")}" }
                        th { "{store.t(\"common.provider\")}" }
                        th { "{store.t(\"common.model\")}" }
                        th { "{store.t(\"usage.in\")}" }
                        th { "{store.t(\"usage.out\")}" }
                        th { "{store.t(\"usage.cache\")}" }
                        th { "{store.t(\"usage.cost\")}" }
                    }
                }
                tbody {
                    for entry in logs.iter() {
                        UsageRow { entry: entry.clone() }
                    }
                    if logs.is_empty() {
                        tr {
                            td { class: "empty-cell", colspan: "9", "{store.t(\"usage.empty\")}" }
                        }
                    }
                }
            }
            div {
                class: "usage-toolbar",
                span { class: "composer-hint", "{total} {store.t(\"usage.entries\")}" }
                button {
                    disabled: *offset.read() <= 0,
                    onclick: move |_| {
                        let mut offset = offset;
                        let current = *offset.read();
                        offset.set((current - PAGE_SIZE).max(0));
                    },
                    {icon_arrow_left()} "{store.t(\"common.prev\")}"
                }
                button {
                    disabled: *offset.read() + PAGE_SIZE >= total as i64,
                    onclick: move |_| {
                        let mut offset = offset;
                        let current = *offset.read();
                        offset.set(current + PAGE_SIZE);
                    },
                    "{store.t(\"common.next\")}" {icon_arrow_right()}
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
    let role = store.t(crate::i18n::role_key(entry.role()));
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
