//! Right inspector: a flat column of collapsible sections (Environment,
//! Context, Plan, Agents, Changes, Terminal), all wired in-process to
//! `locagens_core::api`.

use crate::components::icons::*;
use crate::components::message_thread::DiffView;
use crate::components::start_window_drag;
use crate::markdown::render_markdown;
use crate::state_ui::{core, RightPanelTab, UiStore};
use dioxus::prelude::*;
use locagens_core::api::projects::{GitFileDiff, GitStatus, GitWorktree};
use locagens_core::api::processes::{list_processes, process_logs, spawn_process, SpawnProcessInput};
use locagens_core::services::processes::ProcessInfo;
use serde_json::Value;
use std::time::Duration;

/// i18n key of a changed file's kind: the one-letter badge or its tooltip.
fn file_kind_key(kind: &str, badge: bool) -> &'static str {
    match (kind, badge) {
        ("created", true) => "git.badge_created",
        ("created", false) => "git.kind_created",
        ("deleted", true) => "git.badge_deleted",
        ("deleted", false) => "git.kind_deleted",
        (_, true) => "git.badge_modified",
        (_, false) => "git.kind_modified",
    }
}

#[component]
pub fn RightPanel() -> Element {
    let store: UiStore = use_context();
    let changed_files = store
        .turn_diff
        .read()
        .as_ref()
        .map(|(_, files)| *files)
        .filter(|files| *files > 0);
    let task_count = store
        .plan
        .read()
        .as_ref()
        .and_then(|plan| plan.tasks.as_array().map(|tasks| tasks.len()))
        .filter(|count| *count > 0);
    let agent_count = {
        let messages = store.messages.read();
        let mut names: std::collections::HashSet<String> = messages
            .iter()
            .filter(|message| message.agent_role.is_some())
            .map(|message| message.agent_name.clone().or(message.agent_role.clone()).unwrap_or_default())
            .collect();
        for agent in store.spawned_agents.read().iter() {
            if let Some(name) = agent.get("name").and_then(Value::as_str) {
                names.insert(name.to_string());
            }
        }
        names.len()
    };

    rsx! {
        div {
            class: "right-panel-head",
            // Draggable window titlebar strip (double-click maximizes, see start_window_drag).
            onmousedown: start_window_drag,
            span { class: "head-title", "{store.t(\"inspector.title\")}" }
        }
        div {
            class: "right-panel-body",
            EnvironmentSection {}
            ContextSection {}
            for tab in RightPanelTab::ALL {
                InspectorSection {
                    key: "{tab.label_key()}",
                    tab,
                    count: match tab {
                        RightPanelTab::Plan => task_count,
                        RightPanelTab::Agents => Some(agent_count).filter(|count| *count > 0),
                        RightPanelTab::Git => changed_files,
                        RightPanelTab::Terminal => None,
                    },
                }
            }
        }
    }
}

/// Muted icon, one-line title and optional hint for an empty section.
#[component]
fn EmptyState(icon: Element, title: String, hint: Option<String>) -> Element {
    rsx! {
        div {
            class: "empty-state",
            {icon}
            div { class: "empty-title", "{title}" }
            if let Some(hint) = hint {
                div { class: "empty-hint", "{hint}" }
            }
        }
    }
}

/// One collapsible section: icon, name, optional count chip and the tab body.
#[component]
fn InspectorSection(tab: RightPanelTab, count: Option<usize>) -> Element {
    let store: UiStore = use_context();
    let initially_open = matches!(tab, RightPanelTab::Plan | RightPanelTab::Agents)
        || *store.right_panel_tab.peek() == tab;
    let mut open = use_signal(|| initially_open);
    let (name, icon) = match tab {
        RightPanelTab::Plan => (store.t("inspector.plan"), icon_list_checks()),
        RightPanelTab::Agents => (store.t("inspector.agents"), icon_users()),
        RightPanelTab::Git => (store.t("inspector.changes"), icon_git_branch()),
        RightPanelTab::Terminal => (store.t("inspector.terminal"), icon_terminal()),
    };
    rsx! {
        section {
            class: "inspector-section",
            button {
                class: "section-head",
                title: "{name}",
                onclick: move |_| {
                    let mut store = store;
                    let next = !*open.read();
                    open.set(next);
                    if next {
                        store.right_panel_tab.set(tab);
                    }
                },
                span { class: if *open.read() { "chev open" } else { "chev" }, {icon_chevron_right()} }
                {icon}
                span { class: "section-name", "{name}" }
                if let Some(count) = count {
                    span { class: "count-chip", "{count}" }
                }
            }
            if *open.read() {
                div {
                    class: "section-body",
                    match tab {
                        RightPanelTab::Plan => rsx! { PlanTab {} },
                        RightPanelTab::Agents => rsx! { AgentsTab {} },
                        RightPanelTab::Git => rsx! { GitTab {} },
                        RightPanelTab::Terminal => rsx! { TerminalTab {} },
                    }
                }
            }
        }
    }
}

