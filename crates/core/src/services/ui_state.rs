//! Last-used model, reasoning effort and mode, shared by the desktop app and
//! the TUI so both reopen on what the user picked last (UX-003).
//!
//! File: `~/.locagens/ui_state.json` (`$LOCAGENS_HOME`), written whenever the
//! user picks a model, an effort or a mode in either frontend:
//!
//! ```json
//! {
//!   "model": "openai/gpt-5.1",
//!   "effort": "high",
//!   "mode": "accept_edits",
//!   "recentModels": ["openai/gpt-5.1", "anthropic/claude-sonnet-4-5"]
//! }
//! ```
//!
//! - `model`: `provider/model`; the provider id never contains `/`, the model
//!   id may (`openrouter/meta/llama`), so it splits at the first `/`.
//! - `effort`: a reasoning-effort level (`minimal` … `max`), or absent.
//! - `mode`: `chat`, `plan`, `accept_edits`, `ask_permissions` or `auto`;
//!   any other value is dropped on load.
//! - `recentModels`: newest first, at most [`MAX_RECENT`] entries.
//!
//! Every key is optional; a missing file, bad JSON or a wrong-typed key reads
//! as "nothing remembered" for that key. Writes are atomic (temp file plus
//! rename) and keep unknown keys, so a newer frontend's fields survive.
//!
//! Precedence at start (applied by the frontends): command-line flag, then
//! the opened session's model, then `config.toml`, then this file.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Models kept in `recentModels`.
pub const MAX_RECENT: usize = 5;

/// Modes the file may name.
const MODES: [&str; 5] = ["chat", "plan", "accept_edits", "ask_permissions", "auto"];

/// What the frontends remember between launches.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_models: Vec<String>,
}

impl UiState {
    /// `(provider_id, model_id)` of the remembered model.
    pub fn provider_and_model(&self) -> Option<(&str, &str)> {
        split_model_key(self.model.as_deref()?)
    }
}

/// `provider/model` → `(provider, model)`; `None` when either side is empty.
pub fn split_model_key(key: &str) -> Option<(&str, &str)> {
    let (provider, model) = key.split_once('/')?;
    (!provider.is_empty() && !model.is_empty()).then_some((provider, model))
}

/// `~/.locagens/ui_state.json`.
pub fn default_path() -> PathBuf {
    crate::paths::locagens_home().join("ui_state.json")
}

/// Reads the file; each key that is missing or malformed is left unset.
pub fn load(path: &Path) -> UiState {
    let Some(root) = read_object(path) else { return UiState::default() };
    let text = |key: &str| root.get(key).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
    UiState {
        model: text("model").filter(|key| split_model_key(key).is_some()),
        effort: text("effort"),
        mode: text("mode").filter(|mode| MODES.contains(&mode.as_str())),
        recent_models: root
            .get("recentModels")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .filter(|key| split_model_key(key).is_some())
                    .take(MAX_RECENT)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Writes `state`, keeping keys this version does not know.
pub fn save(path: &Path, state: &UiState) -> std::io::Result<()> {
    let mut root = read_object(path).unwrap_or_default();
    let fresh = serde_json::to_value(state).map_err(std::io::Error::other)?;
    for key in ["model", "effort", "mode", "recentModels"] {
        root.remove(key);
    }
    if let Value::Object(fields) = fresh {
        root.extend(fields);
    }
    let text = serde_json::to_string_pretty(&Value::Object(root)).map_err(std::io::Error::other)?;
    write_atomic(path, &text)
}

/// Records a model pick (and its effort) and moves it to the front of the
/// recent list.
pub fn remember_model(path: &Path, provider_id: &str, model: &str, effort: Option<&str>) -> std::io::Result<()> {
    let key = format!("{provider_id}/{model}");
    if split_model_key(&key).is_none() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "provider and model are required"));
    }
    let mut state = load(path);
    state.recent_models.retain(|existing| existing != &key);
    state.recent_models.insert(0, key.clone());
    state.recent_models.truncate(MAX_RECENT);
    state.model = Some(key);
    if let Some(effort) = effort {
        state.effort = Some(effort.to_string()).filter(|e| !e.is_empty());
    }
    save(path, &state)
}

/// Records an effort pick on its own.
pub fn remember_effort(path: &Path, effort: &str) -> std::io::Result<()> {
    let mut state = load(path);
    state.effort = Some(effort.trim().to_string()).filter(|e| !e.is_empty());
    save(path, &state)
}

/// Records a mode pick; unknown modes are refused.
pub fn remember_mode(path: &Path, mode: &str) -> std::io::Result<()> {
    if !MODES.contains(&mode) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "unknown mode"));
    }
    let mut state = load(path);
    state.mode = Some(mode.to_string());
    save(path, &state)
}

fn read_object(path: &Path) -> Option<serde_json::Map<String, Value>> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<Value>(&text).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

/// Writes through a sibling temp file and a rename, so a crash never leaves
/// a half-written file behind.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lcgn-ui-state-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("ui_state.json")
    }

    #[test]
    fn round_trips_model_effort_mode_and_recent_list() {
        let path = temp_path("round-trip");
        assert_eq!(load(&path), UiState::default());
        remember_model(&path, "openai", "gpt-5", Some("high")).unwrap();
        remember_model(&path, "openrouter", "meta/llama-4", None).unwrap();
        remember_mode(&path, "plan").unwrap();
        let state = load(&path);
        assert_eq!(state.provider_and_model(), Some(("openrouter", "meta/llama-4")));
        assert_eq!(state.effort.as_deref(), Some("high"), "a pick without effort keeps the last effort");
        assert_eq!(state.mode.as_deref(), Some("plan"));
        assert_eq!(state.recent_models, vec!["openrouter/meta/llama-4", "openai/gpt-5"]);

        // Picking a model again moves it to the front without duplicates.
        remember_model(&path, "openai", "gpt-5", Some("low")).unwrap();
        let state = load(&path);
        assert_eq!(state.recent_models, vec!["openai/gpt-5", "openrouter/meta/llama-4"]);
        assert_eq!(state.effort.as_deref(), Some("low"));
        remember_effort(&path, "max").unwrap();
        assert_eq!(load(&path).effort.as_deref(), Some("max"));
    }

    #[test]
    fn recent_list_is_capped() {
        let path = temp_path("cap");
        for index in 0..8 {
            remember_model(&path, "p", &format!("m{index}"), None).unwrap();
        }
        let recent = load(&path).recent_models;
        assert_eq!(recent.len(), MAX_RECENT);
        assert_eq!(recent[0], "p/m7");
    }

    #[test]
    fn bad_values_are_dropped_per_key_and_unknown_keys_survive() {
        let path = temp_path("tolerant");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"model":"no-slash","effort":7,"mode":"turbo","recentModels":["a/b",3,"/x"],"future":{"k":1}}"#).unwrap();
        let state = load(&path);
        assert_eq!(state.model, None);
        assert_eq!(state.effort, None);
        assert_eq!(state.mode, None);
        assert_eq!(state.recent_models, vec!["a/b"]);
        assert!(remember_mode(&path, "turbo").is_err());
        remember_mode(&path, "auto").unwrap();
        let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["future"]["k"], 1);
        assert_eq!(root["mode"], "auto");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load(&path), UiState::default());
    }
}
