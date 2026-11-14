//! Message thread: renders user messages, assistant Markdown answers with
//! collapsible reasoning, tool-call rows and sub-agent groups, plus the end
//! of the thread: the live status row, a failed run's error block and the
//! "what changed" card.

use super::icons::*;
use super::*;
use crate::i18n::{self, Lang};
use crate::markdown::{parse_diff, summarize_args, DiffLine};
use crate::state_ui::{AppPage, SettingsTab, UiStore};
use dioxus::prelude::*;
use locagens_core::db::RunMessage;
use locagens_core::tool_labels::{describe_tool, tool_outcome, ToolOutcome, ToolVerb};
use serde_json::Value;

/// One serialized tool call of an assistant message plus its paired tool
/// response (matched by call id).
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCallView {
    pub id: String,
    pub name: String,
    pub arguments: Value,
    pub response: Option<RunMessage>,
}

impl ToolCallView {
    fn outcome(&self) -> ToolOutcome {
        tool_outcome(self.response.as_ref().map(|response| response.content.as_str()))
    }
}

/// Rendered groups of messages.
#[derive(Clone, Debug, PartialEq)]
pub enum Group {
    User(RunMessage),
    Assistant { message: RunMessage, tool_calls: Vec<ToolCallView> },
    SubAgent { role: String, name: String, groups: Vec<Group> },
}

impl Group {
    /// Stable key of the rendered row: the message id (a sub-agent group is
    /// keyed by its first message), so only a changed message re-renders.
    fn key(&self) -> String {
        match self {
            Group::User(message) | Group::Assistant { message, .. } => message.id.clone(),
            Group::SubAgent { role, name, groups } => {
                format!("{role}/{name}/{}", groups.first().map(Group::key).unwrap_or_default())
            }
        }
    }
}

/// Tool calls serialized in an assistant message's `raw_response`.
fn message_tool_calls(message: &RunMessage) -> Vec<Value> {
    message
        .raw_response
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Vec<Value>>(raw).ok())
        .unwrap_or_default()
}

/// Groups the persisted message list into renderable units.
pub fn build_groups(messages: &[RunMessage]) -> Vec<Group> {
    // Map tool responses by call id.
    let mut tool_responses: std::collections::HashMap<String, RunMessage> = std::collections::HashMap::new();
    for message in messages.iter().filter(|message| message.role == "tool") {
        if let Some(call_id) = message.raw_response.as_deref() {
            tool_responses.insert(call_id.to_string(), message.clone());
        }
    }

    let mut groups: Vec<Group> = Vec::new();
    let mut index = 0;
    while index < messages.len() {
        let message = &messages[index];
        if message.role == "tool" {
            index += 1;
            continue;
        }
        if let Some(role) = message.agent_role.clone() {
            // Collect the sub-agent's consecutive messages.
            let name = message.agent_name.clone().unwrap_or_else(|| role.clone());
            let start = index;
            while index < messages.len() {
                let candidate = &messages[index];
                if candidate.agent_role.as_deref() == Some(role.as_str())
                    && candidate.agent_name.clone().unwrap_or_else(|| role.clone()) == name
                {
                    index += 1;
                } else {
                    break;
                }
            }
            // Grouped as plain messages: they all carry the agent role, and
            // grouping them with it would recurse forever.
            let inner: Vec<RunMessage> = messages[start..index]
                .iter()
                .map(|message| RunMessage { agent_role: None, agent_name: None, ..message.clone() })
                .collect();
            let groups_inner = build_groups(&inner);
            groups.push(Group::SubAgent { role, name, groups: groups_inner });
            continue;
        }
        match message.role.as_str() {
            "user" => {
                groups.push(Group::User(message.clone()));
                index += 1;
            }
            "assistant" => {
                let tool_calls: Vec<ToolCallView> = message_tool_calls(message)
                    .into_iter()
                    .filter_map(|call| {
                        let id = call.get("id").and_then(Value::as_str)?.to_string();
                        let response = tool_responses.get(&id).cloned();
                        let name = call.pointer("/function/name").and_then(Value::as_str).unwrap_or("tool").to_string();
                        let arguments = call
                            .pointer("/function/arguments")
                            .and_then(Value::as_str)
                            .and_then(|arguments| serde_json::from_str(arguments).ok())
                            .unwrap_or(Value::Null);
                        Some(ToolCallView { id, name, arguments, response })
                    })
                    .collect();
                groups.push(Group::Assistant { message: message.clone(), tool_calls });
                index += 1;
            }
            _ => index += 1,
        }
    }
    groups
}

