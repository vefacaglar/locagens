//! Right tool panel (Terminal, Changes review) and the conversation summary
//! card that floats in the chat, all wired in-process to `locagens_core::api`.

use crate::components::icons::*;
use crate::components::start_window_drag;
use crate::markdown::render_markdown;
use crate::state_ui::{core, mode_key, RightPanelTab, UiStore};
use dioxus::prelude::*;
use serde_json::Value;

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
            class: if tab == RightPanelTab::Git { "right-panel-body review-body" } else { "right-panel-body terminal-body" },
            id: if tab == RightPanelTab::Git { "inspector-git" } else { "inspector-terminal" },
            match tab {
                RightPanelTab::Git => rsx! { super::changes_review::ChangesTab {} },
                _ => rsx! { crate::components::terminal::TerminalPanel { project: store.current_project() } },
            }
        }
    }
}

/// Muted icon, one-line title and optional hint for an empty section.
#[component]
pub fn EmptyState(icon: Element, title: String, hint: Option<String>) -> Element {
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