/// Project folder, git branch and execution mode of the current conversation.
#[component]
fn EnvironmentSection() -> Element {
    let store: UiStore = use_context();
    let project_path = inspector_project_path(&store);
    let branch = use_resource(move || async move {
        let path = inspector_project_path(&store)?;
        let state = core();
        locagens_core::api::projects::git_status(&state, &path)
            .await
            .ok()
            .filter(|status| status.is_git)
            .and_then(|status| status.branch)
    });
    let folder = project_path
        .as_deref()
        .and_then(|path| std::path::Path::new(path).file_name())
        .map(|name| name.to_string_lossy().to_string());
    let mode = match store.selected_mode.read().as_str() {
        "plan" => store.t("mode.plan"),
        "auto" => store.t("mode.auto"),
        _ => store.t("mode.build"),
    };
    let branch = branch.read().clone().flatten();
    rsx! {
        section {
            class: "inspector-section",
            div { class: "section-label", "{store.t(\"inspector.environment\")}" }
            div {
                class: "section-body",
                if let Some(folder) = folder {
                    div {
                        class: "info-row",
                        title: "{project_path.clone().unwrap_or_default()}",
                        {icon_folder()}
                        span { class: "grow", "{folder}" }
                    }
                }
                if let Some(branch) = branch {
                    div {
                        class: "info-row",
                        {icon_git_branch()}
                        span { class: "grow mono", "{branch}" }
                    }
                }
                div {
                    class: "info-row",
                    {icon_shield()}
                    span { class: "grow", "{mode}" }
                }
            }
        }
    }
}

/// Context-window fill of the active run: tokens after the main agent's
/// latest model call against the model's window (`api::runs::context_usage`,
/// refreshed on every `usage_recorded` event).
#[component]
fn ContextSection() -> Element {
    let store: UiStore = use_context();
    if store.active_run_id.read().is_none() {
        return rsx! {};
    }
    let context = store.context_usage.read().clone().unwrap_or_default();
    let used = crate::state_ui::fmt_tokens(context.used_tokens);
    let amount = match context.context_window {
        Some(window) => format!("{used} / {}", crate::state_ui::fmt_tokens(window)),
        None => store.tf("header.tokens", &[&used]),
    };
    let meter_class = match context.percent {
        Some(percent) if percent >= 90 => "meter-fill danger",
        Some(percent) if percent >= 75 => "meter-fill warn",
        _ => "meter-fill",
    };
    rsx! {
        section {
            class: "inspector-section",
            div { class: "section-label", "{store.t(\"inspector.context\")}" }
            div {
                class: "section-body",
                div {
                    class: "info-row",
                    span { class: "grow", "{store.t(\"inspector.window_used\")}" }
                    span { class: "mono", "{amount}" }
                    if let Some(percent) = context.percent {
                        span { class: "mono muted", "{percent}%" }
                    }
                }
                if let Some(percent) = context.percent {
                    div {
                        class: "meter",
                        role: "progressbar",
                        "aria-valuemin": "0",
                        "aria-valuemax": "100",
                        "aria-valuenow": "{percent}",
                        "aria-label": "{store.t(\"inspector.window_used\")}",
                        div { class: "{meter_class}", style: "width: {percent}%" }
                    }
                }
            }
        }
    }
}

