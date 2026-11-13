//! Process-wide effective `config.toml` (agent-dir chain + selected profile).
//!
//! The host installs the config at startup ([`init_from_disk`] from
//! `build_core`, again by `lcgn` after `--profile`) and [`reload_if_changed`]
//! picks up edits to any `config.toml` of the chain at the start of each run.
//! Until a config is installed [`current`] returns built-in defaults, which
//! keeps unit tests independent of the machine's real config files.

use locagens_config::config_toml::{Config, ShellEnvironmentPolicyToml};
use locagens_execpolicy::env_policy::{Inherit, ShellEnvironmentPolicy};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::SystemTime;

static CURRENT: RwLock<Option<Arc<Config>>> = RwLock::new(None);

/// `(mtime, size)` of every file of the config chain, in chain order.
type ChainSignature = Vec<Option<(SystemTime, u64)>>;

/// Signature of the files the installed config was loaded from.
static LOADED: Mutex<Option<ChainSignature>> = Mutex::new(None);

fn chain_signature(files: &[PathBuf]) -> ChainSignature {
    files
        .iter()
        .map(|path| {
            let meta = std::fs::metadata(path).ok()?;
            Some((meta.modified().ok()?, meta.len()))
        })
        .collect()
}

/// The installed config, or defaults when none was installed.
pub fn current() -> Arc<Config> {
    CURRENT
        .read()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_default()
}

/// Installs `config` process-wide and applies its shell environment policy.
pub fn install(config: Config) {
    locagens_execpolicy::env_policy::install(shell_policy(&config));
    if let Ok(mut slot) = CURRENT.write() {
        *slot = Some(Arc::new(config));
    }
}

/// Loads the chain from disk (with the file's own `profile`) and installs it
/// unless a config is already installed.
/// A broken config is logged and ignored so the desktop app still starts.
pub fn init_from_disk() {
    if CURRENT.read().map(|slot| slot.is_some()).unwrap_or(false) {
        return; // the host (lcgn --profile) already installed its own
    }
    let files = crate::paths::config_files();
    let signature = chain_signature(&files);
    match Config::load_from_files(&files).and_then(|config| config.with_profile(None)) {
        Ok(config) => {
            install(config);
            *LOADED.lock().unwrap_or_else(PoisonError::into_inner) = Some(signature);
        }
        Err(err) => tracing::warn!("{err}"),
    }
}

/// Re-reads the config chain when any of its files changed since the
/// installed config was loaded (QA-042). The selected profile is kept; a
/// config that no longer parses is logged and the previous one stays active.
/// Cheap (a few `stat`s) — called at the start of every run, so edits to
/// `config.toml` reach a long-running desktop without a restart.
pub fn reload_if_changed() {
    if !CURRENT.read().map(|slot| slot.is_some()).unwrap_or(false) {
        return; // no host installed a config (unit tests): stay on defaults
    }
    let files = crate::paths::config_files();
    let profile = current().profile.clone();
    let mut loaded = LOADED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(config) = changed_config(&files, profile.as_deref(), &mut loaded) {
        drop(loaded);
        install(config);
    }
}

/// The reloaded config when `files` differ from `last_seen` (which is then
/// updated). Parse errors keep the old config and are logged once per change.
fn changed_config(files: &[PathBuf], profile: Option<&str>, last_seen: &mut Option<ChainSignature>) -> Option<Config> {
    let signature = chain_signature(files);
    if last_seen.as_ref() == Some(&signature) {
        return None;
    }
    *last_seen = Some(signature);
    match Config::load_from_files(files).and_then(|config| config.with_profile(profile)) {
        Ok(config) => Some(config),
        Err(err) => {
            tracing::warn!("config.toml reload failed, keeping the previous config: {err}");
            None
        }
    }
}

/// AGENTS.md discovery knobs from `project_doc_max_bytes` /
/// `project_doc_fallback_filenames`.
pub fn doc_options() -> locagens_context::agents_md::DocOptions {
    let config = current();
    doc_options_for(&config)
}

pub fn doc_options_for(config: &Config) -> locagens_context::agents_md::DocOptions {
    let defaults = locagens_context::agents_md::DocOptions::default();
    locagens_context::agents_md::DocOptions {
        max_bytes: config.project_doc_max_bytes.map(|v| v as usize).unwrap_or(defaults.max_bytes),
        fallback_filenames: config.project_doc_fallback_filenames.clone(),
    }
}

/// Whether sandboxed commands may use the network regardless of run mode
/// (`[sandbox_workspace_write] network_access`).
pub fn sandbox_network_access() -> bool {
    current().sandbox_network_access()
}

/// `[sandbox_workspace_write] writable_roots`, `~` expanded.
pub fn sandbox_writable_roots() -> Vec<std::path::PathBuf> {
    let config = current();
    let Some(table) = config.sandbox_workspace_write.as_ref() else { return Vec::new() };
    table
        .writable_roots
        .iter()
        .map(|root| root.trim())
        .filter(|root| !root.is_empty())
        .map(|root| match root.strip_prefix("~/").or_else(|| root.strip_prefix("~\\")) {
            Some(rest) => crate::util::home_dir().join(rest),
            None => std::path::PathBuf::from(root),
        })
        .collect()
}

