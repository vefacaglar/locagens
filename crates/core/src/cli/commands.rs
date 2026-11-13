//! The CLI executor for slash commands, shared by the REPL and the TUI
//! (QA-049). The command table, parser and busy rule live in
//! [`crate::commands`] (also used by the desktop composer). A frontend copies
//! its session state into a [`CommandSession`], calls [`execute`], then
//! renders the returned [`Output`]s and applies the changed state; it only
//! owns what is genuinely UI-specific (streaming a review, clearing a
//! transcript, quitting).
//!
//! Also home of the small helpers every frontend used to copy-paste: the
//! "provider not authenticated" check and the usage summary line.

use crate::cli::models::{list_models_table, resolve_model, ResolvedModel};
use crate::commands::{commands_for, Frontend, SlashCommand};
use crate::db::UsageLogEntry;
use crate::state::AppState;
use std::path::Path;
use std::sync::Arc;

/// Reasoning levels accepted by `/thinking` and `--thinking`.
pub const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Shown when a state-changing command is refused during a run.
pub const BUSY_NOTICE: &str = "Wait for the current run to finish (or press Esc to interrupt).";

/// The session-level state a command may read or change.
#[derive(Clone, Debug)]
pub struct CommandSession {
    pub model: Option<ResolvedModel>,
    /// `None` = off.
    pub thinking: Option<String>,
    pub mode: String,
    /// `--approve` / Full Access was requested: `/build` then means `auto`.
    pub bypass: bool,
    /// The current run (session), if any.
    pub session: Option<String>,
    /// A run is generating.
    pub busy: bool,
}

/// Something for the frontend to show.
#[derive(Clone, Debug, PartialEq)]
pub enum Output {
    Notice(String),
    Error(String),
    /// A unified diff.
    Diff(String),
    /// Preformatted table (model list).
    Table(String),
}

/// What [`execute`] decided.
#[derive(Debug, Default)]
pub struct CommandResult {
    pub outputs: Vec<Output>,
    /// Leave the program.
    pub exit: bool,
    /// Start a review of this target text (the frontend streams it).
    pub review: Option<String>,
    /// The current session was dropped (`/new`): clear the transcript.
    pub reset_view: bool,
}

impl CommandResult {
    fn notice(&mut self, text: impl Into<String>) {
        self.outputs.push(Output::Notice(text.into()));
    }
    fn error(&mut self, text: impl Into<String>) {
        self.outputs.push(Output::Error(text.into()));
    }
}

/// The `/help` text for the plain REPL (the TUI appends its key bindings).
pub fn help_text() -> String {
    let mut text = String::from("Commands:");
    for spec in commands_for(Frontend::Cli) {
        text.push_str(&format!("\n  {:<24}{}", spec.usage(), spec.description));
    }
    text
}

/// Errors with a ready-to-print message when `model`'s provider has no
/// credentials (Ollama needs none).
pub fn ensure_authenticated(state: &AppState, model: &ResolvedModel) -> Result<(), String> {
    if state.pi.has_api_key(&model.provider_id) || model.provider_id == "ollama" {
        Ok(())
    } else {
        Err(format!(
            "provider \"{0}\" is not authenticated. Run `lcgn auth login {0}`.",
            model.provider_id
        ))
    }
}

/// `tokens: X in / Y out · cache read: Z · cost: $C` over usage entries;
/// `None` when there are none.
pub fn usage_line(entries: &[UsageLogEntry]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let summary = crate::api::runs::usage_summary("", entries);
    Some(format!(
        "tokens: {} in / {} out · cache read: {} · cost: ${:.4}",
        summary.total_input_tokens,
        summary.total_output_tokens,
        summary.total_cache_read_tokens,
        summary.total_cost
    ))
}

