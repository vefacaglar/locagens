//! Project trust (Codex `[projects."<path>"] trust_level`).
//!
//! A project folder can ship executable configuration: `.locagens/hooks.json`,
//! `.locagens/mcp.json`, enabled plugins under `.locagens/plugins`,
//! `.locagens/rules/*.rules` and `.locagens/agents/*.toml`. None of it is
//! loaded until the user trusts the folder; untrusted projects run with
//! user-scope configuration only.
//!
//! Decisions are stored in the user's `~/.locagens/config.toml`
//! (`$LOCAGENS_HOME`) as
//!
//! ```toml
//! [projects.'C:\repos\app']
//! trust_level = "trusted"
//! ```
//!
//! and read from the whole agent-dir chain (`~/.locagens`, `~/.codex`,
//! `~/.pi/agent`, primary first), so a folder trusted in Codex is trusted
//! here too. The nearest entry wins: trusting a parent folder trusts every
//! folder below it unless a closer entry says `untrusted`. `lcgn --trust`
//! trusts a folder for the current process only.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

pub const TRUSTED: &str = "trusted";
pub const UNTRUSTED: &str = "untrusted";

/// Folders trusted for this process only (`lcgn --trust`).
static SESSION_TRUSTED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Canonical folder path used as the `[projects."<key>"]` key.
pub fn trust_key(project: &Path) -> Option<String> {
    let canonical = std::fs::canonicalize(project).ok()?;
    Some(crate::util::normalize_canonical(canonical).display().to_string())
}

/// Comparison form of a key: `/` separators, no trailing separator, and
/// lower case on the case-insensitive file systems of Windows and macOS.
fn comparable(key: &str) -> String {
    let unified = key.trim().replace('\\', "/");
    let trimmed = if unified.len() > 1 { unified.trim_end_matches('/') } else { unified.as_str() };
    if cfg!(any(windows, target_os = "macos")) {
        trimmed.to_lowercase()
    } else {
        trimmed.to_string()
    }
}

/// Whether project-controlled executable config of `project` may be loaded.
pub fn is_trusted(project: &Path) -> bool {
    let Some(key) = trust_key(project) else { return false };
    let session = SESSION_TRUSTED.lock().map(|set| set.clone()).unwrap_or_default();
    let levels: BTreeMap<String, String> = store::levels()
        .into_iter()
        .map(|(key, level)| (comparable(&key), level))
        .collect();
    for ancestor in Path::new(&key).ancestors() {
        let candidate = comparable(&ancestor.display().to_string());
        if candidate.is_empty() {
            continue;
        }
        if session.contains(&candidate) {
            return true;
        }
        if let Some(level) = levels.get(&candidate) {
            return level == TRUSTED;
        }
    }
    false
}

/// `is_trusted` for the `Option<&str>` project paths of the services.
pub fn is_trusted_str(project: Option<&str>) -> bool {
    project
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .is_some_and(|path| is_trusted(Path::new(path)))
}

/// Persists a trust decision in `~/.locagens/config.toml`.
pub fn set_trusted(project: &Path, trusted: bool) -> Result<(), String> {
    let key = trust_key(project).ok_or_else(|| format!("Folder {} is not accessible.", project.display()))?;
    let level = if trusted { TRUSTED } else { UNTRUSTED };
    // Serializes the read-modify-write of config.toml within this process.
    static WRITE_LOCK: Mutex<()> = Mutex::new(());
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    locagens_config::config_toml::write_project_trust_level(&store::write_file(), &key, Some(level))
        .map_err(|err| err.to_string())?;
    if !trusted {
        if let Ok(mut set) = SESSION_TRUSTED.lock() {
            set.remove(&comparable(&key));
        }
    }
    Ok(())
}

/// Trusts `project` for the rest of this process without persisting it.
pub fn trust_for_session(project: &Path) {
    if let Some(key) = trust_key(project) {
        if let Ok(mut set) = SESSION_TRUSTED.lock() {
            set.insert(comparable(&key));
        }
    }
}

