//! Plugin discovery / install / toggle / delete — port of `PluginRegistry.ts`.
//!
//! A plugin is a directory containing `plugin.json` (the manifest). User-level
//! plugins live in the app-support `plugins` folder; project plugins live in
//! `<project>/.locagens/plugins` and override same-id user plugins.

use crate::paths;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

pub const MANIFEST_FILENAME: &str = "plugin.json";
pub const BUILTIN_TEMPLATES: &str = include_str!("plugins_templates.json");

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PluginsResponse {
    pub plugins: Vec<Value>,
    pub templates: Vec<Value>,
    pub user_plugins_dir: String,
    pub project_plugins_dir: Option<String>,
}

/// User-level plugins root.
pub fn user_root() -> PathBuf {
    paths::plugins_root()
}

/// Project plugins root (`.locagens/plugins`), or None when unset/inaccessible.
pub fn project_root(project_path: Option<&str>) -> Option<PathBuf> {
    let trimmed = project_path?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let base = std::fs::canonicalize(trimmed).ok()?;
    Some(base.join(".locagens").join("plugins"))
}

pub fn ensure_user_root() -> PathBuf {
    let root = user_root();
    let _ = std::fs::create_dir_all(&root);
    crate::util::normalize_canonical(std::fs::canonicalize(&root).unwrap_or(root))
}

pub fn ensure_project_root(project_path: &str) -> Option<PathBuf> {
    let root = project_root(Some(project_path))?;
    let _ = std::fs::create_dir_all(&root);
    std::fs::canonicalize(&root).ok().map(crate::util::normalize_canonical)
}

fn read_manifest(plugin_dir: &Path) -> Option<Map<String, Value>> {
    let raw = std::fs::read_to_string(plugin_dir.join(MANIFEST_FILENAME)).ok()?;
    let parsed: Value = serde_json::from_str(&raw).ok()?;
    let obj = parsed.as_object()?.clone();
    let id = obj.get("id").and_then(Value::as_str)?;
    let name = obj.get("name").and_then(Value::as_str)?;
    if id.is_empty() || name.is_empty() {
        return None;
    }
    Some(obj)
}

fn manifest_to_response(mut manifest: Map<String, Value>, scope: &str, dir: &Path) -> Value {
    manifest.insert("scope".into(), Value::String(scope.to_string()));
    if !manifest.contains_key("enabled") {
        manifest.insert("enabled".into(), Value::Bool(true));
    }
    manifest.insert("dir".into(), Value::String(dir.to_string_lossy().to_string()));
    if !manifest.contains_key("installedAt") {
        manifest.insert("installedAt".into(), Value::String(crate::util::now_iso()));
    }
    Value::Object(manifest)
}

fn scan_root(root: &Path, scope: &str, by_id: &mut indexmap_like::OrderedMap) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let plugin_dir = entry.path();
        if !plugin_dir.is_dir() {
            continue;
        }
        if let Some(manifest) = read_manifest(&plugin_dir) {
            let id = manifest
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            by_id.insert(
                id,
                manifest_to_response(manifest, scope, &plugin_dir),
            );
        }
    }
}

/// Tiny insertion-ordered map so project plugins override user plugins.
mod indexmap_like {
    use serde_json::Value;
    use std::collections::HashMap;

    pub struct OrderedMap {
        keys: Vec<String>,
        values: HashMap<String, Value>,
    }

    impl OrderedMap {
        pub fn new() -> Self {
            Self {
                keys: Vec::new(),
                values: HashMap::new(),
            }
        }

        pub fn insert(&mut self, key: String, value: Value) {
            if !self.values.contains_key(&key) {
                self.keys.push(key.clone());
            }
            self.values.insert(key, value);
        }

        pub fn into_values(mut self) -> Vec<Value> {
            self.keys
                .into_iter()
                .filter_map(|k| self.values.remove(&k))
                .collect()
        }
    }

    impl Default for OrderedMap {
        fn default() -> Self {
            Self::new()
        }
    }
}

/// Discovers all plugins for a project. Project-level plugins override
/// user-level plugins with the same ID.
pub fn discover(project_path: Option<&str>) -> Vec<Value> {
    let mut by_id = indexmap_like::OrderedMap::new();
    scan_root(&user_root(), "user", &mut by_id);
    if let Some(proj_root) = project_root(project_path) {
        scan_root(&proj_root, "project", &mut by_id);
    }
    let mut plugins = by_id.into_values();
    plugins.sort_by(|a, b| {
        let a_name = a.get("name").and_then(Value::as_str).unwrap_or("");
        let b_name = b.get("name").and_then(Value::as_str).unwrap_or("");
        a_name.to_lowercase().cmp(&b_name.to_lowercase())
    });
    plugins
}

pub fn templates() -> Vec<Value> {
    serde_json::from_str::<Vec<Value>>(BUILTIN_TEMPLATES).unwrap_or_default()
}

fn templates_json() -> Value {
    serde_json::from_str(BUILTIN_TEMPLATES).unwrap_or(Value::Null)
}

