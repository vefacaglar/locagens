//! MCP server configuration store + live MCP client — port of
//! `McpConfigStore.ts` plus the Codex-style JSON-RPC 2.0 client.
//!
//! Reads and writes `{ "mcpServers": { name: entry } }` JSON files: one
//! user-level file (`mcp_servers.json` in app support) and one project-level
//! file (`<project>/.locagens/mcp.json`). Project entries override same-named
//! user entries. Live connections use the JSON-RPC client below: `stdio`
//! servers are spawned as child processes, `http`/`sse`/`streamable-http`
//! servers use the streamable HTTP client in `http.rs` (sessions, SSE replies,
//! bearer/header auth, MCP OAuth in `oauth.rs`), and discovered tools are
//! cached in memory.

mod http;
pub mod oauth;

pub use http::shutdown_sessions;

#[cfg(test)]
mod remote_tests;

use crate::paths;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::Command;

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const MCP_CLIENT_NAME: &str = "locagens";
const MCP_CLIENT_VERSION: &str = "0.1.0";
/// Bound for the whole stdio roundtrip (initialize + target call).
const STDIO_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum characters returned by a single MCP tool call.
const MAX_TOOL_OUTPUT_CHARS: usize = 24_000;

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
    /// Env var holding a bearer token (Codex `bearer_token_env_var`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token_env_var: Option<String>,
    /// Static request headers (Codex `http_headers`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_headers: Option<BTreeMap<String, String>>,
    /// Header name -> env var holding its value (Codex `env_http_headers`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_http_headers: Option<BTreeMap<String, String>>,
    /// Pre-registered OAuth client id (skips dynamic registration).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_client_id: Option<String>,
    /// OAuth scopes to request at login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInfo {
    pub config: McpServerConfig,
    pub status: String,
    pub error: Option<String>,
    pub tools: Vec<Value>,
    /// The server answered 401 and the user must sign in (OAuth).
    #[serde(default)]
    pub needs_login: bool,
    /// An MCP OAuth token is stored for this server.
    #[serde(default)]
    pub signed_in: bool,
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
    #[serde(default, alias = "bearerTokenEnvVar", skip_serializing_if = "Option::is_none")]
    bearer_token_env_var: Option<String>,
    #[serde(default, alias = "httpHeaders", skip_serializing_if = "Option::is_none")]
    http_headers: Option<BTreeMap<String, String>>,
    #[serde(default, alias = "envHttpHeaders", skip_serializing_if = "Option::is_none")]
    env_http_headers: Option<BTreeMap<String, String>>,
    #[serde(default, alias = "oauthClientId", skip_serializing_if = "Option::is_none")]
    oauth_client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scopes: Option<Vec<String>>,
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
                    bearer_token_env_var: entry.bearer_token_env_var,
                    http_headers: entry.http_headers,
                    env_http_headers: entry.env_http_headers,
                    oauth_client_id: entry.oauth_client_id,
                    scopes: entry.scopes,
                });
            }
        }
    }
    configs
}

/// Maps config.toml `[mcp_servers.<name>]` tables onto the user scope.
/// `env_vars` (names to forward) are resolved from the current environment;
/// `cwd` and the timeout keys have no equivalent here and are ignored.
fn config_toml_servers(
    servers: &BTreeMap<String, locagens_config::config_toml::McpServerConfig>,
) -> Vec<McpServerConfig> {
    servers
        .iter()
        .map(|(name, entry)| {
            let mut env = entry.env.clone();
            for var in &entry.env_vars {
                if let Ok(value) = std::env::var(var) {
                    env.entry(var.clone()).or_insert(value);
                }
            }
            let non_empty = |map: &BTreeMap<String, String>| (!map.is_empty()).then(|| map.clone());
            McpServerConfig {
                name: name.trim().to_lowercase(),
                scope: "user".to_string(),
                transport: if entry.url.is_some() { "streamable-http" } else { "stdio" }.to_string(),
                command: entry.command.clone(),
                args: (!entry.args.is_empty()).then(|| entry.args.clone()),
                env: non_empty(&env),
                url: entry.url.clone(),
                enabled: entry.is_enabled(),
                project_path: None,
                bearer_token_env_var: entry.bearer_token_env_var.clone(),
                http_headers: non_empty(&entry.http_headers),
                env_http_headers: non_empty(&entry.env_http_headers),
                oauth_client_id: None,
                scopes: None,
            }
        })
        .collect()
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
            seen.push(cfg.name.clone());
            configs.push(cfg);
        }
    }
    // Codex-style `[mcp_servers.<name>]` from config.toml, lowest precedence:
    // an entry saved from the UI (mcp.json) with the same name overrides it.
    for cfg in config_toml_servers(&crate::config_runtime::current().mcp_servers) {
        if !seen.contains(&cfg.name) {
            configs.push(cfg);
        }
    }

    configs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    configs
}

