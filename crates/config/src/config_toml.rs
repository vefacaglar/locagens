//! Typed `config.toml` (Codex-compatible subset).
//!
//! The file is read from every directory of the agent chain
//! ([`crate::paths::agent_dir_chain`]: `~/.locagens`, `~/.codex`,
//! `~/.pi/agent`). Files are deep-merged table by table with the primary
//! (earliest) directory winning on conflicts; arrays and scalars are replaced
//! wholesale. Unknown keys are ignored. Malformed TOML (or a known key with
//! the wrong type) in the primary file is an error that names the offending
//! file; the read-only fallbacks (`~/.codex`, `~/.pi/agent`) are parsed
//! leniently: a bad file or key is skipped with a warning.
//!
//! Profiles (`[profiles.<name>]` + top-level `profile`) overlay the supported
//! top-level keys via [`Config::with_profile`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `[sandbox_workspace_write]`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxWorkspaceWrite {
    pub writable_roots: Vec<String>,
    pub network_access: bool,
}

/// `[sandbox]` (Locagens: per-domain network proxy allowlist).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxTable {
    pub network: SandboxNetwork,
}

/// `[sandbox.network]`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxNetwork {
    /// Hosts sandboxed commands may reach (`a.com`, `*.a.com`, `**.a.com`).
    pub allowed_domains: Vec<String>,
}

/// `[shell_environment_policy]` as written in the file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellEnvironmentPolicyToml {
    /// `all` (default), `core` or `none`.
    pub inherit: Option<String>,
    pub ignore_default_excludes: Option<bool>,
    /// Case-insensitive glob patterns (`*`, `?`) of variables to drop.
    pub exclude: Vec<String>,
    /// Variables forced to a fixed value.
    pub set: BTreeMap<String, String>,
    /// When non-empty, only variables matching these globs are kept.
    pub include_only: Vec<String>,
}

/// `[mcp_servers.<name>]` (stdio or streamable-HTTP).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServerConfig {
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub env_vars: Vec<String>,
    pub cwd: Option<String>,
    pub url: Option<String>,
    pub bearer_token_env_var: Option<String>,
    pub http_headers: BTreeMap<String, String>,
    pub env_http_headers: BTreeMap<String, String>,
    pub enabled: Option<bool>,
    pub startup_timeout_sec: Option<f64>,
    pub tool_timeout_sec: Option<f64>,
    pub enabled_tools: Option<Vec<String>>,
    pub disabled_tools: Option<Vec<String>>,
}

impl McpServerConfig {
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    /// Whether the model may see and call `tool`: `enabled_tools` (when set)
    /// is an allowlist, `disabled_tools` always wins.
    pub fn tool_allowed(&self, tool: &str) -> bool {
        if self.disabled_tools.as_ref().is_some_and(|list| list.iter().any(|name| name == tool)) {
            return false;
        }
        self.enabled_tools.as_ref().is_none_or(|list| list.iter().any(|name| name == tool))
    }
}

/// `[history]`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    /// `save-all` (default) or `none`.
    pub persistence: Option<String>,
}

/// `[projects."<path>"]` (Codex): per-folder settings. Only `trust_level`
/// (`"trusted"` / `"untrusted"`) is used.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    pub trust_level: Option<String>,
}

/// `[profiles.<name>]`: per-profile overrides of the model / policy keys.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigProfile {
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub model_reasoning_effort: Option<String>,
    pub model_reasoning_summary: Option<String>,
    pub model_verbosity: Option<String>,
    pub model_context_window: Option<u64>,
    pub model_auto_compact_token_limit: Option<u64>,
    pub approval_policy: Option<String>,
    pub sandbox_mode: Option<String>,
}