/// `[sandbox.network] allowed_domains` from the merged config chain.
pub fn sandbox_allowed_domains() -> Vec<String> {
    current().sandbox.as_ref().map(|sandbox| sandbox.network.allowed_domains.clone()).unwrap_or_default()
}

/// Program + args for the `notify` command with the turn-complete event
/// appended as the last argument (Codex contract). `None` when unset.
pub fn notify_invocation(config: &Config, event: &serde_json::Value) -> Option<(String, Vec<String>)> {
    let (program, rest) = config.notify.split_first()?;
    if program.trim().is_empty() {
        return None;
    }
    let mut args = rest.to_vec();
    args.push(event.to_string());
    Some((program.clone(), args))
}

/// Fires the configured `notify` command for a finished turn (detached,
/// failures only logged).
pub fn notify_turn_complete(session_id: &str, cwd: &str, last_assistant_message: &str) {
    let event = serde_json::json!({
        "type": "agent-turn-complete",
        "thread-id": session_id,
        "cwd": cwd,
        "last-assistant-message": last_assistant_message,
    });
    let Some((program, args)) = notify_invocation(&current(), &event) else { return };
    let mut command = std::process::Command::new(&program);
    command
        .args(&args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match command.spawn() {
        Ok(mut child) => {
            // Reap in the background so the child never lingers as a zombie.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(err) => tracing::warn!("notify command {program:?} failed to start: {err}"),
    }
}

/// Converts the TOML policy into the execpolicy type (`None` when unset).
pub fn shell_policy(config: &Config) -> Option<ShellEnvironmentPolicy> {
    let toml: &ShellEnvironmentPolicyToml = config.shell_environment_policy.as_ref()?;
    let inherit = match toml.inherit.as_deref().map(str::trim) {
        Some("core") => Inherit::Core,
        Some("none") => Inherit::None,
        _ => Inherit::All,
    };
    Some(ShellEnvironmentPolicy {
        inherit,
        ignore_default_excludes: toml.ignore_default_excludes.unwrap_or(false),
        exclude: toml.exclude.clone(),
        set: toml.set.clone().into_iter().collect(),
        include_only: toml.include_only.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_options_and_notify_follow_config() {
        let config = Config::from_toml_str(
            "project_doc_max_bytes = 100
project_doc_fallback_filenames = [\"TEAM.md\"]
notify = [\"my-notifier\", \"--flag\"]
",
        )
        .unwrap();
        let options = doc_options_for(&config);
        assert_eq!(options.max_bytes, 100);
        assert_eq!(options.fallback_filenames, vec!["TEAM.md".to_string()]);
        assert_eq!(doc_options_for(&Config::default()).max_bytes, 32 * 1024);

        let event = serde_json::json!({ "type": "agent-turn-complete" });
        let (program, args) = notify_invocation(&config, &event).unwrap();
        assert_eq!(program, "my-notifier");
        assert_eq!(args[0], "--flag");
        assert!(args[1].contains("agent-turn-complete"));
        assert!(notify_invocation(&Config::default(), &event).is_none());
    }

    #[test]
    fn config_edits_are_picked_up_and_bad_edits_keep_the_old_config() {
        let dir = std::env::temp_dir().join(format!("locagens-cfg-reload-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.toml");
        let files = vec![file.clone()];
        std::fs::write(&file, "model = \"one\"\nprofile = \"p\"\n[profiles.p]\nmodel_provider = \"prov\"\n").unwrap();

        let mut seen = None;
        let first = changed_config(&files, None, &mut seen).expect("first load");
        assert_eq!(first.model.as_deref(), Some("one"));
        assert_eq!(first.model_provider.as_deref(), Some("prov"));
        assert!(changed_config(&files, None, &mut seen).is_none(), "unchanged file is not re-read");

        std::fs::write(&file, "model = \"two-longer\"\nprofile = \"p\"\n[profiles.p]\nmodel_provider = \"prov\"\n").unwrap();
        let second = changed_config(&files, Some("p"), &mut seen).expect("edited file reloads");
        assert_eq!(second.model.as_deref(), Some("two-longer"));
        assert_eq!(second.profile.as_deref(), Some("p"), "the profile is kept");

        std::fs::write(&file, "model = = broken, now longer").unwrap();
        assert!(changed_config(&files, Some("p"), &mut seen).is_none(), "a broken edit keeps the old config");
        assert!(changed_config(&files, Some("p"), &mut seen).is_none(), "and is not retried until the next edit");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shell_policy_maps_inherit_and_lists() {
        let config = Config::from_toml_str(
            "[shell_environment_policy]\ninherit = \"core\"\nexclude = [\"A*\"]\ninclude_only = [\"PATH\"]\n[shell_environment_policy.set]\nX = \"1\"\n",
        )
        .unwrap();
        let policy = shell_policy(&config).unwrap();
        assert_eq!(policy.inherit, Inherit::Core);
        assert_eq!(policy.exclude, vec!["A*".to_string()]);
        assert_eq!(policy.set["X"], "1");
        assert!(shell_policy(&Config::default()).is_none());
    }
}
