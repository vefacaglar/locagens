//! Credential locations kept away from sandboxed commands (QA-020).
//!
//! What each backend does with them:
//!
//! | Backend | Reads | Writes |
//! | --- | --- | --- |
//! | macOS Seatbelt | denied (`deny file-read*`) | denied |
//! | Linux Bubblewrap | hidden (empty tmpfs / `/dev/null` bind) | denied |
//! | Windows restricted token | **not restricted** | deny-write ACE where a writable root covers them |
//!
//! Windows cannot deny reads here: the sandbox token is *write-restricted*,
//! so its restricting SIDs (the workspace capability SID among them) are
//! only consulted for the rights `GENERIC_WRITE` maps to. A deny-read ACE
//! for the capability SID is never evaluated for reads (and `DELETE` is not
//! such a right either, so deletes are not restricted by the token at all);
//! only a different token design (AppContainer, a separate user, integrity
//! labels) could hide files from reads.

use std::path::{Path, PathBuf};

/// Credential files and directories under the user's home.
pub const HOME_CREDENTIALS: [&str; 14] = [
    ".ssh",
    ".aws",
    ".azure",
    ".config/gcloud",
    ".config/gh",
    ".git-credentials",
    ".netrc",
    "_netrc",
    ".docker/config.json",
    ".kube/config",
    ".pypirc",
    ".cargo/credentials.toml",
    ".cargo/credentials",
    ".gnupg/private-keys-v1.d",
];

/// Secret files inside each agent config dir (`~/.locagens`, `~/.codex`,
/// `~/.pi/agent`): API keys, OAuth tokens and MCP server credentials.
pub const AGENT_DIR_SECRETS: [&str; 6] =
    ["auth.json", "mcp-oauth.json", "models.json", "mcp.json", "mcp_servers.json", ".credentials.json"];

/// The credential locations under `home` and `agent_dirs` that exist
/// (files, directories or links), without duplicates.
pub fn credential_paths(home: &Path, agent_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let candidates = HOME_CREDENTIALS
        .iter()
        .map(|relative| relative.split('/').fold(home.to_path_buf(), |path, part| path.join(part)))
        .chain(agent_dirs.iter().flat_map(|dir| AGENT_DIR_SECRETS.iter().map(move |name| dir.join(name))));
    let mut paths: Vec<PathBuf> = Vec::new();
    for path in candidates {
        if std::fs::symlink_metadata(&path).is_ok() && !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// The canonical forms of `secrets` a sandbox should mask: existing ones
/// that do not contain (or equal) one of `keep` (the workspace and
/// writable roots, which masking would hide).
pub fn maskable(secrets: &[PathBuf], keep: &[PathBuf]) -> Vec<PathBuf> {
    let keep: Vec<PathBuf> = keep.iter().filter_map(|root| std::fs::canonicalize(root).ok()).collect();
    let mut masked: Vec<PathBuf> = Vec::new();
    for secret in secrets {
        let Ok(canonical) = std::fs::canonicalize(secret) else { continue };
        if keep.iter().any(|root| root.starts_with(&canonical)) || masked.contains(&canonical) {
            continue;
        }
        masked.push(canonical);
    }
    masked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locagens-secrets-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn only_existing_credentials_are_listed() {
        let home = temp_home("list");
        let agent = home.join(".locagens");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(home.join(".config").join("gh")).unwrap();
        std::fs::create_dir_all(home.join(".docker")).unwrap();
        std::fs::write(home.join(".docker").join("config.json"), "{}").unwrap();
        std::fs::write(home.join(".git-credentials"), "https://x:y@host").unwrap();
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(agent.join("auth.json"), "{}").unwrap();
        std::fs::write(agent.join("sessions.txt"), "not a secret").unwrap();
        let missing_agent = home.join(".codex");

        let paths = credential_paths(&home, &[agent.clone(), missing_agent, agent.clone()]);
        assert_eq!(
            paths,
            vec![
                home.join(".ssh"),
                home.join(".config").join("gh"),
                home.join(".git-credentials"),
                home.join(".docker").join("config.json"),
                agent.join("auth.json"),
            ]
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn masks_never_hide_the_workspace() {
        let home = temp_home("mask");
        let ssh = home.join(".ssh");
        let aws = home.join(".aws");
        std::fs::create_dir_all(ssh.join("project")).unwrap();
        std::fs::create_dir_all(&aws).unwrap();
        let workspace = ssh.join("project");
        let masked = maskable(&[ssh.clone(), aws.clone(), aws.clone(), home.join("missing")], &[workspace]);
        assert_eq!(masked, vec![std::fs::canonicalize(&aws).unwrap()]);
        // A secret inside a writable root (e.g. `writable_roots = ["~"]`) is masked.
        assert_eq!(maskable(&[aws.clone()], &[home.clone()]), vec![std::fs::canonicalize(&aws).unwrap()]);
        let _ = std::fs::remove_dir_all(&home);
    }
}