/// The typed, merged `config.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub model_reasoning_effort: Option<String>,
    pub model_reasoning_summary: Option<String>,
    pub model_verbosity: Option<String>,
    pub model_context_window: Option<u64>,
    pub model_auto_compact_token_limit: Option<u64>,

    pub approval_policy: Option<String>,
    pub sandbox_mode: Option<String>,
    pub sandbox_workspace_write: Option<SandboxWorkspaceWrite>,
    pub sandbox: Option<SandboxTable>,
    pub shell_environment_policy: Option<ShellEnvironmentPolicyToml>,

    /// Raw `[model_providers.<id>]` tables (consumed by the provider engine).
    pub model_providers: BTreeMap<String, toml::Value>,
    pub mcp_servers: BTreeMap<String, McpServerConfig>,

    /// Active profile name (`profile = "x"`).
    pub profile: Option<String>,
    pub profiles: BTreeMap<String, ConfigProfile>,

    pub project_doc_max_bytes: Option<u64>,
    pub project_doc_fallback_filenames: Vec<String>,

    /// Program + args run on turn completion; the JSON event is appended as
    /// the last argument.
    pub notify: Vec<String>,
    pub hide_agent_reasoning: Option<bool>,
    pub history: Option<HistoryConfig>,
    /// Per-folder trust decisions, keyed by the canonical folder path.
    pub projects: BTreeMap<String, ProjectConfig>,
}

/// `trust_level` entries of `[projects."<path>"]` across `files`, primary
/// (earliest) file winning per path. Files that are missing or do not parse
/// are skipped, so one broken file never marks a folder trusted.
pub fn project_trust_levels(files: &[PathBuf]) -> BTreeMap<String, String> {
    let mut levels: BTreeMap<String, String> = BTreeMap::new();
    for path in files.iter().rev() {
        let Ok(raw) = std::fs::read_to_string(path) else { continue };
        let Ok(table) = toml::from_str::<toml::Table>(&raw) else { continue };
        let Some(projects) = table.get("projects").and_then(toml::Value::as_table) else { continue };
        for (key, entry) in projects {
            if let Some(level) = entry.get("trust_level").and_then(toml::Value::as_str) {
                levels.insert(key.clone(), level.trim().to_string());
            }
        }
    }
    levels
}

/// Sets (`Some`) or removes (`None`) `[projects."<key>"] trust_level` in one
/// `config.toml`, keeping the rest of the file (comments, order) intact.
/// The file and its directory are created when missing.
pub fn write_project_trust_level(file: &Path, key: &str, level: Option<&str>) -> Result<(), ConfigError> {
    let fail = |message: String| ConfigError { message: format!("{}: {message}", file.display()) };
    let raw = match std::fs::read_to_string(file) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(fail(err.to_string())),
    };
    let mut doc: toml_edit::Document = raw.parse().map_err(|err: toml_edit::TomlError| fail(err.to_string()))?;
    match level {
        Some(level) => {
            if !doc.contains_key("projects") {
                let mut projects = toml_edit::Table::new();
                projects.set_implicit(true);
                doc.insert("projects", toml_edit::Item::Table(projects));
            }
            let projects = doc["projects"]
                .as_table_mut()
                .ok_or_else(|| fail("`projects` is not a table".to_string()))?;
            if !projects.contains_key(key) {
                projects.insert(key, toml_edit::Item::Table(toml_edit::Table::new()));
            }
            let entry = projects[key]
                .as_table_like_mut()
                .ok_or_else(|| fail(format!("`projects.{key}` is not a table")))?;
            entry.insert("trust_level", toml_edit::value(level));
        }
        None => {
            if let Some(entry) = doc
                .get_mut("projects")
                .and_then(|projects| projects.as_table_like_mut())
                .and_then(|projects| projects.get_mut(key))
                .and_then(|entry| entry.as_table_like_mut())
            {
                entry.remove("trust_level");
            }
        }
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|err| fail(err.to_string()))?;
    }
    std::fs::write(file, doc.to_string()).map_err(|err| fail(err.to_string()))
}

/// Loads the merged `[mcp_servers.<name>]` tables from the agent-dir chain
/// (enabled and disabled entries alike).
pub fn load_mcp_servers() -> Result<BTreeMap<String, McpServerConfig>, ConfigError> {
    Ok(Config::load()?.mcp_servers)
}

/// Error from loading or interpreting `config.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigError {
    pub message: String,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ConfigError {}

/// Deep-merges `high` over `low` (tables recurse, everything else replaced).
fn merge_tables(low: &mut toml::Table, high: toml::Table) {
    for (key, value) in high {
        match (low.get_mut(&key), value) {
            (Some(toml::Value::Table(existing)), toml::Value::Table(incoming)) => {
                merge_tables(existing, incoming)
            }
            (_, value) => {
                low.insert(key, value);
            }
        }
    }
}

