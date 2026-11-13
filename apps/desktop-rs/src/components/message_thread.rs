//! Message thread: renders user messages, assistant Markdown answers with
//! collapsible reasoning, tool-call cards and sub-agent groups.

use super::*;
use crate::markdown::{parse_diff, render_markdown, summarize_args, DiffLine};
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

    rsx! {
        div {
            id: "chat-scroll",
            if is_empty {
                div {
                    class: "msg-empty",
                    div { class: "big", "What should we build?" }
                    div { "Pick a workspace and ask the agent to explore, plan, or make changes." }
                    { starter_cards(&store) }
                }
            }
            for group in groups.iter() {
                { render_group(&store, group, 0) }
            }
            if has_active && !groups.is_empty() {
                div {
                    class: "msg-meta",
                    span { class: "status-dot generating" }
                    "working…"
                }
            }
        }
    }
}

fn render_group(_store: &UiStore, group: &Group, depth: usize) -> Element {
    match group {
        Group::User(message) => rsx! {
            div {
                class: "msg-user",
                key: "{message.id}",
                div { class: "bubble", "{message.content}" }
            }
        },
        Group::Assistant { message, tool_calls } => rsx! {
            div {
                class: "msg-assistant",
                key: "{message.id}",
                div {
                    class: "msg-meta",
                    span { class: "status-dot done" }
                    span { "{message.model.clone().unwrap_or_else(|| \"assistant\".to_string())}" }
                }
                if !message.reasoning_content.clone().unwrap_or_default().is_empty() {
                    ReasoningPanel { reasoning: message.reasoning_content.clone().unwrap() }
                }
                if !message.content.is_empty() {
                    div {
                        class: "bubble md",
                        dangerous_inner_html: render_markdown(&message.content),
                    }
                }
                if !tool_calls.is_empty() {
                    div {
                        class: "tool-group",
                        for call in tool_calls.iter() {
                            ToolGroupCard { call: call.clone(), depth: depth as u8 }
                        }
                    }
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
    let starters = [
        ("Explore codebase architecture", "Map the main modules, entry points and how data flows through the project."),
        ("Review uncommitted git changes", "Run `git status` / `git diff` and review the pending changes for issues."),
        ("Run and fix failing tests", "Run the test suite, diagnose failures and fix them."),
        ("Draft a step-by-step plan", "Turn my task into an ordered, checkable plan before making changes."),
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
    let open = use_signal(|| false);
    let arrow = if *open.read() { "▾" } else { "▸" };
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
                span { "{arrow}" }
                span { "Thought · {reasoning.split_whitespace().count()} words" }
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

/// One tool call: name, summarized args, status badge and collapsible output.
#[component]
fn ToolGroupCard(call: ToolCallView, depth: u8) -> Element {
    let open = use_signal(|| false);
    let done = call.response.is_some();
    let status_text = if done { "done" } else { "running" };
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
                onclick: move |_| {
                    let mut open = open;
                    let is_open = *open.read();
                    open.set(!is_open);
                },
                span { if *open.read() { "▾" } else { "▸" } }
                span { class: "tool-name", "{call.name}" }
                span { class: "tool-args", "{args_summary}" }
                span { class: "tool-status", "{status_text}" }
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
                        div { class: "tool-output", "(no output)" }
                    }
                }
            }
        }
    }
}

/// Sub-agent (explorer / worker) group with nested message groups.
#[component]
fn SubAgentCard(role: String, name: String, groups: Vec<Group>, depth: u8) -> Element {
    let open = use_signal(|| true);
    let is_explorer = role == "explorer";
    let arrow = if *open.read() { "▲" } else { "▼" };
    rsx! {
        div {
            class: "coder-group",
            div {
                class: "coder-head",
                onclick: move |_| {
                    let mut open = open;
                    let is_open = *open.read();
                    open.set(!is_open);
                },
                span { class: "badge accent", if is_explorer { "🔎 explorer" } else { "🛠 worker" } }
                span { "{name}" }
                span { style: "margin-left:auto", "{arrow}" }
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
                div { class: "bubble", "{message.content}" }
            }
        },
        Group::Assistant { message, tool_calls } => rsx! {
            div {
                class: "msg-assistant",
                key: "{message.id}",
                if !message.reasoning_content.clone().unwrap_or_default().is_empty() {
                    ReasoningPanel { reasoning: message.reasoning_content.clone().unwrap() }
                }
                if !message.content.is_empty() {
                    div {
                        class: "bubble md",
                        dangerous_inner_html: render_markdown(&message.content),
                    }
                }
                if !tool_calls.is_empty() {
                    div {
                        class: "tool-group",
                        for call in tool_calls.iter() {
                            ToolGroupCard { call: call.clone(), depth }
                        }
                    }
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
