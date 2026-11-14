//! Pending permission approval and question cards, rendered between the chat
//! thread and the composer while a run waits for the user.

use super::icons::{icon_arrow_left, icon_arrow_right, icon_check, icon_chevron_down};
use crate::markdown::{parse_diff, pseudo_diff, DiffLine};
use crate::state_ui::{focus_composer, is_same_request, UiStore};
use dioxus::prelude::*;
use serde_json::Value;

/// Delivers a permission-card answer for `request` and drops the card only
/// while it still shows that request (FR-048): sub-agents and parallel tool
/// calls share one queue, so the next prompt may already be on screen. With
/// a `promptId` the answer can only reach that prompt.
pub(crate) async fn decide(mut store: UiStore, request: Value, decision: &'static str, mut busy: Signal<bool>) {
    busy.set(true);
    let state = crate::state_ui::core();
    let run_id = request.get("runId").and_then(Value::as_str).unwrap_or_default().to_string();
    let result = match request.get("promptId").and_then(Value::as_u64) {
        Some(prompt_id) => locagens_core::api::permissions::answer_permission_prompt(&state, &run_id, prompt_id, decision).await,
        None => locagens_core::api::runs::permission_decision(&state, &run_id, decision).await,
    };
    match result {
        Ok(()) => {
            if is_same_request(store.pending_permission.peek().as_ref(), &request) {
                store.pending_permission.set(None);
            }
            focus_composer();
        }
        Err(err) => store.toast(store.tf("toast.decision_failed", &[&err.to_string()])),
    }
    busy.set(false);
}

/// Renders whichever of the two cards is currently pending.
#[component]
pub fn PendingCards() -> Element {
    let store: UiStore = use_context();
    let permission = store.pending_permission.read().clone();
    let question = store.pending_question.read().clone();
    if let Some(request) = permission {
        // A new prompt is a new card (fresh focus and menu state).
        let key = request_key(&request);
        return rsx! { PermissionCard { key: "{key}", request: request } };
    }
    if let Some(request) = question {
        // FR-058: a second question must not reuse the first one's state.
        let key = request_key(&request);
        return rsx! { QuestionCard { key: "{key}", request: request } };
    }
    rsx! {}
}

/// Stable key of a request payload (its prompt id, else a hash of it).
fn request_key(request: &Value) -> String {
    use std::hash::{Hash, Hasher};
    if let Some(id) = request.get("promptId").and_then(Value::as_u64) {
        return format!("p{id}");
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    request.to_string().hash(&mut hasher);
    format!("h{:x}", hasher.finish())
}

/// The standing choices behind "Always allow ▾", in menu order.
const STANDING: [(&str, &str); 3] = [
    ("perm.allow_run", "allow_run"),
    ("perm.allow_project", "allow_project"),
    ("perm.allow_always", "allow_always"),
];

/// What a permission request is about, for the card title (UX-014).
#[derive(Clone, Debug, PartialEq)]
pub enum PermissionTitle {
    Command,
    Network(String),
    /// A `web_search` call, with its quoted query.
    WebSearch(String),
    Edit(String),
    Tool(String),
}

/// Picks the title from the tool name and preview: commands, network
/// access, file edits, then any other tool by its label.
pub fn permission_title(tool_name: &str, arguments: &Value, preview: Option<&Value>) -> PermissionTitle {
    let text = |key: &str| preview.and_then(|p| p.get(key)).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_string);
    let domains: Vec<String> = preview
        .and_then(|p| p.get("networkDomains"))
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    if tool_name == "web_search" {
        return PermissionTitle::WebSearch(locagens_core::tool_labels::quoted_query(arguments));
    }
    if tool_name == "fetch_url" || (text("command").is_none() && !domains.is_empty()) {
        let host = text("url").map(|url| host_of(&url)).or_else(|| domains.first().cloned()).unwrap_or_default();
        return PermissionTitle::Network(host);
    }
    if text("command").is_some() || locagens_core::tool_labels::is_command_tool(tool_name) {
        return PermissionTitle::Command;
    }
    if locagens_core::tool_labels::is_edit_tool(tool_name) {
        let files: Vec<String> = preview
            .and_then(|p| p.get("files"))
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let target = text("path").or_else(|| (!files.is_empty()).then(|| files.join(", "))).unwrap_or_default();
        return PermissionTitle::Edit(target);
    }
    let (_, target) = locagens_core::tool_labels::describe_tool(tool_name, arguments);
    PermissionTitle::Tool(if target.is_empty() { tool_name.to_string() } else { format!("{tool_name} {target}") })
}