/// Whether each top-level group shows the model label (UX-025): only the
/// first assistant message of the chat, or one whose model differs from the
/// previous assistant message (a model switch mid-chat stays visible).
pub fn model_label_flags(groups: &[Group]) -> Vec<bool> {
    let mut last_model: Option<Option<String>> = None;
    groups
        .iter()
        .map(|group| match group {
            Group::User(_) => false,
            Group::Assistant { message, .. } => {
                let show = last_model.as_ref() != Some(&message.model);
                last_model = Some(message.model.clone());
                show
            }
            Group::SubAgent { .. } => false,
        })
        .collect()
}

/// The newest main-agent tool call that has no result yet: `(id, name, args)`.
pub fn running_tool(messages: &[RunMessage]) -> Option<(String, String, Value)> {
    let answered: std::collections::HashSet<&str> =
        messages.iter().filter(|m| m.role == "tool").filter_map(|m| m.raw_response.as_deref()).collect();
    messages.iter().rev().filter(|m| m.role == "assistant").find_map(|message| {
        message_tool_calls(message).into_iter().rev().find_map(|call| {
            let id = call.get("id").and_then(Value::as_str)?.to_string();
            if answered.contains(id.as_str()) {
                return None;
            }
            let name = call.pointer("/function/name").and_then(Value::as_str).unwrap_or("tool").to_string();
            let args = call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .and_then(|arguments| serde_json::from_str(arguments).ok())
                .unwrap_or(Value::Null);
            Some((id, name, args))
        })
    })
}

/// Translated tool verb (`Ran`, `Edited` …); unknown tools keep their name.
pub fn verb_label(verb: &ToolVerb, lang: Lang) -> String {
    match verb.key() {
        Some(key) => i18n::t(lang, verb_key(key)).to_string(),
        None => verb.english(),
    }
}

fn verb_key(key: &str) -> &'static str {
    match key {
        "ran" => "verb.ran",
        "sent_input" => "verb.sent_input",
        "read" => "verb.read",
        "listed" => "verb.listed",
        "searched" => "verb.searched",
        "wrote" => "verb.wrote",
        "edited" => "verb.edited",
        "applied_patch" => "verb.applied_patch",
        "deleted" => "verb.deleted",
        "created_dir" => "verb.created_dir",
        "moved" => "verb.moved",
        "fetched" => "verb.fetched",
        "spawned_agents" => "verb.spawned_agents",
        "spawned_agent" => "verb.spawned_agent",
        "waited_agents" => "verb.waited_agents",
        "loaded_skill" => "verb.loaded_skill",
        "asked" => "verb.asked",
        "titled_chat" => "verb.titled_chat",
        "updated_plan" => "verb.updated_plan",
        _ => "verb.remembered",
    }
}

/// Whether an error message is about credentials (offers "Models & accounts").
pub fn is_auth_error(error: &str) -> bool {
    let lower = error.to_lowercase();
    ["401", "403", "unauthorized", "not authenticated", "api key", "authentication", "invalid_api_key", "forbidden"]
        .iter()
        .any(|needle| lower.contains(needle))
}

#[component]
pub fn MessageThread() -> Element {
    let store: UiStore = use_context();
    // The memo re-runs per message update but only wakes this component when
    // the grouped result differs; each row below is a memoized component, so
    // a streaming delta re-renders just the streaming message (FR-063).
    let thread = use_memo(move || {
        let groups = build_groups(&store.messages.read());
        let labels = model_label_flags(&groups);
        (groups, labels)
    });
    let (groups, labels) = thread.read().clone();
    let is_empty = groups.is_empty();
    let run = store.active_run.read().clone();
    let has_active = run_active(run.as_ref());
    let streaming_review = run
        .as_ref()
        .is_some_and(|run| has_active && locagens_core::orchestrator::review::is_review_run(run));
    let failure = run
        .as_ref()
        .filter(|run| run.status == "failed")
        .map(|run| run.error_message.clone().unwrap_or_default());
    let stopped = run.as_ref().is_some_and(|run| run.status == "cancelled") && !groups.is_empty();
    let changes = store
        .turn_diff
        .read()
        .clone()
        .filter(|_| !has_active)
        .zip(run.as_ref().map(|run| run.id.clone()));

    rsx! {
        div {
            id: "chat-scroll",
            if is_empty {
                EmptyThread {}
            }
            for (group, show_model) in groups.iter().zip(labels) {
                GroupRow { key: "{group.key()}", group: group.clone(), depth: 0, show_model, streaming_review }
            }
            if let Some(error) = failure {
                FailureBlock { error }
            }
            if stopped {
                div { class: "msg-meta stopped-note", role: "status", "{store.t(\"thread.stopped\")}" }
            }
            if let Some(((diff, _), run_id)) = changes {
                super::changes::ChangesCard { key: "{run_id}", diff, run_id: run_id.clone() }
            }
            if has_active && !groups.is_empty() {
                StatusRow {}
            }
            // Sized by `window.lcgnChat` so a just-sent message can sit at the top.
            div { id: "chat-tail-spacer", class: "chat-tail-spacer" }
        }
    }
}

