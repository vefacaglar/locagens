//! Models the user switched off in Settings → Providers.
//!
//! Stored as `provider/model` ids under the `"hiddenModels"` key of the app's
//! `settings.json`; every other key in that file is preserved on save. A
//! missing file, bad JSON or missing key means nothing is hidden.

use std::collections::BTreeSet;
use std::path::Path;

const SETTINGS_KEY: &str = "hiddenModels";

/// The `provider/model` id used in the hidden set.
pub fn model_key(provider_id: &str, model: &str) -> String {
    format!("{provider_id}/{model}")
}

pub fn load(path: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|root| root.get(SETTINGS_KEY).cloned())
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

/// Hides or shows one `provider/model` id and writes the settings file.
pub fn set_hidden(path: &Path, key: &str, hidden: bool) -> std::io::Result<()> {
    let mut set = load(path);
    if hidden {
        set.insert(key.to_string());
    } else {
        set.remove(key);
    }
    let mut root = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    root[SETTINGS_KEY] = serde_json::to_value(&set).map_err(std::io::Error::other)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(&root).map_err(std::io::Error::other)?;
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_set_round_trips_and_keeps_other_keys() {
        let dir = std::env::temp_dir().join(format!("lcgn-hidden-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        assert!(load(&path).is_empty());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, r#"{"appearance":{"theme":"light"}}"#).unwrap();

        let key = model_key("opencode-go", "glm-5");
        set_hidden(&path, &key, true).unwrap();
        set_hidden(&path, &model_key("xai", "grok"), true).unwrap();
        assert!(load(&path).contains(&key));
        set_hidden(&path, &key, false).unwrap();
        let loaded = load(&path);
        assert!(!loaded.contains(&key) && loaded.len() == 1);

        let root: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(root["appearance"]["theme"], "light");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