/// `https://api.github.com/x` → `api.github.com`.
fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    rest.split(['/', '?', '#']).next().unwrap_or(rest).to_string()
}

/// Permission approval card: title by action, the asking agent, the detail
/// and diff, then `Allow once` / `Always allow ▾` / `Deny`.
/// Focus lands on `Allow once` (Enter), Esc denies (UX-014, UX-027).
#[component]
fn PermissionCard(request: Value) -> Element {
    let store: UiStore = use_context();
    let tool_call = request.get("toolCall").cloned().unwrap_or(Value::Null);
    let tool_name = tool_call.pointer("/function/name").and_then(Value::as_str).unwrap_or("tool").to_string();
    let arguments: Value = tool_call
        .pointer("/function/arguments")
        .and_then(Value::as_str)
        .and_then(|arguments| serde_json::from_str(arguments).ok())
        .unwrap_or(Value::Null);
    let preview = request.get("preview").cloned().filter(Value::is_object);

    let title = match permission_title(&tool_name, &arguments, preview.as_ref()) {
        PermissionTitle::Command => store.t("perm.title_command").to_string(),
        PermissionTitle::Network(host) => store.tf("perm.title_network", &[&host]),
        PermissionTitle::WebSearch(query) => store.tf("perm.title_web_search", &[&query]),
        PermissionTitle::Edit(path) if path.is_empty() => store.t("perm.title_edit_files").to_string(),
        PermissionTitle::Edit(path) => store.tf("perm.title_edit", &[&path]),
        PermissionTitle::Tool(label) => store.tf("perm.title_tool", &[&label]),
    };
    // FR-055: which agent is asking.
    let asked_by = request.get("agentRole").and_then(Value::as_str).map(|role| {
        let role_label = crate::i18n::t(store.lang(), crate::i18n::role_key(role));
        let role_label = if crate::i18n::role_key(role) == "agent.main" { role } else { role_label };
        let name = request.get("agentName").and_then(Value::as_str).unwrap_or_default();
        store.tf("perm.asked_by", &[role_label, name]).trim().to_string()
    });

    let detail: String = match preview.as_ref() {
        Some(preview) => {
            let command = preview.get("command").and_then(Value::as_str);
            let url = preview.get("url").and_then(Value::as_str);
            let query = preview.get("query").and_then(Value::as_str);
            let dest = preview.get("destPath").and_then(Value::as_str);
            let path = preview.get("path").and_then(Value::as_str);
            let network = preview
                .get("networkDomains")
                .and_then(Value::as_array)
                .map(|domains| domains.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            // QA-020: the Windows allowlist is an advisory proxy.
            let url_note = (cfg!(windows) && !network.is_empty())
                .then(|| url.map(|url| format!("{url}  ({})", store.t("perm.network_best_effort"))))
                .flatten();
            let base = command
                .map(str::to_string)
                .or(url_note)
                .or(url.map(str::to_string))
                .or(query.map(str::to_string))
                .or(dest.map(str::to_string))
                .or(path.map(str::to_string))
                .unwrap_or_default();
            if command.is_some() && !network.is_empty() {
                format!("{base}\n{}: {network}", store.t("perm.network"))
            } else {
                base
            }
        }
        None => serde_json::to_string_pretty(&arguments).unwrap_or_default(),
    };

    // Diff preview: a real unified `diff` (apply_patch) wins; otherwise the
    // pseudo diff between old/new content.
    let diff_text: Option<String> = preview
        .as_ref()
        .and_then(|preview| preview.get("diff"))
        .and_then(Value::as_str)
        .filter(|diff| !diff.is_empty())
        .map(str::to_string)
        .or_else(|| {
            preview.as_ref().and_then(|preview| {
                let old = preview.get("oldContent").and_then(Value::as_str);
                let new = preview.get("newContent").and_then(Value::as_str);
                (old.is_some() || new.is_some()).then(|| pseudo_diff(old, new))
            })
        });

    // UX-014: why the card asks, and what a standing answer would cover.
    let reason = preview.as_ref().and_then(|preview| preview.get("reason")).map(|reason| {
        let code = reason.get("code").and_then(Value::as_str).unwrap_or_default();
        match reason_key(code) {
            Some(key) => store.t(key).to_string(),
            None => reason.get("text").and_then(Value::as_str).unwrap_or_default().to_string(),
        }
    });
    let one_time_only = preview.as_ref().and_then(|preview| preview.get("oneTimeOnly")).and_then(Value::as_bool).unwrap_or(false);
    let grant_label = preview
        .as_ref()
        .and_then(|preview| preview.get("grantLabel"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let busy = use_signal(|| false);
    let mut menu_open = use_signal(|| false);
    let answer = move |decision: &'static str| {
        if *busy.peek() {
            return;
        }
        // Set before the task runs, so a double click cannot answer twice.
        let mut busy = busy;
        busy.set(true);
        spawn(decide(store, request.clone(), decision, busy));
    };
    let answer_key = answer.clone();
    let answer_deny = answer.clone();

    rsx! {
        div {
            class: "permission-card card-enter",
            role: "group",
            "aria-labelledby": "pc-title",
            onkeydown: move |event: KeyboardEvent| {
                if event.key() == Key::Escape {
                    event.prevent_default();
                    event.stop_propagation();
                    if *menu_open.peek() {
                        menu_open.set(false);
                    } else {
                        answer_key("deny");
                    }
                }
            },
            div {
                class: "pc-title",
                id: "pc-title",
                span { class: "badge warn", "{store.t(\"perm.requested\")}" }
                span { class: "pc-heading", "{title}" }
            }
            if let Some(asked_by) = asked_by {
                div { class: "pc-agent", "{asked_by}" }
            }
            if let Some(reason) = reason.filter(|reason| !reason.is_empty()) {
                div { class: "pc-reason", "{reason}" }
            }
            div {
                class: "pc-scroll",
                if !detail.is_empty() {
                    div { class: "pc-pathbox", "{detail}" }
                }
                if let Some(diff) = diff_text.clone() {
                    DiffViewInline { diff: diff }
                }
            }
            div {
                class: "pc-buttons",
                button {
                    class: "primary",
                    disabled: *busy.read(),
                    onmounted: move |event| async move {
                        let _ = event.set_focus(true).await;
                    },
                    onclick: {
                        let answer = answer.clone();
                        move |_| answer("allow_once")
                    },
                    "{store.t(\"perm.allow_once\")}"
                }
                if !one_time_only {
                div {
                    class: "pc-menu-wrap",
                    button {
                        disabled: *busy.read(),
                        "aria-haspopup": "menu",
                        "aria-expanded": "{menu_open.read()}",
                        onclick: move |_| {
                            let open = *menu_open.peek();
                            menu_open.set(!open);
                        },
                        "{store.t(\"perm.always_menu\")}"
                        {icon_chevron_down()}
                    }
                    if *menu_open.read() {
                        div {
                            class: "popover-backdrop",
                            onclick: move |_| menu_open.set(false),
                        }
                        div {
                            class: "composer-context-menu pc-menu",
                            role: "menu",
                            for (label, decision) in STANDING {
                                {
                                    let answer = answer.clone();
                                    rsx! {
                                        button {
                                            key: "{decision}",
                                            class: "ccm-item",
                                            role: "menuitem",
                                            onclick: move |_| {
                                                menu_open.set(false);
                                                answer(decision);
                                            },
                                            span {
                                                class: "ccm-item-label",
                                                "{store.t(label)}"
                                                if decision != "allow_run" && !grant_label.is_empty() {
                                                    span { class: "pc-grant", "{grant_label}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                }
                button {
                    class: "danger",
                    disabled: *busy.read(),
                    onclick: move |_| answer_deny("deny"),
                    "{store.t(\"perm.deny\")}"
                }
                span { class: "pc-keys", "{store.t(\"perm.keys\")}" }
            }
        }
    }
}

/// i18n key of a permission reason code (UX-014); unknown codes fall back
/// to the core's English sentence.
pub fn reason_key(code: &str) -> Option<&'static str> {
    match code {
        "protected_path" => Some("perm.reason_protected_path"),
        "dangerous_command" => Some("perm.reason_dangerous_command"),
        "network" => Some("perm.reason_network"),
        "mode" => Some("perm.reason_mode"),
        _ => None,
    }
}

/// Standalone diff renderer shared by cards.
#[component]
pub(crate) fn DiffViewInline(diff: String) -> Element {
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
                    rsx! { div { class: "{class}", "{line.text}" } }
                }
            }
        }
    }
}

/// A pending question card — options (radio / checkbox semantics) plus a
/// custom answer input.
#[component]
fn QuestionCard(request: Value) -> Element {
    let store: UiStore = use_context();
    let run_id = request.get("runId").and_then(Value::as_str).unwrap_or_default().to_string();
    let questions = request.get("questions").and_then(Value::as_array).cloned().unwrap_or_default();

    // Local per-question selection / note state.
    let selections: Signal<Vec<Vec<String>>> = use_signal(|| questions.iter().map(|_| Vec::<String>::new()).collect());
    let mut notes: Signal<Vec<String>> = use_signal(|| questions.iter().map(|_| String::new()).collect());
    let current = use_signal(|| 0usize);
    let busy = use_signal(|| false);

    let total = questions.len().max(1);
    let idx = (*current.read()).min(total - 1);
    let question = questions.get(idx).cloned().unwrap_or(Value::Null);
    let question_text = question.get("question").and_then(Value::as_str).unwrap_or_default().to_string();
    let header = question.get("header").and_then(Value::as_str).unwrap_or_default().to_string();
    let multi = question.get("multiSelect").and_then(Value::as_bool).unwrap_or(false);
    let options: Vec<String> = question
        .get("options")
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .map(|option| match option {
                    Value::String(label) => label.clone(),
                    _ => option.get("label").and_then(Value::as_str).unwrap_or_default().to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    let answered = |selections: &Vec<Vec<String>>, notes: &Vec<String>, index: usize| {
        selections.get(index).map(|selection| !selection.is_empty()).unwrap_or(false)
            || notes.get(index).map(|note| !note.trim().is_empty()).unwrap_or(false)
    };

    let can_submit = (0..questions.len()).all(|index| answered(&selections.read(), &notes.read(), index));

    rsx! {
        div {
            class: "question-card card-enter",
            role: "group",
            "aria-labelledby": "qc-question",
            div {
                class: "qc-title",
                span { class: "badge accent", "{store.t(\"question.title\")}" }
                if questions.len() > 1 {
                    span { class: "badge", "{idx + 1} / {questions.len()}" }
                }
                if !header.is_empty() {
                    span { class: "badge", "{header}" }
                }
            }
            div { class: "qc-question", id: "qc-question", "{question_text}" }
            div {
                class: "qc-options pc-scroll",
                role: if multi { "group" } else { "radiogroup" },
                "aria-labelledby": "qc-question",
                for (option_index, option) in options.iter().enumerate() {
                    {
                        let selected = selections.read().get(idx).map(|selection| selection.contains(option)).unwrap_or(false);
                        let option = option.clone();
                        let option_class = if selected { "qc-option selected" } else { "qc-option" };
                        rsx! {
                            button {
                                key: "{option_index}",
                                class: "{option_class}",
                                role: if multi { "checkbox" } else { "radio" },
                                "aria-checked": "{selected}",
                                onmounted: move |event| async move {
                                    if option_index == 0 {
                                        let _ = event.set_focus(true).await;
                                    }
                                },
                                onclick: move |_| {
                                    let mut selections = selections;
                                    selections.with_mut(|selections| {
                                        if selections.len() <= idx {
                                            selections.resize(idx + 1, Vec::new());
                                        }
                                        let selection = &mut selections[idx];
                                        if multi {
                                            if let Some(position) = selection.iter().position(|item| item == &option) {
                                                selection.remove(position);
                                            } else {
                                                selection.push(option.clone());
                                            }
                                        } else if selection.iter().any(|item| item == &option) {
                                            *selection = Vec::new();
                                        } else {
                                            *selection = vec![option.clone()];
                                        }
                                    });
                                },
                                span {
                                    class: if multi { "qc-mark multi" } else { "qc-mark" },
                                    if selected { {icon_check()} }
                                }
                                "{option}"
                            }
                        }
                    }
                }
            }
            div {
                class: "qc-note",
                input {
                    placeholder: store.t("question.custom"),
                    "aria-label": store.t("question.custom"),
                    value: "{notes.read().get(idx).cloned().unwrap_or_default()}",
                    oninput: move |event| {
                        let value = event.value();
                        notes.with_mut(|notes| {
                            while notes.len() <= idx {
                                notes.push(String::new());
                            }
                            notes[idx] = value.clone();
                        });
                    },
                }
            }
            div {
                class: "pc-buttons",
                if idx > 0 {
                    button {
                        onclick: move |_| {
                            let mut current = current;
                            current.set(idx - 1);
                        },
                        {icon_arrow_left()} "{store.t(\"common.back\")}"
                    }
                }
                if idx < questions.len().saturating_sub(1) {
                    button {
                        disabled: !answered(&selections.read(), &notes.read(), idx),
                        onclick: move |_| {
                            let mut current = current;
                            current.set(idx + 1);
                        },
                        "{store.t(\"common.next\")}" {icon_arrow_right()}
                    }
                }
                button {
                    class: "primary",
                    disabled: !can_submit || *busy.read(),
                    onclick: move |_| {
                        if *busy.peek() {
                            return;
                        }
                        let mut store = store;
                        let mut busy = busy;
                        let run_id = run_id.clone();
                        let request = request.clone();
                        let selections = selections.read().clone();
                        let notes = notes.read().iter().map(|note| note.trim().to_string()).collect();
                        busy.set(true);
                        spawn(async move {
                            let state = crate::state_ui::core();
                            match locagens_core::api::runs::answer_question(&state, &run_id, selections, notes).await {
                                Ok(()) => {
                                    if store.pending_question.peek().as_ref() == Some(&request) {
                                        store.pending_question.set(None);
                                    }
                                    focus_composer();
                                }
                                Err(err) => store.toast(store.tf("toast.answer_failed", &[&err.to_string()])),
                            }
                            busy.set(false);
                        });
                    },
                    "{store.t(\"question.submit\")}"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reason_codes_translate_and_unknown_ones_fall_back() {
        assert_eq!(reason_key("protected_path"), Some("perm.reason_protected_path"));
        assert_eq!(reason_key("network"), Some("perm.reason_network"));
        assert_eq!(reason_key("something_new"), None);
    }

    #[test]
    fn titles_follow_the_action() {
        assert_eq!(permission_title("run_command", &json!({}), Some(&json!({"command": "cargo test"}))), PermissionTitle::Command);
        assert_eq!(
            permission_title("fetch_url", &json!({}), Some(&json!({"url": "https://api.github.com/repos?x=1"}))),
            PermissionTitle::Network("api.github.com".into())
        );
        assert_eq!(
            permission_title("web_search", &json!({"query": "rust serde"}), Some(&json!({"query": "rust serde"}))),
            PermissionTitle::WebSearch("\"rust serde\"".into())
        );
        assert_eq!(
            permission_title("edit_file", &json!({}), Some(&json!({"path": "src/a.rs"}))),
            PermissionTitle::Edit("src/a.rs".into())
        );
        assert_eq!(
            permission_title("apply_patch", &json!({}), Some(&json!({"files": ["a", "b"]}))),
            PermissionTitle::Edit("a, b".into())
        );
        assert_eq!(
            permission_title("mcp__x__y", &json!({"query": "q"}), None),
            PermissionTitle::Tool("mcp__x__y q".into())
        );
    }

    #[test]
    fn request_keys_prefer_the_prompt_id() {
        assert_eq!(request_key(&json!({"promptId": 7})), "p7");
        assert_ne!(request_key(&json!({"a": 1})), request_key(&json!({"a": 2})));
    }
}
