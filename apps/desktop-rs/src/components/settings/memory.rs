//! Memory tab: entries with create, edit and delete.

use crate::state_ui::{core, UiStore};
use crate::components::ConfirmButton;
use dioxus::prelude::*;
use locagens_core::db::Memory;

/// Memory entries with create, edit and delete.
#[component]
pub(super) fn MemoryTab() -> Element {
    let memories: Signal<Vec<Memory>> = use_signal(Vec::new);
    let mut new_memory = use_signal(String::new);
    let mut editing: Signal<Option<i64>> = use_signal(|| None);
    let mut edit_text = use_signal(String::new);
    let store: UiStore = use_context();

    use_effect(move || {
        let mut memories = memories;
        spawn(async move {
            let state = core();
            memories.set(locagens_core::api::memory::list_memories(&state).await.unwrap_or_default());
        });
    });

    let data = memories.read().clone();
    let editing_id = *editing.read();
    rsx! {
        div {
            class: "composer-row-top",
            input {
                placeholder: store.t("memory.placeholder"),
                value: "{new_memory.read()}",
                oninput: move |event| new_memory.set(event.value()),
                onkeydown: move |event: KeyboardEvent| {
                    if event.key() == Key::Enter {
                        let content = new_memory.read().trim().to_string();
                        if !content.is_empty() {
                            let mut memories = memories;
                            spawn(async move {
                                let state = core();
                                if let Err(err) = locagens_core::api::memory::create_memory(
                                    &state,
                                    locagens_core::api::memory::CreateMemoryInput {
                                        scope: Some("global".to_string()),
                                        category: None,
                                        content: Some(content),
                                        project_path: None,
                                    },
                                )
                                .await {
                                    store.toast(store.tf("toast.save_memory_failed", &[&err.to_string()]));
                                }
                                new_memory.set(String::new());
                                memories.set(locagens_core::api::memory::list_memories(&state).await.unwrap_or_default());
                            });
                        }
                    }
                },
            }
        }
        if data.is_empty() {
            div { class: "settings-empty", "{store.t(\"memory.none\")}" }
        }
        for memory in data.iter() {
            {
                let id = memory.id;
                let is_editing = editing_id == Some(id);
                let memory_content = memory.content.clone();
                rsx! {
                    div {
                        key: "{id}",
                        class: "settings-row",
                        if is_editing {
                            textarea {
                                class: "grow",
                                style: "min-height:60px;font-size:12.5px",
                                value: "{edit_text.read()}",
                                oninput: move |event| edit_text.set(event.value()),
                            }
                            button {
                                class: "primary",
                                onclick: move |_| {
                                    let mut memories = memories;
                                    let content = edit_text.read().trim().to_string();
                                    spawn(async move {
                                        let state = core();
                                        if let Err(err) =
                                            locagens_core::api::memory::update_memory(&state, id, &content).await
                                        {
                                            store.toast(store.tf("toast.update_memory_failed", &[&err.to_string()]));
                                        }
                                        editing.set(None);
                                        memories.set(locagens_core::api::memory::list_memories(&state).await.unwrap_or_default());
                                    });
                                },
                                "{store.t(\"common.save\")}"
                            }
                            button {
                                onclick: move |_| editing.set(None),
                                "{store.t(\"common.cancel\")}"
                            }
                        } else {
                            div {
                                class: "grow",
                                style: "white-space:pre-wrap",
                                "{memory.content}"
                            }
                            button {
                                onclick: move |_| {
                                    edit_text.set(memory_content.clone());
                                    editing.set(Some(id));
                                },
                                "{store.t(\"common.edit\")}"
                            }
                            ConfirmButton {
                                label: store.t("common.delete").to_string(),
                                on_confirm: move |_| {
                                    let mut memories = memories;
                                    spawn(async move {
                                        let state = core();
                                        if let Err(err) = locagens_core::api::memory::delete_memory(&state, id).await {
                                            store.toast(store.tf("toast.delete_memory_failed", &[&err.to_string()]));
                                        }
                                        memories.set(locagens_core::api::memory::list_memories(&state).await.unwrap_or_default());
                                    });
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}
