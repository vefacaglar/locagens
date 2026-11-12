//! Platform path resolution shared by every backend subsystem.
//!
//! Mirrors `packages/shared/src/paths.ts`: an OS app-support directory holds
//! settings.json, providers.json, skills/, plugins/ and mcp_servers.json, and
//! the `pi` agent directory (`~/.pi/agent`) is discovered for skills/plugins.

use std::path::PathBuf;

pub const APP_DIR_NAME: &str = "Locagens";

/// Platform app-support directory (macOS Application Support / Windows APPDATA / XDG).
pub fn app_support_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        let mut p = home_dir();
        p.push("Library");
        p.push("Application Support");
        p.push(APP_DIR_NAME);
        p
    } else if cfg!(windows) {
        match std::env::var("APPDATA").ok().filter(|v| !v.is_empty()) {
            Some(appdata) => PathBuf::from(appdata).join(APP_DIR_NAME),
            None => {
                let mut p = home_dir();
                p.push("AppData");
                p.push("Roaming");
                p.push(APP_DIR_NAME);
                p
            }
        }
    } else {
        match std::env::var("XDG_CONFIG_HOME").ok().filter(|v| !v.is_empty()) {
            Some(xdg) => PathBuf::from(xdg).join("locagens"),
            None => {
                let mut p = home_dir();
                p.push(".config");
                p.push("locagens");
                p
            }
        }
    }
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Local application settings file (settings.json).
pub fn default_settings_path() -> PathBuf {
    if let Ok(override_path) = std::env::var("LOCAGENS_SETTINGS_PATH") {
        if !override_path.is_empty() {
            return PathBuf::from(override_path);
        }
    }
    app_support_dir().join("settings.json")
}

/// Provider configuration file (providers.json) — used by later phases.
/// Provider configuration file (providers.json) — used by later phases.
#[allow(dead_code)]
pub fn default_provider_config_path() -> PathBuf {
    if let Ok(override_path) = std::env::var("LOCAGENS_PROVIDER_CONFIG_PATH") {
        if !override_path.is_empty() {
            return PathBuf::from(override_path);
        }
    }
    app_support_dir().join("providers.json")
}

/// User-level skills root (Settings → Skills "Open folder").
pub fn skills_root() -> PathBuf {
    if let Ok(override_path) = std::env::var("LOCAGENS_SKILLS_PATH") {
        if !override_path.is_empty() {
            return PathBuf::from(override_path);
        }
    }
    app_support_dir().join("skills")
}

/// `pi` agent skills directory — also scanned so shared skill packs appear.
pub fn pi_skills_root() -> PathBuf {
    pi_agent_dir().join("skills")
}

/// User-level plugins root.
pub fn plugins_root() -> PathBuf {
    if let Ok(override_path) = std::env::var("LOCAGENS_PLUGINS_PATH") {
        if !override_path.is_empty() {
            return PathBuf::from(override_path);
        }
    }
    app_support_dir().join("plugins")
}

/// User-level MCP servers config file.
pub fn mcp_config_path() -> PathBuf {
    if let Ok(override_path) = std::env::var("LOCAGENS_MCP_CONFIG_PATH") {
        if !override_path.is_empty() {
            return PathBuf::from(override_path);
        }
    }
    app_support_dir().join("mcp_servers.json")
}

/// Project-local dot-folder segments.
#[allow(dead_code)]
pub fn project_dot_dir(project_path: &str, leaf: &str) -> Option<PathBuf> {
    let trimmed = project_path.trim();
    if trimmed.is_empty() {
        return None;
    }
    let base = std::fs::canonicalize(trimmed).ok()?;
    Some(base.join(".locagens").join(leaf))
}

/// The `pi` coding agent directory (auth.json / models.json live here).
pub fn pi_agent_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("PI_CODING_AGENT_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    let mut p = home_dir();
    p.push(".pi");
    p.push("agent");
    p
}
