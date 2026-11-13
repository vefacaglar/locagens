//! Message thread: renders user messages, assistant Markdown answers with
//! collapsible reasoning, tool-call cards and sub-agent groups.

use super::icons::*;
use super::*;
use crate::markdown::{parse_diff, summarize_args, DiffLine};
use crate::i18n::{self, Lang};
use crate::state_ui::UiStore;
use dioxus::prelude::*;
use locagens_core::db::RunMessage;
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

/// Rendered groups of messages.
#[derive(Clone, Debug, PartialEq)]
pub enum Group {
    User(RunMessage),
    Assistant { message: RunMessage, tool_calls: Vec<ToolCallView> },
    SubAgent { role: String, name: String, groups: Vec<Group> },
}

/// Groups the persisted message list into renderable units.
pub fn build_groups(messages: &[RunMessage]) -> Vec<Group> {
    // Map tool responses by call id.
    let mut tool_responses: std::collections::HashMap<String, RunMessage> =
        std::collections::HashMap::new();
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
            let groups_inner = build_groups(&messages[start..index]);
            groups.push(Group::SubAgent { role, name, groups: groups_inner });
            continue;
        }
        match message.role.as_str() {
            "user" | "system" => {
                if message.role == "user" {
                    groups.push(Group::User(message.clone()));
                }
                index += 1;
            }
            "assistant" => {
                let tool_calls: Vec<ToolCallView> = message
                    .raw_response
                    .as_deref()
                    .and_then(|raw| serde_json::from_str::<Vec<Value>>(raw).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|call| {
                        let id = call.get("id").and_then(Value::as_str)?.to_string();
                        let response = tool_responses.get(&id).cloned();
                        let name = call
                            .pointer("/function/name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_string();
                        let arguments = call
                            .pointer("/function/arguments")
                            .and_then(Value::as_str)
                            .and_then(|arguments| serde_json::from_str(arguments).ok())
                            .unwrap_or(Value::Null);
                        Some(ToolCallView { id, name, arguments, response })
                    })
                    .collect();
                groups.push(Group::Assistant {
                    message: message.clone(),
                    tool_calls,
                });
                index += 1;
            }
            _ => index += 1,
        }
    }
    groups
}

#[component]
pub fn MessageThread() -> Element {
    let store: UiStore = use_context();
    let messages = store.messages.read().clone();
    let groups = build_groups(&messages);
    let is_empty = groups.is_empty();
    let has_active = run_active(store.active_run.read().as_ref());
    let project_name = store
        .selected_project
        .read()
        .as_deref()
        .and_then(|path| std::path::Path::new(path).file_name())
        .map(|name| name.to_string_lossy().to_string());
    let empty_title = match project_name {
        Some(name) => store.tf("empty.title_in", &[&name]),
        None => store.t("empty.title").to_string(),
    };

    rsx! {
        div {
            id: "chat-scroll",
            if is_empty {
                div {
                    class: "msg-empty",
                    {logo_mark()}
                    div { class: "big", "{empty_title}" }
                    div { "{store.t(\"empty.subtitle\")}" }
                    { starter_cards(&store) }
                }
            }
            for group in groups.iter() {
                { render_group(&store, group, 0) }
            }
            if has_active && !groups.is_empty() {
                div {
                    class: "msg-meta",
                    {status_dot("generating")}
                    "{store.t(\"thread.working\")}"
                }
                if let Some(progress) = store.patch_progress.read().clone() {
                    div {
                        class: "msg-meta patch-progress",
                        "{store.t(\"thread.writing_patch\")}"
                        for file in patch_progress_files(&progress, store.lang()) {
                            span { class: "mono", " · {file}" }
                        }
                    }
                }
            }
            // Sized by `window.lcgnChat` so a just-sent message can sit at the top.
            div { id: "chat-tail-spacer", class: "chat-tail-spacer" }
        }
    }
}

