//! MCP server configuration store — port of `McpConfigStore.ts`.
//!
//! Reads and writes `{ "mcpServers": { name: entry } }` JSON files: one
//! user-level file (`mcp_servers.json` in app support) and one project-level
//! file (`<project>/.locagens/mcp.json`). Project entries override same-named
//! user entries. Live connections arrive in a later phase; until then the
//! reported status is `disabled` or `disconnected`.

use crate::paths;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Serializable shape of one server config (camelCase for the frontend).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub name: String,
    pub scope: String,
    pub transport: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInfo {
    pub config: McpServerConfig,
    pub status: String,
    pub error: Option<String>,
    pub tools: Vec<Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct McpServersResponse {
    pub servers: Vec<McpServerInfo>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct RawMcpFile {
    #[serde(default, rename = "mcpServers")]
    mcp_servers: BTreeMap<String, RawMcpEntry>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct RawMcpEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    env: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transport: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    disabled: Option<bool>,
}

/// User-level MCP config file path.
pub fn user_config_path() -> PathBuf {
    paths::mcp_config_path()
}

/// Project-level MCP config file path (`<project>/.locagens/mcp.json`).
pub fn project_config_path(project_path: Option<&str>) -> Option<PathBuf> {
    let trimmed = project_path?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let base = std::fs::canonicalize(trimmed).ok()?;
    Some(base.join(".locagens").join("mcp.json"))
}

fn read_file(path: &PathBuf, scope: &str, project_path: Option<&str>) -> Vec<McpServerConfig> {
    let mut configs = Vec::new();
    if let Ok(raw) = std::fs::read_to_string(path) {
        if let Ok(file) = serde_json::from_str::<RawMcpFile>(&raw) {
            for (name, entry) in file.mcp_servers {
                let transport = entry.transport.clone().unwrap_or_else(|| {
                    if entry.url.is_some() {
                        "sse".to_string()
                    } else {
                        "stdio".to_string()
                    }
                });
                configs.push(McpServerConfig {
                    name: name.clone(),
                    scope: scope.to_string(),
                    transport,
                    command: entry.command,
                    args: entry.args,
                    env: entry.env,
                    url: entry.url,
                    enabled: !entry.disabled.unwrap_or(false),
                    project_path: project_path.map(|p| p.to_string()),
                });
            }
        }
    }
    configs
}

/// Project configs take precedence over user configs of the same name.
pub fn list_configs(project_path: Option<&str>) -> Vec<McpServerConfig> {
    let mut configs: Vec<McpServerConfig> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    if let Some(proj_path) = project_config_path(project_path) {
        for cfg in read_file(&proj_path, "project", project_path) {
            seen.push(cfg.name.clone());
            configs.push(cfg);
        }
    }
    for cfg in read_file(&user_config_path(), "user", None) {
        if !seen.contains(&cfg.name) {
            configs.push(cfg);
        }
    }

    configs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    configs
}

fn status_for(config: &McpServerConfig) -> String {
    if config.enabled {
        "disconnected".to_string()
    } else {
        "disabled".to_string()
    }
}

pub fn server_info(config: &McpServerConfig) -> McpServerInfo {
    McpServerInfo {
        config: config.clone(),
        status: status_for(config),
        error: None,
        tools: Vec::new(),
    }
}

/// The effective config for a name (project scope wins over user scope).
pub fn find_config(name: &str, project_path: Option<&str>) -> Option<McpServerConfig> {
    list_configs(project_path)
        .into_iter()
        .find(|cfg| cfg.name == name.trim().to_lowercase())
}

/// Writes one entry into the appropriate scope file, creating it if needed.
pub fn save_config(config: &McpServerConfig) -> Result<(), String> {
    let (path, name, entry) = match config.scope.as_str() {
        "project" => {
            let path = project_config_path(config.project_path.as_deref())
                .ok_or_else(|| "projectPath is required for project MCP servers.".to_string())?;
            (path, config.name.clone(), entry_from_config(config))
        }
        _ => (user_config_path(), config.name.clone(), entry_from_config(config)),
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let mut file = match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str::<RawMcpFile>(&raw).unwrap_or_default(),
        Err(_) => RawMcpFile::default(),
    };
    file.mcp_servers.insert(name, entry);
    let serialized = serde_json::to_string_pretty(&file).map_err(|err| err.to_string())?;
    std::fs::write(&path, serialized).map_err(|err| err.to_string())
}

fn entry_from_config(config: &McpServerConfig) -> RawMcpEntry {
    RawMcpEntry {
        command: config.command.clone(),
        args: config.args.clone(),
        env: config.env.clone(),
        url: config.url.clone(),
        transport: Some(config.transport.clone()),
        disabled: Some(!config.enabled),
    }
}

/// Removes an entry from the file it is stored in. Returns false when missing.
pub fn delete_config(name: &str, project_path: Option<&str>) -> Result<bool, String> {
    let name = name.trim().to_lowercase();
    // Try project file first, then user file.
    if let Some(proj_path) = project_config_path(project_path) {
        if remove_from_file(&proj_path, &name)? {
            return Ok(true);
        }
    }
    remove_from_file(&user_config_path(), &name)
}

fn remove_from_file(path: &PathBuf, name: &str) -> Result<bool, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => return Ok(false),
    };
    let mut file = serde_json::from_str::<RawMcpFile>(&raw).unwrap_or_default();
    if file.mcp_servers.remove(name).is_none() {
        return Ok(false);
    }
    let serialized = serde_json::to_string_pretty(&file).map_err(|err| err.to_string())?;
    std::fs::write(path, serialized).map_err(|err| err.to_string())?;
    Ok(true)
}