// ----------------------------------------------------------------------
// Qualified tool names (Codex-compatible `mcp__<server>__<tool>`)
// ----------------------------------------------------------------------

/// Sanitized server prefix: every char that is not ASCII-alphanumeric, `_`
/// or `-` becomes `_`; the result is wrapped as `mcp__<clean>__`.
pub fn sanitized_server_prefix(server_name: &str) -> String {
    let clean: String = server_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    format!("mcp__{clean}__")
}

/// Splits a qualified `mcp__<server>__<tool>` name into its parts.
pub fn parse_qualified_mcp_tool(tool_name: &str) -> Option<(String, String)> {
    let rest = tool_name.strip_prefix("mcp__")?;
    let (server_key, raw_tool) = match rest.split_once("__") {
        Some((server, tool)) => (server, tool),
        // Degraded form `mcp__<name>`: treat the remainder as the tool name
        // with an empty server key (rare, but keeps the parse total).
        None => ("", rest),
    };
    Some((server_key.to_string(), raw_tool.to_string()))
}

// ----------------------------------------------------------------------
// Effective configs (explicit files + enabled plugin manifests)
// ----------------------------------------------------------------------

/// Builds a config from a plugin-manifest `mcpServers` entry.
fn config_from_plugin_entry(
    name: &str,
    entry: &Value,
    project_path: Option<&str>,
) -> Option<McpServerConfig> {
    let obj = entry.as_object()?;
    let transport = obj
        .get("transport")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            if obj.contains_key("url") {
                "sse".to_string()
            } else {
                "stdio".to_string()
            }
        });
    let command = obj.get("command").and_then(Value::as_str).map(str::to_string);
    let args = obj
        .get("args")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect());
    let env = obj.get("env").and_then(Value::as_object).map(|map| {
        map.iter()
            .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
            .collect::<BTreeMap<String, String>>()
    });
    let url = obj.get("url").and_then(Value::as_str).map(str::to_string);
    let string_map = |key: &str| {
        obj.get(key).and_then(Value::as_object).map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                .collect::<BTreeMap<String, String>>()
        })
    };
    let enabled = obj
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true)
        && !obj.get("disabled").and_then(Value::as_bool).unwrap_or(false);
    Some(McpServerConfig {
        name: name.to_string(),
        scope: "plugin".to_string(),
        transport,
        command,
        args,
        env,
        url,
        enabled,
        project_path: project_path.map(|p| p.to_string()),
        bearer_token_env_var: obj.get("bearer_token_env_var").and_then(Value::as_str).map(str::to_string),
        http_headers: string_map("http_headers"),
        env_http_headers: string_map("env_http_headers"),
        oauth_client_id: None,
        scopes: None,
    })
}

/// Explicit MCP configs (user + project files) merged with the `mcpServers`
/// entries of every enabled plugin manifest. Explicit config entries win;
/// plugin entries are only appended when the name is not taken.
pub fn effective_configs(project_path: Option<&str>) -> Vec<McpServerConfig> {
    let mut configs = list_configs(project_path);
    for manifest in crate::services::plugins::enabled_manifests(project_path) {
        let servers = manifest.get("mcpServers").and_then(Value::as_object);
        let Some(servers) = servers else { continue };
        for (name, entry) in servers {
            if configs.iter().any(|c| c.name.eq_ignore_ascii_case(name)) {
                continue;
            }
            if let Some(config) = config_from_plugin_entry(name, entry, project_path) {
                configs.push(config);
            }
        }
    }
    configs
}

fn status_for(config: &McpServerConfig) -> String {
    if config.enabled {
        "disconnected".to_string()
    } else {
        "disabled".to_string()
    }
}