/// Runs one command against `session`. Never blocks on user input.
pub async fn execute(
    state: &Arc<AppState>,
    project: &Path,
    command: SlashCommand,
    session: &mut CommandSession,
) -> CommandResult {
    let mut result = CommandResult::default();
    if session.busy && command.blocked_while_busy() {
        result.notice(BUSY_NOTICE);
        return result;
    }
    let project_str = project.display().to_string();
    match command {
        SlashCommand::Exit => result.exit = true,
        SlashCommand::Help => result.notice(help_text()),
        SlashCommand::Plan => set_mode(&mut result, session, "plan", "plan (read-only)"),
        SlashCommand::Build => {
            let mode = if session.bypass { "auto" } else { "accept_edits" };
            set_mode(&mut result, session, mode, "build");
        }
        SlashCommand::Auto => set_mode(&mut result, session, "auto", "full access"),
        SlashCommand::Chat => set_mode(&mut result, session, "chat", "chat (no tools)"),
        SlashCommand::Thinking(None) => {
            result.notice(format!("Thinking is {}.", session.thinking.as_deref().unwrap_or("off")));
        }
        SlashCommand::Thinking(Some(level)) => {
            let level = level.to_lowercase();
            if THINKING_LEVELS.contains(&level.as_str()) {
                session.thinking = (level != "off").then(|| level.clone());
                result.notice(format!("Thinking set to {level}."));
            } else {
                result.error(format!("Unknown thinking level \"{level}\"."));
            }
        }
        SlashCommand::Model(None) => result.outputs.push(Output::Table(list_models_table(&state.pi, "").await)),
        SlashCommand::Model(Some(target)) => set_model(state, &mut result, session, &target).await,
        SlashCommand::New => {
            session.session = None;
            result.reset_view = true;
            result.notice("Next message starts a new session.");
        }
        SlashCommand::Fork => fork(state, &mut result, session).await,
        SlashCommand::Undo => {
            match crate::api::projects::undo_last_turn(state, &project_str, session.session.as_deref()).await {
                Ok(snapshot) => {
                    result.notice(format!(
                        "Restored snapshot {} (HEAD {}, {} file(s)).",
                        snapshot.id,
                        snapshot.head_sha,
                        snapshot.restored.len()
                    ));
                    if !snapshot.skipped.is_empty() {
                        result.notice(format!("Left alone (changed after that turn): {}", snapshot.skipped.join(", ")));
                    }
                }
                Err(err) => result.error(format!("undo failed: {err}")),
            }
        }
        SlashCommand::Diff => match crate::api::projects::git_diff(state, &project_str, false).await {
            Ok(diff) if diff.trim().is_empty() => result.notice("Working tree clean."),
            Ok(diff) => result.outputs.push(Output::Diff(diff)),
            Err(err) => result.error(format!("diff failed: {err}")),
        },
        SlashCommand::Review(target) => result.review = Some(target),
        SlashCommand::Mcp => match crate::api::mcp::list_mcp_servers(state, Some(&project_str)).await {
            Ok(response) if response.servers.is_empty() => result.notice("No MCP servers configured."),
            Ok(response) => {
                let lines: Vec<String> = response
                    .servers
                    .iter()
                    .map(|s| format!("{} — {} ({})", s.config.name, s.config.transport, s.status))
                    .collect();
                result.notice(lines.join("\n"));
            }
            Err(err) => result.error(format!("mcp list failed: {err}")),
        },
        SlashCommand::Skills => match crate::api::skills::list_skills(state, Some(&project_str)).await {
            Ok(response) if response.skills.is_empty() => result.notice("No skills discovered."),
            Ok(response) => {
                let lines: Vec<String> = response
                    .skills
                    .iter()
                    .map(|s| format!("${} — {} [{}]", s.name, s.description, s.source))
                    .collect();
                result.notice(lines.join("\n"));
            }
            Err(err) => result.error(format!("skills list failed: {err}")),
        },
        SlashCommand::Sessions => {
            let runs = crate::cli::auth::sessions_for(state, Some(&project_str));
            if runs.is_empty() {
                result.notice("No sessions for this directory yet.");
            } else {
                let lines: Vec<String> = runs
                    .iter()
                    .take(15)
                    .map(|run| {
                        let id: String = run.id.chars().take(20).collect();
                        let title = run.title.lines().next().unwrap_or("").trim();
                        format!("{id}  {:<10} {title}", run.status)
                    })
                    .collect();
                result.notice(lines.join("\n"));
            }
        }
        SlashCommand::Usage => match session.session.as_deref() {
            None => result.notice("No active session yet."),
            Some(run_id) => {
                let entries = state.db.list_usage(Some(run_id));
                match usage_line(&entries) {
                    Some(line) => result.notice(format!("{} call(s) · {line}", entries.len())),
                    None => result.notice("No usage recorded for this session yet."),
                }
            }
        },
        SlashCommand::Unknown(name) => result.error(format!("Unknown command \"{name}\". Type /help.")),
    }
    result
}