/// One message row. Props compare equal for an unchanged message, so Dioxus
/// skips re-rendering it.
#[component]
fn GroupRow(group: Group, depth: u8, show_model: bool, streaming_review: bool) -> Element {
    match &group {
        Group::User(message) => rsx! {
            div {
                class: "msg-user",
                key: "{message.id}",
                "data-mid": "{message.id}",
                if !message.images.is_empty() {
                    div {
                        class: "msg-images",
                        for (position, image) in message.images.iter().enumerate() {
                            div {
                                key: "{position}",
                                class: "attach-thumb",
                                img { src: "data:{image.mime};base64,{image.data}", alt: "" }
                            }
                        }
                    }
                }
                if !message.content.is_empty() || message.images.is_empty() {
                    div { class: "bubble", "{message.content}" }
                }
                if !message.content.is_empty() {
                    MessageFooter { text: message.content.clone(), created_at: message.created_at.clone(), reuse: depth == 0 }
                }
            }
        },
        Group::Assistant { message, tool_calls } => rsx! {
            div {
                class: "msg-assistant",
                key: "{message.id}",
                if show_model {
                    div {
                        class: "msg-meta",
                        span { class: "model-label", "{message.model.clone().unwrap_or_default()}" }
                    }
                }
                if !message.reasoning_content.clone().unwrap_or_default().is_empty() {
                    ReasoningPanel { reasoning: message.reasoning_content.clone().unwrap_or_default() }
                }
                super::review::AssistantBody { message: message.clone(), streaming_review }
                if !tool_calls.is_empty() {
                    ToolActivity { calls: tool_calls.clone() }
                }
                if !message.content.is_empty() && depth == 0 {
                    MessageFooter { text: message.content.clone(), created_at: message.created_at.clone(), reuse: false }
                }
            }
        },
        Group::SubAgent { role, name, groups } => rsx! {
            SubAgentCard { role: role.clone(), name: name.clone(), groups: groups.clone(), depth }
        },
    }
}

/// What the empty thread offers: connect a provider (UX-001), open a folder
/// (UX-004), or contextual starters.
#[component]
fn EmptyThread() -> Element {
    let mut store: UiStore = use_context();
    let no_provider = *store.providers_loaded.read() && store.visible_catalog().is_empty();
    let no_folder = store.projects.read().is_empty() && store.current_project().is_none();
    if no_provider {
        return rsx! {
            div {
                class: "msg-empty",
                {logo_mark()}
                div { class: "big", "{store.t(\"empty.connect_title\")}" }
                div { "{store.t(\"empty.connect_body\")}" }
                button {
                    class: "primary empty-cta",
                    onclick: move |_| store.page.set(AppPage::Settings(SettingsTab::Providers)),
                    "{store.t(\"empty.connect_button\")}"
                }
                div { class: "empty-hint", "{store.t(\"empty.connect_hint\")}" }
            }
        };
    }
    if no_folder {
        return rsx! {
            div {
                class: "msg-empty",
                {logo_mark()}
                div { class: "big", "{store.t(\"empty.folder_title\")}" }
                div { "{store.t(\"empty.folder_body\")}" }
                button {
                    class: "primary empty-cta",
                    onclick: move |_| super::sidebar::open_folder(store),
                    "{store.t(\"empty.folder_button\")}"
                }
            }
        };
    }
    let project = store.current_project().unwrap_or_default();
    let name = std::path::Path::new(&project).file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
    let title = if name.is_empty() { store.t("empty.title").to_string() } else { store.tf("empty.title_in", &[&name]) };
    rsx! {
        div {
            class: "msg-empty",
            {logo_mark()}
            div { class: "big", "{title}" }
            div { "{store.t(\"empty.subtitle\")}" }
            Starters { key: "{project}", project: project.clone() }
        }
    }
}

