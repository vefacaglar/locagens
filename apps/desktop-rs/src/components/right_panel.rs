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
            // Draggable window titlebar strip (double-click to maximize).
            onmousedown: start_window_drag,
            ondoubleclick: move |_| {
                dioxus::desktop::use_window().toggle_maximized();
            },
            span { class: "head-title", "Inspector" }
        }
        div {
            class: "right-panel-body",
            EnvironmentSection {}
            ContextSection {}
            for tab in RightPanelTab::ALL {
                InspectorSection {
                    key: "{tab.label()}",
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
        RightPanelTab::Plan => ("Plan", icon_list_checks()),
        RightPanelTab::Agents => ("Agents", icon_users()),
        RightPanelTab::Git => ("Changes", icon_git_branch()),
        RightPanelTab::Terminal => ("Terminal", icon_terminal()),
    };
    rsx! {
        section {
            class: "inspector-section",
            button {
                class: "section-head",
                title: "{tab.label()}",
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
        "plan" => "Plan",
        "auto" => "Full Access",
        _ => "Build",
    };
    let branch = branch.read().clone().flatten();
    rsx! {
        section {
            class: "inspector-section",
            div { class: "section-label", "Environment" }
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

/// Context usage of the conversation: average input tokens per model call
/// against the selected model's context window, when the catalog knows it.
#[component]
fn ContextSection() -> Element {
    let store: UiStore = use_context();
    let Some(usage) = store.run_usage.read().clone() else {
        return rsx! {};
    };
    let selected_provider = store.selected_provider.read().clone();
    let selected_model = store.selected_model.read().clone();
    let limit = store
        .providers_catalog
        .read()
        .iter()
        .find(|provider| provider.id == selected_provider)
        .and_then(|provider| provider.model_settings.as_ref())
        .and_then(|settings| settings.get(selected_model.as_str()))
        .and_then(|entry| entry.get("contextLimit"))
        .and_then(Value::as_i64)
        .filter(|limit| *limit > 0);
    let per_call = if usage.total_calls > 0 {
        usage.total_input_tokens / usage.total_calls as i64
    } else {
        0
    };
    let percent = limit.map(|limit| ((per_call * 100) / limit).clamp(0, 100));
    rsx! {
        section {
            class: "inspector-section",
            div { class: "section-label", "Context" }
            div {
                class: "section-body",
                div {
                    class: "info-row",
                    span { class: "grow", "Window used" }
                    if let Some(percent) = percent {
                        span { class: "mono", "{percent}%" }
                    } else {
                        span { class: "mono", "{crate::state_ui::fmt_tokens(per_call)} tok" }
                    }
                }
                if let Some(percent) = percent {
                    div {
                        class: "meter",
                        div { class: "meter-fill", style: "width: {percent}%" }
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
                title: "No plan yet",
                hint: "Switch to Plan mode and ask for one.".to_string(),
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
            "{plan.title} · v{plan.version}"
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
                title: "No sub-agents yet",
                hint: "Agents the model spawns will show up here.".to_string(),
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
                    "Active Turn Diff ({files_changed} files)"
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
                                span { class: "tree-state dirty", "Uncommitted changes" }
                            } else {
                                span { class: "tree-state clean", "Clean" }
                            }
                        }
                    }
                    if !changed_files.is_empty() {
                        div {
                            class: "panel-card",
                            div {
                                class: "panel-row",
                                span { class: "panel-section-title grow", "Changed files" }
                                button {
                                    class: "subtle small",
                                    title: "Restore the project to the pre-turn snapshot (git)",
                                    onclick: move |_| {
                                        let path = path_for_undo.clone();
                                        let run_id = active_run_id.clone();
                                        spawn(async move {
                                            let state = core();
                                            let Some(path) = path else { return };
                                            match locagens_core::api::projects::undo_last_turn(&state, &path, run_id.as_deref()).await {
                                                Ok(snapshot) => {
                                                    store.toast(format!("Restored snapshot {}", snapshot.id));
                                                }
                                                Err(err) => store.toast(format!("Undo failed: {err}")),
                                            }
                                            version.with_mut(|value| *value += 1);
                                        });
                                    },
                                    "Undo"
                                }
                                button {
                                    class: "subtle small",
                                    onclick: move |_| {
                                        let path = path_for_stage_all.clone();
                                        spawn(async move {
                                            let state = core();
                                            let Some(path) = path else { return };
                                            if let Err(err) = locagens_core::api::projects::git_stage(&state, &path, &[]).await {
                                                store.toast(format!("Stage failed: {err}"));
                                            }
                                            version.with_mut(|value| *value += 1);
                                        });
                                    },
                                    "Stage all"
                                }
                                button {
                                    class: "subtle small",
                                    onclick: move |_| {
                                        let path = path_for_unstage_all.clone();
                                        spawn(async move {
                                            let state = core();
                                            let Some(path) = path else { return };
                                            if let Err(err) = locagens_core::api::projects::git_unstage(&state, &path, &[]).await {
                                                store.toast(format!("Unstage failed: {err}"));
                                            }
                                            version.with_mut(|value| *value += 1);
                                        });
                                    },
                                    "Unstage all"
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
                                                title: "{file_kind}",
                                                match file_kind.as_str() {
                                                    "created" => "A",
                                                    "deleted" => "D",
                                                    _ => "M",
                                                }
                                            }
                                            div {
                                                class: "file-main",
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
                                                            store.toast(format!("Stage failed: {err}"));
                                                        }
                                                        version.with_mut(|value| *value += 1);
                                                    });
                                                },
                                                "Stage"
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
                                                            store.toast(format!("Unstage failed: {err}"));
                                                        }
                                                        version.with_mut(|value| *value += 1);
                                                    });
                                                },
                                                "Unstage"
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
                                "Diff · {diff_path}"
                            }
                            DiffView { diff: diff_text }
                        }
                    }
                    div {
                        class: "panel-card",
                        div {
                            class: "panel-section-title",
                            "Worktrees"
                        }
                        div {
                            class: "panel-row",
                            input {
                                placeholder: "Branch…",
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
                                                store.toast(format!("Worktree created ({branch})"));
                                                store.refresh_projects();
                                            }
                                            Err(err) => store.toast(format!("Worktree failed: {err}")),
                                        }
                                        version.with_mut(|value| *value += 1);
                                    });
                                },
                                {icon_plus()} "Worktree"
                            }
                        }
                        if worktree_list.is_empty() {
                            div { class: "empty-hint inline", "No worktrees." }
                        }
                        for worktree in worktree_list.iter() {
                            {
                                let wt_path = worktree.path.clone();
                                let wt_branch = worktree.branch.clone().unwrap_or_else(|| "(detached)".to_string());
                                let switch_path = worktree.path.clone();
                                let remove_path = worktree.path.clone();
                                let remove_project = project_path.clone();
                                rsx! {
                                    div {
                                        key: "{wt_path}",
                                        class: "settings-row",
                                        div {
                                            class: "grow clickable",
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
                                                        store.toast(format!("Switch failed: {err}"));
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
                                                title: "Remove worktree",
                                                onclick: move |_| {
                                                    let project = remove_project.clone();
                                                    let remove_path = remove_path.clone();
                                                    spawn(async move {
                                                        let state = core();
                                                        let Some(project) = project else { return };
                                                        match locagens_core::api::projects::remove_worktree(&state, &project, &remove_path, false).await {
                                                            Ok(()) => {
                                                                store.toast("Worktree removed".to_string());
                                                                store.refresh_projects();
                                                            }
                                                            Err(err) => store.toast(format!("Remove failed: {err}")),
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
                            placeholder: "Commit message…",
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
                                        store.toast(format!("Commit failed: {err}"));
                                    } else {
                                        store.toast("Committed.".to_string());
                                        message.set(String::new());
                                    }
                                    busy.set(false);
                                    version.with_mut(|value| *value += 1);
                                });
                            },
                            "Commit"
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
                                        store.toast(format!("Push failed: {err}"));
                                    } else {
                                        store.toast("Pushed.".to_string());
                                    }
                                    busy.set(false);
                                });
                            },
                            "Push"
                        }
                    }
                }
            }
            Some(_) => rsx! {
                EmptyState { icon: icon_git_branch(), title: "Not a git repository", hint: "The selected folder has no git history.".to_string() }
            },
            None => rsx! {
                EmptyState { icon: icon_git_branch(), title: "No project selected", hint: "Select a project to inspect its changes.".to_string() }
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
                placeholder: "Command to run in the background…",
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
                                Err(err) => store.toast(format!("Failed to start: {err}")),
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
                            Err(err) => store.toast(format!("Failed to start: {err}")),
                        }
                        busy.set(false);
                    });
                },
                "Run"
            }
        }
        if process_list.is_empty() {
            EmptyState { icon: icon_terminal(), title: "No processes", hint: "Run a command above to start one in the background.".to_string() }
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
                        onclick: move |_| selected.set(if is_selected { None } else { Some(id.clone()) }),
                        div {
                            class: "grow",
                            div { class: "mono proc-command", "{process.command}" }
                            span { class: "badge", "{process_status} · {id}" }
                        }
                        button {
                            class: "icon-btn danger",
                            title: "Stop process",
                            onclick: move |event: Event<MouseData>| {
                                event.stop_propagation();
                                let kill_id = kill_id.clone();
                                spawn(async move {
                                    let state = core();
                                    match locagens_core::api::processes::kill_process(&state, &kill_id).await {
                                        Ok(_) => store.toast("Process stopped.".to_string()),
                                        Err(err) => store.toast(format!("Stop failed: {err}")),
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
                    "Logs · {data.process.command}"
                }
                div {
                    class: "tool-output log-view",
                    if data.logs.is_empty() {
                        "No output yet…"
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
