//! `lcgn auth` subcommands and session listing/resume helpers.

use super::args::AuthAction;
use crate::db::Run;
use crate::pi_providers::{PiProviderManager, SetPiAuthInput};
use crate::state::AppState;
use serde_json::json;
use std::io::Write;
use std::path::Path;

/// Dispatches `lcgn auth <action>`.
pub async fn auth_command(pi: &PiProviderManager, action: AuthAction) -> i32 {
    match action {
        AuthAction::Login { provider, api_key } => login(pi, &provider, api_key.as_deref()),
        AuthAction::Logout { provider } => logout(pi, &provider),
        AuthAction::Check { provider, json } => check(pi, provider.as_deref(), json),
    }
}

fn login(pi: &PiProviderManager, provider: &str, api_key: Option<&str>) -> i32 {
    if provider.trim().is_empty() {
        eprintln!("lcgn: provider name is required.");
        return 2;
    }
    let key = match api_key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(key) => key.to_string(),
        None => match rpassword::prompt_password(format!("API key for {provider}: ")) {
            Ok(key) => key,
            Err(err) => {
                eprintln!("lcgn: failed to read the key: {err}");
                return 2;
            }
        },
    };
    if key.trim().is_empty() {
        eprintln!("lcgn: empty API key.");
        return 2;
    }
    match pi.set_pi_auth(&SetPiAuthInput {
        provider_id: provider.trim().to_string(),
        action: "set".to_string(),
        api_key: Some(key),
    }) {
        Ok(()) => {
            println!("Logged in to {provider} (stored in ~/.pi/agent/auth.json).");
            0
        }
        Err(err) => {
            eprintln!("lcgn: {err}");
            2
        }
    }
}

fn logout(pi: &PiProviderManager, provider: &str) -> i32 {
    match pi.set_pi_auth(&SetPiAuthInput {
        provider_id: provider.trim().to_string(),
        action: "delete".to_string(),
        api_key: None,
    }) {
        Ok(()) => {
            println!("Logged out of {provider}.");
            0
        }
        Err(err) => {
            eprintln!("lcgn: {err}");
            2
        }
    }
}

fn check(pi: &PiProviderManager, provider: Option<&str>, json: bool) -> i32 {
    let status = pi.pi_auth_status();
    let ready = match provider.map(str::trim).filter(|p| !p.is_empty()) {
        Some(provider) => pi.has_api_key(provider),
        None => !status.is_empty(),
    };
    if json {
        let payload = json!({
            "ready": ready,
            "provider": provider,
            "credentials": status,
        });
        println!("{payload}");
    } else if ready {
        match provider {
            Some(provider) => println!("{provider}: ready."),
            None => println!(
                "{} authenticated provider(s): ready.",
                status.len()
            ),
        }
    } else {
        match provider {
            Some(provider) => eprintln!("{provider}: not ready. Run `lcgn auth login {provider}`."),
            None => eprintln!("No authenticated providers. Run `lcgn auth login <provider>`."),
        }
    }
    if ready {
        0
    } else {
        1
    }
}

/// Sessions for one project directory, most recent first.
pub fn sessions_for(state: &AppState, project_path: Option<&str>) -> Vec<Run> {
    state.db.refresh_runs();
    let mut runs: Vec<Run> = state
        .db
        .list_runs()
        .into_iter()
        .filter(|run| {
            project_path
                .map(|project| run.project_path == project)
                .unwrap_or(true)
        })
        .collect();
    runs.sort_by(|a, b| {
        b.last_active_at
            .clone()
            .unwrap_or_default()
            .cmp(&a.last_active_at.clone().unwrap_or_default())
    });
    runs
}

/// `lcgn sessions [--json]`.
pub fn sessions_command(state: &AppState, project: &Path, json: bool) -> i32 {
    let runs = sessions_for(state, Some(&project.display().to_string()));
    if json {
        for run in &runs {
            if let Ok(line) = serde_json::to_string(run) {
                println!("{line}");
            }
        }
        return 0;
    }
    if runs.is_empty() {
        println!("No sessions for {} yet.", project.display());
        return 0;
    }
    println!("{:<22} {:<12} {:<12} title", "id", "status", "updated");
    for run in &runs {
        let id = run.id.chars().take(20).collect::<String>();
        let updated = run
            .last_active_at
            .clone()
            .unwrap_or_else(|| run.updated_at.clone())
            .replace('T', " ");
        let updated: String = updated.chars().take(19).collect();
        println!(
            "{:<22} {:<12} {:<12} {}",
            id,
            run.status,
            updated,
            first_line(&run.title)
        );
    }
    0
}

/// The most recent session for a project (`--continue`).
pub fn latest_session(state: &AppState, project: &Path) -> Option<Run> {
    sessions_for(state, Some(&project.display().to_string())).into_iter().next()
}

/// Resolves a (partial) session id; prefers sessions in the given project.
pub fn resolve_session(state: &AppState, partial: &str, project: &Path) -> Result<Run, String> {
    let runs = sessions_for(state, None);
    let project_path = project.display().to_string();
    let mut matches: Vec<&Run> = runs
        .iter()
        .filter(|run| run.id.starts_with(partial))
        .collect();
    if matches.len() > 1 {
        matches.retain(|run| run.project_path == project_path);
    }
    match matches.len() {
        0 => Err(format!("No session matching \"{partial}\".")),
        1 => Ok(matches[0].clone()),
        _ => Err(format!(
            "Session id \"{partial}\" is ambiguous; use a longer prefix."
        )),
    }
}

/// Interactive session picker (`--resume`): prints the list, reads a choice.
pub async fn pick_session(state: &AppState, project: &Path) -> Option<Run> {
    let runs = sessions_for(state, Some(&project.display().to_string()));
    if runs.is_empty() {
        eprintln!("lcgn: no sessions for {} yet.", project.display());
        return None;
    }
    eprintln!("Sessions for {}:", project.display());
    for (index, run) in runs.iter().enumerate().take(20) {
        let id = run.id.chars().take(18).collect::<String>();
        eprintln!(
            "  {}. {id}  [{}] {}",
            index + 1,
            run.status,
            first_line(&run.title)
        );
    }
    eprint!("Pick a session (number, empty to cancel): ");
    let _ = std::io::stderr().flush();
    let choice = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(_) => line,
            Err(_) => String::new(),
        }
    })
    .await
    .unwrap_or_default();
    let number: usize = choice.trim().parse().ok()?;
    runs.get(number.checked_sub(1)?).cloned()
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > 60 {
        let mut cut: String = line.chars().take(60).collect();
        cut.push('…');
        cut
    } else {
        line.to_string()
    }
}