/// Installs a plugin into user or project scope. Supports built-in templates
/// (`source: "template"`, `uri` = template id) and local folders
/// (`source: "local"`, `uri` = absolute directory path). npm/GitHub sources
/// need a network fetcher and are rejected with a clear error for now.
pub fn install(
    source: &str,
    uri: &str,
    scope: &str,
    project_path: Option<&str>,
) -> Result<Value, String> {
    if source != "template" && source != "local" && source != "npm" && source != "github" {
        return Err(format!("Unknown plugin source: {source:?}"));
    }
    let target_dir = match scope {
        "project" => {
            let raw = project_path.unwrap_or("").trim().to_string();
            if raw.is_empty() {
                return Err("projectPath is required for project-scoped plugins.".to_string());
            }
            ensure_project_root(&raw)
                .ok_or_else(|| "Failed to resolve project plugins directory.".to_string())?
        }
        _ => ensure_user_root(),
    };

    let manifest: Map<String, Value> = match source {
        "template" => {
            let templates = templates_json();
            let found = templates
                .as_array()
                .and_then(|list| {
                    list.iter().find(|t| {
                        t.get("id").and_then(Value::as_str) == Some(uri)
                    })
                })
                .and_then(|t| t.get("manifest").cloned());
            let manifest = found.ok_or_else(|| format!("Template \"{uri}\" not found."))?;
            let mut obj = manifest
                .as_object()
                .cloned()
                .ok_or_else(|| "Template manifest is invalid.".to_string())?;
            obj.insert("enabled".into(), Value::Bool(true));
            obj.insert("installedAt".into(), Value::String(crate::util::now_iso()));
            obj
        }
        "npm" | "github" => {
            return Err(
                "npm and GitHub plugin sources are not supported in this build yet. Use a template or a local folder."
                    .to_string(),
            )
        }
        "local" => {
            let source_dir = PathBuf::from(uri.trim());
            if !source_dir.is_dir() {
                return Err(format!("Local plugin folder does not exist: {uri}"));
            }
            let folder_name = source_dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if folder_name.is_empty() || folder_name.contains("..") {
                return Err("Invalid local plugin folder name.".to_string());
            }
            let dest = target_dir.join(&folder_name);
            copy_dir_recursive(&source_dir, &dest)
                .map_err(|err| format!("Failed to copy plugin folder: {err}"))?;
            read_manifest(&dest).ok_or_else(|| {
                format!("Copied plugin has no valid {MANIFEST_FILENAME}.")
            })?
        }
        _ => unreachable!(),
    };

    let id = manifest
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if id.is_empty() {
        return Err("Plugin manifest has no id.".to_string());
    }

    let dir = target_dir.join(sanitize_folder(&id)?);
    let _ = std::fs::create_dir_all(&dir);
    let manifest_path = dir.join(MANIFEST_FILENAME);
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&Value::Object(manifest.clone())).map_err(|err| err.to_string())?)
        .map_err(|err| format!("Failed to write plugin manifest: {err}"))?;

    Ok(manifest_to_response(manifest, scope, &dir))
}

fn sanitize_folder(id: &str) -> Result<String, String> {
    if id.is_empty() || id.contains("..") || id.contains('/') || id.contains('\\') {
        return Err("Invalid plugin id.".to_string());
    }
    Ok(id.to_string())
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let entry_path = entry.path();
        let target = dest.join(entry.file_name());
        if entry_path.is_dir() {
            copy_dir_recursive(&entry_path, &target)?;
        } else {
            std::fs::copy(&entry_path, &target)?;
        }
    }
    Ok(())
}

/// Locates the directory of an installed plugin by id across both roots.
fn find_plugin_dir(id: &str, scope: &str, project_path: Option<&str>) -> Option<(PathBuf, Map<String, Value>)> {
    let roots: Vec<(PathBuf, &str)> = match scope {
        "project" => project_root(project_path)
            .map(|p| (p, "project"))
            .into_iter()
            .collect(),
        _ => vec![(user_root(), "user")],
    };
    for (root, root_scope) in roots {
        if !root.is_dir() {
            continue;
        }
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            if let Some(manifest) = read_manifest(&dir) {
                if manifest.get("id").and_then(Value::as_str) == Some(id) {
                    let _ = root_scope;
                    return Some((dir, manifest));
                }
            }
        }
    }
    None
}

/// Toggles a plugin's enabled flag in its plugin.json.
pub fn toggle(
    id: &str,
    enabled: bool,
    scope: &str,
    project_path: Option<&str>,
) -> Result<Value, String> {
    let (dir, mut manifest) =
        find_plugin_dir(id, scope, project_path).ok_or_else(|| format!("Plugin \"{id}\" not found."))?;
    manifest.insert("enabled".into(), Value::Bool(enabled));
    std::fs::write(
        dir.join(MANIFEST_FILENAME),
        serde_json::to_vec_pretty(&Value::Object(manifest.clone())).map_err(|err| err.to_string())?,
    )
    .map_err(|err| format!("Failed to update plugin manifest: {err}"))?;

    let dir_scope = if scope == "project" { "project" } else { "user" };
    Ok(manifest_to_response(manifest, dir_scope, &dir))
}

/// Deletes a plugin directory by id.
pub fn delete(id: &str, scope: &str, project_path: Option<&str>) -> Result<bool, String> {
    match find_plugin_dir(id, scope, project_path) {
        Some((dir, _)) => {
            std::fs::remove_dir_all(&dir)
                .map_err(|err| format!("Failed to delete plugin: {err}"))?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Convenience re-exports for routes that need typed payloads.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InstallPayload {
    pub source: String,
    pub uri: String,
    pub scope: Option<String>,
    pub project_path: Option<String>,
}
