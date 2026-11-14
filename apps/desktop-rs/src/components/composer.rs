//! Composer: queued-message strip, prompt textarea (Enter to send,
//! Shift+Enter newline, `Up` recalls), keyboard `/` and `@` completion at the
//! caret, file attachments, the model picker, mode selector and the Send /
//! Continue / Queue action.

use super::icons::*;
use super::model_picker::{effective_effort, model_efforts, ModelPicker};
use super::slash::{self, SlashAction};
use crate::state_ui::{core, mode_key, AppPage, Overlay, QueuedMessage, SettingsTab, UiStore};
use dioxus::prelude::*;
use locagens_core::api::runs::{ContinueRunPayload, CreateRunPayload};
use locagens_core::commands::{detect_trigger, TriggerKind};

/// Byte offset of UTF-16 offset `utf16` in `text` (the webview reports the
/// caret in UTF-16 code units), clamped to the text.
pub fn utf16_to_byte(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if units >= utf16 {
            return byte;
        }
        units += ch.len_utf16();
    }
    text.len()
}

/// Replaces `text[start..end]` (bytes) with `insert`; returns the new text
/// and the caret after the insertion in UTF-16 units.
pub fn splice(text: &str, start: usize, end: usize, insert: &str) -> (String, usize) {
    let (start, end) = (start.min(text.len()), end.min(text.len()).max(start.min(text.len())));
    let mut out = String::with_capacity(text.len() + insert.len());
    out.push_str(&text[..start]);
    out.push_str(insert);
    let caret = out.encode_utf16().count();
    out.push_str(&text[end..]);
    (out, caret)
}

/// Reports the composer caret (UTF-16 `selectionStart`) after every input,
/// key, click or focus in the textarea.
const CARET_SCRIPT: &str = r#"
const report = (e) => {
  const t = e.target;
  if (t && t.matches && t.matches('.composer textarea')) dioxus.send(t.selectionStart || 0);
};
for (const kind of ['input', 'keyup', 'click', 'focus', 'select']) document.addEventListener(kind, report, true);
"#;

/// Puts the caret at UTF-16 offset `at` once the new value has rendered.
fn place_caret(at: usize) {
    const SCRIPT: &str = "requestAnimationFrame(() => requestAnimationFrame(() => { const t = document.querySelector('.composer textarea'); if (t) { t.focus(); t.setSelectionRange(AT, AT); } }));";
    dioxus::document::eval(&SCRIPT.replace("AT", &at.to_string()));
}