/// Parses one read-only fallback file without ever failing: a file that is not
/// TOML is skipped, and top-level keys that do not type-check are dropped, each
/// with a warning, so a broken `~/.codex/config.toml` cannot take the valid
/// `~/.locagens` settings down with it.
fn parse_file_lenient(path: &Path, raw: &str) -> Option<toml::Table> {
    let mut table: toml::Table = match toml::from_str(raw) {
        Ok(table) => table,
        Err(err) => {
            tracing::warn!("Ignoring config file {}: {}", path.display(), err.message());
            return None;
        }
    };
    let keys: Vec<String> = table.keys().cloned().collect();
    for key in keys {
        let mut single = toml::Table::new();
        single.insert(key.clone(), table[&key].clone());
        if let Err(err) = toml::Value::Table(single).try_into::<Config>() {
            tracing::warn!("Ignoring key `{key}` in {}: {}", path.display(), err.message());
            table.remove(&key);
        }
    }
    Some(table)
}

/// Parses and validates one `config.toml`, naming `path` on failure.
fn parse_file(path: &Path, raw: &str) -> Result<toml::Table, ConfigError> {
    let table: toml::Table = toml::from_str(raw).map_err(|err| ConfigError {
        message: format!("Invalid config file {}: {}", path.display(), err.message()),
    })?;
    toml::Value::Table(table.clone())
        .try_into::<Config>()
        .map_err(|err| ConfigError {
            message: format!("Invalid config file {}: {}", path.display(), err.message()),
        })?;
    Ok(table)
}

impl Config {
    /// The `config.toml` files to read, primary first.
    pub fn config_files() -> Vec<PathBuf> {
        crate::paths::agent_dir_chain()
            .into_iter()
            .map(|dir| dir.join("config.toml"))
            .collect()
    }

    /// Loads and merges the agent-dir chain.
    pub fn load() -> Result<Config, ConfigError> {
        Self::load_from_files(&Self::config_files())
    }

    /// Loads and merges the given files, `files[0]` winning. Missing files
    /// are skipped. `files[0]` (the primary file) must be valid; the
    /// fallbacks after it are parsed leniently (see [`parse_file_lenient`]).
    pub fn load_from_files(files: &[PathBuf]) -> Result<Config, ConfigError> {
        let mut merged = toml::Table::new();
        // Lowest priority first so higher priority files overwrite.
        for (index, path) in files.iter().enumerate().rev() {
            let Ok(raw) = std::fs::read_to_string(path) else { continue };
            let table = if index == 0 {
                parse_file(path, &raw)?
            } else {
                let Some(table) = parse_file_lenient(path, &raw) else { continue };
                table
            };
            merge_tables(&mut merged, table);
        }
        Self::from_table(merged)
    }

    /// Parses a single document (used by tests and callers with inline TOML).
    pub fn from_toml_str(raw: &str) -> Result<Config, ConfigError> {
        let table = parse_file(Path::new("<inline>"), raw)?;
        Self::from_table(table)
    }

    fn from_table(table: toml::Table) -> Result<Config, ConfigError> {
        toml::Value::Table(table).try_into::<Config>().map_err(|err| ConfigError {
            message: format!("Invalid merged config.toml: {}", err.message()),
        })
    }

    /// Applies the named profile (or the file's own `profile`) over the
    /// top-level keys. An unknown profile name is an error.
    pub fn with_profile(mut self, requested: Option<&str>) -> Result<Config, ConfigError> {
        let name = requested
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .or_else(|| self.profile.clone());
        let Some(name) = name else { return Ok(self) };
        let Some(profile) = self.profiles.get(&name).cloned() else {
            return Err(ConfigError {
                message: format!(
                    "Config profile \"{name}\" not found (known: {}).",
                    if self.profiles.is_empty() {
                        "none".to_string()
                    } else {
                        self.profiles.keys().cloned().collect::<Vec<_>>().join(", ")
                    }
                ),
            });
        };
        macro_rules! overlay {
            ($($field:ident),*) => {$(
                if profile.$field.is_some() { self.$field = profile.$field.clone(); }
            )*};
        }
        overlay!(
            model,
            model_provider,
            model_reasoning_effort,
            model_reasoning_summary,
            model_verbosity,
            model_context_window,
            model_auto_compact_token_limit,
            approval_policy,
            sandbox_mode
        );
        self.profile = Some(name);
        Ok(self)
    }

    // ------------------------------------------------------------------
    // Derived values
    // ------------------------------------------------------------------

