//! `lcgn trust` and the one-time trust prompt for project folders.
//!
//! An untrusted folder runs with user-scope configuration only: its
//! `.locagens` hooks, MCP servers, plugins, rules and agent roles are
//! ignored (see `services::project_trust`).

use crate::services::project_trust;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

/// `lcgn trust [PATH] [--revoke | --status]`.
pub fn trust_command(cli: &super::args::Cli, path: Option<&str>, revoke: bool, status: bool) -> i32 {
    let target = match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(path) => crate::util::canonical_project_path(path),
        None => cli.project_dir().and_then(|dir| crate::util::canonical_project_path(&dir.to_string_lossy())),
    };
    let target = match target {
        Ok(target) => target,
        Err(message) => {
            eprintln!("lcgn: {message}");
            return 2;
        }
    };
    if status {
        let trusted = project_trust::is_trusted(&target);
        println!("{} is {}.", target.display(), if trusted { "trusted" } else { "not trusted" });
        for file in project_trust::ignored_config_files(&target) {
            println!("  ignored: {}", relative(&target, &file));
        }
        return if trusted { 0 } else { 1 };
    }
    match project_trust::set_trusted(&target, !revoke) {
        Ok(()) if revoke => {
            println!("{} is no longer trusted; its project config is ignored.", target.display());
            0
        }
        Ok(()) => {
            println!("Trusted {}: its .locagens hooks, MCP servers, plugins, rules and agent roles now load.", target.display());
            0
        }
        Err(message) => {
            eprintln!("lcgn: {message}");
            1
        }
    }
}

/// Runs before a session starts. `--trust` trusts the folder for this
/// process; otherwise an untrusted folder that ships project config gets a
/// one-time `[y/N]` prompt on a terminal (yes is saved) or a one-line
/// notice when nobody can answer (`-p`, pipes).
pub fn ensure_project_trust(project: &Path, trust_flag: bool, interactive: bool) {
    if trust_flag {
        project_trust::trust_for_session(project);
        return;
    }
    let ignored = project_trust::ignored_config_files(project);
    if ignored.is_empty() {
        return;
    }
    let can_ask = interactive && std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    if !can_ask {
        eprintln!("{}", untrusted_notice(project));
        return;
    }
    eprintln!("This folder ships project configuration that can run code on your machine:");
    for file in &ignored {
        eprintln!("  {}", relative(project, file));
    }
    eprint!("Trust {} and load it? [y/N] ", project.display());
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().lock().read_line(&mut answer);
    if !is_yes(&answer) {
        eprintln!("Continuing without the project configuration (`lcgn trust` changes this later).");
        return;
    }
    match project_trust::set_trusted(project, true) {
        Ok(()) => eprintln!("Trusted {}.", project.display()),
        Err(message) => {
            // Still honour the answer for this session.
            eprintln!("lcgn: could not save the trust decision: {message}");
            project_trust::trust_for_session(project);
        }
    }
}

/// The one-line stderr notice for non-interactive runs.
pub fn untrusted_notice(project: &Path) -> String {
    format!(
        "lcgn: ignoring project config in {} (folder not trusted; run `lcgn trust` or pass --trust).",
        project.join(".locagens").display()
    )
}

fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes" | "e" | "evet")
}

fn relative(root: &Path, file: &Path) -> String {
    file.strip_prefix(root).map(PathBuf::from).unwrap_or_else(|_| file.to_path_buf()).display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_yes_trusts() {
        for yes in ["y", "Y\n", " yes ", "evet"] {
            assert!(is_yes(yes), "{yes:?}");
        }
        for no in ["", "\n", "n", "no", "sure", "yy"] {
            assert!(!is_yes(no), "{no:?}");
        }
    }

    #[test]
    fn non_interactive_runs_never_trust_and_trust_flag_is_session_only() {
        let project = std::env::temp_dir().join(format!("locagens-cli-trust-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(project.join(".locagens")).unwrap();
        std::fs::write(project.join(".locagens").join("hooks.json"), "{}").unwrap();
        ensure_project_trust(&project, false, false);
        assert!(!project_trust::is_trusted(&project));
        assert!(untrusted_notice(&project).contains("lcgn trust"));
        ensure_project_trust(&project, true, false);
        assert!(project_trust::is_trusted(&project));
        let _ = std::fs::remove_dir_all(&project);
    }
}