fn set_mode(result: &mut CommandResult, session: &mut CommandSession, mode: &str, label: &str) {
    session.mode = mode.to_string();
    result.notice(format!("Mode set to {label}."));
}

async fn set_model(
    state: &Arc<AppState>,
    result: &mut CommandResult,
    session: &mut CommandSession,
    target: &str,
) {
    match resolve_model(&state.pi, Some(target), None).await {
        Ok(resolved) => match ensure_authenticated(state, &resolved) {
            Ok(()) => {
                result.notice(format!("Model set to {}.", resolved.label()));
                session.model = Some(resolved);
            }
            Err(message) => result.error(message),
        },
        Err(message) => {
            result.error(message);
            let table = list_models_table(&state.pi, target).await;
            if !table.trim().is_empty() {
                result.outputs.push(Output::Table(table));
            }
        }
    }
}

async fn fork(state: &Arc<AppState>, result: &mut CommandResult, session: &mut CommandSession) {
    let Some(run_id) = session.session.clone() else {
        result.notice("No active session yet - send a message first.");
        return;
    };
    match crate::api::runs::fork_run(state, &run_id, None).await {
        Ok(forked) => {
            result.notice(format!("Forked {run_id} -> {}", forked.id));
            session.session = Some(forked.id);
        }
        Err(err) => result.error(format!("fork failed: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> CommandSession {
        CommandSession {
            model: None,
            thinking: None,
            mode: "accept_edits".into(),
            bypass: false,
            session: Some("run-1".into()),
            busy: false,
        }
    }

    #[test]
    fn help_lists_every_cli_command() {
        let help = help_text();
        for spec in crate::commands::COMMANDS {
            assert!(help.contains(&spec.usage()), "{}", spec.name);
        }
    }

    #[test]
    fn mode_and_thinking_commands_update_the_session() {
        let mut s = session();
        let mut r = CommandResult::default();
        set_mode(&mut r, &mut s, "plan", "plan");
        assert_eq!(s.mode, "plan");
        s.bypass = true;
        let mode = if s.bypass { "auto" } else { "accept_edits" };
        set_mode(&mut r, &mut s, mode, "build");
        assert_eq!(s.mode, "auto");
        assert_eq!(r.outputs.len(), 2);
    }

    #[test]
    fn usage_line_sums_entries() {
        assert_eq!(usage_line(&[]), None);
        let entry = |input, output, cost| UsageLogEntry {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: 5,
            cost,
            ..Default::default()
        };
        let line = usage_line(&[entry(100, 10, 0.5), entry(200, 20, 0.25)]).unwrap();
        assert_eq!(line, "tokens: 300 in / 30 out · cache read: 10 · cost: $0.7500");
    }

    fn test_state() -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("lcgn-commands-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Arc::new(AppState {
            db: Arc::new(crate::db::Db::open(&dir).unwrap()),
            pi: Arc::new(crate::pi_providers::PiProviderManager::with_dirs(dir.clone(), dir.clone())),
            orchestrator: Arc::new(crate::orchestrator::Orchestrator::new()),
            settings_path: dir.join("settings.json"),
            default_project_path: dir.clone(),
            processes: Arc::new(crate::services::processes::ProcessManager::new()),
        })
    }

    async fn run_command(state: &Arc<AppState>, line: &str, session: &mut CommandSession) -> CommandResult {
        let command = SlashCommand::parse(line).unwrap();
        execute(state, Path::new("."), command, session).await
    }

    #[tokio::test]
    async fn busy_sessions_refuse_new_fork_undo_and_review() {
        let state = test_state();
        for line in ["/new", "/fork", "/undo", "/review"] {
            let mut s = CommandSession { busy: true, ..session() };
            let result = run_command(&state, line, &mut s).await;
            assert_eq!(result.outputs, vec![Output::Notice(BUSY_NOTICE.into())], "{line}");
            assert_eq!(s.session.as_deref(), Some("run-1"), "{line} must not touch the session");
            assert!(!result.reset_view && result.review.is_none());
        }
        // Read-only and mode commands still work while a run generates.
        let mut s = CommandSession { busy: true, ..session() };
        run_command(&state, "/plan", &mut s).await;
        assert_eq!(s.mode, "plan");
    }

    #[tokio::test]
    async fn mode_commands_cover_auto_and_chat_like_the_desktop() {
        let state = test_state();
        let mut s = session();
        run_command(&state, "/auto", &mut s).await;
        assert_eq!(s.mode, "auto");
        run_command(&state, "/chat", &mut s).await;
        assert_eq!(s.mode, "chat");
        run_command(&state, "/build", &mut s).await;
        assert_eq!(s.mode, "accept_edits");
        run_command(&state, "/thinking max", &mut s).await;
        assert_eq!(s.thinking.as_deref(), Some("max"));
        run_command(&state, "/thinking off", &mut s).await;
        assert_eq!(s.thinking, None);
        let result = run_command(&state, "/thinking ultra", &mut s).await;
        assert!(matches!(result.outputs[0], Output::Error(_)));
    }

    #[tokio::test]
    async fn new_fork_usage_and_exit() {
        let state = test_state();
        state.db.create_run(crate::db::Run {
            id: "run-1".into(),
            status: "done".into(),
            title: "t".into(),
            created_at: crate::util::now_iso(),
            ..Default::default()
        });
        state.db.append_usage(UsageLogEntry {
            run_id: "run-1".into(),
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        });
        let mut s = session();

        let usage = run_command(&state, "/usage", &mut s).await;
        assert_eq!(usage.outputs.len(), 1);
        assert!(matches!(&usage.outputs[0], Output::Notice(t) if t.starts_with("1 call(s) · tokens: 10 in / 5 out")));

        let fork = run_command(&state, "/fork", &mut s).await;
        assert!(matches!(&fork.outputs[0], Output::Notice(t) if t.starts_with("Forked run-1 ->")));
        assert_ne!(s.session.as_deref(), Some("run-1"), "the fork becomes current");

        let new = run_command(&state, "/new", &mut s).await;
        assert!(new.reset_view && s.session.is_none());
        let nothing = run_command(&state, "/fork", &mut s).await;
        assert!(matches!(&nothing.outputs[0], Output::Notice(t) if t.contains("No active session")));

        assert!(run_command(&state, "/exit", &mut s).await.exit);
        let review = run_command(&state, "/review base main", &mut s).await;
        assert_eq!(review.review.as_deref(), Some("base main"));
        let unknown = run_command(&state, "/bogus", &mut s).await;
        assert!(matches!(&unknown.outputs[0], Output::Error(t) if t.contains("/bogus")));
    }

    #[test]
    fn unauthenticated_providers_get_one_consistent_message() {
        let state = test_state();
        let model = ResolvedModel { provider_id: "nobody".into(), model: "m".into() };
        let message = ensure_authenticated(&state, &model).unwrap_err();
        assert!(message.contains("\"nobody\"") && message.contains("lcgn auth login nobody"), "{message}");
        let ollama = ResolvedModel { provider_id: "ollama".into(), model: "m".into() };
        assert!(ensure_authenticated(&state, &ollama).is_ok());
    }
}