fn render_group(_store: &UiStore, group: &Group, depth: usize) -> Element {
    match group {
        Group::User(message) => rsx! {
            div {
                class: "msg-user",
                key: "{message.id}",
                "data-mid": "{message.id}",
                div { class: "bubble", "{message.content}" }
                if !message.content.is_empty() {
                    MessageFooter { text: message.content.clone(), created_at: message.created_at.clone() }
                }
            }
        },
        Group::Assistant { message, tool_calls } => rsx! {
            div {
                class: "msg-assistant",
                key: "{message.id}",
                div {
                    class: "msg-meta",
                    span { class: "model-label", "{message.model.clone().unwrap_or_else(|| \"assistant\".to_string())}" }
                }
                if !message.reasoning_content.clone().unwrap_or_default().is_empty() {
                    ReasoningPanel { reasoning: message.reasoning_content.clone().unwrap() }
                }
                super::review::AssistantBody { message: message.clone() }
                if !tool_calls.is_empty() {
                    ToolActivity { calls: tool_calls.clone(), depth: depth as u8 }
                }
                if !message.content.is_empty() {
                    MessageFooter { text: message.content.clone(), created_at: message.created_at.clone() }
                }
            }
        },
        Group::SubAgent { role, name, groups } => rsx! {
            SubAgentCard { role: role.clone(), name: name.clone(), groups: groups.clone(), depth: depth as u8 }
        },
    }
}