/// Starter cards for `project`: "Review my uncommitted changes" (runs
/// `/review`) first when the tree is dirty, then up to three recent chats of
/// the project as "Continue: <title> · 2h" (UX-004).
#[component]
fn Starters(project: String) -> Element {
    let mut store: UiStore = use_context();
    let path = project.clone();
    // The path is a reactive dependency, so another project re-checks.
    let dirty = use_resource(use_reactive!(|(path,)| {
        async move {
            locagens_core::api::projects::git_status(&crate::state_ui::core(), &path)
                .await
                .ok()
                .is_some_and(|status| status.is_git && status.has_changes.unwrap_or(false))
        }
    }));
    let dirty = dirty.read().unwrap_or(false);
    let t = |key| store.t(key);
    let mut starters: Vec<(&'static str, &'static str)> = vec![
        (t("starter.explore.title"), t("starter.explore.prompt")),
        (t("starter.tests.title"), t("starter.tests.prompt")),
        (t("starter.plan.title"), t("starter.plan.prompt")),
    ];
    if !dirty {
        starters.insert(1, (t("starter.review.title"), t("starter.review.prompt")));
    }
    let recent: Vec<(String, String, String)> = store
        .runs
        .read()
        .iter()
        .filter(|run| run.project_path == project)
        .take(3)
        .map(|run| {
            let age_source = if run.updated_at.is_empty() { run.created_at.clone() } else { run.updated_at.clone() };
            (run.id.clone(), run.title.clone(), super::sidebar::relative_age(&age_source, store.lang()))
        })
        .collect();
    rsx! {
        div {
            class: "starter-grid",
            if dirty {
                button {
                    class: "starter-card",
                    onclick: move |_| {
                        super::composer::run_command_line(store, "/review");
                    },
                    "{store.t(\"starter.review_changes\")}"
                }
            }
            for (index, (title, prompt)) in starters.into_iter().enumerate() {
                button {
                    key: "starter-{index}",
                    class: "starter-card",
                    onclick: move |_| {
                        store.prompt.set(prompt.to_string());
                        crate::state_ui::focus_composer();
                    },
                    "{title}"
                }
            }
        }
        if !recent.is_empty() {
            div {
                class: "recent-chats",
                for (run_id, title, age) in recent {
                    button {
                        key: "{run_id}",
                        class: "recent-chat",
                        onclick: move |_| store.select_run(run_id.clone()),
                        "{store.tf(\"empty.continue\", &[&title, &age])}"
                    }
                }
            }
        }
    }
}

/// One live line under the thread while a run works (UX-010): what it is
/// doing, for how long, and how to stop it. Waiting states do not pulse.
#[component]
fn StatusRow() -> Element {
    let store: UiStore = use_context();
    let mut since: Signal<Option<(String, std::time::Instant)>> = use_signal(|| None);
    let mut tick = use_signal(|| 0u64);
    use_future(move || async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            tick += 1;
        }
    });
    let _ = tick.read();
    let lang = store.lang();
    let status = store.active_run.read().as_ref().map(|run| run.status.clone()).unwrap_or_default();
    let running = running_tool(&store.messages.read());
    let progress = store.patch_progress.read().clone();

    // Elapsed time of the current tool call (reset when the call changes).
    let key = running.as_ref().map(|(id, _, _)| id.clone()).unwrap_or_default();
    if since.peek().as_ref().map(|(id, _)| id.as_str()) != Some(key.as_str()) {
        since.set(Some((key.clone(), std::time::Instant::now())));
    }
    let elapsed = since.peek().as_ref().map(|(_, at)| at.elapsed().as_secs()).unwrap_or(0);
    let elapsed_label = format_elapsed(elapsed);

    let (dot, text) = match status.as_str() {
        "awaiting_permission" => ("awaiting_permission", store.t("status_row.approval").to_string()),
        "awaiting_input" => ("awaiting_input", store.t("status_row.answer").to_string()),
        _ => {
            let text = if let Some(progress) = progress.as_ref() {
                let files = patch_progress_files(progress, lang).join(", ");
                store.tf("status_row.writing", &[&files])
            } else if let Some((_, name, args)) = running.as_ref() {
                let (verb, target) = describe_tool(name, args);
                let label = if verb == ToolVerb::Ran {
                    store.tf("status_row.running_command", &[&target])
                } else {
                    format!("{} {target}", verb_label(&verb, lang)).trim().to_string()
                };
                format!("{label} · {elapsed_label}")
            } else {
                store.t("status_row.thinking").to_string()
            };
            ("generating", format!("{text} · {}", store.t("status_row.esc")))
        }
    };
    rsx! {
        div {
            class: "msg-meta status-row",
            role: "status",
            "aria-live": "polite",
            {status_dot(dot)}
            span { class: "status-text", "{text}" }
        }
    }
}

