//! Locagens paths: `locagens_config::paths`, plus the `config.toml` chain.
//!
//! Unit tests of this crate never see the developer's real `~/.locagens`,
//! `~/.codex`, `~/.pi/agent` or app-data folders: under `cfg(test)` every
//! home-derived path resolves inside a per-process temp directory (QA-071).
//! Core code must therefore go through `crate::paths`, never
//! `locagens_config::paths` directly.

use std::path::PathBuf;

pub use locagens_config::paths::*;

#[cfg(test)]
pub use test_home::{
    agent_dir_chain, app_support_dir, codex_auth_path, codex_config_path, codex_home, default_provider_config_path,
    default_settings_path, locagens_home, mcp_config_path, opencode_data_dir, pi_agent_dir, pi_skills_root,
    plugins_root, skills_root,
};

/// The user's home directory (the isolated test home under `cfg(test)`).
pub fn user_home() -> PathBuf {
    #[cfg(test)]
    return test_home::root();
    #[cfg(not(test))]
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// The `config.toml` files of the agent-dir chain, primary first.
pub fn config_files() -> Vec<PathBuf> {
    agent_dir_chain().into_iter().map(|dir| dir.join("config.toml")).collect()
}

#[cfg(test)]
mod test_home {
    use std::path::PathBuf;

    /// The isolated home shared by every test of this process.
    pub fn root() -> PathBuf {
        std::env::temp_dir().join(format!("locagens-core-test-home-{}", std::process::id()))
    }

    pub fn locagens_home() -> PathBuf {
        root().join(".locagens")
    }

    pub fn codex_home() -> PathBuf {
        root().join(".codex")
    }

    pub fn pi_agent_dir() -> PathBuf {
        root().join(".pi").join("agent")
    }

    pub fn agent_dir_chain() -> Vec<PathBuf> {
        vec![locagens_home(), codex_home(), pi_agent_dir()]
    }

    pub fn codex_auth_path() -> PathBuf {
        codex_home().join("auth.json")
    }

    pub fn codex_config_path() -> PathBuf {
        codex_home().join("config.toml")
    }

    pub fn app_support_dir() -> PathBuf {
        root().join("app-support")
    }

    pub fn default_settings_path() -> PathBuf {
        app_support_dir().join("settings.json")
    }

    pub fn default_provider_config_path() -> PathBuf {
        app_support_dir().join("providers.json")
    }

    pub fn skills_root() -> PathBuf {
        app_support_dir().join("skills")
    }

    pub fn pi_skills_root() -> PathBuf {
        pi_agent_dir().join("skills")
    }

    pub fn plugins_root() -> PathBuf {
        app_support_dir().join("plugins")
    }

    pub fn mcp_config_path() -> PathBuf {
        app_support_dir().join("mcp_servers.json")
    }

    pub fn opencode_data_dir() -> PathBuf {
        root().join(".local").join("share").join("opencode")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn unit_tests_never_resolve_the_real_home() {
        let test_home = super::test_home::root();
        for path in super::agent_dir_chain()
            .into_iter()
            .chain(super::config_files())
            .chain([super::plugins_root(), super::skills_root(), super::mcp_config_path(), super::default_settings_path()])
        {
            assert!(path.starts_with(&test_home), "{} escapes the test home", path.display());
        }
    }
}
