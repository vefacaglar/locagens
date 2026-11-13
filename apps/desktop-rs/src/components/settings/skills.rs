//! Skills tab: list, create / edit `SKILL.md`, delete.

use crate::state_ui::{core, UiStore};
use dioxus::prelude::*;
use locagens_core::services::skills::SkillSummary;

const NEW_SKILL_TEMPLATE: &str = "---\nname: my-skill\ndescription: One-line description shown to the agent.\n---\n\n# My Skill\n\nInstructions the agent follows when this skill is loaded.\n";

/// State of the SKILL.md editor card (`name == None` means "creating new").
#[derive(Clone, Debug)]
struct SkillEditor {
    name: Option<String>,
    target: String, // "user" | "project"
    content: String,
}


/// Skills: list, create / edit `SKILL.md` and delete (user + project scope).
#[component]
pub(super) fn SkillsTab() -> Element {
    let store: UiStore = use_context();
    let skills: Signal<Vec<SkillSummary>> = use_signal(Vec::new);
    let mut editor: Signal<Option<SkillEditor>> = use_signal(|| None);
    let project_filter = store.selected_project.read().clone();

    use_effect(move || {
        let mut skills = skills;
        let project = project_filter.clone();
        spawn(async move {
            let state = core();
            if let Ok(response) = locagens_core::api::skills::list_skills(&state, project.as_deref()).await {
                skills.set(response.skills);
            }
        });
    });

    let data = skills.read().clone();
    let editor_state = editor.read().clone();
    let selected_project = store.selected_project.read().clone();
    let project_label = selected_project
        .clone()
        .unwrap_or_else(|| store.t("skills.no_project").to_string());

    rsx! {
        div {
            class: "panel-section-title", "{store.t(\"tab.skills\")}"
            span { style: "flex:1" }
            button {
                class: "subtle",
                onclick: move |_| {
                    editor.set(Some(SkillEditor {
                        name: None,
                        target: "user".to_string(),
                        content: NEW_SKILL_TEMPLATE.to_string(),
                    }));
                },
                "+ {store.t(\"skills.new\")}"
            }
        }
        if let Some(edit_state) = editor_state {
            div {
                class: "panel-card",
                div {
                    class: "panel-section-title",
                    if let Some(name) = &edit_state.name { "{store.t(\"skills.edit\")} · {name}" } else { "{store.t(\"skills.new_md\")}" }
                }
                div {
                    class: "composer-row-top",
                    select {
                        value: "{edit_state.target}",
                        onchange: move |event| {
                            let value = event.value();
                            editor.with_mut(|option| {
                                if let Some(state) = option {
                                    state.target = value;
                                }
                            });
                        },
                        option { value: "user", "{store.t(\"skills.user_scope\")}" }
                        option { value: "project", "{store.t(\"skills.project_scope\")}" }
                    }
                    if edit_state.target == "project" {
                        span {
                            class: "mono",
                            style: "font-size:11px",
                            "→ {project_label}"
                        }
                    }
                }
                textarea {
                    style: "width:100%;min-height:180px;font-size:12px",
                    value: "{edit_state.content}",
                    oninput: move |event| {
                        let value = event.value();
                        editor.with_mut(|option| {
                            if let Some(state) = option {
                                state.content = value;
                            }
                        });
                    },
                }
                div {
                    style: "display:flex;gap:8px",
                    button {
                        class: "primary",
                        onclick: move |_| {
                            let mut skills = skills;
                            let Some(edit_state) = editor.read().clone() else { return };
                            let project = store.selected_project.read().clone();
                            spawn(async move {
                                let state = core();
                                if edit_state.target == "project" && project.is_none() {
                                    store.toast(store.t("skills.need_project").to_string());
                                    return;
                                }
                                let result = locagens_core::api::skills::install_skill(
                                    &state,
                                    locagens_core::api::skills::InstallSkillInput {
                                        target: Some(edit_state.target.clone()),
                                        content: Some(edit_state.content.clone()),
                                        project_path: project.clone(),
                                    },
                                )
                                .await;
                                match result {
                                    Ok(skill) => {
                                        store.toast(store.tf("toast.skill_saved", &[&skill.name]));
                                        editor.set(None);
                                    }
                                    Err(err) => store.toast(store.tf("toast.save_skill_failed", &[&err.to_string()])),
                                }
                                if let Ok(response) = locagens_core::api::skills::list_skills(&state, project.as_deref()).await {
                                    skills.set(response.skills);
                                }
                            });
                        },
                        "{store.t(\"common.save\")}"
                    }
                    button {
                        onclick: move |_| editor.set(None),
                        "{store.t(\"common.cancel\")}"
                    }
                }
            }
        }
        if data.is_empty() {
            div { class: "settings-empty", "{store.t(\"skills.none\")}" }
        }
        for skill in data.iter() {
            {
                let name = skill.name.clone();
                let source = skill.source.clone();
                let description = skill.description.clone();
                let body = skill.body.clone().unwrap_or_default();
                let edit_name = name.clone();
                let edit_source = source.clone();
                let edit_body = body.clone();
                let delete_name = name.clone();
                let delete_source = source.clone();
                rsx! {
                    div {
                        key: "{name}",
                        class: "settings-row",
                        div {
                            class: "grow",
                            div {
                                style: "font-weight:600",
                                "{name} "
                                span { class: "badge", "{source}" }
                            }
                            div {
                                style: "font-size:11.5px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap",
                                "{description}"
                            }
                        }
                        button {
                            onclick: move |_| {
                                editor.set(Some(SkillEditor {
                                    name: Some(edit_name.clone()),
                                    target: edit_source.clone(),
                                    content: edit_body.clone(),
                                }));
                            },
                            "{store.t(\"common.edit\")}"
                        }
                        button {
                            class: "danger",
                            onclick: move |_| {
                                let mut skills = skills;
                                let delete_name = delete_name.clone();
                                let target = delete_source.clone();
                                let project = store.selected_project.read().clone();
                                spawn(async move {
                                    let state = core();
                                    if let Err(err) = locagens_core::api::skills::delete_skill(
                                        &state,
                                        &delete_name,
                                        Some(target.as_str()),
                                        project.as_deref(),
                                    )
                                    .await {
                                        store.toast(store.tf("toast.delete_skill_failed", &[&err.to_string()]));
                                    }
                                    if let Ok(response) = locagens_core::api::skills::list_skills(&state, project.as_deref()).await {
                                        skills.set(response.skills);
                                    }
                                });
                            },
                            "{store.t(\"common.delete\")}"
                        }
                    }
                }
            }
        }
    }
}