/// The active plan: title, Markdown body and tracked tasks.
#[component]
fn PlanTab() -> Element {
    let store: UiStore = use_context();
    let Some(plan) = store.plan.read().clone() else {
        return rsx! {
            EmptyState {
                icon: icon_list_checks(),
                title: store.t("inspector.no_plan"),
                hint: store.t("inspector.no_plan_hint").to_string(),
            }
        };
    };
    let tasks: Vec<(String, String)> = plan
        .tasks
        .as_array()
        .map(|tasks| {
            tasks
                .iter()
                .filter_map(|task| {
                    let text = task.get("text").and_then(Value::as_str)?;
                    let status = task.get("status").and_then(Value::as_str).unwrap_or("pending");
                    Some((text.to_string(), status.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();

    rsx! {
        div {
            class: "panel-section-title",
            "{store.tf(\"inspector.plan_version\", &[&plan.title, &plan.version.to_string()])}"
        }
        if let Some(body) = plan.body.clone() {
            div {
                class: "panel-card md",
                dangerous_inner_html: render_markdown(&body),
            }
        }
        if !tasks.is_empty() {
            div {
                class: "panel-card",
                for (index, (text, status)) in tasks.iter().enumerate() {
                    div {
                        key: "{index}",
                        class: "plan-task task-{status}",
                        span {
                            class: "task-icon",
                            match status.as_str() {
                                "completed" => icon_task_done(),
                                "in_progress" => icon_task_active(),
                                _ => icon_task_pending(),
                            }
                        }
                        span { class: "task-text", "{text}" }
                    }
                }
            }
        }
    }
}

/// Sub-agent summary for the active run: live `spawn_agent` / `spawn_agents`
/// registrations (role + status, visible immediately) combined with agents
/// derived from persisted messages.
#[component]
fn AgentsTab() -> Element {
    let store: UiStore = use_context();
    let messages = store.messages.read().clone();
    let spawned = store.spawned_agents.read().clone();
    let agents: Vec<(String, String, usize)> = {
        // Group by (role, name).
        let mut map: std::collections::BTreeMap<(String, String), usize> =
            std::collections::BTreeMap::new();
        for message in messages.iter() {
            if let Some(role) = message.agent_role.clone() {
                let name = message.agent_name.clone().unwrap_or_else(|| role.clone());
                *map.entry((role, name)).or_insert(0) += 1;
            }
        }
        map.into_iter().map(|((role, name), count)| (role, name, count)).collect()
    };

    // Names already covered by a live spawned-agent entry (avoid duplicates).
    let spawned_names: std::collections::HashSet<String> = spawned
        .iter()
        .filter_map(|agent| agent.get("name").and_then(Value::as_str).map(str::to_string))
        .collect();
    let derived: Vec<(String, String, usize)> = agents
        .into_iter()
        .filter(|(_, name, _)| !spawned_names.contains(name))
        .collect();

    if spawned.is_empty() && derived.is_empty() {
        return rsx! {
            EmptyState {
                icon: icon_users(),
                title: store.t("inspector.no_agents"),
                hint: store.t("inspector.no_agents_hint").to_string(),
            }
        };
    }
    rsx! {
        for (index, agent) in spawned.iter().enumerate() {
            {
                let agent_id = agent.get("agentId").and_then(Value::as_str).unwrap_or("").to_string();
                let name = agent.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                let role = agent.get("role").and_then(Value::as_str).unwrap_or("explorer").to_string();
                let status = agent.get("status").and_then(Value::as_str).unwrap_or("running").to_string();
                rsx! {
                    div {
                        key: "spawned-{index}-{agent_id}",
                        class: "list-row",
                        if role == "worker" { {icon_wrench()} } else { {icon_search()} }
                        span { class: "grow", "{name}" }
                        span { class: "badge", "{role}" }
                        span { class: "badge", "{status}" }
                    }
                }
            }
        }
        for (index, (role, name, count)) in derived.iter().enumerate() {
            div {
                key: "derived-{index}",
                class: "list-row",
                if role == "worker" { {icon_wrench()} } else { {icon_search()} }
                span { class: "grow", "{name}" }
                span { class: "badge", "{role}" }
                span { class: "count-chip", "{count}" }
            }
        }
    }
}

/// Resolves the project folder the inspector acts on: the selected project,
/// falling back to the active run's project.
fn inspector_project_path(store: &UiStore) -> Option<String> {
    store
        .selected_project
        .read()
        .clone()
        .or_else(|| store.active_run.read().as_ref().map(|run| run.project_path.clone()))
}

/// Git workbench: branch status, changed files with stage / unstage, diffs,
/// commit and push. Data refreshes every few seconds and after each action.
#[component]
fn GitTab() -> Element {
    let store: UiStore = use_context();
    let status: Signal<Option<GitStatus>> = use_signal(|| None);
    let diffs: Signal<Vec<GitFileDiff>> = use_signal(Vec::new);
    let worktrees: Signal<Vec<GitWorktree>> = use_signal(Vec::new);
    let mut selected_file: Signal<Option<String>> = use_signal(|| None);
    let mut message = use_signal(String::new);
    let mut worktree_branch = use_signal(String::new);
    let busy = use_signal(|| false);
    let mut version = use_signal(|| 0u32);

    // Load status + changed files, then keep them fresh. Bumping `version`
    // restarts the poll loop immediately (used after staging actions).
    use_effect(move || {
        let path = inspector_project_path(&store);
        let mut status = status;
        let mut diffs = diffs;
        let mut worktrees = worktrees;
        let current_version = *version.read();
        spawn(async move {
            let state = core();
            loop {
                if *version.read() != current_version {
                    break;
                }
                if let Some(path) = path.as_deref() {
                    status.set(locagens_core::api::projects::git_status(&state, path).await.ok());
                    diffs.set(
                        locagens_core::api::projects::git_diff_details(&state, path)
                            .await
                            .unwrap_or_default(),
                    );
                    worktrees.set(
                        locagens_core::api::projects::list_worktrees(&state, path)
                            .await
                            .unwrap_or_default(),
                    );
                } else {
                    status.set(None);
                    diffs.set(Vec::new());
                    worktrees.set(Vec::new());
                }
                tokio::time::sleep(Duration::from_millis(4000)).await;
            }
        });
    });

    let project_path = inspector_project_path(&store);
    let path_for_commit = project_path.clone().unwrap_or_default();
    let path_for_push = project_path.clone().unwrap_or_default();
    let current_status = status.read().clone();
    let changed_files = diffs.read().clone();
    let selected = selected_file.read().clone();
    let selected_diff: Option<(String, String)> = selected
        .as_ref()
        .and_then(|path| changed_files.iter().find(|diff| &diff.path == path))
        .map(|diff| {
            let text = match diff.kind.as_str() {
                "created" => crate::markdown::pseudo_diff(None, Some(diff.new_text.as_str())),
                "deleted" => crate::markdown::pseudo_diff(Some(diff.old_text.as_str()), None),
                _ => crate::markdown::pseudo_diff(
                    Some(diff.old_text.as_str()),
                    Some(diff.new_text.as_str()),
                ),
            };
            (diff.path.clone(), text)
        });

    let path_for_stage_all = project_path.clone();
    let path_for_unstage_all = project_path.clone();
    let path_for_undo = project_path.clone();
    let path_for_worktree_create = project_path.clone();
    let worktree_list = worktrees.read().clone();
    let active_run_id = store.active_run_id.read().clone();
    let turn_diff = store.turn_diff.read().clone();
    let active_turn_diff: Option<(String, usize)> = turn_diff
        .filter(|(diff, _)| !diff.trim().is_empty());

    rsx! {
        if let Some((diff, files_changed)) = active_turn_diff {
            div {
                class: "panel-card",
                div {
                    class: "panel-section-title",
                    "{store.tf(\"git.turn_diff\", &[&files_changed.to_string()])}"
                }
                crate::components::cards::DiffViewInline { diff: diff }
            }
        }
        match current_status {
            Some(git_status) if git_status.is_git => {
                rsx! {
                    div {
                        class: "panel-card",
                        div {
                            class: "panel-row",
                            {icon_git_branch()}
                            span { class: "grow mono", "{git_status.branch.clone().unwrap_or_default()}" }
                            if git_status.has_changes.unwrap_or(false) {
                                span { class: "tree-state dirty", "{store.t(\"git.dirty\")}" }
                            } else {
                                span { class: "tree-state clean", "{store.t(\"git.clean\")}" }
                            }
                        }
                    }
                    if !changed_files.is_empty() {
                        div {
                            class: "panel-card",
                            div {
                                class: "panel-row",
                                span { class: "panel-section-title grow", "{store.t(\"git.changed_files\")}" }
                                button {
                                    class: "subtle small",
                                    title: store.t("git.undo_title"),
                                    onclick: move |_| {
                                        let path = path_for_undo.clone();
                                        let run_id = active_run_id.clone();
                                        spawn(async move {
                                            let state = core();
                                            let Some(path) = path else { return };
                                            match locagens_core::api::projects::undo_last_turn(&state, &path, run_id.as_deref()).await {
                                                Ok(snapshot) => {
                                                    store.toast(store.tf("toast.restored", &[&snapshot.id]));
                                                }
                                                Err(err) => store.toast(store.tf("toast.undo_failed", &[&err.to_string()])),
                                            }
                                            version.with_mut(|value| *value += 1);
                                        });
                                    },
                                    "{store.t(\"git.undo\")}"
                                }
                                button {
                                    class: "subtle small",
                                    onclick: move |_| {
                                        let path = path_for_stage_all.clone();
                                        spawn(async move {
                                            let state = core();
                                            let Some(path) = path else { return };
                                            if let Err(err) = locagens_core::api::projects::git_stage(&state, &path, &[]).await {
                                                store.toast(store.tf("toast.stage_failed", &[&err.to_string()]));
                                            }
                                            version.with_mut(|value| *value += 1);
                                        });
                                    },
                                    "{store.t(\"git.stage_all\")}"
                                }
                                button {
                                    class: "subtle small",
                                    onclick: move |_| {
                                        let path = path_for_unstage_all.clone();
                                        spawn(async move {
                                            let state = core();
                                            let Some(path) = path else { return };
                                            if let Err(err) = locagens_core::api::projects::git_unstage(&state, &path, &[]).await {
                                                store.toast(store.tf("toast.unstage_failed", &[&err.to_string()]));
                                            }
                                            version.with_mut(|value| *value += 1);
                                        });
                                    },
                                    "{store.t(\"git.unstage_all\")}"
                                }
                            }
                            for diff in changed_files.iter() {
                                {
                                    let file_path = diff.path.clone();
                                    let display_path = diff.path.clone();
                                    let file_kind = diff.kind.clone();
                                    let stage_path = diff.path.clone();
                                    let unstage_path = diff.path.clone();
                                    let stage_project = project_path.clone();
                                    let unstage_project = project_path.clone();
                                    let selected_now = selected.clone();
                                    rsx! {
                                        div {
                                            key: "{file_path}",
                                            class: "file-row",
                                            span {
                                                class: "file-kind {file_kind}",
                                                title: store.t(file_kind_key(&file_kind, false)),
                                                {store.t(file_kind_key(&file_kind, true))}
                                            }
                                            div {
                                                class: "file-main",
                                                role: "button",
                                                tabindex: "0",
                                                onclick: move |_| selected_file.set(if selected_now.as_ref() == Some(&file_path) { None } else { Some(file_path.clone()) }),
                                                div { class: "file-name", title: "{display_path}", "{display_path}" }
                                            }
                                            button {
                                                class: "subtle small",
                                                onclick: move |_| {
                                                    let path = stage_project.clone();
                                                    let files = vec![stage_path.clone()];
                                                    spawn(async move {
                                                        let state = core();
                                                        let Some(path) = path else { return };
                                                        if let Err(err) = locagens_core::api::projects::git_stage(&state, &path, &files).await {
                                                            store.toast(store.tf("toast.stage_failed", &[&err.to_string()]));
                                                        }
                                                        version.with_mut(|value| *value += 1);
                                                    });
                                                },
                                                "{store.t(\"git.stage\")}"
                                            }
                                            button {
                                                class: "subtle small",
                                                onclick: move |_| {
                                                    let path = unstage_project.clone();
                                                    let files = vec![unstage_path.clone()];
                                                    spawn(async move {
                                                        let state = core();
                                                        let Some(path) = path else { return };
                                                        if let Err(err) = locagens_core::api::projects::git_unstage(&state, &path, &files).await {
                                                            store.toast(store.tf("toast.unstage_failed", &[&err.to_string()]));
                                                        }
                                                        version.with_mut(|value| *value += 1);
                                                    });
                                                },
                                                "{store.t(\"git.unstage\")}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Some((diff_path, diff_text)) = selected_diff {
                        div {
                            class: "panel-card",
                            div {
                                class: "panel-section-title",
                                "{store.t(\"git.diff\")} · {diff_path}"
                            }
                            DiffView { diff: diff_text }
                        }
                    }
                    div {
                        class: "panel-card",
                        div {
                            class: "panel-section-title",
                            "{store.t(\"git.worktrees\")}"
                        }
                        div {
                            class: "panel-row",
                            input {
                                placeholder: store.t("git.branch"),
                                class: "grow",
                                value: "{worktree_branch.read()}",
                                oninput: move |event| worktree_branch.set(event.value()),
                            }
                            button {
                                class: "primary",
                                disabled: worktree_branch.read().trim().is_empty(),
                                onclick: move |_| {
                                    let path = path_for_worktree_create.clone();
                                    let branch = worktree_branch.read().trim().to_string();
                                    spawn(async move {
                                        let state = core();
                                        let Some(path) = path else { return };
                                        match locagens_core::api::projects::create_worktree(&state, &path, &branch, true).await {
                                            Ok(_) => {
                                                worktree_branch.set(String::new());
                                                store.toast(store.tf("toast.worktree_created", &[&branch]));
                                                store.refresh_projects();
                                            }
                                            Err(err) => store.toast(store.tf("toast.worktree_failed", &[&err.to_string()])),
                                        }
                                        version.with_mut(|value| *value += 1);
                                    });
                                },
                                {icon_plus()} "{store.t(\"git.worktree\")}"
                            }
                        }
                        if worktree_list.is_empty() {
                            div { class: "empty-hint inline", "{store.t(\"git.no_worktrees\")}" }
                        }
                        for worktree in worktree_list.iter() {
                            {
                                let wt_path = worktree.path.clone();
                                let wt_branch = worktree.branch.clone().unwrap_or_else(|| store.t("git.detached").to_string());
                                let switch_path = worktree.path.clone();
                                let remove_path = worktree.path.clone();
                                let remove_project = project_path.clone();
                                rsx! {
                                    div {
                                        key: "{wt_path}",
                                        class: "settings-row",
                                        div {
                                            class: "grow clickable",
                                            role: "button",
                                            tabindex: "0",
                                            onclick: move |_| {
                                                let switch_path = switch_path.clone();
                                                spawn(async move {
                                                    let mut store = store;
                                                    let state = core();
                                                    if let Err(err) = locagens_core::api::projects::create_project(
                                                        &state,
                                                        locagens_core::api::projects::CreateProjectInput {
                                                            path: Some(switch_path.clone()),
                                                            name: None,
                                                        },
                                                    ).await {
                                                        store.toast(store.tf("toast.switch_failed", &[&err.to_string()]));
                                                        return;
                                                    }
                                                    store.refresh_projects();
                                                    store.selected_project.set(Some(switch_path));
                                                });
                                            },
                                            div {
                                                class: "panel-row",
                                                span { class: "mono grow", "{wt_branch}" }
                                                if worktree.is_main {
                                                    span { class: "badge", "main" }
                                                }
                                            }
                                            div {
                                                class: "mono path-line",
                                                title: "{wt_path}",
                                                "{wt_path}"
                                            }
                                        }
                                        if !worktree.is_main {
                                            button {
                                                class: "icon-btn danger",
                                                title: store.t("git.remove_worktree"),
                                                onclick: move |_| {
                                                    let project = remove_project.clone();
                                                    let remove_path = remove_path.clone();
                                                    spawn(async move {
                                                        let state = core();
                                                        let Some(project) = project else { return };
                                                        match locagens_core::api::projects::remove_worktree(&state, &project, &remove_path, false).await {
                                                            Ok(()) => {
                                                                store.toast(store.t("toast.worktree_removed").to_string());
                                                                store.refresh_projects();
                                                            }
                                                            Err(err) => store.toast(store.tf("toast.remove_failed", &[&err.to_string()])),
                                                        }
                                                        version.with_mut(|value| *value += 1);
                                                    });
                                                },
                                                {icon_x()}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    div {
                        class: "composer-row-top",
                        input {
                            placeholder: store.t("git.commit_message"),
                            value: "{message.read()}",
                            oninput: move |event| message.set(event.value()),
                        }
                    }
                    div {
                        class: "panel-actions",
                        button {
                            class: "primary",
                            disabled: *busy.read() || message.read().trim().is_empty(),
                            onclick: move |_| {
                                let mut busy = busy;
                                let path = path_for_commit.clone();
                                let commit_message = message.read().trim().to_string();
                                busy.set(true);
                                spawn(async move {
                                    let state = core();
                                    let result = locagens_core::api::projects::git_commit(
                                        &state,
                                        locagens_core::api::projects::GitCommitInput {
                                            path: Some(path),
                                            message: Some(commit_message),
                                            action: Some("commit".to_string()),
                                        },
                                    )
                                    .await;
                                    if let Err(err) = result {
                                        store.toast(store.tf("toast.commit_failed", &[&err.to_string()]));
                                    } else {
                                        store.toast(store.t("toast.committed").to_string());
                                        message.set(String::new());
                                    }
                                    busy.set(false);
                                    version.with_mut(|value| *value += 1);
                                });
                            },
                            "{store.t(\"git.commit\")}"
                        }
                        button {
                            disabled: *busy.read(),
                            onclick: move |_| {
                                let mut busy = busy;
                                let path = path_for_push.clone();
                                busy.set(true);
                                spawn(async move {
                                    let state = core();
                                    let result = locagens_core::api::projects::git_commit(
                                        &state,
                                        locagens_core::api::projects::GitCommitInput {
                                            path: Some(path),
                                            message: None,
                                            action: Some("push".to_string()),
                                        },
                                    )
                                    .await;
                                    if let Err(err) = result {
                                        store.toast(store.tf("toast.push_failed", &[&err.to_string()]));
                                    } else {
                                        store.toast(store.t("toast.pushed").to_string());
                                    }
                                    busy.set(false);
                                });
                            },
                            "{store.t(\"git.push\")}"
                        }
                    }
                }
            }
            Some(_) => rsx! {
                EmptyState { icon: icon_git_branch(), title: store.t("git.not_repo"), hint: store.t("git.not_repo_hint").to_string() }
            },
            None => rsx! {
                EmptyState { icon: icon_git_branch(), title: store.t("git.no_project"), hint: store.t("git.no_project_hint").to_string() }
            },
        }
    }
}

/// Terminal / Processes: launch background commands in the selected project,
/// watch their stdout/stderr logs and stop them.
#[component]
fn TerminalTab() -> Element {
    let store: UiStore = use_context();
    let processes: Signal<Vec<ProcessInfo>> = use_signal(Vec::new);
    let mut selected: Signal<Option<String>> = use_signal(|| None);
    let logs: Signal<Option<locagens_core::api::processes::ProcessLogs>> = use_signal(|| None);
    let mut command = use_signal(String::new);
    let busy = use_signal(|| false);
    let mut version = use_signal(|| 0u32);

    let project_path = inspector_project_path(&store);

    // Load + poll the process list; bumping `version` restarts it.
    use_effect(move || {
        let path = inspector_project_path(&store);
        let mut processes = processes;
        let current_version = *version.read();
        spawn(async move {
            let state = core();
            loop {
                if *version.read() != current_version {
                    break;
                }
                processes.set(
                    list_processes(&state, path.as_deref())
                        .await
                        .unwrap_or_default(),
                );
                tokio::time::sleep(Duration::from_millis(2500)).await;
            }
        });
    });

    // Poll the selected process's logs; ends when the process disappears or
    // the selection changes.
    use_effect(move || {
        let id = selected.read().clone();
        let mut logs = logs;
        spawn(async move {
            let state = core();
            let Some(id) = id else {
                logs.set(None);
                return;
            };
            loop {
                if selected.read().as_deref() != Some(id.as_str()) {
                    break;
                }
                match process_logs(&state, &id).await {
                    Ok(data) => logs.set(Some(data)),
                    Err(_) => {
                        logs.set(None);
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(1200)).await;
            }
        });
    });

    let process_list = processes.read().clone();
    let selected_id = selected.read().clone();
    let log_data = logs.read().clone();
    let path_for_spawn_key = project_path.clone();
    let path_for_spawn_run = project_path.clone();

    rsx! {
        div {
            class: "composer-row-top",
            input {
                placeholder: store.t("term.command"),
                value: "{command.read()}",
                oninput: move |event| command.set(event.value()),
                onkeydown: move |event: KeyboardEvent| {
                    if event.key() == Key::Enter && !command.read().trim().is_empty() {
                        let mut busy = busy;
                        let cmd = command.read().trim().to_string();
                        let path = path_for_spawn_key.clone();
                        busy.set(true);
                        spawn(async move {
                            let state = core();
                            match spawn_process(
                                &state,
                                &SpawnProcessInput {
                                    command: Some(cmd),
                                    project_path: path.clone(),
                                    env: None,
                                },
                            )
                            .await
                            {
                                Ok(info) => {
                                    command.set(String::new());
                                    selected.set(Some(info.id.clone()));
                                    version.with_mut(|value| *value += 1);
                                }
                                Err(err) => store.toast(store.tf("toast.proc_start_failed", &[&err.to_string()])),
                            }
                            busy.set(false);
                        });
                    }
                },
            }
            button {
                class: "primary",
                disabled: *busy.read() || command.read().trim().is_empty(),
                onclick: move |_| {
                    let mut busy = busy;
                    let cmd = command.read().trim().to_string();
                    let path = path_for_spawn_run.clone();
                    busy.set(true);
                    spawn(async move {
                        let state = core();
                        match spawn_process(
                            &state,
                            &SpawnProcessInput {
                                command: Some(cmd),
                                project_path: path.clone(),
                                env: None,
                            },
                        )
                        .await
                        {
                            Ok(info) => {
                                command.set(String::new());
                                selected.set(Some(info.id.clone()));
                                version.with_mut(|value| *value += 1);
                            }
                            Err(err) => store.toast(store.tf("toast.proc_start_failed", &[&err.to_string()])),
                        }
                        busy.set(false);
                    });
                },
                "{store.t(\"term.run\")}"
            }
        }
        if process_list.is_empty() {
            EmptyState { icon: icon_terminal(), title: store.t("term.none"), hint: store.t("term.none_hint").to_string() }
        }
        for process in process_list.iter() {
            {
                let id = process.id.clone();
                let is_selected = selected_id.as_deref() == Some(process.id.as_str());
                let process_status = process.status.clone();
                let kill_id = process.id.clone();
                rsx! {
                    div {
                        key: "{id}",
                        class: if is_selected { "settings-row selected" } else { "settings-row" },
                        role: "button",
                        tabindex: "0",
                        onclick: move |_| selected.set(if is_selected { None } else { Some(id.clone()) }),
                        div {
                            class: "grow",
                            div { class: "mono proc-command", "{process.command}" }
                            span { class: "badge", "{process_status} · {id}" }
                        }
                        button {
                            class: "icon-btn danger",
                            title: store.t("term.stop"),
                            onclick: move |event: Event<MouseData>| {
                                event.stop_propagation();
                                let kill_id = kill_id.clone();
                                spawn(async move {
                                    let state = core();
                                    match locagens_core::api::processes::kill_process(&state, &kill_id).await {
                                        Ok(_) => store.toast(store.t("toast.proc_stopped").to_string()),
                                        Err(err) => store.toast(store.tf("toast.stop_proc_failed", &[&err.to_string()])),
                                    }
                                    version.with_mut(|value| *value += 1);
                                });
                            },
                            {icon_stop()}
                        }
                    }
                }
            }
        }
        if let Some(data) = log_data {
            div {
                class: "panel-card",
                div {
                    class: "panel-section-title",
                    "{store.t(\"term.logs\")} · {data.process.command}"
                }
                div {
                    class: "tool-output log-view",
                    if data.logs.is_empty() {
                        "{store.t(\"term.no_output\")}"
                    }
                    for (index, entry) in data.logs.iter().enumerate() {
                        div {
                            key: "{index}",
                            class: "log-line",
                            span { class: "badge", "{entry.stream}" }
                            span { class: "mono", "{entry.text}" }
                        }
                    }
                }
            }
        }
    }
}