/// Toggles `disabled` for a server, returning the updated info.
pub fn toggle(name: &str, enabled: bool, project_path: Option<&str>) -> Result<McpServerInfo, String> {
    let name = name.trim().to_lowercase();
    let existing =
        find_config(&name, project_path).ok_or_else(|| format!("MCP server \"{name}\" not found."))?;
    let mut config = existing;
    config.enabled = enabled;
    save_config(&config)?;
    Ok(server_info(&config))
}

/// Re-reads the config for a server. (Live reconnects arrive with the MCP
/// client integration; for now "restart" just refreshes the stored status.)
pub fn restart(name: &str, project_path: Option<&str>) -> Result<McpServerInfo, String> {
    let config =
        find_config(name, project_path).ok_or_else(|| format!("MCP server \"{name}\" not found."))?;
    Ok(server_info(&config))
}

/// Input payload for POST /api/mcp/servers.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInput {
    pub name: Option<String>,
    pub scope: Option<String>,
    pub transport: Option<String>,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<BTreeMap<String, String>>,
    pub url: Option<String>,
    pub enabled: Option<bool>,
    pub project_path: Option<String>,
}

/// Validates + normalizes a create/update payload (port of the mcp.ts route).
pub fn validate_input(input: &McpServerInput, registered_project: Option<&str>) -> Result<McpServerConfig, String> {
    let name = input.name.clone().unwrap_or_default().trim().to_lowercase();
    let name_ok = !name.is_empty()
        && name.chars().next().map(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).unwrap_or(false)
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if !name_ok {
        return Err("Server name is required (lowercase alphanumeric, dash, underscore only).".to_string());
    }

    let scope = if input.scope.as_deref() == Some("project") { "project" } else { "user" };
    let mut project_path: Option<String> = None;
    if scope == "project" {
        let raw = input.project_path.clone().unwrap_or_default();
        if raw.trim().is_empty() {
            return Err("projectPath is required for project MCP servers.".to_string());
        }
        let canonical = registered_project
            .map(|p| p.to_string())
            .or_else(|| std::fs::canonicalize(raw.trim()).ok().map(|p| p.to_string_lossy().to_string()));
        project_path = canonical;
    }

    let transport = if input.transport.as_deref() == Some("sse") { "sse" } else { "stdio" };
    if transport == "stdio" && input.command.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err("command is required for stdio transport.".to_string());
    }
    if transport == "sse" && input.url.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err("url is required for sse transport.".to_string());
    }

    Ok(McpServerConfig {
        name,
        scope: scope.to_string(),
        transport: transport.to_string(),
        command: input.command.clone().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()),
        args: input.args.clone(),
        env: input.env.clone(),
        url: input.url.clone().map(|u| u.trim().to_string()).filter(|u| !u.is_empty()),
        enabled: input.enabled.unwrap_or(true),
        project_path,
    })
}

