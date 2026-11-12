//! Right inspector panel: Plan, Agents, Git and Terminal (Processes) tabs,
//! all wired in-process to `locagens_core::api`.

use crate::components::message_thread::DiffView;
use crate::markdown::render_markdown;
use crate::state_ui::{core, UiStore};
use dioxus::prelude::*;
use locagens_core::api::projects::{GitFileDiff, GitStatus};
use locagens_core::api::processes::{list_processes, process_logs, spawn_process, SpawnProcessInput};
use locagens_core::services::processes::ProcessInfo;
use serde_json::Value;
use std::time::Duration;

#[component]
pub fn RightPanel() -> Element {
    let store: UiStore = use_context();
    let tab = *store.right_panel_tab.read();

    rsx! {
        div {
            class: "right-panel-tabs",
            for tab_item in crate::state_ui::RightPanelTab::ALL.iter() {
                button {
                    key: "{tab_item.label()}",
                    class: if *tab_item == tab { "active" } else { "" },
                    onclick: move |_| {
                        let mut store = store;
                        store.right_panel_tab.set(*tab_item);
                    },
                    "{tab_item.label()}"
                }
            }
        }
        div {
            class: "right-panel-body",
            match tab {
                crate::state_ui::RightPanelTab::Plan => rsx! { PlanTab {} },
                crate::state_ui::RightPanelTab::Agents => rsx! { AgentsTab {} },
                crate::state_ui::RightPanelTab::Git => rsx! { GitTab {} },
                crate::state_ui::RightPanelTab::Terminal => rsx! { TerminalTab {} },
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
            div { class: "settings-empty", "No plan for this conversation yet. Switch the mode to Plan and ask for one." }
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
                        span { class: "task-status", "{status}" }
                        span { class: "task-text", "{text}" }
                    }
                }
            }
        }
    }
}

/// Sub-agent summary for the active run.
#[component]
fn AgentsTab() -> Element {
    let store: UiStore = use_context();
    let messages = store.messages.read().clone();
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

    if agents.is_empty() {
        return rsx! {
            div { class: "settings-empty", "No sub-agents ran in this conversation." }
        };
    }
    rsx! {
        for (index, (role, name, count)) in agents.iter().enumerate() {
            div {
                key: "{index}",
                class: "panel-card",
                div {
                    style: "display:flex;align-items:center;gap:8px",
                    span { class: "badge accent", "{role}" }
                    span { style: "flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap", "{name}" }
                    span { class: "badge", "{count} msgs" }
                }
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
    let mut selected_file: Signal<Option<String>> = use_signal(|| None);
    let mut message = use_signal(String::new);
    let busy = use_signal(|| false);
    let mut version = use_signal(|| 0u32);

    // Load status + changed files, then keep them fresh. Bumping `version`
    // restarts the poll loop immediately (used after staging actions).
    use_effect(move || {
        let path = inspector_project_path(&store);
        let mut status = status;
        let mut diffs = diffs;
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
                } else {
                    status.set(None);
                    diffs.set(Vec::new());
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

    rsx! {
        div {
            class: "panel-section-title", "Git"
        }
        match current_status {
            Some(git_status) if git_status.is_git => {
                rsx! {
                    div {
                        class: "panel-card",
                        div { "Branch: " span { class: "mono", "{git_status.branch.clone().unwrap_or_default()}" } }
                        if git_status.has_changes.unwrap_or(false) {
                            div { style: "color:#e6b455", "Working tree has changes." }
                        } else {
                            div { style: "color:#4ecf7a", "Working tree clean." }
                        }
                    }
                    if !changed_files.is_empty() {
                        div {
                            class: "panel-card",
                            div {
                                style: "display:flex;align-items:center;gap:6px",
                                span { class: "panel-section-title", "Changed files" }
                                span { style: "flex:1" }
                                button {
                                    class: "subtle",
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
                                    class: "subtle",
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
                                    let file_kind = diff.kind.clone();
                                    let stage_path = diff.path.clone();
                                    let unstage_path = diff.path.clone();
                                    let stage_project = project_path.clone();
                                    let unstage_project = project_path.clone();
                                    let selected_now = selected.clone();
                                    rsx! {
                                        div {
                                            key: "{file_path}",
                                            class: "settings-row",
                                            div {
                                                class: "grow",
                                                onclick: move |_| selected_file.set(if selected_now.as_ref() == Some(&file_path) { None } else { Some(file_path.clone()) }),
                                                style: "cursor:pointer",
                                                div {
                                                    style: "font-weight:600;overflow:hidden;text-overflow:ellipsis;white-space:nowrap",
                                                    "{file_path}"
                                                }
                                                span { class: "badge", "{file_kind}" }
                                            }
                                            button {
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
                        class: "composer-row-top",
                        input {
                            placeholder: "Commit message…",
                            value: "{message.read()}",
                            oninput: move |event| message.set(event.value()),
                        }
                    }
                    div {
                        style: "display:flex;gap:8px",
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
                div { class: "settings-empty", "The selected folder is not a git repository." }
            },
            None => rsx! {
                div { class: "settings-empty", "Select a project to inspect its git status." }
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
            class: "panel-section-title", "Terminal · Processes"
        }
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
            div { class: "settings-empty", "No background processes yet. Enter a command above to launch one." }
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
                        class: "settings-row",
                        style: if is_selected { "border-color:color-mix(in srgb, #60cdff 45%, transparent)" } else { "" },
                        onclick: move |_| selected.set(if is_selected { None } else { Some(id.clone()) }),
                        div {
                            class: "grow",
                            div {
                                class: "mono",
                                style: "font-size:12px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap",
                                "{process.command}"
                            }
                            span { class: "badge", "{process_status} · {id}" }
                        }
                        button {
                            class: "danger",
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
                            "Stop"
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
                    class: "tool-output",
                    style: "max-height:340px",
                    if data.logs.is_empty() {
                        "No output yet…"
                    }
                    for (index, entry) in data.logs.iter().enumerate() {
                        div {
                            key: "{index}",
                            style: "display:flex;gap:8px",
                            span {
                                class: "badge",
                                style: "font-size:10px;flex:none",
                                "{entry.stream}"
                            }
                            span { class: "mono", style: "font-size:11.5px;white-space:pre-wrap;word-break:break-word", "{entry.text}" }
                        }
                    }
                }
            }
        }
    }
}
