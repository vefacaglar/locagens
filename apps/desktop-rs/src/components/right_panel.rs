//! Right tool panel (Terminal, Changes review) and the conversation summary
//! card that floats in the chat, all wired in-process to `locagens_core::api`.

use crate::components::icons::*;
use crate::components::message_thread::DiffView;
use crate::components::start_window_drag;
use crate::markdown::render_markdown;
use crate::state_ui::{core, mode_key, RightPanelTab, UiStore};
use dioxus::prelude::*;
use locagens_core::api::projects::{GitFileDiff, GitStatus, GitWorktree};
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

/// Tool panel beside the chat: a tab strip in the titlebar row and the
/// active tool below it.
#[component]
pub fn RightPanel() -> Element {
    let store: UiStore = use_context();
    let tab = *store.right_panel_tab.read();
    let tabs = [
        (RightPanelTab::Terminal, store.t("inspector.terminal"), icon_terminal()),
        (RightPanelTab::Git, store.t("inspector.changes"), icon_git_branch()),
    ];
    rsx! {
        div {
            class: "right-panel-head",
            // Draggable window titlebar strip (double-click maximizes, see start_window_drag).
            onmousedown: start_window_drag,
            div {
                class: "panel-tabs",
                role: "tablist",
                for (item, label, icon) in tabs {
                    button {
                        key: "{label}",
                        class: if item == tab { "panel-tab active" } else { "panel-tab" },
                        role: "tab",
                        "aria-selected": "{item == tab}",
                        onmousedown: move |event| event.stop_propagation(),
                        onclick: move |_| {
                            let mut store = store;
                            store.right_panel_tab.set(item);
                        },
                        {icon}
                        span { "{label}" }
                    }
                }
            }
        }
        div {
            class: if tab == RightPanelTab::Git { "right-panel-body" } else { "right-panel-body terminal-body" },
            id: if tab == RightPanelTab::Git { "inspector-git" } else { "inspector-terminal" },
            match tab {
                RightPanelTab::Git => rsx! { GitTab {} },
                _ => rsx! { crate::components::terminal::TerminalPanel { project: store.current_project() } },
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

/// Added and removed lines of a unified diff (file headers excluded).
pub fn diff_line_counts(diff: &str) -> (usize, usize) {
    diff.lines().fold((0, 0), |(added, removed), line| {
        if line.starts_with('+') && !line.starts_with("+++") {
            (added + 1, removed)
        } else if line.starts_with('-') && !line.starts_with("---") {
            (added, removed + 1)
        } else {
            (added, removed)
        }
    })
}

/// Files the main agent read or edited in this conversation, in first-use
/// order (the `path` argument of its file tools).
pub fn source_files(messages: &[locagens_core::db::RunMessage]) -> Vec<String> {
    const FILE_TOOLS: &[&str] = &["read_file", "write_file", "edit_file", "view_image"];
    let mut files: Vec<String> = Vec::new();
    for message in messages.iter().filter(|m| m.role == "assistant" && m.agent_role.is_none()) {
        let calls = message
            .raw_response
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Vec<Value>>(raw).ok())
            .unwrap_or_default();
        for call in calls {
            let name = call.pointer("/function/name").and_then(Value::as_str).unwrap_or("");
            if !FILE_TOOLS.contains(&name) {
                continue;
            }
            let path = call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .and_then(|args| serde_json::from_str::<Value>(args).ok())
                .and_then(|args| args.get("path").and_then(Value::as_str).map(str::to_string));
            if let Some(path) = path.filter(|path| !path.is_empty() && !files.contains(path)) {
                files.push(path);
            }
        }
    }
    files
}

/// Role and derived state of every sub-agent of the conversation.
fn agent_states(store: &UiStore) -> Vec<(String, &'static str)> {
    let messages = store.messages.read();
    let run_live = super::run_active(store.active_run.read().as_ref());
    let mut agents: Vec<(String, String)> = Vec::new();
    for agent in store.spawned_agents.read().iter() {
        if let Some(name) = agent.get("name").and_then(Value::as_str) {
            let role = agent.get("role").and_then(Value::as_str).unwrap_or("explorer");
            agents.push((name.to_string(), role.to_string()));
        }
    }
    for message in messages.iter() {
        if let Some(role) = message.agent_role.clone() {
            let name = message.agent_name.clone().unwrap_or_else(|| role.clone());
            if !agents.iter().any(|(known, _)| *known == name) {
                agents.push((name, role));
            }
        }
    }
    agents.into_iter().map(|(name, role)| (role, agent_status(&messages, &name, run_live))).collect()
}

const SOURCES_PREVIEW: usize = 4;

/// Codex-style summary card floating at the top right of the chat: project,
/// branch and mode; changed lines (opens the review tab); sub-agents and
/// plan (expand in place); context fill; files used. In a narrow chat only
/// the head shows until it is clicked.
#[component]
pub fn SummaryCard() -> Element {
    let store: UiStore = use_context();
    let mut show_all_sources = use_signal(|| false);
    let mut expanded = use_signal(|| None::<RightPanelTab>);
    let mut card_open = use_signal(|| false);
    let mut toggle = move |tab: RightPanelTab| {
        let next = if *expanded.peek() == Some(tab) { None } else { Some(tab) };
        expanded.set(next);
    };
    let is_open = |tab: RightPanelTab| *expanded.read() == Some(tab);
    let plan_progress = store.plan.read().as_ref().and_then(|plan| {
        let tasks = plan.tasks.as_array()?;
        let done = tasks.iter().filter(|task| task.get("status").and_then(Value::as_str) == Some("completed")).count();
        (!tasks.is_empty()).then(|| format!("{done}/{}", tasks.len()))
    });
    let has_plan = store.plan.read().is_some();
    let project_path = inspector_project_path(&store);
    let branch = use_resource(move || async move {
        let path = inspector_project_path(&store)?;
        locagens_core::api::projects::git_status(&core(), &path)
            .await
            .ok()
            .filter(|status| status.is_git)
            .and_then(|status| status.branch)
    });
    // After every hook: a brand-new chat with no project shows no card.
    if project_path.is_none() && store.active_run_id.read().is_none() {
        return rsx! {};
    }
    let branch = branch.read().clone().flatten();
    let folder = project_path
        .as_deref()
        .and_then(|path| std::path::Path::new(path).file_name())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| store.t("inspector.no_project").to_string());
    let selected_mode = store.selected_mode.read().clone();
    let mode = store.t(mode_key(&selected_mode));
    let mode_class = if selected_mode == "auto" { "summary-mode warn" } else { "summary-mode" };

    let (added, removed) = store.turn_diff.read().as_ref().map(|(diff, _)| diff_line_counts(diff)).unwrap_or_default();
    let agents = agent_states(&store);
    let working = agents.iter().filter(|(_, status)| *status != "agents.finished").count();
    let finished = agents.len() - working;
    let agent_text = match (working, finished) {
        (0, 0) => store.t("inspector.none").to_string(),
        (0, done) => store.tf("inspector.n_finished", &[&done.to_string()]),
        (busy, _) => store.tf("inspector.n_working", &[&busy.to_string()]),
    };
    let context = store
        .active_run_id
        .read()
        .is_some()
        .then(|| store.context_usage.read().clone())
        .flatten()
        .filter(|context| context.used_tokens > 0);
    let sources = source_files(&store.messages.read());
    let shown: Vec<String> = if *show_all_sources.read() { sources.clone() } else { sources.iter().take(SOURCES_PREVIEW).cloned().collect() };

    rsx! {
        div {
            class: if *card_open.read() { "summary-card summary-float open" } else { "summary-card summary-float" },
            button {
                class: "summary-head",
                title: "{project_path.clone().unwrap_or_default()}",
                "aria-expanded": "{card_open.read()}",
                onclick: move |_| {
                    let next = !*card_open.read();
                    card_open.set(next);
                },
                span { class: "summary-project", "{folder}" }
                if let Some(branch) = branch {
                    span { class: "summary-branch", {icon_git_branch()} "{branch}" }
                } else {
                    span { class: "summary-branch" }
                }
                span { class: "{mode_class}", "{mode}" }
            }
            div {
                class: "summary-body",
                button {
                    class: "summary-row",
                    onclick: move |_| crate::components::changes::open_changes(store),
                    span { class: "summary-label", "{store.t(\"inspector.changes\")}" }
                    if added + removed == 0 {
                        span { class: "summary-muted", "{store.t(\"inspector.none\")}" }
                    } else {
                        span { class: "summary-add", "+{added}" }
                        span { class: "summary-del", "−{removed}" }
                    }
                    span { class: "chev", {icon_chevron_right()} }
                }
            button {
                class: "summary-row",
                "aria-expanded": "{is_open(RightPanelTab::Agents)}",
                onclick: move |_| toggle(RightPanelTab::Agents),
                span { class: "summary-label", "{store.t(\"inspector.subagents\")}" }
                span {
                    class: "summary-agents",
                    for (index, (role, status)) in agents.iter().take(6).enumerate() {
                        span {
                            key: "{index}",
                            class: if *status == "agents.finished" { "agent-dot" } else { "agent-dot live" },
                            title: "{role_label(&store, role)} · {store.t(status)}",
                            if role == "worker" { {icon_wrench()} } else { {icon_search()} }
                        }
                    }
                }
                span { class: "summary-muted", "{agent_text}" }
                span { class: if is_open(RightPanelTab::Agents) { "chev open" } else { "chev" }, {icon_chevron_right()} }
            }
            if is_open(RightPanelTab::Agents) {
                div { class: "summary-detail", AgentsTab {} }
            }
            button {
                class: "summary-row",
                "aria-expanded": "{is_open(RightPanelTab::Plan)}",
                onclick: move |_| toggle(RightPanelTab::Plan),
                span { class: "summary-label", "{store.t(\"inspector.plan\")}" }
                match (plan_progress.clone(), has_plan) {
                    (Some(progress), _) => rsx! { span { class: "summary-muted", "{progress}" } },
                    (None, true) => rsx! {},
                    (None, false) => rsx! { span { class: "summary-muted", "{store.t(\"inspector.none\")}" } },
                }
                span { class: if is_open(RightPanelTab::Plan) { "chev open" } else { "chev" }, {icon_chevron_right()} }
            }
            if is_open(RightPanelTab::Plan) {
                div { class: "summary-detail", PlanTab {} }
            }
            if let Some(context) = context {
                div {
                    class: "summary-row static",
                    span { class: "summary-label", "{store.t(\"inspector.context\")}" }
                    if let Some(percent) = context.percent {
                        div {
                            class: "meter summary-meter",
                            role: "progressbar",
                            "aria-valuemin": "0",
                            "aria-valuemax": "100",
                            "aria-valuenow": "{percent}",
                            "aria-label": "{store.t(\"inspector.window_used\")}",
                            div {
                                class: if percent >= 90 { "meter-fill danger" } else if percent >= 75 { "meter-fill warn" } else { "meter-fill" },
                                style: "width: {percent}%",
                            }
                        }
                    }
                    span {
                        class: "summary-muted",
                        title: "{store.t(\"inspector.window_used\")}",
                        match context.context_window {
                            Some(window) => format!("{} / {}", crate::state_ui::fmt_tokens(context.used_tokens), crate::state_ui::fmt_tokens(window)),
                            None => crate::state_ui::fmt_tokens(context.used_tokens),
                        }
                    }
                }
            }
            div {
                class: "summary-sources",
                div {
                    class: "summary-row static",
                    span { class: "summary-label", "{store.t(\"inspector.sources\")}" }
                    if !sources.is_empty() {
                        span { class: "summary-muted", "{sources.len()}" }
                    }
                }
                if sources.is_empty() {
                    div { class: "summary-hint", "{store.t(\"inspector.no_sources\")}" }
                }
                for path in shown.iter() {
                    div {
                        key: "{path}",
                        class: "source-row",
                        title: "{path}",
                        span { class: "source-name", "{source_name(path)}" }
                        span { class: "source-dir", "{source_dir(path)}" }
                    }
                }
                if sources.len() > SOURCES_PREVIEW {
                    button {
                        class: "summary-more",
                        onclick: move |_| {
                            let next = !*show_all_sources.read();
                            show_all_sources.set(next);
                        },
                        if *show_all_sources.read() { "{store.t(\"inspector.show_less\")}" } else { "{store.t(\"inspector.show_all\")}" }
                    }
                }
            }
            }
        }
    }
}

/// File name of a workspace path.
fn source_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Parent folder of a workspace path ("" at the root).
fn source_dir(path: &str) -> &str {
    let name = source_name(path);
    path[..path.len() - name.len()].trim_end_matches(['/', '\\'])
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
    let run_live = super::run_active(store.active_run.read().as_ref());
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
                // FR-058: no event reports an agent's end, so it is derived.
                let status = agent_status(&messages, &name, run_live);
                rsx! {
                    div {
                        key: "spawned-{index}-{agent_id}",
                        class: "list-row",
                        if role == "worker" { {icon_wrench()} } else { {icon_search()} }
                        span { class: "grow", "{name}" }
                        span { class: "badge", "{role_label(&store, &role)}" }
                        span { class: "badge", "{store.t(status)}" }
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
                span { class: "badge", "{role_label(&store, role)}" }
                span { class: "badge", "{store.t(agent_status(&messages, name, run_live))}" }
                span { class: "count-chip", "{count}" }
            }
        }
    }
}

/// Translated built-in role, or a custom role's own name.
fn role_label(store: &UiStore, role: &str) -> String {
    match role {
        "explorer" | "worker" => store.t(crate::i18n::role_key(role)).to_string(),
        other => other.to_string(),
    }
}

/// i18n key of an agent's state, derived from its messages: working while
/// the run is live and its newest message is not a finished answer, else
/// idle (live run) or finished.
pub fn agent_status(messages: &[locagens_core::db::RunMessage], name: &str, run_live: bool) -> &'static str {
    if !run_live {
        return "agents.finished";
    }
    let last = messages
        .iter()
        .rev()
        .find(|message| message.agent_name.as_deref() == Some(name) || (message.agent_name.is_none() && message.agent_role.as_deref() == Some(name)));
    match last {
        Some(message) if message.role == "assistant" && !message.content.is_empty() && message.raw_response.as_deref().is_none_or(|raw| raw == "[]") => "agents.idle",
        _ => "agents.working",
    }
}