    /// Reasoning effort in lcgn vocabulary (`none` -> `off`).
    pub fn reasoning_effort(&self) -> Option<String> {
        let level = self.model_reasoning_effort.as_deref()?.trim().to_lowercase();
        match level.as_str() {
            "" => None,
            "none" => Some("off".to_string()),
            _ => Some(level),
        }
    }

    /// Auto-compaction threshold in estimated tokens: the explicit limit,
    /// else 90% of the context window, else `None` (built-in default).
    pub fn auto_compact_limit(&self) -> Option<usize> {
        if let Some(limit) = self.model_auto_compact_token_limit.filter(|v| *v > 0) {
            return Some(limit as usize);
        }
        self.model_context_window
            .filter(|v| *v > 0)
            .map(|window| (window as f64 * 0.9) as usize)
    }

    /// Run mode implied by `approval_policy` / `sandbox_mode`, if either is set.
    pub fn default_run_mode(&self) -> Option<String> {
        let approval = self.approval_policy.as_deref().map(|v| v.trim().to_lowercase());
        let sandbox = self.sandbox_mode.as_deref().map(|v| v.trim().to_lowercase());
        if approval.is_none() && sandbox.is_none() {
            return None;
        }
        let mode = match (approval.as_deref(), sandbox.as_deref()) {
            (_, Some("danger-full-access")) | (Some("never"), _) => "auto",
            (_, Some("read-only")) => "plan",
            (Some("untrusted"), _) => "ask_permissions",
            _ => "accept_edits",
        };
        Some(mode.to_string())
    }

    /// Whether `history.jsonl` should be written (`history.persistence`).
    pub fn history_persistence_enabled(&self) -> bool {
        !matches!(
            self.history.as_ref().and_then(|h| h.persistence.as_deref()).map(str::trim),
            Some("none")
        )
    }

    /// Whether sandboxed commands may use the network.
    pub fn sandbox_network_access(&self) -> bool {
        self.sandbox_workspace_write.as_ref().map(|s| s.network_access).unwrap_or(false)
    }

    /// Whether reasoning output should be rendered.
    pub fn show_reasoning(&self) -> bool {
        !self.hide_agent_reasoning.unwrap_or(false)
    }

    /// The `[mcp_servers]` entry for `name` (trimmed, case-insensitive).
    pub fn mcp_server(&self, name: &str) -> Option<&McpServerConfig> {
        let wanted = name.trim().to_lowercase();
        self.mcp_servers
            .iter()
            .find(|(key, _)| key.trim().to_lowercase() == wanted)
            .map(|(_, server)| server)
    }

    // ------------------------------------------------------------------
    // Display
    // ------------------------------------------------------------------

    /// The effective config as TOML with secrets masked.
    pub fn to_masked_toml(&self) -> String {
        let mut json = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
        strip_nulls(&mut json);
        let mut value = toml::Value::try_from(json).unwrap_or_else(|_| toml::Value::Table(Default::default()));
        mask_secrets(&mut value, false);
        toml::to_string_pretty(&value).unwrap_or_default()
    }
}

/// Removes `null`s (unset options) so the document serializes as TOML.
fn strip_nulls(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.retain(|_, child| !child.is_null());
            map.values_mut().for_each(strip_nulls);
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(strip_nulls),
        _ => {}
    }
}

/// Table names whose string values are always secret-bearing.
const SECRET_TABLES: [&str; 4] = ["env", "http_headers", "set", "env_http_headers"];

fn key_is_secret(key: &str) -> bool {
    let key = key.to_lowercase();
    // Names of environment variables are not secrets themselves.
    if key.ends_with("_env") || key.ends_with("env_key") || key.ends_with("env_var") || key == "env_vars" {
        return false;
    }
    ["key", "token", "secret", "password", "authorization", "credential"]
        .iter()
        .any(|needle| key.contains(needle))
}

fn mask_secrets(value: &mut toml::Value, inside_secret_table: bool) {
    match value {
        toml::Value::Table(table) => {
            for (key, child) in table.iter_mut() {
                let secret_table = SECRET_TABLES.contains(&key.as_str());
                if (inside_secret_table || key_is_secret(key)) && child.is_str() {
                    *child = toml::Value::String("***".to_string());
                } else {
                    mask_secrets(child, secret_table);
                }
            }
        }
        toml::Value::Array(items) => {
            mask_argument_list(items);
            for item in items {
                mask_secrets(item, false);
            }
        }
        toml::Value::String(text) => *text = mask_url_credentials(text),
        _ => {}
    }
}