/// `45s`, `1m 12s`.
pub fn format_elapsed(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    }
}

/// A failed run's reason, kept at the end of the thread (UX-008): rendered
/// from the run's persisted `error_message`, so reopening the chat shows it.
#[component]
fn FailureBlock(error: String) -> Element {
    let mut store: UiStore = use_context();
    let mut copied = use_signal(|| false);
    let auth = is_auth_error(&error);
    let text = if error.trim().is_empty() { store.t("failure.no_reason").to_string() } else { error.clone() };
    rsx! {
        div {
            class: "failure-block",
            role: "alert",
            div { class: "failure-title", {icon_x()} "{store.t(\"failure.title\")}" }
            pre { class: "failure-text", "{text}" }
            div {
                class: "changes-actions",
                button {
                    onclick: move |_| {
                        let error = error.clone();
                        spawn(async move {
                            if crate::clipboard::copy_text(&error).await {
                                copied.set(true);
                                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                                copied.set(false);
                            } else {
                                store.toast(store.t("toast.copy_failed").to_string());
                            }
                        });
                    },
                    if *copied.read() { "{store.t(\"failure.copied\")}" } else { "{store.t(\"failure.copy\")}" }
                }
                if auth {
                    button {
                        onclick: move |_| store.page.set(AppPage::Settings(SettingsTab::Providers)),
                        "{store.t(\"failure.open_accounts\")}"
                    }
                }
            }
            div { class: "failure-hint", "{store.t(\"failure.hint\")}" }
        }
    }
}

/// Collapsible reasoning panel shown above the assistant answer.
#[component]
fn ReasoningPanel(reasoning: String) -> Element {
    let store: UiStore = use_context();
    let open = use_signal(|| false);
    rsx! {
        div {
            class: "collapsible reasoning-panel",
            button {
                class: "collapsible-header",
                "aria-expanded": "{open.read()}",
                onclick: move |_| {
                    let mut open = open;
                    let is_open = *open.read();
                    open.set(!is_open);
                },
                {chevron(*open.read())}
                span { "{store.tf(\"thread.thought\", &[&reasoning.split_whitespace().count().to_string()])}" }
            }
            if *open.read() {
                div {
                    class: "collapsible-body",
                    "{reasoning}"
                }
            }
        }
    }
}

/// Footer under a message: relative age, Copy, and for user messages
/// `Reuse` (puts the text back into the composer, UX-025). Quiet until the
/// message is hovered or focused.
#[component]
fn MessageFooter(text: String, created_at: String, reuse: bool) -> Element {
    let mut store: UiStore = use_context();
    let mut copied = use_signal(|| false);
    let age = super::sidebar::relative_age(&created_at, store.lang());
    let reuse_text = text.clone();
    rsx! {
        div {
            class: "msg-footer",
            span { class: "msg-time", "{age}" }
            button {
                class: "icon-btn",
                title: store.t("thread.copy"),
                onclick: move |_| {
                    let text = text.clone();
                    spawn(async move {
                        // FR-060: the check mark only after a real copy.
                        if crate::clipboard::copy_text(&text).await {
                            copied.set(true);
                            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                            copied.set(false);
                        } else {
                            store.toast(store.t("toast.copy_failed").to_string());
                        }
                    });
                },
                if *copied.read() { {icon_check()} } else { {icon_copy()} }
            }
            if reuse {
                button {
                    class: "subtle small",
                    title: store.t("thread.reuse_title"),
                    onclick: move |_| {
                        store.prompt.set(reuse_text.clone());
                        crate::state_ui::focus_composer();
                    },
                    "{store.t(\"thread.reuse\")}"
                }
            }
        }
    }
}