/// Builds an info record, flagging stored/missing OAuth sign-in for remote servers.
fn info_for(config: &McpServerConfig, status: String, error: Option<String>, tools: Vec<Value>) -> McpServerInfo {
    let remote = http::is_remote(&config.transport);
    McpServerInfo {
        config: config.clone(),
        status,
        needs_login: error.as_deref().is_some_and(|e| e.contains(http::LOGIN_HINT)),
        signed_in: remote && oauth::is_signed_in(&oauth::TokenStore::default_location(), config),
        error,
        tools,
    }
}

pub fn server_info(config: &McpServerConfig) -> McpServerInfo {
    info_for(config, status_for(config), None, Vec::new())
}

/// The effective config for a name (project scope wins over user scope).
pub fn find_config(name: &str, project_path: Option<&str>) -> Option<McpServerConfig> {
    list_configs(project_path)
        .into_iter()
        .find(|cfg| cfg.name == name.trim().to_lowercase())
}

/// Carries auth settings the edit form cannot express (bearer env var,
/// headers, OAuth client id, scopes) over from the stored entry with the same
/// name and scope, so saving a remote server from the UI never drops them.
pub fn keep_stored_auth(mut config: McpServerConfig) -> McpServerConfig {
    let Some(stored) = find_config(&config.name, config.project_path.as_deref())
        .filter(|stored| stored.scope == config.scope)
    else {
        return config;
    };
    config.bearer_token_env_var = config.bearer_token_env_var.or(stored.bearer_token_env_var);
    config.http_headers = config.http_headers.or(stored.http_headers);
    config.env_http_headers = config.env_http_headers.or(stored.env_http_headers);
    config.oauth_client_id = config.oauth_client_id.or(stored.oauth_client_id);
    config.scopes = config.scopes.or(stored.scopes);
    config
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
        bearer_token_env_var: config.bearer_token_env_var.clone(),
        http_headers: config.http_headers.clone(),
        env_http_headers: config.env_http_headers.clone(),
        oauth_client_id: config.oauth_client_id.clone(),
        scopes: config.scopes.clone(),
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

    let transport = match input.transport.as_deref() {
        Some(t @ ("sse" | "http" | "streamable-http")) => t,
        _ => "stdio",
    };
    if transport == "stdio" && input.command.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err("command is required for stdio transport.".to_string());
    }
    if transport != "stdio" && input.url.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err("url is required for remote (http/sse) transport.".to_string());
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
        bearer_token_env_var: None,
        http_headers: None,
        env_http_headers: None,
        oauth_client_id: None,
        scopes: None,
    })
}

// ----------------------------------------------------------------------
// In-memory cache of probed server info
// ----------------------------------------------------------------------

fn cache() -> &'static Mutex<HashMap<String, McpServerInfo>> {
    static CACHE: OnceLock<Mutex<HashMap<String, McpServerInfo>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_key(project_path: Option<&str>, name: &str) -> String {
    format!("{}::{name}", project_path.unwrap_or(""))
}

fn cache_get(project_path: Option<&str>, name: &str) -> Option<McpServerInfo> {
    let key = cache_key(project_path, name);
    cache().lock().ok()?.get(&key).cloned()
}

fn cache_put(project_path: Option<&str>, info: &McpServerInfo) {
    let key = cache_key(project_path, &info.config.name);
    if let Ok(mut cache) = cache().lock() {
        cache.insert(key, info.clone());
    }
}

/// Drops every cached probe for a server name (any project scope). Used by
/// the Settings facade on upsert/delete/toggle/restart.
pub fn invalidate_server(name: &str) {
    http::forget_server(name);
    let suffix = format!("::{name}");
    if let Ok(mut cache) = cache().lock() {
        let stale: Vec<String> = cache
            .keys()
            .filter(|key| key.ends_with(&suffix))
            .cloned()
            .collect();
        for key in stale {
            cache.remove(&key);
        }
    }
}

// ----------------------------------------------------------------------
// JSON-RPC 2.0 client (stdio + HTTP/SSE)
// ----------------------------------------------------------------------

async fn rpc_call(config: &McpServerConfig, method: &str, params: &Value) -> Result<Value, String> {
    match config.transport.as_str() {
        "http" | "sse" | "streamable-http" => http::rpc_http(config, method, params).await,
        _ => rpc_stdio(config, method, params).await,
    }
}