/// Masks secrets inside a command-line style list (`args`, `notify`):
/// `--token=abc`, `API_KEY=abc`, and the value after a bare `--api-key` flag.
fn mask_argument_list(items: &mut [toml::Value]) {
    let mut mask_next = false;
    for item in items.iter_mut() {
        let Some(text) = item.as_str().map(str::to_string) else {
            mask_next = false;
            continue;
        };
        if mask_next {
            *item = toml::Value::String("***".to_string());
            mask_next = false;
            continue;
        }
        let flag = text.trim_start_matches('-');
        match flag.split_once('=') {
            Some((name, _)) if key_is_secret(name) && !text.contains("://") => {
                *item = toml::Value::String(format!("{}=***", &text[..text.len() - flag.len() + name.len()]));
            }
            None if text.starts_with('-') && key_is_secret(flag) => mask_next = true,
            _ => {}
        }
    }
}

/// Masks `user:password@` in URLs and the values of secret-looking query
/// parameters (`?token=...&api_key=...`).
fn mask_url_credentials(text: &str) -> String {
    let Some(scheme_end) = text.find("://") else { return text.to_string() };
    let (scheme, rest) = text.split_at(scheme_end + 3);
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let authority = match authority.rfind('@') {
        Some(at) => format!("***{}", &authority[at..]),
        None => authority.to_string(),
    };
    let tail = match tail.split_once('?') {
        Some((path, query)) => {
            let (query, fragment) = match query.split_once('#') {
                Some((query, fragment)) => (query, Some(fragment)),
                None => (query, None),
            };
            let masked: Vec<String> = query
                .split('&')
                .map(|pair| match pair.split_once('=') {
                    Some((name, _)) if key_is_secret(name) => format!("{name}=***"),
                    _ => pair.to_string(),
                })
                .collect();
            let mut out = format!("{path}?{}", masked.join("&"));
            if let Some(fragment) = fragment {
                out.push('#');
                out.push_str(fragment);
            }
            out
        }
        None => tail.to_string(),
    };
    format!("{scheme}{authority}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn chain_merge_primary_wins_and_tables_deep_merge() {
        let tmp = tempfile::tempdir().unwrap();
        let primary = write(
            &tmp.path().join("a"),
            "config.toml",
            "model = \"primary-model\"\n[sandbox_workspace_write]\nnetwork_access = true\n",
        );
        let fallback = write(
            &tmp.path().join("b"),
            "config.toml",
            "model = \"fallback-model\"\nmodel_provider = \"fb\"\n[sandbox_workspace_write]\nwritable_roots = [\"/x\"]\nnetwork_access = false\n",
        );
        let missing = tmp.path().join("c").join("config.toml");
        let config = Config::load_from_files(&[primary, fallback, missing]).unwrap();
        assert_eq!(config.model.as_deref(), Some("primary-model"));
        assert_eq!(config.model_provider.as_deref(), Some("fb"));
        let sandbox = config.sandbox_workspace_write.unwrap();
        assert!(sandbox.network_access);
        assert_eq!(sandbox.writable_roots, vec!["/x".to_string()]);
    }

    #[test]
    fn unknown_keys_are_ignored_and_malformed_names_the_file() {
        let config = Config::from_toml_str("future_key = 3
model = \"m\"
[unknown]
a = 1
").unwrap();
        assert_eq!(config.model.as_deref(), Some("m"));

        let tmp = tempfile::tempdir().unwrap();
        let bad = write(tmp.path(), "config.toml", "model = = oops");
        let err = Config::load_from_files(&[bad.clone()]).unwrap_err();
        assert!(err.message.contains(&bad.display().to_string()), "{err}");
        let bad_type = write(tmp.path(), "type.toml", "model_context_window = \"big\"");
        let err = Config::load_from_files(&[bad_type.clone()]).unwrap_err();
        assert!(err.message.contains("type.toml"), "{err}");
    }

    #[test]
    fn profiles_overlay_top_level_keys() {
        let raw = "model = \"base\"\nprofile = \"fast\"\napproval_policy = \"on-request\"\n\
                   [profiles.fast]\nmodel = \"quick\"\napproval_policy = \"never\"\n\
                   [profiles.deep]\nmodel_reasoning_effort = \"high\"\n";
        let config = Config::from_toml_str(raw).unwrap();
        let fast = config.clone().with_profile(None).unwrap();
        assert_eq!(fast.model.as_deref(), Some("quick"));
        assert_eq!(fast.default_run_mode().as_deref(), Some("auto"));
        let deep = config.clone().with_profile(Some("deep")).unwrap();
        assert_eq!(deep.model.as_deref(), Some("base"));
        assert_eq!(deep.reasoning_effort().as_deref(), Some("high"));
        let err = config.with_profile(Some("nope")).unwrap_err();
        assert!(err.message.contains("nope"));
    }

    #[test]
    fn derived_values_map_codex_keys_onto_locagens() {
        let config = Config::from_toml_str(
            "model_context_window = 100000\nsandbox_mode = \"read-only\"\nmodel_reasoning_effort = \"none\"\n\
             hide_agent_reasoning = true\n[history]\npersistence = \"none\"\n",
        )
        .unwrap();
        assert_eq!(config.auto_compact_limit(), Some(90_000));
        assert_eq!(config.default_run_mode().as_deref(), Some("plan"));
        assert_eq!(config.reasoning_effort().as_deref(), Some("off"));
        assert!(!config.show_reasoning());
        assert!(!config.history_persistence_enabled());

        let explicit = Config::from_toml_str("model_auto_compact_token_limit = 5000\nmodel_context_window = 100000").unwrap();
        assert_eq!(explicit.auto_compact_limit(), Some(5000));
        assert_eq!(Config::default().auto_compact_limit(), None);
        assert!(Config::default().history_persistence_enabled());
        assert_eq!(
            Config::from_toml_str("approval_policy = \"untrusted\"").unwrap().default_run_mode().as_deref(),
            Some("ask_permissions")
        );
        assert_eq!(Config::default().default_run_mode(), None);
    }

    #[test]
    fn mcp_servers_parse_and_secrets_are_masked_in_show() {
        let config = Config::from_toml_str(
            "[mcp_servers.fs]\ncommand = \"npx\"\nargs = [\"-y\", \"srv\"]\n[mcp_servers.fs.env]\nAPI_TOKEN = \"hunter2\"\n\
             [mcp_servers.web]\nurl = \"https://x.test/mcp\"\nbearer_token_env_var = \"WEB_TOKEN\"\nenabled = false\n\
             [mcp_servers.web.http_headers]\nX-Auth = \"abc123\"\n\
             [model_providers.p]\nname = \"P\"\nexperimental_bearer_token = \"sk-live\"\nenv_key = \"P_KEY\"\n\
             [shell_environment_policy]\ninherit = \"core\"\n[shell_environment_policy.set]\nFOO = \"bar-secret\"\n",
        )
        .unwrap();
        assert_eq!(config.mcp_servers["fs"].args, vec!["-y", "srv"]);
        assert_eq!(config.mcp_server("FS").unwrap().args, vec!["-y", "srv"]);
        let shown = config.to_masked_toml();
        for leaked in ["hunter2", "abc123", "sk-live", "bar-secret"] {
            assert!(!shown.contains(leaked), "leaked {leaked}:\n{shown}");
        }
        assert!(shown.contains("WEB_TOKEN"), "env var names stay visible:\n{shown}");
        assert!(shown.contains("P_KEY"));
        assert!(shown.contains("***"));
    }

    #[test]
    fn project_trust_levels_roundtrip_and_keep_the_rest_of_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let primary = write(
            &tmp.path().join("a"),
            "config.toml",
            "# my settings\nmodel = \"m\"\n\n[projects.'C:\\work\\x']\ntrust_level = \"untrusted\"\n",
        );
        let fallback = write(
            &tmp.path().join("b"),
            "config.toml",
            "[projects.\"C:\\\\work\\\\x\"]\ntrust_level = \"trusted\"\n[projects.\"/srv/y\"]\ntrust_level = \"trusted\"\n",
        );
        let broken = write(&tmp.path().join("c"), "config.toml", "[projects.\"/evil\"]\ntrust_level = = \"trusted\"\n");
        let levels = project_trust_levels(&[primary.clone(), fallback, broken]);
        assert_eq!(levels.get("C:\\work\\x").map(String::as_str), Some("untrusted"), "primary wins");
        assert_eq!(levels.get("/srv/y").map(String::as_str), Some("trusted"));
        assert!(!levels.contains_key("/evil"), "unparsable files are ignored");

        write_project_trust_level(&primary, "C:\\work\\x", Some("trusted")).unwrap();
        write_project_trust_level(&primary, "/new/project", Some("trusted")).unwrap();
        let text = std::fs::read_to_string(&primary).unwrap();
        assert!(text.starts_with("# my settings\nmodel = \"m\""), "{text}");
        let config = Config::from_toml_str(&text).unwrap();
        assert_eq!(config.projects["C:\\work\\x"].trust_level.as_deref(), Some("trusted"));
        assert_eq!(config.projects["/new/project"].trust_level.as_deref(), Some("trusted"));

        write_project_trust_level(&primary, "/new/project", None).unwrap();
        let levels = project_trust_levels(&[primary.clone()]);
        assert!(!levels.contains_key("/new/project"));

        let created = tmp.path().join("fresh").join("config.toml");
        write_project_trust_level(&created, "/p", Some("trusted")).unwrap();
        assert_eq!(project_trust_levels(&[created]).get("/p").map(String::as_str), Some("trusted"));
    }

    #[test]
    fn bad_fallback_files_are_skipped_but_a_bad_primary_still_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let primary = write(&tmp.path().join("a"), "config.toml", "model = \"mine\"\nmodel_provider = \"p\"\n");
        let broken_toml = write(&tmp.path().join("b"), "config.toml", "model = = oops");
        let bad_key = write(
            &tmp.path().join("c"),
            "config.toml",
            "model_context_window = \"big\"\nproject_doc_max_bytes = 5\nsandbox_mode = \"read-only\"\n",
        );
        let config = Config::load_from_files(&[primary.clone(), broken_toml, bad_key]).unwrap();
        assert_eq!(config.model.as_deref(), Some("mine"));
        assert_eq!(config.model_provider.as_deref(), Some("p"));
        // Valid keys of a partly bad fallback survive; the bad key is dropped.
        assert_eq!(config.project_doc_max_bytes, Some(5));
        assert_eq!(config.sandbox_mode.as_deref(), Some("read-only"));
        assert_eq!(config.model_context_window, None);

        // A broken primary file is still a hard error naming the file.
        let broken_primary = write(&tmp.path().join("d"), "config.toml", "model = = oops");
        let err = Config::load_from_files(&[broken_primary.clone(), primary]).unwrap_err();
        assert!(err.message.contains(&broken_primary.display().to_string()), "{err}");
    }

    #[test]
    fn mcp_tool_filters_apply_allow_then_deny() {
        let config = Config::from_toml_str(
            "[mcp_servers.a]\ncommand = \"x\"\nenabled_tools = [\"read\", \"write\"]\ndisabled_tools = [\"write\"]\n\
             [mcp_servers.b]\ncommand = \"y\"\ndisabled_tools = [\"rm\"]\n",
        )
        .unwrap();
        let a = config.mcp_server("a").unwrap();
        assert!(a.tool_allowed("read"));
        assert!(!a.tool_allowed("write"), "deny wins over allow");
        assert!(!a.tool_allowed("other"), "not in the allowlist");
        let b = config.mcp_server("b").unwrap();
        assert!(b.tool_allowed("anything") && !b.tool_allowed("rm"));
    }

    #[test]
    fn masking_covers_array_items_and_url_credentials() {
        let config = Config::from_toml_str(
            "notify = [\"notifier\", \"--api-key\", \"sk-secret-1\", \"--token=sk-secret-2\", \"PASSWORD=sk-secret-3\"]\n\
             [mcp_servers.s]\ncommand = \"npx\"\nargs = [\"-y\", \"srv\", \"--secret\", \"sk-secret-4\", \"--verbose\"]\n\
             [mcp_servers.r]\nurl = \"https://user:sk-secret-5@host.test/mcp?token=sk-secret-6&mode=fast\"\n",
        )
        .unwrap();
        let shown = config.to_masked_toml();
        for leaked in ["sk-secret-1", "sk-secret-2", "sk-secret-3", "sk-secret-4", "sk-secret-5", "sk-secret-6"] {
            assert!(!shown.contains(leaked), "leaked {leaked}:\n{shown}");
        }
        for kept in ["notifier", "--verbose", "host.test/mcp", "mode=fast", "--token=***"] {
            assert!(shown.contains(kept), "lost {kept}:\n{shown}");
        }
    }
}
