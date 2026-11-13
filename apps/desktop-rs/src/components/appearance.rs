//! Settings > Appearance: language, theme, interface and code fonts,
//! ligatures and font size. Every change is applied live through
//! `UiStore::set_appearance`, which also persists it.

use crate::i18n::Lang;
use crate::prefs::{Appearance, CodeFont, FontSize, ThemeChoice, UiFont};
use crate::state_ui::UiStore;
use dioxus::prelude::*;

#[component]
pub fn AppearanceTab() -> Element {
    let store: UiStore = use_context();
    let current = store.appearance.read().clone();
    let t = |key| store.t(key);
    let update = move |change: &dyn Fn(&mut Appearance)| {
        let mut next = store.appearance.read().clone();
        change(&mut next);
        store.set_appearance(next);
    };

    let language = segmented(
        &Lang::ALL.map(|lang| (lang, lang.native_name())),
        current.language,
        move |lang| update(&|a| a.language = lang),
    );
    let theme = segmented(
        &[
            (ThemeChoice::Dark, t("appearance.theme.dark")),
            (ThemeChoice::Light, t("appearance.theme.light")),
            (ThemeChoice::System, t("appearance.theme.system")),
        ],
        current.theme,
        move |theme| update(&|a| a.theme = theme),
    );
    let ui_font = segmented(
        &[
            (UiFont::System, t("appearance.font.system")),
            (UiFont::Inter, "Inter"),
            (UiFont::Geist, "Geist"),
            (UiFont::CascadiaCode, "Cascadia Code"),
            (UiFont::Custom, t("appearance.font.custom")),
        ],
        current.ui_font,
        move |font| update(&|a| a.ui_font = font),
    );
    let code_font = segmented(
        &[
            (CodeFont::CascadiaCode, "Cascadia Code"),
            (CodeFont::JetBrainsMono, "JetBrains Mono"),
            (CodeFont::Consolas, "Consolas"),
            (CodeFont::Custom, t("appearance.font.custom")),
        ],
        current.code_font,
        move |font| update(&|a| a.code_font = font),
    );
    let ligatures = segmented(
        &[(true, t("appearance.on")), (false, t("appearance.off"))],
        current.code_ligatures,
        move |on| update(&|a| a.code_ligatures = on),
    );
    let size = segmented(
        &[
            (FontSize::Small, t("appearance.size.small")),
            (FontSize::Medium, t("appearance.size.medium")),
            (FontSize::Large, t("appearance.size.large")),
        ],
        current.font_size,
        move |size| update(&|a| a.font_size = size),
    );

    rsx! {
        div { class: "panel-section-title", "{t(\"appearance.title\")}" }
        {row(t("appearance.language"), t("appearance.language_desc"), language)}
        {row(t("appearance.theme"), t("appearance.theme_desc"), theme)}
        {row(
            t("appearance.ui_font"),
            t("appearance.ui_font_desc"),
            rsx! {
                {ui_font}
                if current.ui_font == UiFont::Custom {
                    input {
                        r#type: "text",
                        placeholder: t("appearance.font_placeholder"),
                        value: "{current.ui_font_custom}",
                        oninput: move |event| {
                            let value = event.value();
                            update(&|a| a.ui_font_custom = value.clone());
                        },
                    }
                }
            },
        )}
        div {
            class: "appearance-preview-row",
            div { class: "font-preview", style: "font-family: var(--font-ui)", "{t(\"appearance.preview_ui\")}" }
        }
        {row(
            t("appearance.code_font"),
            t("appearance.code_font_desc"),
            rsx! {
                {code_font}
                if current.code_font == CodeFont::Custom {
                    input {
                        r#type: "text",
                        placeholder: t("appearance.font_placeholder"),
                        value: "{current.code_font_custom}",
                        oninput: move |event| {
                            let value = event.value();
                            update(&|a| a.code_font_custom = value.clone());
                        },
                    }
                }
            },
        )}
        {row(t("appearance.ligatures"), t("appearance.ligatures_desc"), ligatures)}
        div {
            class: "appearance-preview-row",
            div { class: "font-preview mono", "{t(\"appearance.preview_code\")}" }
        }
        {row(t("appearance.size"), t("appearance.size_desc"), size)}
    }
}

/// One option row: label + muted description on the left, controls right.
fn row(label: &'static str, description: &'static str, controls: Element) -> Element {
    rsx! {
        div {
            class: "appearance-row",
            div {
                div { class: "ar-label", "{label}" }
                div { class: "ar-desc", "{description}" }
            }
            div { class: "ar-controls", {controls} }
        }
    }
}

/// Segmented control: one button per option, the current one highlighted.
fn segmented<T: Copy + PartialEq + 'static>(
    options: &[(T, &'static str)],
    current: T,
    on_pick: impl Fn(T) + Clone + 'static,
) -> Element {
    rsx! {
        div {
            class: "seg",
            for (index, (value, label)) in options.iter().copied().enumerate() {
                {
                    let on_pick = on_pick.clone();
                    rsx! {
                        button {
                            key: "{index}",
                            class: if value == current { "active" } else { "" },
                            onclick: move |_| on_pick(value),
                            "{label}"
                        }
                    }
                }
            }
        }
    }
}