async fn write_rpc_line<W>(stdin: &mut W, value: &Value) -> Result<(), String>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut line = serde_json::to_string(value).map_err(|err| err.to_string())?;
    line.push('\n');
    stdin.write_all(line.as_bytes()).await.map_err(|err| err.to_string())?;
    stdin.flush().await.map_err(|err| err.to_string())
}

/// Reads JSON-RPC response lines until one matches `id`, then unwraps
/// `result` (or maps `error` into a string error).
async fn read_rpc_response<R>(reader: &mut R, id: u64) -> Result<Value, String>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).await.map_err(|err| err.to_string())?;
        if bytes == 0 {
            return Err("MCP server closed stdout before responding.".to_string());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            // Non-JSON log output from the server process is ignored.
            continue;
        };
        if value.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            return Err(format!("MCP server error: {error}"));
        }
        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
    }
}

async fn rpc_stdio(config: &McpServerConfig, method: &str, params: &Value) -> Result<Value, String> {
    let command = config
        .command
        .clone()
        .filter(|command| !command.trim().is_empty())
        .ok_or_else(|| format!("MCP server '{}' has no command.", config.name))?;
    let mut cmd = Command::new(&command);
    if let Some(args) = &config.args {
        cmd.args(args);
    }
    if let Some(env) = &config.env {
        cmd.envs(env.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|err| format!("Failed to start MCP server '{}': {err}", config.name))?;

    let roundtrip = async {
        let mut stdin = child.stdin.take().ok_or("MCP server stdin unavailable.")?;
        let stdout = child.stdout.take().ok_or("MCP server stdout unavailable.")?;
        let mut reader = tokio::io::BufReader::new(stdout);

        write_rpc_line(
            &mut stdin,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": { "name": MCP_CLIENT_NAME, "version": MCP_CLIENT_VERSION }
                }
            }),
        )
        .await?;
        let _init = read_rpc_response(&mut reader, 1).await?;
        write_rpc_line(
            &mut stdin,
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        )
        .await?;
        write_rpc_line(
            &mut stdin,
            &json!({ "jsonrpc": "2.0", "id": 2, "method": method, "params": params }),
        )
        .await?;
        read_rpc_response(&mut reader, 2).await
    };

    let result = tokio::time::timeout(STDIO_TIMEOUT, roundtrip)
        .await
        .map_err(|_| format!("MCP server '{}' timed out after 10s.", config.name))??;
    Ok(result)
}

fn unwrap_rpc_response(value: Value) -> Result<Value, String> {
    if let Some(error) = value.get("error") {
        return Err(format!("MCP server error: {error}"));
    }
    Ok(value.get("result").cloned().unwrap_or(Value::Null))
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let head: String = text.chars().take(max).collect();
        format!("{head}…")
    }
}

// ----------------------------------------------------------------------
// Probe / discover / call
// ----------------------------------------------------------------------

/// Probes one server: spawns/requests `tools/list` and caches the result.
/// Disabled servers report `disabled` without being spawned.
pub async fn probe_server(config: &McpServerConfig) -> McpServerInfo {
    if !config.enabled {
        return info_for(config, "disabled".to_string(), None, Vec::new());
    }
    match rpc_call(config, "tools/list", &json!({})).await {
        Ok(result) => {
            let tools = result
                .get("tools")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let info = info_for(config, "connected".to_string(), None, tools);
            cache_put(config.project_path.as_deref(), &info);
            info
        }
        Err(error) => info_for(config, "error".to_string(), Some(error), Vec::new()),
    }
}

/// Probes every effective config concurrently (disabled servers return
/// their `disabled` status without spawning).
pub async fn probe_all(project_path: Option<&str>) -> Vec<McpServerInfo> {
    let configs = effective_configs(project_path);
    futures::future::join_all(configs.iter().map(probe_server)).await
}

/// Discovers MCP tools and converts them into OpenAI function-tool defs with
/// qualified names (`mcp__<server>__<tool>`). Cached tools are reused;
/// servers not yet probed are probed on demand.
pub async fn discover_tool_defs(project_path: Option<&str>) -> Vec<Value> {
    let mut defs = Vec::new();
    for config in effective_configs(project_path).iter().filter(|c| c.enabled) {
        let tools = match cache_get(project_path, &config.name) {
            Some(info) if info.status == "connected" && !info.tools.is_empty() => info.tools,
            _ => {
                let probed = probe_server(config).await;
                if probed.status == "connected" { probed.tools } else { Vec::new() }
            }
        };
        let prefix = sanitized_server_prefix(&config.name);
        for tool in tools {
            let name = tool.get("name").and_then(Value::as_str).unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            defs.push(json!({
                "type": "function",
                "function": {
                    "name": format!("{prefix}{name}"),
                    "description": tool
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("MCP tool")
                        .to_string(),
                    "parameters": tool
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({ "type": "object" })),
                }
            }));
        }
    }
    defs
}