/// Hero empty state: headline, subtitle and clickable starter cards that
/// pre-fill the composer prompt.
fn starter_cards(store: &UiStore) -> Element {
    let t = |key| store.t(key);
    let starters = [
        (t("starter.explore.title"), t("starter.explore.prompt")),
        (t("starter.review.title"), t("starter.review.prompt")),
        (t("starter.tests.title"), t("starter.tests.prompt")),
        (t("starter.plan.title"), t("starter.plan.prompt")),
    ];
    rsx! {
        div {
            class: "starter-grid",
            for (index, (title, prompt)) in starters.iter().enumerate() {
                {
                    let prompt = prompt.to_string();
                    let mut store = *store;
                    rsx! {
                        button {
                            key: "starter-{index}",
                            class: "starter-card",
                            onclick: move |_| store.prompt.set(prompt.clone()),
                            "{title}"
                        }
                    }
                }
            }
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

/// Footer under an assistant answer: relative age plus a copy button, both
/// quiet until the message is hovered.
#[component]
fn MessageFooter(text: String, created_at: String) -> Element {
    let store: UiStore = use_context();
    let mut copied = use_signal(|| false);
    let age = super::sidebar::relative_age(&created_at, store.lang());
    rsx! {
        div {
            class: "msg-footer",
            span { class: "msg-time", "{age}" }
            button {
                class: "icon-btn",
                title: store.t("thread.copy"),
                onclick: move |_| {
                    // The text travels as a JSON string literal, never concatenated raw.
                    let literal = serde_json::to_string(&text).unwrap_or_else(|_| "\"\"".to_string());
                    dioxus::document::eval(&format!("navigator.clipboard.writeText({literal})"));
                    copied.set(true);
                    spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                        copied.set(false);
                    });
                },
                if *copied.read() { {icon_check()} } else { {icon_copy()} }
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

/// One-line summary of a turn's tool calls ("Ran 3 commands, edited 2 files").
fn activity_summary(calls: &[ToolCallView], lang: Lang) -> String {
    let count = |names: &[&str]| calls.iter().filter(|call| names.contains(&call.name.as_str())).count();
    let commands = count(&["run_command", "exec_command", "write_stdin"]);
    let edits = count(&["write_file", "edit_file", "apply_patch", "delete_file", "move_file"]);
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
    parts.join(", ")
}

/// Agent activity of one assistant turn, collapsed into a muted one-line row
/// that expands into the individual tool cards.
#[component]
fn ToolActivity(calls: Vec<ToolCallView>, depth: u8) -> Element {
    let store: UiStore = use_context();
    let running = calls.iter().any(|call| call.response.is_none());
    let mut open = use_signal(|| running);
    let summary = activity_summary(&calls, store.lang());
    rsx! {
        div {
            class: "tool-group",
            button {
                class: "activity-row",
                onclick: move |_| {
                    let is_open = *open.read();
                    open.set(!is_open);
                },
                span { "{summary}" }
                {chevron(*open.read())}
            }
            if *open.read() {
                for call in calls.iter() {
                    ToolGroupCard { call: call.clone(), depth }
                }
            }
        }
    }
}

/// One tool call: name, summarized args, status badge and collapsible output.
#[component]
fn ToolGroupCard(call: ToolCallView, depth: u8) -> Element {
    let store: UiStore = use_context();
    let open = use_signal(|| false);
    let done = call.response.is_some();
    let status_text = if done { store.t("tool.done") } else { store.t("tool.running") };
    let args_summary = summarize_args(&call.arguments);
    let response_text = call
        .response
        .as_ref()
        .map(|response| response.content.clone())
        .unwrap_or_default();

    // A diff preview for file-mutating tools.
    let diff_text: Option<String> = match call.name.as_str() {
        "write_file" => call.arguments.get("content").and_then(Value::as_str).map(|content| {
            crate::markdown::pseudo_diff(None, Some(content))
        }),
        "edit_file" => {
            let old = call.arguments.get("old_text").and_then(Value::as_str);
            let new = call.arguments.get("new_text").and_then(Value::as_str);
            old.map(|old| crate::markdown::pseudo_diff(Some(old), Some(new.unwrap_or(""))))
        }
        _ => {
            if crate::markdown::looks_like_diff(&response_text) {
                Some(response_text.clone())
            } else {
                None
            }
        }
    };

    rsx! {
        div {
            class: "tool-card",
            key: "{call.id}",
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
                span { class: "tool-name", "{call.name}" }
                span { class: "tool-args", "{args_summary}" }
                span {
                    class: "tool-status",
                    {status_dot(if done { "done" } else { "generating" })}
                    "{status_text}"
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
                    } else if done {
                        div { class: "tool-output", "{store.t(\"tool.no_output\")}" }
                    }
                }
            }
        }
    }
}

/// Sub-agent (explorer / worker) group with nested message groups.
#[component]
fn SubAgentCard(role: String, name: String, groups: Vec<Group>, depth: u8) -> Element {
    let store: UiStore = use_context();
    let open = use_signal(|| true);
    let is_explorer = role == "explorer";
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
                    if is_explorer { {icon_search()} "{store.t(\"agent.explorer\")}" } else { {icon_wrench()} "{store.t(\"agent.worker\")}" }
                }
                span { "{name}" }
                span { class: "spacer", {chevron(*open.read())} }
            }
            if *open.read() {
                div {
                    class: "coder-body",
                    for group in groups.iter() {
                        { render_group_owned(group, depth + 1) }
                    }
                }
            }
        }
    }
}

/// Same as `render_group` but with an owned group reference (for loops).
fn render_group_owned(group: &Group, depth: u8) -> Element {
    // Delegate to the plain renderer; depth is only used for nesting style.
    match group {
        Group::User(message) => rsx! {
            div {
                class: "msg-user",
                key: "{message.id}",
                "data-mid": "{message.id}",
                div { class: "bubble", "{message.content}" }
                if !message.content.is_empty() {
                    MessageFooter { text: message.content.clone(), created_at: message.created_at.clone() }
                }
            }
        },
        Group::Assistant { message, tool_calls } => rsx! {
            div {
                class: "msg-assistant",
                key: "{message.id}",
                if !message.reasoning_content.clone().unwrap_or_default().is_empty() {
                    ReasoningPanel { reasoning: message.reasoning_content.clone().unwrap() }
                }
                super::review::AssistantBody { message: message.clone() }
                if !tool_calls.is_empty() {
                    ToolActivity { calls: tool_calls.clone(), depth }
                }
            }
        },
        Group::SubAgent { role, name, groups } => rsx! {
            SubAgentCard { role: role.clone(), name: name.clone(), groups: groups.clone(), depth: depth + 1 }
        },
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
}
