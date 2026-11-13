//! Windows command sandbox — the non-elevated ("legacy") backend of Codex
//! `windows-sandbox-rs`, reduced to what Locagens needs.
//!
//! A sandboxed command runs with a *write-restricted* copy of the user's own
//! token whose restricting SIDs are a per-workspace capability SID, the
//! logon-session SID and Everyone, lowered to Low integrity. The workspace
//! root and a private temp directory get an inheritable allow-write ACE for
//! the capability SID and a Low integrity label; `.git/hooks`, `.git/config`
//! and `.locagens` get a deny-write ACE and a Medium label; credential
//! files get a Medium no-read-up label. Nearly everything else is
//! unlabeled (Medium), so the command cannot write, delete or rename it;
//! objects already labeled Low (`AppData\LocalLow`, app-container data,
//! other sandboxed workspaces) can still be deleted, as `DELETE` is never
//! checked against the restricting SIDs. For the same reason the command
//! can rename `.git`, `.git/hooks`, `.git/config` and `.locagens` and put a
//! replacement there; [`guard`] detects and undoes that after the command.
//! The process
//! tree runs in a kill-on-close Job Object. No admin rights, no service, no
//! extra user accounts. See `docs/SANDBOX.md`.

pub mod plan;

#[cfg(windows)]
mod acl;
#[cfg(windows)]
pub mod guard;
#[cfg(windows)]
mod job;
#[cfg(windows)]
mod process;
#[cfg(windows)]
mod token;

#[cfg(windows)]
pub use job::JobObject;
#[cfg(windows)]
pub use process::{revoke_workspace_access, spawn_sandboxed, ProcessWaiter, SandboxedChild, SpawnRequest};