/// Finds an enabled server by exact name, sanitized name or prefix form.
fn find_enabled_server(server_key: &str, project_path: Option<&str>) -> Result<McpServerConfig, String> {
    let key = server_key.trim();
    if key.is_empty() {
        return Err("MCP server key is empty.".to_string());
    }
    for config in effective_configs(project_path) {
        if !config.enabled {
            continue;
        }
        let sanitized = sanitized_server_prefix(&config.name);
        if config.name == key
            || sanitized.trim_start_matches("mcp__").trim_end_matches("__") == key
            || sanitized == key
            || format!("mcp__{}", config.name) == key
        {
            return Ok(config);
        }
    }
    Err(format!("MCP server '{server_key}' not found or disabled."))
}

fn extract_content_text(result: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        for item in content {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
            } else if item.get("type").and_then(Value::as_str) == Some("resource") {
                if let Some(text) = item.pointer("/resource/text").and_then(Value::as_str) {
                    parts.push(text.to_string());
                } else if let Ok(json) = serde_json::to_string(item) {
                    parts.push(json);
                }
            } else if let Ok(json) = serde_json::to_string(item) {
                parts.push(json);
            }
        }
    }
    if parts.is_empty() {
        // Fall back to the raw result so the model still sees something.
        serde_json::to_string_pretty(result).unwrap_or_else(|_| "{}".to_string())
    } else {
        parts.join("\n")
    }
}

/// Calls `tools/call` on a matching enabled server and extracts the text or
/// JSON content from `result.content` (or returns the `isError` reason).
pub async fn call_mcp_tool(
    project_path: Option<&str>,
    server_key: &str,
    tool_name: &str,
    arguments: &Value,
) -> Result<String, String> {
    let server = find_enabled_server(server_key, project_path)?;
    let result = rpc_call(&server, "tools/call", &json!({ "name": tool_name, "arguments": arguments })).await?;
    if result.get("isError").and_then(Value::as_bool).unwrap_or(false) {
        let text = extract_content_text(&result);
        return Err(if text.trim().is_empty() {
            format!("MCP tool '{tool_name}' failed.")
        } else {
            text
        });
    }
    Ok(truncate_chars(&extract_content_text(&result), MAX_TOOL_OUTPUT_CHARS))
}

/// Calls `resources/list` on matching enabled servers and returns pretty
/// JSON `{ "resources": [...] }`.
pub async fn list_resources(
    project_path: Option<&str>,
    server_filter: Option<&str>,
    cursor: Option<&str>,
) -> Result<String, String> {
    let mut resources: Vec<Value> = Vec::new();
    for config in effective_configs(project_path).iter().filter(|c| c.enabled) {
        if let Some(filter) = server_filter.map(str::trim).filter(|f| !f.is_empty()) {
            if config.name != filter
                && sanitized_server_prefix(&config.name).trim_start_matches("mcp__").trim_end_matches("__") != filter
            {
                continue;
            }
        }
        let mut params = json!({});
        if let Some(cursor) = cursor {
            params["cursor"] = json!(cursor);
        }
        if let Ok(result) = rpc_call(config, "resources/list", &params).await {
            if let Some(list) = result.get("resources").and_then(Value::as_array) {
                resources.extend(list.clone());
            }
        }
    }
    serde_json::to_string_pretty(&json!({ "resources": resources })).map_err(|err| err.to_string())
}

