//! Sidebar: draggable brand strip with a navigation toolbar, top controls,
//! project tree with runs, a Conversations section for orphaned runs and the
//! footer navigation between Chat / Usage / Customizations / Settings.

use std::collections::HashSet;

use super::icons::*;
use super::*;
use crate::state_ui::UiStore;
use dioxus::prelude::*;

/// Days since 1970-01-01 via Howard Hinnant's `days_from_civil` (pure Rust,
/// no chrono dependency).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// Parses the leading `YYYY-MM-DDTHH:MM:SS` of an ISO timestamp into seconds
/// since the Unix epoch (ignoring any fractional seconds / offset suffix).
fn epoch_seconds(iso: &str) -> Option<i64> {
    let b = iso.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |range: std::ops::Range<usize>| iso.get(range)?.parse::<i64>().ok();
    let year = num(0..4)?;
    let month = num(5..7)?;
    let day = num(8..10)?;
    let hour = num(11..13)?;
    let minute = num(14..16)?;
    let second = num(17..19)?;
    let separators_ok = b[4] == b'-'
        && b[7] == b'-'
        && (b[10] == b'T' || b[10] == b' ')
        && b[13] == b':'
        && b[16] == b':';
    if !separators_ok {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86400 + hour * 3600 + minute * 60 + second)
}

/// Compact relative age of a timestamp: `"now"` (< 60s), `"5m"` (< 60m),
/// `"3h"` (< 24h) or `"14d"` (days). Empty string when parsing fails.
pub fn relative_age(iso: &str, lang: crate::i18n::Lang) -> String {
    let Some(then) = epoch_seconds(iso) else {
        return String::new();
    };
    let Some(now) = epoch_seconds(&locagens_core::util::now_iso()) else {
        return String::new();
    };
    let elapsed = (now - then).max(0);
    if elapsed < 60 {
        crate::i18n::t(lang, "time.now").to_string()
    } else if elapsed < 3600 {
        crate::i18n::tf(lang, "time.minutes", &[&(elapsed / 60).to_string()])
    } else if elapsed < 86400 {
        crate::i18n::tf(lang, "time.hours", &[&(elapsed / 3600).to_string()])
    } else {
        crate::i18n::tf(lang, "time.days", &[&(elapsed / 86400).to_string()])
    }
}

