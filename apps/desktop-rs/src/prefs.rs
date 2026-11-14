//! Appearance preferences (language, theme, fonts, size) and their
//! persistence.
//!
//! Stored under the `"appearance"` key of the app's existing `settings.json`
//! (`locagens_core::paths::default_settings_path()`); every other key in that
//! file is preserved on save. Missing keys (old settings files) fall back to
//! the defaults.

use crate::i18n::Lang;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Key inside `settings.json` that holds the appearance object.
const SETTINGS_KEY: &str = "appearance";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
    System,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiFont {
    #[default]
    System,
    Inter,
    Geist,
    CascadiaCode,
    Custom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeFont {
    #[default]
    CascadiaCode,
    JetBrainsMono,
    Consolas,
    Custom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FontSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl FontSize {
    pub fn factor(self) -> f32 {
        match self {
            FontSize::Small => 0.92,
            FontSize::Medium => 1.0,
            FontSize::Large => 1.1,
        }
    }
}

/// Everything the Appearance tab controls.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub language: Lang,
    pub theme: ThemeChoice,
    pub ui_font: UiFont,
    pub ui_font_custom: String,
    pub code_font: CodeFont,
    pub code_font_custom: String,
    pub code_ligatures: bool,
    pub font_size: FontSize,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            language: Lang::En,
            theme: ThemeChoice::Dark,
            ui_font: UiFont::System,
            ui_font_custom: String::new(),
            code_font: CodeFont::CascadiaCode,
            code_font_custom: String::new(),
            code_ligatures: true,
            font_size: FontSize::Medium,
        }
    }
}

/// Reads the appearance object out of a settings file; any problem (missing
/// file, bad JSON, no `appearance` key) yields the defaults.
pub fn load_from(path: &Path) -> Appearance {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|root| root.get(SETTINGS_KEY).cloned())
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

/// Writes the appearance object into the settings file, keeping every other
/// key that is already there.
pub fn save_to(path: &Path, appearance: &Appearance) -> std::io::Result<()> {
    let mut root = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    root[SETTINGS_KEY] = serde_json::to_value(appearance).map_err(std::io::Error::other)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(&root).map_err(std::io::Error::other)?;
    std::fs::write(path, text)
}

/// Default and allowed widths (px) of the resizable side columns.
pub const SIDEBAR_DEFAULT: i32 = 264;
pub const SIDEBAR_RANGE: (i32, i32) = (200, 480);
pub const PANEL_DEFAULT: i32 = 380;
pub const PANEL_RANGE: (i32, i32) = (260, 640);

/// Column widths, stored under the `"layout"` key of `settings.json`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub sidebar_width: i32,
    pub right_panel_width: i32,
}

impl Default for Layout {
    fn default() -> Self {
        Self { sidebar_width: SIDEBAR_DEFAULT, right_panel_width: PANEL_DEFAULT }
    }
}

impl Layout {
    /// The same layout with both widths clamped to their ranges.
    pub fn clamped(self) -> Self {
        Self {
            sidebar_width: self.sidebar_width.clamp(SIDEBAR_RANGE.0, SIDEBAR_RANGE.1),
            right_panel_width: self.right_panel_width.clamp(PANEL_RANGE.0, PANEL_RANGE.1),
        }
    }
}

pub fn load_layout_from(path: &Path) -> Layout {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|root| root.get("layout").cloned())
        .and_then(|value| serde_json::from_value::<Layout>(value).ok())
        .unwrap_or_default()
        .clamped()
}

/// Writes the layout object, keeping every other key in the file.
pub fn save_layout_to(path: &Path, layout: &Layout) -> std::io::Result<()> {
    let mut root = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    root["layout"] = serde_json::to_value(layout.clamped()).map_err(std::io::Error::other)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(&root).map_err(std::io::Error::other)?;
    std::fs::write(path, text)
}

pub fn load_layout() -> Layout {
    load_layout_from(&locagens_core::paths::default_settings_path())
}

pub fn save_layout(layout: &Layout) {
    let _ = save_layout_to(&locagens_core::paths::default_settings_path(), layout);
}

/// Loads from the app's settings file.
pub fn load() -> Appearance {
    load_from(&locagens_core::paths::default_settings_path())
}

/// Saves to the app's settings file; failures are ignored (preferences are
/// not worth interrupting the user for).
pub fn save(appearance: &Appearance) {
    let _ = save_to(&locagens_core::paths::default_settings_path(), appearance);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("locagens-prefs-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
    }

    #[test]
    fn layout_round_trips_clamps_and_keeps_other_keys() {
        let path = temp_file("layout");
        assert_eq!(load_layout_from(&path), Layout::default());
        std::fs::write(&path, r#"{"appearance": {"language": "tr"}}"#).unwrap();
        save_layout_to(&path, &Layout { sidebar_width: 300, right_panel_width: 9999 }).unwrap();
        assert_eq!(load_layout_from(&path), Layout { sidebar_width: 300, right_panel_width: 640 });
        assert_eq!(load_from(&path).language, Lang::Tr);
    }

    #[test]
    fn defaults_match_the_spec() {
        let a = Appearance::default();
        assert_eq!(a.language, Lang::En);
        assert_eq!(a.theme, ThemeChoice::Dark);
        assert_eq!(a.ui_font, UiFont::System);
        assert_eq!(a.code_font, CodeFont::CascadiaCode);
        assert!(a.code_ligatures);
        assert_eq!(a.font_size, FontSize::Medium);
    }

    #[test]
    fn old_settings_files_fall_back_to_defaults() {
        let path = temp_file("old");
        std::fs::write(&path, r#"{"someOtherKey": 1}"#).unwrap();
        assert_eq!(load_from(&path), Appearance::default());
        // Partial object: only the provided field changes.
        std::fs::write(&path, r#"{"appearance": {"language": "tr"}}"#).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.language, Lang::Tr);
        assert_eq!(loaded.ui_font, UiFont::System);
        // Garbage and missing files.
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load_from(&path), Appearance::default());
        assert_eq!(load_from(&path.with_file_name("missing.json")), Appearance::default());
    }

    #[test]
    fn roundtrip_preserves_other_keys() {
        let path = temp_file("roundtrip");
        std::fs::write(&path, r#"{"keep": {"a": 1}}"#).unwrap();
        let chosen = Appearance {
            language: Lang::Tr,
            theme: ThemeChoice::System,
            ui_font: UiFont::CascadiaCode,
            code_font: CodeFont::Custom,
            code_font_custom: "Fira Code".into(),
            code_ligatures: false,
            font_size: FontSize::Large,
            ..Appearance::default()
        };
        save_to(&path, &chosen).unwrap();
        assert_eq!(load_from(&path), chosen);
        let root: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["keep"]["a"], 1);
    }
}