/// Chevron that points right when collapsed and rotates down when open.
fn chevron(open: bool) -> Element {
    rsx! {
        span { class: if open { "chev open" } else { "chev" }, {icon_chevron_right()} }
    }
}

/// One-line summary of a turn's tool calls ("Ran 3 commands, edited 2 files
/// · 1 failed").
fn activity_summary(calls: &[ToolCallView], lang: Lang) -> String {
    let count = |test: fn(&str) -> bool| calls.iter().filter(|call| test(&call.name)).count();
    let commands = count(locagens_core::tool_labels::is_command_tool);
    let edits = count(locagens_core::tool_labels::is_edit_tool);
    let other = calls.len() - commands - edits;
    let phrase = |n: usize, one: &'static str, many: &'static str, template: &'static str| {
        let noun = i18n::t(lang, if n == 1 { one } else { many });
        i18n::tf(lang, template, &[&n.to_string(), noun])
    };
    let mut parts = Vec::new();
    if commands > 0 {
        parts.push(phrase(commands, "act.command", "act.commands", "act.ran"));
    }
    if edits > 0 {
        parts.push(phrase(edits, "act.file", "act.files", "act.edited"));
    }
    if other > 0 {
        parts.push(phrase(other, "act.tool", "act.tools", "act.used"));
    }
    // Only the first clause keeps its capital ("Ran 3 commands, edited 2 files").
    for part in parts.iter_mut().skip(1) {
        if let Some(first) = part.chars().next() {
            let lower: String = first.to_lowercase().collect();
            part.replace_range(..first.len_utf8(), &lower);
        }
    }
    let mut summary = parts.join(", ");
    let failed = calls.iter().filter(|call| call.outcome() == ToolOutcome::Failed).count();
    if failed > 0 {
        summary.push_str(" · ");
        summary.push_str(&i18n::tf(lang, "act.failed", &[&failed.to_string()]));
    }
    summary
}

/// Agent activity of one assistant turn, collapsed into a muted one-line row
/// that expands into the individual tool rows. It starts collapsed; a failed
/// call only tints the summary.
#[component]
fn ToolActivity(calls: Vec<ToolCallView>) -> Element {
    let store: UiStore = use_context();
    let failed = calls.iter().any(|call| call.outcome() == ToolOutcome::Failed);
    let mut open = use_signal(|| false);
    let summary = activity_summary(&calls, store.lang());
    rsx! {
        div {
            class: "tool-group",
            button {
                class: if failed { "activity-row has-failure" } else { "activity-row" },
                "aria-expanded": "{open.read()}",
                onclick: move |_| {
                    let is_open = *open.read();
                    open.set(!is_open);
                },
                span { "{summary}" }
                {chevron(*open.read())}
            }
            if *open.read() {
                div {
                    class: "tool-list",
                    for call in calls.iter() {
                        ToolGroupCard { key: "{call.id}", call: call.clone() }
                    }
                }
            }
        }
    }
}