/// The project folder the inspector acts on (FR-050: the open chat's).
fn inspector_project_path(store: &UiStore) -> Option<String> {
    store.current_project()
}

/// The section of file `path` in a multi-file `git diff` (from its
/// `diff --git` header to the next one), capped at `max_lines` lines with a
/// trailing count of what was cut.
pub fn file_diff_section(diff: &str, path: &str, max_lines: usize) -> Option<String> {
    let header = format!(" b/{path}");
    let mut lines = diff.lines();
    let mut section: Vec<&str> = Vec::new();
    let mut inside = false;
    for line in lines.by_ref() {
        if line.starts_with("diff --git ") {
            if inside {
                break;
            }
            inside = line.ends_with(&header);
            continue;
        }
        if inside {
            section.push(line);
        }
    }
    if section.is_empty() {
        return None;
    }
    let total = section.len();
    let mut text = section.into_iter().take(max_lines).collect::<Vec<_>>().join("\n");
    if total > max_lines {
        text.push_str(&format!("\n… +{}", total - max_lines));
    }
    Some(text)
}

/// Lines shown of one file's diff in the Git tab.
const DIFF_LINE_CAP: usize = 600;

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
    // restarts the poll loop immediately (used after staging actions); a
    // project switch re-runs the effect and the old loop ends (FR-049).
    use_effect(move || {
        let path = inspector_project_path(&store);
        let mut status = status;
        let mut diffs = diffs;
        let mut worktrees = worktrees;
        let current_version = *version.read();
        spawn(async move {
            let state = core();
            loop {
                if *version.peek() != current_version || store.current_project_peek() != path {
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
                    // A switch that landed during the calls above must not
                    // leave the old project's data on screen.
                    if store.current_project_peek().as_deref() != Some(path) {
                        break;
                    }
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
    // FR-059: the selected file's real `git diff` hunks (unstaged, else
    // staged); a new untracked file shows its content. Capped in length.
    let git_text = use_resource(move || {
        let file = selected_file.read().clone();
        let path = inspector_project_path(&store);
        let _ = *version.read();
        async move {
            let (Some(file), Some(path)) = (file, path) else { return None };
            let state = core();
            let unstaged = locagens_core::api::projects::git_diff(&state, &path, false).await.unwrap_or_default();
            if let Some(section) = file_diff_section(&unstaged, &file, DIFF_LINE_CAP) {
                return Some(section);
            }
            let staged = locagens_core::api::projects::git_diff(&state, &path, true).await.unwrap_or_default();
            file_diff_section(&staged, &file, DIFF_LINE_CAP)
        }
    });
    let selected_diff: Option<(String, String)> = selected
        .as_ref()
        .and_then(|path| changed_files.iter().find(|diff| &diff.path == path))
        .map(|diff| {
            let text = git_text.read().clone().flatten().unwrap_or_else(|| {
                let capped = |text: &str| text.lines().take(DIFF_LINE_CAP).collect::<Vec<_>>().join("\n");
                match diff.kind.as_str() {
                    "created" => crate::markdown::pseudo_diff(None, Some(&capped(&diff.new_text))),
                    "deleted" => crate::markdown::pseudo_diff(Some(&capped(&diff.old_text)), None),
                    _ => String::new(),
                }
            });
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
                                                    store.toast(super::changes::undo_summary(&store, &snapshot));
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
                                                    span { class: "badge", "{store.t(\"git.main\")}" }
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

#[cfg(test)]
mod tests {
    use super::*;
    use locagens_core::db::RunMessage;

    #[test]
    fn diff_counts_skip_file_headers() {
        let diff = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-old\n+new\n+more\n ctx\n";
        assert_eq!(diff_line_counts(diff), (2, 1));
    }

    #[test]
    fn sources_are_unique_main_agent_file_paths() {
        let call = |name: &str, path: &str| serde_json::json!({ "function": { "name": name, "arguments": serde_json::json!({ "path": path }).to_string() } });
        let main = RunMessage {
            role: "assistant".into(),
            raw_response: Some(serde_json::json!([call("read_file", "src/a.rs"), call("run_command", "x"), call("edit_file", "src/a.rs"), call("write_file", "b.md")]).to_string()),
            ..Default::default()
        };
        let child = RunMessage { agent_role: Some("explorer".into()), ..main.clone() };
        assert_eq!(source_files(&[main, child]), ["src/a.rs", "b.md"]);
        assert_eq!((source_name("src/a.rs"), source_dir("src/a.rs")), ("a.rs", "src"));
        assert_eq!(source_dir("b.md"), "");
    }
}