/// Calls `resources/read` on one server and returns the JSON contents.
pub async fn read_resource(project_path: Option<&str>, server_name: &str, uri: &str) -> Result<String, String> {
    let server = find_enabled_server(server_name, project_path)?;
    let result = rpc_call(&server, "resources/read", &json!({ "uri": uri })).await?;
    serde_json::to_string_pretty(&result).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pure-POSIX MCP stdio mock: reads JSON-RPC lines and answers with
    /// canned responses keyed by method name (no external tooling needed).
    const MOCK_SERVER_SCRIPT: &str = r#"while IFS= read -r line; do
  case "$line" in
    *'"initialize"'*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"mock","version":"0.1.0"}}}';;
    *'"tools/list"'*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","description":"Echo back a message","inputSchema":{"type":"object","properties":{"message":{"type":"string"}},"required":["message"]}}]}}';;
    *'"tools/call"'*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"hello from mock"}]}}';;
    *'"resources/list"'*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"resources":[{"uri":"mock://doc","name":"Doc","mimeType":"text/plain"}]}}';;
    *'"resources/read"'*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"contents":[{"uri":"mock://doc","text":"resource body"}]}}';;
  esac
done"#;

    fn mock_config() -> McpServerConfig {
        McpServerConfig {
            name: "mock".to_string(),
            scope: "user".to_string(),
            transport: "stdio".to_string(),
            command: Some("sh".to_string()),
            args: Some(vec!["-c".to_string(), MOCK_SERVER_SCRIPT.to_string()]),
            env: None,
            url: None,
            enabled: true,
            project_path: None,
            bearer_token_env_var: None,
            http_headers: None,
            env_http_headers: None,
            oauth_client_id: None,
            scopes: None,
        }
    }

    #[test]
    fn qualified_mcp_tool_names_parse() {
        assert_eq!(sanitized_server_prefix("github"), "mcp__github__");
        assert_eq!(sanitized_server_prefix("my weird server!"), "mcp__my_weird_server___");
        assert_eq!(parse_qualified_mcp_tool("mcp__github__list_issues"),
            Some(("github".to_string(), "list_issues".to_string())));
        assert_eq!(parse_qualified_mcp_tool("mcp__srv__plain_tool_name"),
            Some(("srv".to_string(), "plain_tool_name".to_string())));
        assert_eq!(parse_qualified_mcp_tool("read_file"), None);
    }

    #[tokio::test]
    async fn qualified_mcp_tool_names_and_stdio_roundtrip() {
        // Register the mock server in a hermetic project-level config.
        let project = std::env::temp_dir().join(format!(
            "locagens-mcp-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(project.join(".locagens")).unwrap();
        let mut config = mock_config();
        config.scope = "project".to_string();
        config.project_path = Some(project.display().to_string());
        std::fs::write(
            project.join(".locagens").join("mcp.json"),
            serde_json::to_vec(&json!({
                "mcpServers": {
                    "mock": {
                        "command": config.command,
                        "args": config.args,
                        "transport": "stdio"
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let project_str = project.display().to_string();

        // probe_server → connected with discovered tools.
        let info = probe_server(&config).await;
        assert_eq!(info.status, "connected", "error: {:?}", info.error);
        assert_eq!(info.tools.len(), 1);
        assert_eq!(info.tools[0]["name"], "echo");

        // Disabled servers are never spawned.
        let mut disabled = config.clone();
        disabled.enabled = false;
        let info = probe_server(&disabled).await;
        assert_eq!(info.status, "disabled");

        // call_mcp_tool → text content extracted from result.content.
        let output = call_mcp_tool(Some(&project_str), "mock", "echo", &json!({ "message": "hi" }))
            .await
            .expect("tools/call should succeed");
        assert_eq!(output, "hello from mock");

        // Qualified name lookup: sanitized prefix form also resolves.
        let output = call_mcp_tool(Some(&project_str), &sanitized_server_prefix("mock"), "echo", &json!({}))
            .await
            .expect("qualified prefix should resolve");
        assert_eq!(output, "hello from mock");

        // Unknown server key fails cleanly.
        assert!(call_mcp_tool(Some(&project_str), "missing", "echo", &json!({})).await.is_err());

        // Resources list/read roundtrip.
        let list = list_resources(Some(&project_str), Some("mock"), None).await.expect("resources/list");
        let parsed: Value = serde_json::from_str(&list).unwrap();
        assert_eq!(parsed["resources"][0]["uri"], "mock://doc");
        let read = read_resource(Some(&project_str), "mock", "mock://doc").await.expect("resources/read");
        let parsed: Value = serde_json::from_str(&read).unwrap();
        assert_eq!(parsed["contents"][0]["text"], "resource body");

        let _ = std::fs::remove_dir_all(&project);
    }
}

