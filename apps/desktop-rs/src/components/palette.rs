//! Command palette (`Ctrl+K`) and the shortcuts sheet (`Ctrl+/`, `/help`)
//! of the keyboard layer (UX-019).
//!
//! The palette is one search field over three groups: chats (title and first
//! prompt, all projects), the shared command table and the settings tabs.

use crate::state_ui::{AppPage, Overlay, SettingsTab, UiStore};
use dioxus::prelude::*;
use locagens_core::commands::{commands_for, Frontend};

/// One palette entry.
#[derive(Clone, Debug, PartialEq)]
pub enum PaletteItem {
    Chat { run_id: String, title: String, detail: String },
    Command { name: &'static str, usage: String, takes_args: bool },
    Settings(SettingsTab),
}

/// Case-insensitive match of every whitespace-separated word of `query`.
pub fn matches_query(haystack: &str, query: &str) -> bool {
    let haystack = haystack.to_lowercase();
    query.split_whitespace().all(|word| haystack.contains(&word.to_lowercase()))
}

/// The entries matching `query`: chats first (at most 20), then commands
/// and settings tabs (left out when `chats_only`).
fn palette_items(store: &UiStore, query: &str, chats_only: bool) -> Vec<PaletteItem> {
    let mut items: Vec<PaletteItem> = store
        .runs
        .read()
        .iter()
        .filter(|run| matches_query(&format!("{} {} {}", run.title, run.task, run.project_name), query))
        .take(20)
        .map(|run| PaletteItem::Chat {
            run_id: run.id.clone(),
            title: run.title.clone(),
            detail: run.project_name.clone(),
        })
        .collect();
    if chats_only {
        return items;
    }
    let lang = store.lang();
    items.extend(
        commands_for(Frontend::Desktop)
            .filter(|spec| matches_query(&format!("/{} {}", spec.name, crate::i18n::command_description(lang, spec)), query))
            .map(|spec| PaletteItem::Command { name: spec.name, usage: spec.usage(), takes_args: !spec.args.is_empty() && spec.args.starts_with('<') }),
    );
    items.extend(
        SettingsTab::ALL
            .into_iter()
            .filter(|tab| matches_query(&format!("{} {}", store.t("settings.title"), store.t(tab.label_key())), query))
            .map(PaletteItem::Settings),
    );
    items
}

/// Runs a palette entry.
fn run_item(store: UiStore, item: PaletteItem) {
    let mut store = store;
    store.overlay.set(Overlay::None);
    match item {
        PaletteItem::Chat { run_id, .. } => {
            store.page.set(AppPage::Chat);
            store.select_run(run_id);
            crate::state_ui::focus_composer();
        }
        PaletteItem::Command { name, takes_args, .. } => {
            store.page.set(AppPage::Chat);
            if takes_args {
                // `/thinking <level>` needs its argument: type the rest.
                store.prompt.set(format!("/{name} "));
                crate::state_ui::focus_composer();
            } else {
                super::composer::run_command_line(store, &format!("/{name}"));
            }
        }
        PaletteItem::Settings(tab) => store.page.set(AppPage::Settings(tab)),
    }
}

#[component]
pub fn Overlays() -> Element {
    let store: UiStore = use_context();
    let overlay = *store.overlay.read();
    match overlay {
        Overlay::None => rsx! {},
        Overlay::Palette { chats_only } => rsx! { Palette { chats_only } },
        Overlay::Shortcuts => rsx! { ShortcutsSheet {} },
    }
}

#[component]
fn Palette(chats_only: bool) -> Element {
    let mut store: UiStore = use_context();
    let mut query = use_signal(String::new);
    let mut highlighted = use_signal(|| 0usize);
    let items = palette_items(&store, &query.read(), chats_only);
    let index = (*highlighted.read()).min(items.len().saturating_sub(1));
    let items_for_keys = items.clone();
    let placeholder = if chats_only { store.t("palette.search_chats") } else { store.t("palette.search") };

    rsx! {
        div {
            class: "modal-backdrop palette-backdrop",
            onclick: move |_| store.overlay.set(Overlay::None),
            div {
                class: "modal palette",
                role: "dialog",
                "aria-modal": "true",
                "aria-label": store.t("palette.title"),
                onclick: move |event| event.stop_propagation(),
                input {
                    class: "palette-search",
                    r#type: "text",
                    placeholder: placeholder,
                    "aria-label": placeholder,
                    role: "combobox",
                    "aria-controls": "palette-list",
                    value: "{query.read()}",
                    onmounted: move |event| async move {
                        let _ = event.set_focus(true).await;
                    },
                    oninput: move |event| {
                        query.set(event.value());
                        highlighted.set(0);
                    },
                    onkeydown: move |event: KeyboardEvent| {
                        let last = items_for_keys.len().saturating_sub(1);
                        match event.key() {
                            Key::ArrowDown => {
                                event.prevent_default();
                                let next = (*highlighted.peek() + 1).min(last);
                                highlighted.set(next);
                            }
                            Key::ArrowUp => {
                                event.prevent_default();
                                let next = highlighted.peek().saturating_sub(1);
                                highlighted.set(next);
                            }
                            Key::Enter => {
                                event.prevent_default();
                                if let Some(item) = items_for_keys.get((*highlighted.peek()).min(last)).cloned() {
                                    run_item(store, item);
                                }
                            }
                            _ => {}
                        }
                    },
                }
                div {
                    class: "palette-list",
                    id: "palette-list",
                    role: "listbox",
                    if items.is_empty() {
                        div { class: "ccm-empty", "{store.t(\"palette.none\")}" }
                    }
                    for (position, item) in items.iter().enumerate() {
                        {
                            let item_run = item.clone();
                            let (group, label, detail) = match item {
                                PaletteItem::Chat { title, detail, .. } => (store.t("palette.chat"), title.clone(), detail.clone()),
                                PaletteItem::Command { name, usage, .. } => {
                                    let spec = locagens_core::commands::find(name);
                                    let description = spec.map(|spec| crate::i18n::command_description(store.lang(), spec)).unwrap_or_default();
                                    (store.t("palette.command"), usage.clone(), description.to_string())
                                }
                                PaletteItem::Settings(tab) => (store.t("palette.settings"), store.t(tab.label_key()).to_string(), String::new()),
                            };
                            rsx! {
                                button {
                                    key: "{position}",
                                    class: if position == index { "palette-item active" } else { "palette-item" },
                                    role: "option",
                                    "aria-selected": "{position == index}",
                                    onmouseenter: move |_| highlighted.set(position),
                                    onclick: move |_| run_item(store, item_run.clone()),
                                    span { class: "palette-group", "{group}" }
                                    span { class: "palette-label", "{label}" }
                                    span { class: "palette-detail", "{detail}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// `(keys, i18n key of the action)` rows of the shortcuts sheet.
fn shortcut_rows() -> Vec<(String, &'static str)> {
    use super::shortcut_label as key;
    let mac = cfg!(target_os = "macos");
    vec![
        ("Esc".to_string(), "keys.escape"),
        (key("N"), "keys.new_chat"),
        (key("K"), "keys.palette"),
        (key("L"), "keys.focus"),
        (if mac { "\u{2318}\u{21e7}M".to_string() } else { key("M") }, "keys.model"),
        (key("B"), "keys.sidebar"),
        (if mac { "\u{2318}\u{2325}B".to_string() } else { "Ctrl+Alt+B".to_string() }, "keys.inspector"),
        (key("."), "keys.mode"),
        (key(","), "keys.settings"),
        (key("/"), "keys.help"),
        ("\u{2191}".to_string(), "keys.recall"),
    ]
}

#[component]
fn ShortcutsSheet() -> Element {
    let mut store: UiStore = use_context();
    let lang = store.lang();
    rsx! {
        div {
            class: "modal-backdrop",
            onclick: move |_| store.overlay.set(Overlay::None),
            div {
                class: "modal shortcuts-sheet",
                role: "dialog",
                "aria-modal": "true",
                "aria-labelledby": "shortcuts-title",
                onclick: move |event| event.stop_propagation(),
                h3 { id: "shortcuts-title", "{store.t(\"keys.title\")}" }
                div {
                    class: "shortcuts-grid",
                    for (keys, action) in shortcut_rows() {
                        kbd { key: "{action}-k", "{keys}" }
                        span { "{store.t(action)}" }
                    }
                }
                h3 { "{store.t(\"composer.commands\")}" }
                div {
                    class: "shortcuts-grid",
                    for spec in commands_for(Frontend::Desktop) {
                        code { key: "{spec.name}-k", "{spec.usage()}" }
                        span { "{crate::i18n::command_description(lang, spec)}" }
                    }
                }
                div {
                    class: "modal-actions",
                    button {
                        class: "primary",
                        onmounted: move |event| async move {
                            let _ = event.set_focus(true).await;
                        },
                        onclick: move |_| store.overlay.set(Overlay::None),
                        "{store.t(\"common.close\")}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_words_must_all_match() {
        assert!(matches_query("Fix the parser · locagens", "parser fix"));
        assert!(matches_query("anything", ""));
        assert!(!matches_query("Fix the parser", "lexer"));
        assert!(matches_query("/review code review", "REV"));
    }
}