/// One completion entry under the caret.
#[derive(Clone, Debug, PartialEq)]
enum Completion {
    Command { name: &'static str, usage: String, takes_args: bool },
    File(String),
}

#[component]
pub fn Composer() -> Element {
    let mut store: UiStore = use_context();
    let busy = use_signal(|| false);
    let mut context_menu: Signal<Option<&'static str>> = use_signal(|| None);
    let skills: Signal<Vec<locagens_core::services::skills::SkillSummary>> = use_signal(Vec::new);
    let mut file_query = use_signal(String::new);
    let picker_files: Signal<Vec<String>> = use_signal(Vec::new);
    // Caret in UTF-16 units, and the prompt text whose menu Esc dismissed.
    let mut caret = use_signal(|| 0usize);
    let mut dismissed: Signal<Option<String>> = use_signal(|| None);
    let mut highlighted = use_signal(|| 0usize);

    use_future(move || async move {
        let mut channel = dioxus::document::eval(CARET_SCRIPT);
        while let Ok(position) = channel.recv::<usize>().await {
            if *caret.peek() != position {
                caret.set(position);
            }
        }
    });

    let prompt_text = store.prompt.read().clone();
    let caret_byte = utf16_to_byte(&prompt_text, *caret.read());
    let trigger = detect_trigger(&prompt_text, caret_byte).filter(|_| dismissed.read().as_deref() != Some(prompt_text.as_str()));
    let mention_query = trigger.as_ref().filter(|t| t.kind == TriggerKind::File).map(|t| t.query.clone());

    // `@` mentions: fuzzy file list of the current project for the token.
    let mention_files = use_resource(move || {
        let query = mention_query.clone();
        let project = store.current_project();
        async move {
            let (Some(query), Some(project)) = (query, project) else { return Vec::new() };
            locagens_core::api::projects::list_files(&core(), &project, Some(&query)).await.unwrap_or_default()
        }
    });
    // `+` → Mentions picker list.
    use_effect(move || {
        if *context_menu.read() != Some("mentions") {
            return;
        }
        let query = file_query.read().trim().to_string();
        let path = store.current_project().unwrap_or_default();
        let mut picker_files = picker_files;
        spawn(async move {
            let files = locagens_core::api::projects::list_files(&core(), &path, Some(&query)).await.unwrap_or_default();
            picker_files.set(files);
        });
    });

    let completions: Vec<Completion> = match trigger.as_ref().map(|t| t.kind) {
        Some(TriggerKind::Slash) => slash::suggestions(&prompt_text[..caret_byte.min(prompt_text.len())])
            .into_iter()
            .map(|spec| Completion::Command { name: spec.name, usage: spec.usage(), takes_args: spec.args.starts_with('<') })
            .collect(),
        Some(TriggerKind::File) => mention_files.read().clone().unwrap_or_default().into_iter().take(8).map(Completion::File).collect(),
        None => Vec::new(),
    };
    let menu_open = !completions.is_empty() && context_menu.read().is_none();
    let index = (*highlighted.read()).min(completions.len().saturating_sub(1));

    // Accepts a completion: a command is inserted (or run when it takes no
    // argument and `run` is set), a file replaces the `@token`.
    let completions_for_keys = completions.clone();
    let trigger_for_keys = trigger.clone();
    let mut accept = move |completion: Completion, run: bool| {
        let text = store.prompt.peek().clone();
        let end = utf16_to_byte(&text, *caret.peek());
        match completion {
            Completion::Command { name, takes_args, .. } => {
                if run && !takes_args {
                    run_command_line(store, &format!("/{name}"));
                } else {
                    let (next, at) = splice(&text, 0, end, &format!("/{name} "));
                    store.prompt.set(next);
                    place_caret(at);
                }
            }
            Completion::File(path) => {
                let start = trigger_for_keys.as_ref().map(|t| t.start).unwrap_or(end);
                let (next, at) = splice(&text, start, end, &format!("@{path} "));
                store.prompt.set(next);
                place_caret(at);
            }
        }
        highlighted.set(0);
    };
    let accept_click = accept.clone();

    let selected_mode = store.selected_mode.read().clone();
    let mode_label = store.t(mode_key(&selected_mode));
    let mode_class = match selected_mode.as_str() {
        "auto" => "composer-select-btn mode-btn mode-auto",
        "plan" => "composer-select-btn mode-btn mode-plan",
        _ => "composer-select-btn mode-btn",
    };

    let run = store.active_run.read().clone();
    let run_status = run.as_ref().map(|run| run.status.clone());
    let is_generating = run_status.as_deref() == Some("generating");
    let is_awaiting = matches!(run_status.as_deref(), Some("awaiting_permission") | Some("awaiting_input"));
    let has_run = run.is_some();
    let send_label = if is_generating {
        store.t("composer.queue")
    } else if has_run {
        store.t("composer.continue")
    } else {
        store.t("composer.send")
    };
    // UX-001 / UX-004: nothing to run on yet.
    let no_provider = *store.providers_loaded.read() && store.visible_catalog().is_empty();
    let no_folder = store.projects.read().is_empty() && store.current_project().is_none();
    let blocked_reason = if no_provider {
        Some(store.t("composer.connect_first"))
    } else if no_folder {
        Some(store.t("composer.folder_first"))
    } else {
        None
    };
    let placeholder = if no_folder { store.t("composer.folder_first") } else { store.t("composer.placeholder") };
    let queued: Vec<String> = {
        let active = store.active_run_id.read().clone();
        store.queued.read().iter().filter(|queued| Some(&queued.run_id) == active.as_ref()).map(|queued| queued.text.clone()).collect()
    };

    rsx! {
        if !queued.is_empty() {
            div {
                class: "queued-strip",
                role: "region",
                "aria-label": store.t("queue.title"),
                div { class: "queued-head", "{store.t(\"queue.title\")}" }
                for (position, text) in queued.iter().enumerate() {
                    {
                        let text_remove = text.clone();
                        rsx! {
                            div {
                                key: "{position}",
                                class: "queued-item",
                                span { class: "queued-text", title: "{text}", "{text}" }
                                button {
                                    class: "icon-btn",
                                    title: store.t("queue.remove"),
                                    onclick: move |_| withdraw(store, text_remove.clone()),
                                    {icon_x()}
                                }
                            }
                        }
                    }
                }
            }
        }
        div {
            class: if selected_mode == "auto" { "composer mode-auto" } else { "composer" },
            // Click-outside-to-close backdrop for open menus.
            if context_menu.read().is_some() {
                div {
                    class: "popover-backdrop",
                    onclick: move |e| {
                        e.stop_propagation();
                        context_menu.set(None);
                    },
                }
            }
            // `/` and `@` completions at the caret (keyboard: ↑↓ Tab Enter Esc).
            if menu_open {
                div {
                    class: "composer-context-menu slash-menu",
                    role: "listbox",
                    id: "composer-completions",
                    div {
                        class: "ccm-header",
                        if matches!(completions.first(), Some(Completion::File(_))) { "{store.t(\"composer.mentions\")}" } else { "{store.t(\"composer.commands\")}" }
                    }
                    div {
                        class: "ccm-list",
                        for (position, completion) in completions.iter().enumerate() {
                            {
                                let item = completion.clone();
                                let mut accept = accept_click.clone();
                                let class = if position == index { "ccm-item active" } else { "ccm-item" };
                                match completion {
                                    Completion::Command { name, usage, .. } => {
                                        let description = locagens_core::commands::find(name)
                                            .map(|spec| crate::i18n::command_description(store.lang(), spec))
                                            .unwrap_or_default();
                                        rsx! {
                                            button {
                                                key: "cmd-{name}",
                                                class: "{class}",
                                                role: "option",
                                                "aria-selected": "{position == index}",
                                                title: "{description}",
                                                onmouseenter: move |_| highlighted.set(position),
                                                onclick: move |_| accept(item.clone(), true),
                                                span { class: "ccm-cmd-usage", "{usage}" }
                                                span { class: "ccm-cmd-desc", "{description}" }
                                            }
                                        }
                                    }
                                    Completion::File(path) => rsx! {
                                        button {
                                            key: "file-{path}",
                                            class: "{class}",
                                            role: "option",
                                            "aria-selected": "{position == index}",
                                            title: "{path}",
                                            onmouseenter: move |_| highlighted.set(position),
                                            onclick: move |_| accept(item.clone(), false),
                                            span { class: "ccm-item-label mono", "{path}" }
                                        }
                                    },
                                }
                            }
                        }
                    }
                }
            }
            // Full-width borderless prompt input.
            textarea {
                placeholder: placeholder,
                "aria-label": store.t("composer.placeholder"),
                "aria-controls": "composer-completions",
                "aria-expanded": "{menu_open}",
                disabled: blocked_reason.is_some(),
                value: "{prompt_text}",
                rows: "2",
                oninput: move |event| {
                    store.prompt.set(event.value());
                    dismissed.set(None);
                },
                onkeydown: move |event: KeyboardEvent| {
                    let key = event.key();
                    if menu_open {
                        let last = completions_for_keys.len().saturating_sub(1);
                        match key {
                            Key::ArrowDown => {
                                event.prevent_default();
                                let next = (*highlighted.peek() + 1).min(last);
                                highlighted.set(next);
                                return;
                            }
                            Key::ArrowUp => {
                                event.prevent_default();
                                let next = highlighted.peek().saturating_sub(1);
                                highlighted.set(next);
                                return;
                            }
                            Key::Tab | Key::Enter if !event.modifiers().shift() => {
                                event.prevent_default();
                                if let Some(item) = completions_for_keys.get((*highlighted.peek()).min(last)).cloned() {
                                    accept(item, key == Key::Enter);
                                }
                                return;
                            }
                            Key::Escape => {
                                event.prevent_default();
                                event.stop_propagation();
                                dismissed.set(Some(store.prompt.peek().clone()));
                                return;
                            }
                            _ => {}
                        }
                    }
                    if key == Key::Enter && !event.modifiers().shift() {
                        event.prevent_default();
                        submit(&store, busy);
                    } else if key == Key::ArrowUp && store.prompt.peek().is_empty() {
                        // Recall the previous prompt (UX-019).
                        if let Some(last) = store.prompt_history.peek().last().cloned() {
                            event.prevent_default();
                            let at = last.encode_utf16().count();
                            store.prompt.set(last);
                            place_caret(at);
                        }
                    }
                },
                onclick: move |_| context_menu.set(None),
            }
            div {
                class: "composer-row-bottom",
                // "Add Context" (+) button with popover menu.
                div {
                    class: "composer-dropdown",
                    button {
                        class: "composer-plus-btn",
                        title: store.t("composer.add_context"),
                        disabled: blocked_reason.is_some(),
                        onclick: move |_| {
                            let next = if *context_menu.read() == Some("root") { None } else { Some("root") };
                            context_menu.set(next);
                        },
                        {icon_plus()}
                    }
                    if let Some(menu_kind) = *context_menu.read() {
                        {match menu_kind {
                        "root" => rsx! {
                            div {
                                class: "composer-context-menu",
                                onclick: move |e| e.stop_propagation(),
                                div { class: "ccm-header", "{store.t(\"composer.add_context\")}" }
                                button {
                                    class: "ccm-item",
                                    onclick: move |_| {
                                        context_menu.set(None);
                                        spawn(async move {
                                            if let Some(files) = rfd::AsyncFileDialog::new().pick_files().await {
                                                let references: String = files.iter().map(|file| format!("@{} ", file.path().to_string_lossy())).collect();
                                                insert_at_caret(store, caret, &references);
                                            }
                                        });
                                    },
                                    {icon_paperclip()}
                                    span { "{store.t(\"composer.files\")}" }
                                }
                                button {
                                    class: "ccm-item",
                                    onclick: move |_| {
                                        context_menu.set(None);
                                        spawn(async move {
                                            if let Some(folder) = rfd::AsyncFileDialog::new().pick_folder().await {
                                                insert_at_caret(store, caret, &format!("@{} ", folder.path().to_string_lossy()));
                                            }
                                        });
                                    },
                                    {icon_folder()}
                                    span { "{store.t(\"composer.folder\")}" }
                                }
                                button {
                                    class: "ccm-item",
                                    onclick: move |_| {
                                        file_query.set(String::new());
                                        context_menu.set(Some("mentions"));
                                    },
                                    {icon_at()}
                                    span { "{store.t(\"composer.mentions\")}" }
                                }
                                button {
                                    class: "ccm-item",
                                    onclick: move |_| {
                                        let path = store.current_project().unwrap_or_default();
                                        let mut skills = skills;
                                        spawn(async move {
                                            let list = locagens_core::api::skills::list_skills(&core(), Some(&path))
                                                .await
                                                .map(|response| response.skills)
                                                .unwrap_or_default();
                                            skills.set(list);
                                        });
                                        context_menu.set(Some("actions"));
                                    },
                                    {icon_actions()}
                                    span { "{store.t(\"composer.actions\")}" }
                                }
                            }
                        },
                        "mentions" => rsx! {
                            div {
                                class: "composer-context-menu",
                                onclick: move |e| e.stop_propagation(),
                                div {
                                    class: "ccm-header",
                                    button {
                                        class: "subtle icon-btn",
                                        onclick: move |_| context_menu.set(Some("root")),
                                        "← {store.t(\"composer.add_context\")}"
                                    }
                                }
                                input {
                                    class: "ccm-search",
                                    r#type: "text",
                                    placeholder: store.t("composer.filter_files"),
                                    value: "{file_query.read()}",
                                    onmounted: move |event| async move {
                                        let _ = event.set_focus(true).await;
                                    },
                                    oninput: move |event| file_query.set(event.value()),
                                }
                                div {
                                    class: "ccm-list",
                                    if picker_files.read().is_empty() {
                                        div { class: "ccm-empty", "{store.t(\"composer.no_files\")}" }
                                    }
                                    for (index, file) in picker_files.read().iter().take(10).enumerate() {
                                        {
                                            let file = file.clone();
                                            rsx! {
                                                button {
                                                    key: "file-{index}",
                                                    class: "ccm-item",
                                                    title: "{file}",
                                                    onclick: move |_| {
                                                        context_menu.set(None);
                                                        insert_at_caret(store, caret, &format!("@{file} "));
                                                    },
                                                    "{file}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        "actions" => rsx! {
                            div {
                                class: "composer-context-menu",
                                onclick: move |e| e.stop_propagation(),
                                div {
                                    class: "ccm-header",
                                    button {
                                        class: "subtle icon-btn",
                                        onclick: move |_| context_menu.set(Some("root")),
                                        "← {store.t(\"composer.add_context\")}"
                                    }
                                }
                                div {
                                    class: "ccm-list",
                                    if skills.read().is_empty() {
                                        div { class: "ccm-empty", "{store.t(\"composer.no_skills\")}" }
                                    }
                                    for (index, skill) in skills.read().iter().take(12).enumerate() {
                                        {
                                            let skill_name = skill.name.clone();
                                            let skill_title = format!("{} — {}", skill.name, skill.description);
                                            rsx! {
                                                button {
                                                    key: "skill-{index}",
                                                    class: "ccm-item",
                                                    title: "{skill_title}",
                                                    onclick: move |_| {
                                                        context_menu.set(None);
                                                        insert_at_caret(store, caret, &format!("${skill_name} "));
                                                    },
                                                    "{skill_name}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        _ => rsx! {},
                        }}
                    }
                }
                ModelPicker {}
                span { class: "composer-spacer" }
                // Run tokens and cost, quiet, next to the controls.
                if let Some(usage) = store.run_usage.read().as_ref() {
                    span {
                        class: "composer-usage",
                        title: store.t("composer.usage_title"),
                        {store.tf(
                            "header.tokens_cost",
                            &[
                                &crate::state_ui::fmt_tokens(usage.total_input_tokens + usage.total_output_tokens),
                                &crate::state_ui::fmt_cost(usage.total_cost),
                            ],
                        )}
                    }
                }
                // Execution mode selector (UX-012: Plan in accent, Full Access in warn).
                div {
                    class: "composer-dropdown",
                    button {
                        class: if *context_menu.read() == Some("mode") { format!("{mode_class} open") } else { mode_class.to_string() },
                        title: store.tf("composer.mode_title", &[&super::shortcut_label(".")]),
                        "aria-haspopup": "menu",
                        onclick: move |_| {
                            let next = if *context_menu.read() == Some("mode") { None } else { Some("mode") };
                            context_menu.set(next);
                        },
                        {icon_shield()}
                        span { "{mode_label}" }
                        {icon_chevron_down()}
                    }
                    if *context_menu.read() == Some("mode") {
                        div {
                            class: "composer-context-menu align-right mode-menu",
                            role: "menu",
                            onclick: move |e| e.stop_propagation(),
                            div { class: "ccm-header", "{store.t(\"composer.mode\")}" }
                            div {
                                class: "ccm-list",
                                for (val, description) in [("accept_edits", "mode.build_desc"), ("plan", "mode.plan_desc"), ("auto", "mode.auto_desc")] {
                                    {
                                        let is_active = val == selected_mode;
                                        rsx! {
                                            button {
                                                key: "mode-{val}",
                                                class: if is_active { "ccm-item mode-item active" } else { "ccm-item mode-item" },
                                                role: "menuitemradio",
                                                "aria-checked": "{is_active}",
                                                onclick: move |_| {
                                                    store.pick_mode(val);
                                                    context_menu.set(None);
                                                },
                                                span {
                                                    class: "ccm-item-label",
                                                    span { class: if val == "auto" { "mode-name warn" } else { "mode-name" }, "{store.t(mode_key(val))}" }
                                                    span { class: "mode-desc", "{store.t(description)}" }
                                                }
                                                if is_active { {icon_check()} }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if is_generating {
                    button {
                        class: "icon-btn danger",
                        title: store.t("header.stop"),
                        onclick: move |_| {
                            let Some(run_id) = run.as_ref().map(|run| run.id.clone()) else {
                                return;
                            };
                            spawn(async move {
                                let _ = locagens_core::api::runs::cancel_run(&core(), &run_id).await;
                            });
                        },
                        {icon_stop()}
                    }
                }
                button {
                    class: "composer-send-btn",
                    disabled: *busy.read() || is_awaiting || prompt_text.trim().is_empty() || blocked_reason.is_some(),
                    title: if let Some(reason) = blocked_reason {
                        reason.to_string()
                    } else if is_awaiting {
                        store.t("composer.resolve_first").to_string()
                    } else {
                        send_label.to_string()
                    },
                    onclick: move |_| submit(&store, busy),
                    {icon_arrow_up()}
                }
            }
        }
    }
}

/// Inserts `text` at the caret (not at the end, UX-020).
fn insert_at_caret(store: UiStore, caret: Signal<usize>, text: &str) {
    let mut store = store;
    let current = store.prompt.peek().clone();
    let at = utf16_to_byte(&current, *caret.peek());
    let (next, position) = splice(&current, at, at, text);
    store.prompt.set(next);
    place_caret(position);
}

/// Withdraws a queued message before the model takes it (UX-011).
fn withdraw(store: UiStore, text: String) {
    let mut store = store;
    let Some(run_id) = store.active_run_id.peek().clone() else { return };
    spawn(async move {
        let removed = locagens_core::api::queue::withdraw_queued_message(&core(), &run_id, &text).await.unwrap_or(false);
        let entry = QueuedMessage { run_id: run_id.clone(), text: text.clone() };
        store.queued.with_mut(|queue| {
            if let Some(index) = queue.iter().position(|queued| *queued == entry) {
                queue.remove(index);
            }
        });
        if removed {
            store.prompt.with_mut(|prompt| {
                if prompt.trim().is_empty() {
                    *prompt = text;
                }
            });
        } else {
            store.toast(store.t("queue.already_sent").to_string());
        }
    });
}

/// Sends the prompt: a slash command, or create_run (no active run),
/// continue_run (finished run) or queue_message (a generating run). Attaches
/// the event stream that was subscribed before the drive task started.
pub(crate) fn submit(store: &UiStore, busy: Signal<bool>) {
    let store = *store;
    let prompt = store.prompt.read().trim().to_string();
    if prompt.is_empty() || *busy.read() {
        return;
    }
    if run_command_line(store, &prompt) {
        return;
    }
    // A pending card must be resolved first.
    if store.pending_permission.read().is_some() || store.pending_question.read().is_some() {
        store.toast(store.t("toast.resolve_first").to_string());
        return;
    }
    start_turn(store, prompt, busy);
}

/// Runs `line` when it is a slash command (shared table and busy rule);
/// `false` when it is a plain prompt.
pub fn run_command_line(store: UiStore, line: &str) -> bool {
    let generating = store.active_run.peek().as_ref().is_some_and(|run| run.status == "generating");
    match slash::resolve(line, generating) {
        Some(action) => {
            run_slash(store, action);
            true
        }
        None => false,
    }
}

/// Executes a slash command on the desktop.
fn run_slash(mut store: UiStore, action: SlashAction) {
    let clears_prompt = !matches!(action, SlashAction::Busy(_) | SlashAction::Unavailable(_) | SlashAction::Unknown(_));
    if clears_prompt {
        store.prompt.set(String::new());
    }
    match action {
        SlashAction::Reset => {
            store.reset_conversation();
            crate::state_ui::focus_composer();
        }
        SlashAction::Fork => {
            if let Some(run_id) = store.active_run_id.read().clone() {
                spawn(async move {
                    match locagens_core::api::runs::fork_run(&core(), &run_id, None).await {
                        Ok(forked) => {
                            store.refresh_runs();
                            store.select_run(forked.id);
                            store.toast(store.t("toast.forked").to_string());
                        }
                        Err(err) => store.toast(store.tf("toast.fork_failed", &[&err.to_string()])),
                    }
                });
            } else {
                store.toast(store.t("toast.no_fork").to_string());
            }
        }
        SlashAction::Undo => {
            let Some(run_id) = store.active_run_id.read().clone() else {
                store.toast(store.t("toast.select_project").to_string());
                return;
            };
            spawn(async move {
                match super::changes::undo_turn(store, &run_id).await {
                    Ok(snapshot) => store.toast(super::changes::undo_summary(&store, &snapshot)),
                    Err(err) => store.toast(store.tf("toast.undo_failed", &[&err])),
                }
            });
        }
        SlashAction::Review(argument) => super::review::start_from_command(&store, &argument),
        // UX-012: the mode control shows the change; no toast.
        SlashAction::SetMode(mode) => store.pick_mode(mode),
        SlashAction::Help => store.overlay.set(Overlay::Shortcuts),
        SlashAction::Model(None) => store.model_picker_open.set(true),
        SlashAction::Model(Some(key)) => {
            let catalog = store.visible_catalog();
            let found = locagens_core::services::ui_state::split_model_key(&key)
                .filter(|(provider, model)| catalog.iter().any(|p| p.id == *provider && p.models.iter().any(|m| m == model)));
            match found {
                Some((provider, model)) => store.pick_model(provider, model, None),
                None => store.toast(store.tf("toast.unknown_model", &[&key])),
            }
        }
        SlashAction::Thinking(level) => {
            let provider = store.selected_provider.peek().clone();
            let model = store.selected_model.peek().clone();
            let catalog = store.visible_catalog();
            let efforts = model_efforts(catalog.iter().find(|p| p.id == provider), &model);
            match level.as_deref().map(|level| slash::thinking_level(level, &efforts)) {
                None => store.model_picker_open.set(true),
                Some(Some(effort)) => store.pick_effort(&effort),
                Some(None) => {
                    let levels = if efforts.is_empty() { store.t("toast.no_effort").to_string() } else { efforts.join(", ") };
                    store.toast(store.tf("toast.unknown_effort", &[&levels]));
                }
            }
        }
        SlashAction::Diff => super::changes::open_changes(store),
        SlashAction::Mcp => store.page.set(AppPage::Settings(SettingsTab::Mcp)),
        SlashAction::Skills => store.page.set(AppPage::Settings(SettingsTab::Skills)),
        SlashAction::Sessions => store.overlay.set(Overlay::Palette { chats_only: true }),
        SlashAction::Usage => store.page.set(AppPage::UsageLogs),
        SlashAction::Busy(name) => store.toast(store.tf("toast.command_busy", &[&name])),
        SlashAction::Unavailable(name) => store.toast(store.tf("toast.command_cli_only", &[&name])),
        SlashAction::Unknown(head) => {
            store.toast(store.tf("toast.unknown_command", &[&head, &slash::command_names()]));
        }
    }
}

/// Sends a (non-command) prompt: queue on a generating run, continue a
/// finished one, or create a new run.
fn start_turn(mut store: UiStore, prompt: String, busy: Signal<bool>) {
    let provider_id = store.selected_provider.read().clone();
    let model = store.selected_model.read().clone();
    if provider_id.is_empty() || model.is_empty() {
        store.toast(store.t("toast.select_provider").to_string());
        return;
    }
    let mode = store.selected_mode.read().clone();
    let effort = store.selected_effort.read().clone();
    let project_path = store.current_project();

    // Clamp the effort to the selected model's supported levels.
    let providers_catalog = store.providers_catalog.read().clone();
    let current_provider = providers_catalog.iter().find(|p| p.id == provider_id);
    let efforts = model_efforts(current_provider, &model);
    let effort = effective_effort(&effort, &efforts).map(str::to_string);

    let run = store.active_run.read().clone();
    let status = run.as_ref().map(|run| run.status.clone());

    let mut busy = busy;
    busy.set(true);
    store.prompt_history.with_mut(|history| {
        history.retain(|old| old != &prompt);
        history.push(prompt.clone());
    });

    if status.as_deref() == Some("generating") {
        // Mid-run: queue the message on the active run's inbox; it shows in
        // the queued strip until the model picks it up (UX-011).
        let run_id = run.as_ref().map(|run| run.id.clone()).unwrap_or_default();
        spawn(async move {
            match locagens_core::api::runs::queue_message(&core(), &run_id, &prompt).await {
                Ok(()) => {
                    store.prompt.set(String::new());
                    store.queued.with_mut(|queue| queue.push(QueuedMessage { run_id, text: prompt.clone() }));
                }
                Err(err) => store.toast(store.tf("toast.queue_failed", &[&err.to_string()])),
            }
            busy.set(false);
        });
        return;
    }

    if let Some(run) = run {
        // Finished run → continue it.
        let run_id = run.id.clone();
        spawn(async move {
            let state = core();
            let payload = ContinueRunPayload {
                task: prompt.clone(),
                provider_id: Some(provider_id.clone()),
                model: Some(model.clone()),
                reasoning_effort: effort.clone(),
                mode: Some(mode.clone()),
                bypass_permissions: mode == "auto",
                ..Default::default()
            };
            match locagens_core::api::runs::continue_run(&state, &run_id, &payload).await {
                Ok(continued) => {
                    store.prompt.set(String::new());
                    store.active_run_id.set(Some(run_id.clone()));
                    store.turn_diff.set(None);
                    store.spawned_agents.set(Vec::new());
                    // Show the appended user message at once instead of
                    // waiting for the first streamed event.
                    let msgs = locagens_core::api::runs::list_messages(&state, &run_id).await.unwrap_or_default();
                    crate::state_ui::pin_sent_message(&msgs);
                    store.messages.set(msgs);
                    if let Ok(updated_run) = locagens_core::api::runs::get_run(&state, &run_id).await {
                        store.active_run.set(Some(updated_run));
                    }
                    store.attach_event_stream(run_id, continued.events);
                    store.refresh_runs();
                }
                Err(err) => store.toast(store.tf("toast.continue_failed", &[&err.to_string()])),
            }
            busy.set(false);
        });
        return;
    }

    // No active run → create one.
    spawn(async move {
        let state = core();
        let payload = CreateRunPayload {
            task: prompt.clone(),
            project_path: project_path.clone(),
            project_name: None,
            provider_id: provider_id.clone(),
            model: model.clone(),
            reasoning_effort: effort.clone(),
            mode: Some(mode.clone()),
            bypass_permissions: mode == "auto",
            system_prompt: None,
            append_system_prompt: None,
            auto_title: true,
            ..Default::default()
        };
        match locagens_core::api::runs::create_run(&state, &payload).await {
            Ok(started) => {
                let run_id = started.run.id.clone();
                store.reset_conversation();
                store.prompt.set(String::new());
                store.active_run_id.set(Some(run_id.clone()));
                store.active_run.set(Some(started.run));
                // Load the initial user message so it appears immediately.
                let initial_msgs = locagens_core::api::runs::list_messages(&state, &run_id).await.unwrap_or_default();
                crate::state_ui::pin_sent_message(&initial_msgs);
                store.messages.set(initial_msgs);
                store.attach_event_stream(run_id, started.events);
                store.refresh_runs();
            }
            Err(err) => store.toast(store.tf("toast.start_failed", &[&err.to_string()])),
        }
        busy.set(false);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_offsets_map_to_byte_offsets() {
        assert_eq!(utf16_to_byte("abc", 2), 2);
        assert_eq!(utf16_to_byte("éa", 1), 2);
        // An emoji is two UTF-16 units and four bytes.
        assert_eq!(utf16_to_byte("\u{1F600}x", 2), 4);
        assert_eq!(utf16_to_byte("ab", 99), 2);
    }

    #[test]
    fn splice_replaces_the_token_and_reports_the_caret() {
        let (text, caret) = splice("see @sr now", 4, 7, "@src/main.rs ");
        assert_eq!(text, "see @src/main.rs  now");
        assert_eq!(caret, 17);
        let (text, caret) = splice("é", 2, 2, "@x ");
        assert_eq!((text.as_str(), caret), ("é@x ", 4));
        let (text, _) = splice("/re", 0, 3, "/review ");
        assert_eq!(text, "/review ");
    }
}