#[component]
pub fn Sidebar() -> Element {
    let store: UiStore = use_context();
    let mut show_add_modal = use_signal(|| false);
    let mut open_run_menu: Signal<Option<String>> = use_signal(|| None);
    let mut open_project_menu: Signal<Option<String>> = use_signal(|| None);
    let mut search_open = use_signal(|| false);
    let expanded_all_projects = use_signal(HashSet::<String>::new);
    // Page the Back arrow came from, so Forward can return to it.
    let mut forward_page: Signal<Option<crate::state_ui::AppPage>> = use_signal(|| None);

    // Filtered runs by the search query (title/task match).
    let query = store.search_query.read().to_lowercase();
    let all_runs = store.runs.read().clone();
    let filtered: Vec<&Run> = all_runs
        .iter()
        .filter(|run| {
            query.is_empty()
                || run.title.to_lowercase().contains(&query)
                || run.task.to_lowercase().contains(&query)
        })
        .collect();

    // Group runs by project.
    let projects = store.projects.read().clone();
    let selected_project = store.selected_project.read().clone();
    let active_run_id = store.active_run_id.read().clone();
    // Collapsed workspace groups (by project path).
    let collapsed_projects = use_signal(HashSet::<String>::new);

    let page_now = store.page.read().clone();
    let on_chat = page_now == crate::state_ui::AppPage::Chat;
    let forward_target = *forward_page.read();
    let forward_available = on_chat && forward_target.is_some();

    rsx! {
        aside {
            class: "sidebar",
            onclick: move |_| {
                open_run_menu.set(None);
                open_project_menu.set(None);
            },
            if open_run_menu.read().is_some() || open_project_menu.read().is_some() {
                div {
                    class: "popover-backdrop",
                    onclick: move |e| {
                        e.stop_propagation();
                        open_run_menu.set(None);
                        open_project_menu.set(None);
                    },
                }
            }
            div {
                class: "sidebar-brand",
                // Draggable window titlebar strip (double-click to maximize).
                onmousedown: start_window_drag,
                ondoubleclick: move |_| {
                    dioxus::desktop::use_window().toggle_maximized();
                },
                div { class: "brand",
                    {logo_mark()}
                    span { class: "brand-title", "Locagens" }
                }
                div { class: "sidebar-toolbar",
                    ondoubleclick: move |event| event.stop_propagation(),
                    button {
                        class: "toolbar-btn",
                        title: store.t("sidebar.toggle"),
                        onmousedown: move |event| event.stop_propagation(),
                        onclick: move |_| {
                            if !on_chat {
                                let mut store = store;
                                store.page.set(crate::state_ui::AppPage::Chat);
                            } else {
                                let closing = *search_open.read();
                                if closing {
                                    let mut store = store;
                                    store.search_query.set(String::new());
                                }
                                search_open.set(!closing);
                            }
                        },
                        {icon_sidebar()}
                    }
                    button {
                        class: "toolbar-btn",
                        title: store.t("sidebar.back"),
                        disabled: on_chat,
                        onmousedown: move |event| event.stop_propagation(),
                        onclick: move |_| {
                            if !on_chat {
                                let mut store = store;
                                forward_page.set(Some(store.page.read().clone()));
                                store.page.set(crate::state_ui::AppPage::Chat);
                            }
                        },
                        {icon_arrow_left()}
                    }
                    button {
                        class: "toolbar-btn",
                        title: store.t("sidebar.forward"),
                        disabled: !forward_available,
                        onmousedown: move |event| event.stop_propagation(),
                        onclick: move |_| {
                            let mut store = store;
                            if let Some(target) = forward_target {
                                forward_page.set(None);
                                store.page.set(target);
                            }
                        },
                        {icon_arrow_right()}
                    }
                    button {
                        class: "toolbar-btn",
                        title: store.t("sidebar.search"),
                        onmousedown: move |event| event.stop_propagation(),
                        onclick: move |_| {
                            let closing = *search_open.read();
                            if closing {
                                let mut store = store;
                                store.search_query.set(String::new());
                            }
                            search_open.set(!closing);
                        },
                        {icon_search()}
                    }
                }
            }
            div {
                class: "sidebar-top-controls",
                button {
                    class: "new-conv-btn",
                    onclick: move |_| new_chat(&store),
                    span { class: "sidebar-nav-icon", {icon_plus()} }
                    span { "{store.t(\"conversation.new\")}" }
                }
                button {
                    class: if *store.page.read() == crate::state_ui::AppPage::UsageLogs { "sidebar-nav-item active" } else { "sidebar-nav-item" },
                    onclick: move |_| {
                        let mut store = store;
                        forward_page.set(None);
                        store.page.set(crate::state_ui::AppPage::UsageLogs);
                    },
                    span { class: "sidebar-nav-icon", {icon_clock()} }
                    span { "{store.t(\"sidebar.usage\")}" }
                }
                button {
                    class: if matches!(*store.page.read(), crate::state_ui::AppPage::Settings(crate::state_ui::SettingsTab::Skills | crate::state_ui::SettingsTab::Plugins)) { "sidebar-nav-item active" } else { "sidebar-nav-item" },
                    onclick: move |_| {
                        let mut store = store;
                        forward_page.set(None);
                        store.page.set(crate::state_ui::AppPage::Settings(crate::state_ui::SettingsTab::Skills));
                    },
                    span { class: "sidebar-nav-icon", {icon_briefcase()} }
                    span { "{store.t(\"sidebar.customizations\")}" }
                }
            }
            div {
                class: "sidebar-tree",
                if *search_open.read() || !store.search_query.read().is_empty() {
                    div { class: "sidebar-search",
                        input {
                            r#type: "text",
                            placeholder: store.t("sidebar.filter_conversations"),
                            value: "{store.search_query.read()}",
                            oninput: move |event| {
                                let mut store = store;
                                store.search_query.set(event.value());
                            },
                        }
                    }
                }
                div { class: "sidebar-section-header",
                    span { "{store.t(\"sidebar.projects\")}" }
                    div { class: "sidebar-section-actions",
                        button {
                            class: "subtle icon-btn",
                            title: store.t("sidebar.filter_projects"),
                            onmousedown: move |event| event.stop_propagation(),
                            onclick: move |_| {
                                let closing = *search_open.read();
                                if closing {
                                    let mut store = store;
                                    store.search_query.set(String::new());
                                }
                                search_open.set(!closing);
                            },
                            {icon_filter()}
                        }
                        button {
                            class: "subtle icon-btn",
                            title: store.t("sidebar.add_project"),
                            onmousedown: move |event| event.stop_propagation(),
                            onclick: move |_| show_add_modal.set(true),
                            {icon_folder_plus()}
                        }
                    }
                }
                for project in projects.iter() {
                    {
                        let runs: Vec<&Run> = filtered
                            .iter()
                            .copied()
                            .filter(|run| run.project_path == project.path)
                            .collect();
                        let is_selected = selected_project.as_deref() == Some(project.path.as_str());
                        let project_menu_open = open_project_menu.read().as_deref() == Some(project.path.as_str());
                        let project_row_class = if is_selected {
                            if project_menu_open { "project-row selected menu-open" } else { "project-row selected" }
                        } else if project_menu_open {
                            "project-row menu-open"
                        } else {
                            "project-row"
                        };
                        let path = project.path.clone();
                        let path_select = path.clone();
                        let path_delete = path.clone();
                        let path_toggle = path.clone();
                        let path_menu = path.clone();
                        let path_new = path.clone();
                        let path_expand = path.clone();
                        let is_collapsed = collapsed_projects.read().contains(&path);
                        let runs_expanded = expanded_all_projects.read().contains(&path) || runs.len() <= 6;
                        let run_count = runs.len();
                        rsx! {
                            div { class: "project-group",
                                div {
                                    class: "{project_row_class}",
                                    title: "{project.path}",
                                    onclick: move |_| {
                                        let mut store = store;
                                        store.selected_project.set(Some(path_select.clone()));
                                        let mut collapsed = collapsed_projects;
                                        if collapsed.contains(&path_toggle) {
                                            collapsed.remove(&path_toggle);
                                        } else {
                                            collapsed.insert(path_toggle.clone());
                                        }
                                    },
                                    span { class: "project-folder-icon", if is_collapsed { {icon_folder()} } else { {icon_folder_open()} } }
                                    span { class: "project-name", "{project.name}" }
                                    button {
                                        class: "subtle icon-btn project-menu-btn",
                                        title: store.t("sidebar.project_options"),
                                        onmousedown: move |event| event.stop_propagation(),
                                        onclick: move |event| {
                                            event.stop_propagation();
                                            open_run_menu.set(None);
                                            let current = open_project_menu.read().clone();
                                            if current.as_deref() == Some(path_menu.as_str()) {
                                                open_project_menu.set(None);
                                            } else {
                                                open_project_menu.set(Some(path_menu.clone()));
                                            }
                                        },
                                        {icon_dots_vertical()}
                                    }
                                    button {
                                        class: "subtle icon-btn project-new-btn",
                                        title: store.t("sidebar.new_in_project"),
                                        onmousedown: move |event| event.stop_propagation(),
                                        onclick: move |event| {
                                            event.stop_propagation();
                                            let mut store = store;
                                            store.selected_project.set(Some(path_new.clone()));
                                            new_chat(&store);
                                        },
                                        {icon_plus()}
                                    }
                                    if project_menu_open {
                                        div {
                                            class: "row-context-menu",
                                            onclick: move |event| event.stop_propagation(),
                                            button {
                                                class: "danger",
                                                onclick: move |_| {
                                                    open_project_menu.set(None);
                                                    delete_project(&store, &path_delete.clone());
                                                },
                                                "{store.t(\"sidebar.remove_workspace\")}"
                                            }
                                        }
                                    }
                                }
                                if !runs.is_empty() && !is_collapsed {
                                    div { class: "project-runs",
                                        if runs_expanded {
                                            for run in runs.iter() {
                                                { run_item(&store, run, active_run_id.as_deref(), open_run_menu, open_project_menu) }
                                            }
                                        } else {
                                            for run in runs.iter().take(6) {
                                                { run_item(&store, run, active_run_id.as_deref(), open_run_menu, open_project_menu) }
                                            }
                                            button {
                                                class: "see-all-btn",
                                                onclick: move |event| {
                                                    event.stop_propagation();
                                                    let mut expanded = expanded_all_projects;
                                                    expanded.insert(path_expand.clone());
                                                },
                                                "{store.t(\"sidebar.see_all\")} ({run_count})"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // Orphaned runs (project not registered) in their own section.
                div { class: "sidebar-section-header",
                    span { "{store.t(\"sidebar.conversations\")}" }
                    button {
                        class: "subtle icon-btn",
                        title: store.t("sidebar.new_conversation"),
                        onmousedown: move |event| event.stop_propagation(),
                        onclick: move |_| new_chat(&store),
                        {icon_plus()}
                    }
                }
                {
                    let known: Vec<String> = projects.iter().map(|project| project.path.clone()).collect();
                    let orphaned: Vec<&Run> = filtered
                        .iter()
                        .copied()
                        .filter(|run| !known.contains(&run.project_path))
                        .collect();
                    rsx! {
                        for run in orphaned.iter() {
                            { run_item(&store, run, active_run_id.as_deref(), open_run_menu, open_project_menu) }
                        }
                    }
                }
                if filtered.is_empty() {
                    div { class: "settings-empty", "{store.t(\"sidebar.empty\")}" }
                }
            }
            div {
                class: "sidebar-footer",
                button {
                    class: if matches!(*store.page.read(), crate::state_ui::AppPage::Settings(_))
                        && !matches!(*store.page.read(), crate::state_ui::AppPage::Settings(crate::state_ui::SettingsTab::Skills | crate::state_ui::SettingsTab::Plugins))
                    { "sidebar-nav-item active" } else { "sidebar-nav-item" },
                    onclick: move |_| {
                        let mut store = store;
                        forward_page.set(None);
                        store.page.set(crate::state_ui::AppPage::Settings(crate::state_ui::SettingsTab::Providers));
                    },
                    span { class: "sidebar-nav-icon", {icon_gear()} }
                    span { "{store.t(\"settings.title\")}" }
                }
            }
        }
        if *show_add_modal.read() {
            AddProjectModal { on_close: move || show_add_modal.set(false) }
        }
    }
}

/// One run row: title, status dot, relative age and a hover-only options menu
/// button. The age and the action button occupy the same 22px slot so the row
/// never shifts on hover.
fn run_item(store: &UiStore, run: &Run, active_run_id: Option<&str>, mut open_run_menu: Signal<Option<String>>, mut open_project_menu: Signal<Option<String>>) -> Element {
    let store = *store;
    let run_id = run.id.clone();
    let run_id_select = run_id.clone();
    let run_id_fork = run_id.clone();
    let run_id_delete = run_id.clone();
    let run_id_menu = run_id.clone();
    let run_title = run.title.clone();
    let model = run.model.clone();
    let status = run.status.clone();
    let is_live = status == "generating" || status == "created";
    let age_source = if run.updated_at.is_empty() { run.created_at.clone() } else { run.updated_at.clone() };
    let age = relative_age(&age_source, store.lang());
    let is_active = active_run_id == Some(run.id.as_str());
    let menu_open = open_run_menu.read().as_deref() == Some(run.id.as_str());
    let item_class = match (is_active, menu_open) {
        (true, true) => "run-item active menu-open",
        (true, false) => "run-item active",
        (false, true) => "run-item menu-open",
        (false, false) => "run-item",
    };
    rsx! {
        div {
            class: "{item_class}",
            title: "{run_title} · {model}",
            onclick: move |_| store.select_run(run_id_select.clone()),
            span { class: "run-title", "{run_title}" }
            if is_live {
                {status_dot(&status)}
            }
            span { class: "run-age", "{age}" }
            button {
                class: "subtle icon-btn run-action",
                title: store.t("sidebar.conversation_options"),
                onmousedown: move |event| event.stop_propagation(),
                onclick: move |event| {
                    event.stop_propagation();
                    open_project_menu.set(None);
                    let current = open_run_menu.read().clone();
                    if current.as_deref() == Some(run_id_menu.as_str()) {
                        open_run_menu.set(None);
                    } else {
                        open_run_menu.set(Some(run_id_menu.clone()));
                    }
                },
                {icon_dots_vertical()}
            }
            if menu_open {
                div {
                    class: "row-context-menu",
                    onclick: move |event| event.stop_propagation(),
                    button {
                        onclick: move |_| {
                            open_run_menu.set(None);
                            fork_run_action(&store, &run_id_fork.clone());
                        },
                        "{store.t(\"sidebar.fork\")}"
                    }
                    button {
                        class: "danger",
                        onclick: move |_| {
                            open_run_menu.set(None);
                            delete_run(&store, &run_id_delete.clone());
                        },
                        "{store.t(\"common.delete\")}"
                    }
                }
            }
        }
    }
}

/// Modal to register a project: manual path input or native folder picker.
#[component]
fn AddProjectModal(on_close: EventHandler) -> Element {
    let store: UiStore = use_context();
    let mut path = use_signal(String::new);
    let busy = use_signal(|| false);

    rsx! {
        div {
            class: "modal-backdrop",
            onclick: move |_| on_close.call(()),
            div {
                class: "modal",
                onclick: move |event| event.stop_propagation(),
                h3 { "{store.t(\"sidebar.add_project_title\")}" }
                label { "{store.t(\"sidebar.project_folder\")}" }
                div {
                    class: "composer-row-top",
                    input {
                        placeholder: r"C:\path\to\project",
                        value: "{path.read()}",
                        oninput: move |event| path.set(event.value()),
                    }
                    button {
                        disabled: *busy.read(),
                        onclick: move |_| {
                            let mut path = path;
                            spawn(async move {
                                if let Some(folder) = rfd::AsyncFileDialog::new().pick_folder().await {
                                    path.set(folder.path().to_string_lossy().to_string());
                                }
                            });
                        },
                        "{store.t(\"common.browse\")}"
                    }
                }
                div {
                    class: "modal-actions",
                    button { onclick: move |_| on_close.call(()), "{store.t(\"common.cancel\")}" }
                    button {
                        class: "primary",
                        disabled: *busy.read() || path.read().trim().is_empty(),
                        onclick: move |_| {
                            let store = store;
                            let mut busy = busy;
                            let raw = path.read().trim().to_string();
                            busy.set(true);
                            spawn(async move {
                                let state = crate::state_ui::core();
                                let result = locagens_core::api::projects::create_project(
                                    &state,
                                    locagens_core::api::projects::CreateProjectInput {
                                        path: Some(raw.clone()),
                                        name: None,
                                    },
                                )
                                .await;
                                match result {
                                    Ok(_project) => {
                                        store.refresh_projects();
                                        on_close.call(());
                                    }
                                    Err(err) => store.toast(store.tf("toast.add_project_failed", &[&err.to_string()])),
                                }
                                busy.set(false);
                            });
                        },
                        "{store.t(\"sidebar.add_project_title\")}"
                    }
                }
            }
        }
    }
}

/// Clears the active conversation so the composer starts a fresh run.
fn new_chat(store: &UiStore) {
    let mut store = *store;
    store.active_run_id.set(None);
    store.active_run.set(None);
    store.messages.set(Vec::new());
    store.plan.set(None);
    store.run_usage.set(None);
    store.pending_permission.set(None);
    store.pending_question.set(None);
    store.turn_diff.set(None);
    store.spawned_agents.set(Vec::new());
    store.page.set(crate::state_ui::AppPage::Chat);
}

/// Registers nothing — removes a project registration.
fn delete_project(store: &UiStore, path: &str) {
    let mut store = *store;
    let path = path.to_string();
    spawn(async move {
        let state = crate::state_ui::core();
        if let Err(err) = locagens_core::api::projects::delete_project(&state, &path).await {
            store.toast(store.tf("toast.remove_project_failed", &[&err.to_string()]));
        }
        let projects = locagens_core::api::projects::list_projects(&state).await.unwrap_or_default();
        store.projects.set(projects);
        let still_registered = store
            .projects
            .read()
            .iter()
            .any(|project| project.path == path);
        if !still_registered && store.selected_project.read().as_deref() == Some(path.as_str()) {
            store.selected_project.set(store.projects.read().first().map(|project| project.path.clone()));
        }
    });
}

/// Forks a run into an independent copy and selects it.
fn fork_run_action(store: &UiStore, run_id: &str) {
    let store = *store;
    let run_id = run_id.to_string();
    spawn(async move {
        let state = crate::state_ui::core();
        match locagens_core::api::runs::fork_run(&state, &run_id, None).await {
            Ok(forked) => {
                store.refresh_runs();
                store.select_run(forked.id.clone());
                store.toast(store.t("toast.forked").to_string());
            }
            Err(err) => store.toast(store.tf("toast.fork_failed", &[&err.to_string()])),
        }
    });
}

/// Deletes a run (direct DB call; the facade does not expose a wrapper).
fn delete_run(store: &UiStore, run_id: &str) {
    let mut store = *store;
    let run_id = run_id.to_string();
    spawn(async move {
        let state = crate::state_ui::core();
        state.db.delete_run(&run_id);
        if store.active_run_id.read().as_deref() == Some(run_id.as_str()) {
            store.active_run_id.set(None);
            store.active_run.set(None);
            store.messages.set(Vec::new());
            store.plan.set(None);
            store.run_usage.set(None);
            store.pending_permission.set(None);
            store.pending_question.set(None);
            store.turn_diff.set(None);
            store.spawned_agents.set(Vec::new());
        }
        store.refresh_runs();
    });
}
