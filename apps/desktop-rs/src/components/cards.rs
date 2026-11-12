//! Pending permission approval and question cards, rendered between the chat
//! thread and the composer while a run waits for the user.

use crate::markdown::{parse_diff, pseudo_diff, DiffLine};
use crate::state_ui::UiStore;
use dioxus::prelude::*;
use serde_json::Value;

/// Renders whichever of the two cards is currently pending.
#[component]
pub fn PendingCards() -> Element {
    let store: UiStore = use_context();
    let permission = store.pending_permission.read().clone();
    let question = store.pending_question.read().clone();
    if let Some(request) = permission {
        return rsx! { PermissionCard { request: request } };
    }
    if let Some(request) = question {
        return rsx! { QuestionCard { request: request } };
    }
    rsx! {}
}

/// The five permission decisions in display order.
const DECISIONS: [(&str, &str, &str); 5] = [
    ("Allow Once", "allow_once", "primary"),
    ("Allow for Run", "allow_run", ""),
    ("Allow in Project", "allow_project", ""),
    ("Always Allow", "allow_always", ""),
    ("Deny", "deny", "danger"),
];

/// Permission approval card with the five decision buttons.
#[component]
fn PermissionCard(request: Value) -> Element {
    let store: UiStore = use_context();
    let run_id = request.get("runId").and_then(Value::as_str).unwrap_or_default().to_string();
    let tool_call = request.get("toolCall").cloned().unwrap_or(Value::Null);
    let tool_name = tool_call
        .pointer("/function/name")
        .and_then(Value::as_str)
        .unwrap_or("tool")
        .to_string();
    let arguments: Value = tool_call
        .pointer("/function/arguments")
        .and_then(Value::as_str)
        .and_then(|arguments| serde_json::from_str(arguments).ok())
        .unwrap_or(Value::Null);
    let preview = request.get("preview").cloned().filter(Value::is_object);

    let path = preview
        .as_ref()
        .and_then(|preview| preview.get("path"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let detail: String = match preview.as_ref() {
        Some(preview) => {
            let command = preview.get("command").and_then(Value::as_str);
            let url = preview.get("url").and_then(Value::as_str);
            let query = preview.get("query").and_then(Value::as_str);
            let dest = preview.get("destPath").and_then(Value::as_str);
            let network = preview
                .get("networkDomains")
                .and_then(Value::as_array)
                .map(|domains| {
                    domains
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            command
                .or(url)
                .or(query)
                .or(dest)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    if network.is_empty() {
                        path.clone()
                    } else {
                        format!("{}  (network: {})", path, network)
                    }
                })
        }
        None => {
            // Raw arguments fallback for unknown tools.
            serde_json::to_string_pretty(&arguments).unwrap_or_default()
        }
    };

    // Diff preview for file edits.
    let diff_text: Option<String> = preview.as_ref().and_then(|preview| {
        let old = preview.get("oldContent").and_then(Value::as_str);
        let new = preview.get("newContent").and_then(Value::as_str);
        match (old, new) {
            (Some(old), Some(new)) => Some(pseudo_diff(Some(old), Some(new))),
            (Some(old), None) => Some(pseudo_diff(Some(old), None)),
            (None, Some(new)) => Some(pseudo_diff(None, Some(new))),
            _ => None,
        }
    });

    let busy = use_signal(|| false);

    async fn decide(mut store: UiStore, run_id: String, decision: &'static str, mut busy: Signal<bool>) {
        busy.set(true);
        let state = crate::state_ui::core();
        if let Err(err) =
            locagens_core::api::runs::permission_decision(&state, &run_id, decision).await
        {
            store.toast(format!("Failed to apply decision: {err}"));
        }
        store.pending_permission.set(None);
        busy.set(false);
    }

    rsx! {
        div {
            class: "permission-card",
            div {
                class: "pc-title",
                span { class: "badge warn", "Permission requested" }
                span { class: "tool-name mono", style: "font-family:monospace;font-size:12px", "{tool_name}" }
            }
            div {
                class: "pc-detail",
                "The agent wants to run "
                span { style: "font-weight:600", "{tool_name}" }
                if !path.is_empty() {
                    " on "
                    span { style: "font-weight:600", "{path}" }
                }
                "."
            }
            if !detail.is_empty() {
                div { class: "pc-pathbox", "{detail}" }
            }
            if let Some(diff) = diff_text.clone() {
                DiffViewInline { diff: diff }
            }
            div {
                class: "pc-buttons",
                for (label, decision, class) in DECISIONS {
                    {
                        let run_id_for_call = run_id.clone();
                        rsx! {
                            button {
                                key: "{decision}",
                                class: "{class}",
                                disabled: *busy.read(),
                                onclick: move |_| {
                                    let store = store;
                                    let busy = busy;
                                    let run_id_call = run_id_for_call.clone();
                                    spawn(async move { decide(store, run_id_call, decision, busy).await });
                                },
                                "{label}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Standalone diff renderer shared by cards.
#[component]
fn DiffViewInline(diff: String) -> Element {
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

/// A pending question card — clickable options plus a custom answer input.
#[component]
fn QuestionCard(request: Value) -> Element {
    let store: UiStore = use_context();
    let run_id = request.get("runId").and_then(Value::as_str).unwrap_or_default().to_string();
    let questions = request
        .get("questions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // Local per-question selection / note state.
    let selections: Signal<Vec<Vec<String>>> = use_signal(|| {
        questions.iter().map(|_| Vec::<String>::new()).collect()
    });
    let mut notes: Signal<Vec<String>> =
        use_signal(|| questions.iter().map(|_| String::new()).collect());
    let current = use_signal(|| 0usize);
    let busy = use_signal(|| false);

    let total = questions.len().max(1);
    let idx = (*current.read()).min(total - 1);
    let question = questions.get(idx).cloned().unwrap_or(Value::Null);
    let question_text = question
        .get("question")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let header = question
        .get("header")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let multi = question
        .get("multiSelect")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let options: Vec<String> = question
        .get("options")
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .map(|option| match option {
                    Value::String(label) => label.clone(),
                    _ => option
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    let answered = |selections: &Vec<Vec<String>>, notes: &Vec<String>, index: usize| {
        selections.get(index).map(|selection| !selection.is_empty()).unwrap_or(false)
            || notes
                .get(index)
                .map(|note| !note.trim().is_empty())
                .unwrap_or(false)
    };

    let can_submit = (0..questions.len()).all(|index| answered(&*selections.read(), &*notes.read(), index));

    rsx! {
        div {
            class: "question-card",
            div {
                class: "qc-title",
                span { class: "badge accent", "Agent question" }
                if questions.len() > 1 {
                    span { class: "badge", "{idx + 1} / {questions.len()}" }
                }
                if !header.is_empty() {
                    span { class: "badge", "{header}" }
                }
            }
            div { class: "qc-question", "{question_text}" }
            div {
                class: "qc-options",
                for (option_index, option) in options.iter().enumerate() {
                    {
                        let selected = selections.read().get(idx).map(|selection| selection.contains(option)).unwrap_or(false);
                        let option = option.clone();
                        let option_class = if selected { "qc-option selected" } else { "qc-option" };
                        rsx! {
                            button {
                                key: "{option_index}",
                                class: "{option_class}",
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
                                if multi && selected { "☑ " } else if multi { "☐ " } else if selected { "● " } else { "○ " }
                                "{option}"
                            }
                        }
                    }
                }
            }
            div {
                class: "qc-note",
                input {
                    placeholder: "Or type a custom answer…",
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
                        "← Back"
                    }
                }
                if idx < questions.len().saturating_sub(1) {
                    button {
                        disabled: !answered(&*selections.read(), &*notes.read(), idx),
                        onclick: move |_| {
                            let mut current = current;
                            current.set(idx + 1);
                        },
                        "Next →"
                    }
                }
                button {
                    class: "primary",
                    disabled: !can_submit || *busy.read(),
                    onclick: move |_| {
                        let mut store = store;
                        let mut busy = busy;
                        let run_id = run_id.clone();
                        let selections = selections.read().clone();
                        let notes = notes.read().iter().map(|note| note.trim().to_string()).collect();
                        busy.set(true);
                        spawn(async move {
                            let state = crate::state_ui::core();
                            if let Err(err) =
                                locagens_core::api::runs::answer_question(&state, &run_id, selections, notes).await
                            {
                                store.toast(format!("Failed to answer: {err}"));
                            }
                            store.pending_question.set(None);
                            busy.set(false);
                        });
                    },
                    "Submit Answer"
                }
            }
        }
    }
}
