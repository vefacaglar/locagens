//! Policy → token/ACL plan for the Windows sandbox (pure data, compiled on
//! every platform so it can be unit-tested anywhere).
//!
//! The plan says which capability SID the restricted token carries, which
//! directories get an inheritable *allow write* ACE for that SID, and which
//! protected paths get a *deny write* ACE — the Windows equivalent of the
//! macOS Seatbelt profile's `PROJECT_ROOT` / `GIT_HOOKS_DIR` /
//! `GIT_CONFIG_FILE` / `LOCAGENS_CONFIG_DIR` rules.
//!
//! Credential locations ([`crate::secret_paths`]) that a writable root
//! covers get a deny-write ACE too (QA-020). Nothing else is needed for
//! the rest: the write-restricted token can only write where the
//! capability SID is granted. Reads of credentials cannot be denied with
//! this token (its restricting SIDs are not consulted for reads).

use std::path::{Path, PathBuf};

use locagens_protocol::SandboxMode;

/// Which kind of restricted token the command runs with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenMode {
    /// No ACL grants: writes only succeed where `Everyone` / the logon
    /// session may already write.
    ReadOnly,
    /// The workspace capability SID is granted write on the writable roots.
    WorkspaceWrite,
}

/// Everything the Windows backend changes or uses for one command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowsSandboxPlan {
    pub token_mode: TokenMode,
    /// String form (`S-1-5-21-…`) of the capability SID added as a
    /// restricting SID to the token.
    pub capability_sid: String,
    /// Directories that receive an inheritable allow-write ACE for the
    /// capability SID (workspace root + the private sandbox temp dir).
    pub writable_roots: Vec<PathBuf>,
    /// Paths that receive a deny-write ACE for the capability SID.
    pub deny_write_paths: Vec<PathBuf>,
    /// Credential files / directories inside a writable root: they get a
    /// deny-write ACE for the capability SID as well.
    pub secret_deny_write_paths: Vec<PathBuf>,
    /// Directories created before the command starts (the temp dir, and
    /// `.locagens` so a sandboxed command cannot create it first).
    pub create_dirs: Vec<PathBuf>,
    /// `TEMP` / `TMP` for the child (the private sandbox temp dir).
    pub temp_dir: Option<PathBuf>,
}

/// Builds the plan for one command. An error means the sandbox must refuse
/// to start the command (fail closed).
pub fn build_plan(
    mode: SandboxMode,
    project_path: &Path,
    temp_base: &Path,
    extra_writable_roots: &[PathBuf],
    secret_paths: &[PathBuf],
) -> Result<WindowsSandboxPlan, String> {
    if mode == SandboxMode::DangerFullAccess {
        return Err("danger-full-access does not use the Windows sandbox".to_string());
    }
    let root = std::fs::canonicalize(project_path).map_err(|err| {
        format!(
            "Windows sandbox: workspace {} is not accessible: {err}",
            project_path.display()
        )
    })?;
    let root = strip_verbatim_prefix(&root);

    if mode == SandboxMode::ReadOnly {
        return Ok(WindowsSandboxPlan {
            token_mode: TokenMode::ReadOnly,
            capability_sid: capability_sid_for("read-only"),
            writable_roots: Vec::new(),
            deny_write_paths: Vec::new(),
            secret_deny_write_paths: Vec::new(),
            create_dirs: Vec::new(),
            temp_dir: None,
        });
    }

    let key = workspace_key(&root);
    let capability_sid = capability_sid_for(&format!("workspace:{key}"));
    let temp_dir = temp_base
        .join("locagens-sandbox")
        .join(format!("tmp-{:016x}", fnv1a64(key.as_bytes(), FNV_OFFSET)));
    // Shared with the file tools (`protected_paths`): pre-created entries
    // are always denied, the git ones when they exist.
    let mut deny_write_paths = Vec::new();
    let mut create_dirs = vec![temp_dir.clone()];
    for entry in crate::protected_paths::SANDBOX_PROTECTED_PATHS {
        let path = entry.under(&root);
        if entry.pre_create {
            create_dirs.push(path.clone());
            deny_write_paths.push(path);
        } else if path.exists() {
            deny_write_paths.push(path);
        }
    }

    let writable_roots: Vec<PathBuf> = [root, temp_dir.clone()]
        .into_iter()
        .chain(
            extra_writable_roots
                .iter()
                .filter_map(|extra| std::fs::canonicalize(extra).ok())
                .map(|extra| strip_verbatim_prefix(&extra)),
        )
        .collect();
    let secret_deny_write_paths = secrets_under(&writable_roots, secret_paths);

    Ok(WindowsSandboxPlan {
        token_mode: TokenMode::WorkspaceWrite,
        capability_sid,
        writable_roots,
        deny_write_paths,
        secret_deny_write_paths,
        create_dirs,
        temp_dir: Some(temp_dir),
    })
}