/// One tool call: status glyph, "Ran `cargo test`", a badge only when it
/// failed or was denied (UX-008), and the collapsible output.
#[component]
fn ToolGroupCard(call: ToolCallView) -> Element {
    let store: UiStore = use_context();
    let outcome = call.outcome();
    let open = use_signal(|| outcome == ToolOutcome::Failed);
    let (verb, target) = describe_tool(&call.name, &call.arguments);
    let verb_text = verb_label(&verb, store.lang());
    let target = if target.is_empty() && verb.key().is_none() { summarize_args(&call.arguments) } else { target };
    let response_text = call.response.as_ref().map(|response| response.content.clone()).unwrap_or_default();
    let dot = match outcome {
        ToolOutcome::Running => "generating",
        ToolOutcome::Ok => "done",
        ToolOutcome::Failed => "failed",
        ToolOutcome::Denied => "cancelled",
    };

    // A diff preview for file-mutating tools.
    let diff_text: Option<String> = match call.name.as_str() {
        "write_file" => call.arguments.get("content").and_then(Value::as_str).map(|content| crate::markdown::pseudo_diff(None, Some(content))),
        "edit_file" => {
            let old = call.arguments.get("old_text").and_then(Value::as_str);
            let new = call.arguments.get("new_text").and_then(Value::as_str);
            old.map(|old| crate::markdown::pseudo_diff(Some(old), Some(new.unwrap_or(""))))
        }
        _ => crate::markdown::looks_like_diff(&response_text).then(|| response_text.clone()),
    };

    rsx! {
        div {
            class: if outcome == ToolOutcome::Failed { "tool-card failed" } else { "tool-card" },
            div {
                class: "tool-head",
                role: "button",
                tabindex: "0",
                "aria-expanded": "{*open.read()}",
                onclick: move |_| {
                    let mut open = open;
                    let is_open = *open.read();
                    open.set(!is_open);
                },
                {chevron(*open.read())}
                {status_dot(dot)}
                span { class: "tool-name", "{verb_text}" }
                span { class: "tool-args", title: "{target}", "{target}" }
                match outcome {
                    ToolOutcome::Failed => rsx! { span { class: "badge err", "{store.t(\"tool.failed\")}" } },
                    ToolOutcome::Denied => rsx! { span { class: "badge", "{store.t(\"tool.denied\")}" } },
                    ToolOutcome::Running => {
                        // The pending call of a prompt has not run yet.
                        let awaiting = store.active_run.read().as_ref().is_some_and(|run| run.status == "awaiting_permission");
                        let key = if awaiting { "tool.awaiting_approval" } else { "tool.running" };
                        rsx! { span { class: "tool-status", "{store.t(key)}" } }
                    }
                    ToolOutcome::Ok => rsx! {},
                }
            }
            if *open.read() {
                div {
                    class: "tool-body",
                    if let Some(diff) = diff_text.clone() {
                        DiffView { diff: diff }
                    }
                    if !response_text.is_empty() {
                        div { class: "tool-output", "{response_text}" }
                    } else if outcome != ToolOutcome::Running {
                        div { class: "tool-output", "{store.t(\"tool.no_output\")}" }
                    }
                }
            }
        }
    }
}

/// Sub-agent group with nested message groups. Built-in roles get their
/// label; custom roles (`agents/*.toml`) show their own name (FR-065).
#[component]
fn SubAgentCard(role: String, name: String, groups: Vec<Group>, depth: u8) -> Element {
    let store: UiStore = use_context();
    let open = use_signal(|| true);
    let role_label = match role.as_str() {
        "explorer" | "worker" => store.t(i18n::role_key(&role)).to_string(),
        other => other.to_string(),
    };
    rsx! {
        div {
            class: "coder-group",
            div {
                class: "coder-head",
                role: "button",
                tabindex: "0",
                "aria-expanded": "{*open.read()}",
                onclick: move |_| {
                    let mut open = open;
                    let is_open = *open.read();
                    open.set(!is_open);
                },
                span {
                    class: "badge",
                    if role == "explorer" { {icon_search()} } else { {icon_wrench()} }
                    "{role_label}"
                }
                span { "{name}" }
                span { class: "spacer", {chevron(*open.read())} }
            }
            if *open.read() {
                div {
                    class: "coder-body",
                    for group in groups.iter() {
                        GroupRow { key: "{group.key()}", group: group.clone(), depth: depth + 1, show_model: false, streaming_review: false }
                    }
                }
            }
        }
    }
}

/// Unified diff renderer (`DiffView`).
#[component]
pub fn DiffView(diff: String) -> Element {
    let lines: Vec<DiffLine> = parse_diff(&diff);
    rsx! {
        div {
            class: "diff-view",
            for line in lines.iter() {
                {
                    let class = match line.kind {
                        '+' => "diff-line add",
                        '-' => "diff-line del",
                        '@' => "diff-line hunk",
                        ' ' => "diff-line ctx",
                        _ => "diff-line plain",
                    };
                    rsx! {
                        div { class: "{class}", "{line.text}" }
                    }
                }
            }
        }
    }
}

/// `path +added −removed` labels from a `patch_progress` event.
fn patch_progress_files(event: &serde_json::Value, lang: crate::i18n::Lang) -> Vec<String> {
    let Some(files) = event.get("files").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    files
        .iter()
        .map(|file| {
            let path = file.get("path").and_then(serde_json::Value::as_str).unwrap_or("?");
            let count = |key: &str| file.get(key).and_then(serde_json::Value::as_u64).unwrap_or(0);
            match file.get("op").and_then(serde_json::Value::as_str) {
                Some("delete") => crate::i18n::tf(lang, "thread.file_deleted", &[path]),
                _ => format!("{path} +{} \u{2212}{}", count("added"), count("removed")),
            }
        })
        .collect()
}

