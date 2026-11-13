//! `lcgn auth` subcommands and session listing/resume helpers.

use super::args::AuthAction;
use crate::db::Run;
use crate::api::providers as provider_api;
use crate::pi_providers::{LoginProvider, LoginStatus, OAuthFlow, PiProviderManager, SetPiAuthInput};
use crate::state::AppState;
use serde_json::json;
use std::io::{IsTerminal, Write};
use std::path::Path;

/// Dispatches `lcgn auth <action>`.
pub async fn auth_command(state: &AppState, action: Option<AuthAction>) -> i32 {
    let pi = &state.pi;
    match action {
        None => {
            print!("{}", providers_table(&pi.login_providers(), false));
            println!("\nSign in:  lcgn auth login <provider>");
            println!("Sign out: lcgn auth logout <provider>");
            println!("Status:   lcgn auth check");
            0
        }
        Some(AuthAction::Login { provider: Some(provider), api_key }) => {
            login_with(state, provider.trim(), api_key.as_deref()).await
        }
        Some(AuthAction::Login { provider: None, api_key }) => match pick_provider(pi).await {
            Some(provider) => login_with(state, &provider, api_key.as_deref()).await,
            None => 2,
        },
        Some(AuthAction::Logout { provider }) => logout(pi, &provider),
        Some(AuthAction::Check { provider, json }) => check(pi, provider.as_deref(), json),
    }
}

/// OAuth where the provider supports it (and no key was given), else API key.
async fn login_with(state: &AppState, provider: &str, api_key: Option<&str>) -> i32 {
    let has_oauth = locagens_config::oauth::oauth_provider_info(provider).is_some();
    let wants_key = api_key.map(str::trim).is_some_and(|k| !k.is_empty());
    if has_oauth && !wants_key {
        oauth_login(state, provider).await
    } else {
        login(&state.pi, provider, api_key)
    }
}

/// `lcgn auth login` without a provider: show the numbered table and, on a
/// terminal, ask which one. Returns `None` after printing a hint otherwise.
async fn pick_provider(pi: &PiProviderManager) -> Option<String> {
    let providers = pi.login_providers();
    print!("{}", providers_table(&providers, true));
    if !std::io::stdin().is_terminal() {
        eprintln!("lcgn: no provider given. Run `lcgn auth login <provider>`.");
        return None;
    }
    eprint!("Provider number or id: ");
    let _ = std::io::stderr().flush();
    let choice = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        line
    })
    .await
    .unwrap_or_default();
    let provider = resolve_choice(&providers, choice.trim());
    if provider.is_none() {
        eprintln!("lcgn: no provider selected.");
    }
    provider
}

/// A 1-based table number or a provider id.
fn resolve_choice(providers: &[LoginProvider], choice: &str) -> Option<String> {
    if choice.is_empty() {
        return None;
    }
    match choice.parse::<usize>() {
        Ok(number) => providers.get(number.checked_sub(1)?).map(|p| p.id.clone()),
        Err(_) => Some(choice.to_string()),
    }
}

fn sign_in_methods(provider: &LoginProvider) -> String {
    if provider.id == "ollama" {
        return "none (local)".to_string();
    }
    match &provider.oauth_label {
        Some(label) => format!("OAuth ({label}), API key"),
        None => "API key".to_string(),
    }
}

fn status_label(status: &LoginStatus) -> String {
    match status {
        LoginStatus::OAuth => "signed in (oauth)".to_string(),
        LoginStatus::ApiKey => "signed in (api key)".to_string(),
        LoginStatus::Env(var) => format!("env ${var}"),
        LoginStatus::SignedOut => "-".to_string(),
    }
}

/// Column-aligned provider table; `numbered` prepends a 1-based `#` column.
fn providers_table(providers: &[LoginProvider], numbered: bool) -> String {
    let mut header = vec!["provider", "name", "sign-in", "status"].into_iter().map(str::to_string).collect::<Vec<_>>();
    let mut rows: Vec<Vec<String>> = providers
        .iter()
        .map(|p| vec![p.id.clone(), p.name.clone(), sign_in_methods(p), status_label(&p.status)])
        .collect();
    if numbered {
        header.insert(0, "#".to_string());
        for (index, row) in rows.iter_mut().enumerate() {
            row.insert(0, (index + 1).to_string());
        }
    }
    let widths: Vec<usize> = (0..header.len())
        .map(|i| rows.iter().map(|row| row[i].chars().count()).chain([header[i].len()]).max().unwrap_or(0))
        .collect();
    let mut out = table_line(&header, &widths);
    out.extend(widths.iter().flat_map(|w| std::iter::repeat_n('-', w + 2)));
    out.push('\n');
    for row in &rows {
        out.push_str(&table_line(row, &widths));
    }
    out
}

fn table_line(cells: &[String], widths: &[usize]) -> String {
    let text: String = cells
        .iter()
        .zip(widths)
        .map(|(value, width)| format!("{value:<width$}  "))
        .collect();
    format!("{}\n", text.trim_end())
}