/// The existing credential locations (canonical) that lie inside one of
/// `writable_roots` without containing one: those the capability SID could
/// otherwise write through an inherited grant.
fn secrets_under(writable_roots: &[PathBuf], secret_paths: &[PathBuf]) -> Vec<PathBuf> {
    let kept: Vec<PathBuf> = crate::secret_paths::maskable(secret_paths, writable_roots)
        .iter()
        .map(|secret| strip_verbatim_prefix(secret))
        .collect();
    kept.into_iter()
        .filter(|secret| {
            let key = workspace_key(secret);
            writable_roots.iter().any(|root| key.starts_with(&format!("{}\\", workspace_key(root))))
        })
        .collect()
}

/// `\\?\C:\x` → `C:\x` (canonicalize adds the verbatim prefix on Windows,
/// which `cmd.exe` cannot use as a working directory).
pub fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => path.to_path_buf(),
    }
}

/// Case-insensitive key for a workspace root (Windows paths are
/// case-insensitive; `/` and `\` are equivalent).
fn workspace_key(root: &Path) -> String {
    root.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_OFFSET_ALT: u64 = 0x84222325_cbf29ce4;

fn fnv1a64(bytes: &[u8], offset: u64) -> u64 {
    bytes.iter().fold(offset, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Deterministic capability SID (`S-1-5-21-a-b-c-d`, the same shape Codex
/// uses) derived from `label`.
///
/// Codex stores random SIDs in `cap_sid`; deriving them avoids a state file
/// and is equally safe: a restricting SID can only *reduce* a token's
/// access, and no ACE outside the sandbox's own grants names it.
pub fn capability_sid_for(label: &str) -> String {
    let input = format!("locagens-sandbox-v1:{label}");
    let first = fnv1a64(input.as_bytes(), FNV_OFFSET);
    let second = fnv1a64(input.as_bytes(), FNV_OFFSET_ALT);
    format!(
        "S-1-5-21-{}-{}-{}-{}",
        (first >> 32) as u32,
        first as u32,
        (second >> 32) as u32,
        second as u32
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locagens-plan-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        strip_verbatim_prefix(&std::fs::canonicalize(&dir).unwrap())
    }

    #[test]
    fn workspace_write_plan_grants_root_and_temp_and_protects_metadata() {
        let root = temp_workspace("ww");
        std::fs::create_dir_all(root.join(".git").join("hooks")).unwrap();
        std::fs::write(root.join(".git").join("config"), "[core]\n").unwrap();
        let temp_base = std::env::temp_dir();

        let plan = build_plan(SandboxMode::WorkspaceWrite, &root, &temp_base, &[], &[]).unwrap();
        assert_eq!(plan.token_mode, TokenMode::WorkspaceWrite);
        assert_eq!(plan.writable_roots[0], root);
        let temp_dir = plan.temp_dir.clone().unwrap();
        assert!(temp_dir.starts_with(temp_base.join("locagens-sandbox")));
        assert_eq!(plan.writable_roots, vec![root.clone(), temp_dir.clone()]);
        assert_eq!(
            plan.deny_write_paths,
            vec![
                root.join(".locagens"),
                root.join(".git").join("hooks"),
                root.join(".git").join("config")
            ]
        );
        assert_eq!(plan.create_dirs, vec![temp_dir, root.join(".locagens")]);
        assert!(plan.capability_sid.starts_with("S-1-5-21-"));
        assert_eq!(plan.capability_sid.split('-').count(), 8);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_git_metadata_is_not_planned() {
        let root = temp_workspace("nogit");
        let plan = build_plan(SandboxMode::WorkspaceWrite, &root, &std::env::temp_dir(), &[], &[]).unwrap();
        assert_eq!(plan.deny_write_paths, vec![root.join(".locagens")]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn capability_sids_are_stable_per_workspace_and_distinct_across_workspaces() {
        let first = temp_workspace("a");
        let second = temp_workspace("b");
        let temp_base = std::env::temp_dir();
        let plan_a = build_plan(SandboxMode::WorkspaceWrite, &first, &temp_base, &[], &[]).unwrap();
        let plan_a_again = build_plan(SandboxMode::WorkspaceWrite, &first, &temp_base, &[], &[]).unwrap();
        let plan_b = build_plan(SandboxMode::WorkspaceWrite, &second, &temp_base, &[], &[]).unwrap();
        assert_eq!(plan_a.capability_sid, plan_a_again.capability_sid);
        assert_ne!(plan_a.capability_sid, plan_b.capability_sid);
        assert_ne!(plan_a.temp_dir, plan_b.temp_dir);

        let upper = PathBuf::from(first.to_string_lossy().to_uppercase());
        if upper.exists() {
            // Case-insensitive file systems map both spellings to one SID.
            let plan_upper = build_plan(SandboxMode::WorkspaceWrite, &upper, &temp_base, &[], &[]).unwrap();
            assert_eq!(plan_upper.capability_sid, plan_a.capability_sid);
        }
        let _ = std::fs::remove_dir_all(&first);
        let _ = std::fs::remove_dir_all(&second);
    }

    #[test]
    fn read_only_plan_grants_nothing() {
        let root = temp_workspace("ro");
        let plan = build_plan(SandboxMode::ReadOnly, &root, &std::env::temp_dir(), &[], &[]).unwrap();
        assert_eq!(plan.token_mode, TokenMode::ReadOnly);
        assert!(plan.writable_roots.is_empty());
        assert!(plan.deny_write_paths.is_empty());
        assert!(plan.create_dirs.is_empty());
        assert_eq!(plan.temp_dir, None);
        assert!(plan.secret_deny_write_paths.is_empty());
        assert_eq!(plan.capability_sid, capability_sid_for("read-only"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn plan_fails_closed_for_missing_workspace_and_full_access() {
        let missing = std::env::temp_dir().join(format!("locagens-plan-missing-{}", uuid::Uuid::new_v4()));
        let err = build_plan(SandboxMode::WorkspaceWrite, &missing, &std::env::temp_dir(), &[], &[]).unwrap_err();
        assert!(err.contains("not accessible"), "{err}");
        assert!(build_plan(SandboxMode::DangerFullAccess, &std::env::temp_dir(), &std::env::temp_dir(), &[], &[]).is_err());
    }

    /// QA-020: credentials a writable root covers are write-protected;
    /// those outside every writable root need nothing (the token cannot
    /// write there anyway).
    #[test]
    fn credentials_inside_writable_roots_are_write_protected() {
        let root = temp_workspace("secrets");
        let home = temp_workspace("secrets-home");
        let other_home = temp_workspace("secrets-other");
        for dir in [home.join(".ssh"), home.join(".locagens"), other_home.join(".aws")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(home.join(".locagens").join("auth.json"), "{}").unwrap();
        let agent_dirs = [home.join(".locagens")];
        let mut secrets = crate::secret_paths::credential_paths(&home, &agent_dirs);
        secrets.extend(crate::secret_paths::credential_paths(&other_home, &[]));
        assert_eq!(secrets.len(), 3, "{secrets:?}");

        let plan = build_plan(SandboxMode::WorkspaceWrite, &root, &std::env::temp_dir(), std::slice::from_ref(&home), &secrets).unwrap();
        assert_eq!(
            plan.secret_deny_write_paths,
            vec![home.join(".ssh"), home.join(".locagens").join("auth.json")]
        );
        // Without the writable root nothing is planned.
        let plan = build_plan(SandboxMode::WorkspaceWrite, &root, &std::env::temp_dir(), &[], &secrets).unwrap();
        assert!(plan.secret_deny_write_paths.is_empty());
        // A credential dir that contains a writable root is never protected
        // (that would deny the workspace itself).
        let nested = home.join(".ssh").join("project");
        std::fs::create_dir_all(&nested).unwrap();
        let plan = build_plan(SandboxMode::WorkspaceWrite, &nested, &std::env::temp_dir(), std::slice::from_ref(&home), &secrets).unwrap();
        assert_eq!(plan.secret_deny_write_paths, vec![home.join(".locagens").join("auth.json")]);
        for dir in [root, home, other_home] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn verbatim_prefixes_are_stripped() {
        assert_eq!(strip_verbatim_prefix(Path::new(r"\\?\C:\work")), PathBuf::from(r"C:\work"));
        assert_eq!(strip_verbatim_prefix(Path::new(r"\\?\UNC\srv\share")), PathBuf::from(r"\\srv\share"));
        assert_eq!(strip_verbatim_prefix(Path::new("/tmp/x")), PathBuf::from("/tmp/x"));
    }

    #[test]
    fn extra_writable_roots_are_granted_when_they_exist() {
        let root = std::env::temp_dir().join(format!("locagens-plan-extra-{}", std::process::id()));
        let extra = root.join("extra");
        std::fs::create_dir_all(&extra).unwrap();
        let missing = root.join("missing");
        let plan = build_plan(SandboxMode::WorkspaceWrite, &root, &std::env::temp_dir(), &[extra.clone(), missing], &[]).unwrap();
        let extra = strip_verbatim_prefix(&std::fs::canonicalize(&extra).unwrap());
        assert!(plan.writable_roots.contains(&extra));
        assert_eq!(plan.writable_roots.len(), 3, "workspace + temp + the existing extra root");
        let _ = std::fs::remove_dir_all(&root);
    }

}