#[cfg(test)]
mod patch_progress_tests {
    use super::*;

    #[test]
    fn labels_files_with_line_counts() {
        let event = serde_json::json!({ "files": [
            { "path": "src/a.rs", "op": "update", "added": 3, "removed": 1 },
            { "path": "old.txt", "op": "delete" }
        ]});
        assert_eq!(
            super::patch_progress_files(&event, crate::i18n::Lang::En),
            vec!["src/a.rs +3 \u{2212}1".to_string(), "old.txt (deleted)".to_string()]
        );
    }

    fn message(role: &str, model: Option<&str>) -> RunMessage {
        RunMessage { id: format!("{role}-{model:?}"), role: role.into(), model: model.map(str::to_string), ..Default::default() }
    }

    /// A sub-agent's messages once overflowed the stack (endless recursion).
    #[test]
    fn sub_agent_messages_group_without_recursing() {
        let sub = |content: &str| RunMessage {
            agent_role: Some("explorer".into()),
            agent_name: Some("lister".into()),
            content: content.into(),
            ..message("assistant", Some("m1"))
        };
        let messages = vec![message("user", None), message("assistant", Some("m1")), sub("a"), sub("b"), message("assistant", Some("m1"))];
        let groups = build_groups(&messages);
        assert_eq!(groups.len(), 4);
        match &groups[2] {
            Group::SubAgent { role, name, groups } => {
                assert_eq!((role.as_str(), name.as_str()), ("explorer", "lister"));
                assert!(!groups.is_empty());
            }
            _ => panic!("expected the sub-agent group"),
        }
    }

    #[test]
    fn model_label_only_on_the_first_answer_or_on_a_change() {
        let groups = vec![
            Group::User(message("user", None)),
            Group::Assistant { message: message("assistant", Some("m1")), tool_calls: vec![] },
            Group::Assistant { message: message("assistant", Some("m1")), tool_calls: vec![] },
            Group::Assistant { message: message("assistant", Some("m2")), tool_calls: vec![] },
            Group::User(message("user", None)),
            Group::Assistant { message: message("assistant", Some("m2")), tool_calls: vec![] },
        ];
        assert_eq!(model_label_flags(&groups), vec![false, true, false, true, false, false]);
    }

    #[test]
    fn running_tool_is_the_newest_call_without_result() {
        let calls = serde_json::json!([
            { "id": "a", "function": { "name": "read_file", "arguments": "{\"path\":\"x\"}" } },
            { "id": "b", "function": { "name": "run_command", "arguments": "{\"command\":\"cargo test\"}" } }
        ]);
        let assistant = RunMessage { role: "assistant".into(), raw_response: Some(calls.to_string()), ..Default::default() };
        let done_a = RunMessage { role: "tool".into(), raw_response: Some("a".into()), ..Default::default() };
        let (id, name, args) = running_tool(&[assistant.clone(), done_a]).unwrap();
        assert_eq!((id.as_str(), name.as_str(), args["command"].as_str()), ("b", "run_command", Some("cargo test")));
        let done_b = RunMessage { role: "tool".into(), raw_response: Some("b".into()), ..Default::default() };
        let done_a = RunMessage { role: "tool".into(), raw_response: Some("a".into()), ..Default::default() };
        assert!(running_tool(&[assistant, done_a, done_b]).is_none());
    }

    #[test]
    fn auth_errors_are_recognized() {
        assert!(is_auth_error("Provider returned 401 Unauthorized"));
        assert!(is_auth_error("provider \"x\" is not authenticated"));
        assert!(!is_auth_error("connection reset by peer"));
        assert_eq!(format_elapsed(45), "45s");
        assert_eq!(format_elapsed(72), "1m 12s");
    }

    #[test]
    fn failed_calls_are_counted_in_the_summary() {
        let failed = ToolCallView {
            id: "1".into(),
            name: "run_command".into(),
            arguments: Value::Null,
            response: Some(RunMessage { content: "Tool error: exit 1".into(), ..Default::default() }),
        };
        let ok = ToolCallView { id: "2".into(), name: "edit_file".into(), arguments: Value::Null, response: Some(RunMessage::default()) };
        assert_eq!(activity_summary(&[failed, ok], Lang::En), "Ran 1 command, edited 1 file · 1 failed");
    }
}