/// OAuth login: print the code or URL, open the browser, wait for the
/// approval or the redirect. Ctrl+C cancels.
async fn oauth_login(state: &AppState, provider: &str) -> i32 {
    let enterprise_domain = if provider == "github-copilot" { ask_enterprise_domain().await } else { None };
    let start = match provider_api::start_oauth_login_with_domain(state, provider, enterprise_domain.as_deref()).await {
        Ok(start) => start,
        Err(err) => {
            eprintln!("lcgn: {err}");
            return 2;
        }
    };
    match start.flow {
        OAuthFlow::DeviceCode => {
            println!("Open this URL in your browser and approve the sign-in:");
            println!("  {}", start.url_to_open());
            println!("Code: {}", start.user_code.as_deref().unwrap_or_default());
        }
        OAuthFlow::AuthUrl => {
            println!("Open this URL in your browser and sign in:");
            println!("  {}", start.url_to_open());
        }
    }
    match provider_api::open_verification_url(start.url_to_open()) {
        Ok(()) => println!("(opened in your default browser)"),
        Err(err) => eprintln!("lcgn: {err}; open the URL manually."),
    }
    let outcome = match start.flow {
        OAuthFlow::DeviceCode => wait_for_approval(state, &start).await,
        OAuthFlow::AuthUrl => wait_for_redirect(state, &start).await,
    };
    match outcome {
        LoginOutcome::Done => {
            println!("Logged in to {provider} (stored in ~/.locagens/auth.json; tokens refresh automatically).");
            0
        }
        LoginOutcome::Failed(err) => {
            eprintln!("lcgn: {err}");
            2
        }
        LoginOutcome::Cancelled => {
            eprintln!("lcgn: login cancelled.");
            130
        }
    }
}

enum LoginOutcome {
    Done,
    Failed(String),
    Cancelled,
}

impl LoginOutcome {
    fn from_result(result: Result<(), impl std::fmt::Display>) -> Self {
        match result {
            Ok(()) => Self::Done,
            Err(err) => Self::Failed(err.to_string()),
        }
    }
}

async fn wait_for_approval(state: &AppState, start: &provider_api::OAuthLoginStart) -> LoginOutcome {
    println!("Waiting for approval (Ctrl+C to cancel)...");
    tokio::select! {
        result = provider_api::finish_oauth_login(state, start) => LoginOutcome::from_result(result),
        _ = tokio::signal::ctrl_c() => LoginOutcome::Cancelled,
    }
}

/// Waits for the loopback redirect; on a terminal the user may instead paste
/// the final redirect URL (browser on another machine).
async fn wait_for_redirect(state: &AppState, start: &provider_api::OAuthLoginStart) -> LoginOutcome {
    println!("Waiting for the browser redirect (Ctrl+C to cancel)...");
    let mut pasted_lines = std::io::stdin().is_terminal().then(stdin_lines);
    if pasted_lines.is_some() {
        println!("If the browser cannot reach this machine, paste the final redirect URL here and press Enter.");
    }
    let finish = provider_api::finish_oauth_login(state, start);
    tokio::pin!(finish);
    loop {
        tokio::select! {
            result = &mut finish => return LoginOutcome::from_result(result),
            _ = tokio::signal::ctrl_c() => return LoginOutcome::Cancelled,
            line = next_line(&mut pasted_lines) => match line {
                None => pasted_lines = None,
                Some(pasted) if pasted.trim().is_empty() => {}
                Some(pasted) => match provider_api::submit_oauth_redirect(state, start, pasted.trim()).await {
                    Ok(()) => return LoginOutcome::Done,
                    Err(err) => eprintln!("lcgn: {err}\nPaste the redirect URL again, or finish in the browser."),
                },
            },
        }
    }
}

/// Reads stdin lines on a detached thread, so a pending read never keeps
/// the process (or the runtime) alive after the login finished.
fn stdin_lines() -> tokio::sync::mpsc::UnboundedReceiver<String> {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        while std::io::stdin().read_line(&mut line).is_ok_and(|read| read > 0) {
            if sender.send(std::mem::take(&mut line)).is_err() {
                break;
            }
        }
    });
    receiver
}

/// The next pasted line; pending forever when there is no terminal input.
async fn next_line(lines: &mut Option<tokio::sync::mpsc::UnboundedReceiver<String>>) -> Option<String> {
    match lines {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

/// GitHub Enterprise URL/domain for Copilot; blank (or no terminal) means github.com.
async fn ask_enterprise_domain() -> Option<String> {
    if !std::io::stdin().is_terminal() {
        return None;
    }
    eprint!("GitHub Enterprise URL/domain (blank for github.com): ");
    let _ = std::io::stderr().flush();
    let answer = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        line
    })
    .await
    .unwrap_or_default();
    Some(answer.trim().to_string()).filter(|domain| !domain.is_empty())
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
            println!("Logged in to {provider} (stored in ~/.locagens/auth.json; Codex ~/.codex/auth.json and ~/.pi/agent/auth.json are read as fallbacks).");
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
        print_oauth_status(&status);
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

/// One line per OAuth credential (expiry only, never token values).
fn print_oauth_status(status: &[serde_json::Value]) {
    for entry in status.iter().filter(|entry| entry["type"] == "oauth") {
        let id = entry["providerId"].as_str().unwrap_or("?");
        let state = if entry["expired"] == true { "access token expired (refreshes on next use)" } else { "access token valid" };
        println!("{id}: oauth, {state}.");
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