/// Project-controlled executable config files that exist in `project`
/// (what an untrusted project would run if it were trusted).
pub fn project_config_files(project: &Path) -> Vec<PathBuf> {
    let dot = project.join(".locagens");
    let mut found: Vec<PathBuf> = ["hooks.json", "mcp.json"]
        .iter()
        .map(|name| dot.join(name))
        .filter(|path| path.is_file())
        .collect();
    let files_in = |dir: PathBuf, extension: &str| -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == extension))
            .collect();
        files.sort();
        files
    };
    found.extend(files_in(dot.join("rules"), "rules"));
    found.extend(files_in(dot.join("agents"), "toml"));
    if let Ok(entries) = std::fs::read_dir(dot.join("plugins")) {
        let mut manifests: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path().join(crate::services::plugins::MANIFEST_FILENAME))
            .filter(|path| path.is_file())
            .collect();
        manifests.sort();
        found.extend(manifests);
    }
    found
}

/// Untrusted project with executable config: the files that are ignored.
/// Empty when the project is trusted or ships no such files.
pub fn ignored_config_files(project: &Path) -> Vec<PathBuf> {
    let files = project_config_files(project);
    if files.is_empty() || is_trusted(project) {
        return Vec::new();
    }
    files
}

/// Where trust decisions are read from and written to. Unit tests use a
/// private temp store so they never touch the real `~/.locagens`.
mod store {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[cfg(not(test))]
    pub fn read_files() -> Vec<PathBuf> {
        crate::paths::config_files()
    }

    #[cfg(not(test))]
    pub fn write_file() -> PathBuf {
        crate::paths::locagens_home().join("config.toml")
    }

    #[cfg(test)]
    pub fn read_files() -> Vec<PathBuf> {
        vec![write_file()]
    }

    #[cfg(test)]
    pub fn write_file() -> PathBuf {
        static FILE: std::sync::LazyLock<PathBuf> = std::sync::LazyLock::new(|| {
            std::env::temp_dir()
                .join(format!("locagens-trust-store-{}", uuid::Uuid::new_v4().simple()))
                .join("config.toml")
        });
        FILE.clone()
    }

    pub fn levels() -> BTreeMap<String, String> {
        locagens_config::config_toml::project_trust_levels(&read_files())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_project(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locagens-trust-{label}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn folders_start_untrusted_and_trust_persists_and_inherits() {
        let parent = temp_project("parent");
        let child = parent.join("child");
        std::fs::create_dir_all(&child).unwrap();
        assert!(!is_trusted(&parent));
        assert!(!is_trusted(&child));
        assert!(!is_trusted_str(None));

        set_trusted(&parent, true).unwrap();
        assert!(is_trusted(&parent));
        assert!(is_trusted(&child), "a trusted parent trusts its subfolders");
        assert!(is_trusted_str(Some(&child.display().to_string())));

        // A closer `untrusted` entry wins over the trusted parent.
        set_trusted(&child, false).unwrap();
        assert!(!is_trusted(&child));
        assert!(is_trusted(&parent));
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn session_trust_is_not_persisted() {
        let project = temp_project("session");
        trust_for_session(&project);
        assert!(is_trusted(&project));
        let key = trust_key(&project).unwrap();
        assert!(!store::levels().contains_key(&key));
        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn ignored_config_files_lists_project_executable_config() {
        let project = temp_project("files");
        assert!(ignored_config_files(&project).is_empty(), "no config, nothing to report");
        let dot = project.join(".locagens");
        std::fs::create_dir_all(dot.join("rules")).unwrap();
        std::fs::create_dir_all(dot.join("agents")).unwrap();
        std::fs::create_dir_all(dot.join("plugins").join("p")).unwrap();
        std::fs::write(dot.join("hooks.json"), "{}").unwrap();
        std::fs::write(dot.join("rules").join("x.rules"), "").unwrap();
        std::fs::write(dot.join("agents").join("r.toml"), "").unwrap();
        std::fs::write(dot.join("plugins").join("p").join("plugin.json"), "{}").unwrap();
        std::fs::write(dot.join("snapshots.json"), "[]").unwrap();
        let ignored = ignored_config_files(&project);
        assert_eq!(ignored.len(), 4, "{ignored:?}");
        trust_for_session(&project);
        assert!(ignored_config_files(&project).is_empty());
        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn missing_folders_are_never_trusted() {
        let missing = std::env::temp_dir().join(format!("locagens-trust-missing-{}", uuid::Uuid::new_v4()));
        assert!(!is_trusted(&missing));
        assert!(set_trusted(&missing, true).is_err());
    }
}
