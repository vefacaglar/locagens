//! `lcgn` command line definition (clap) and prompt assembly.

use clap::{Parser, Subcommand};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(
    name = "lcgn",
    version,
    about = "lcgn — the Locagens coding agent on your command line",
    long_about = "A pi-like CLI running the Locagens Rust core in-process:\n\
orchestrator, tools, permissions and file-based sessions — no HTTP server,\n\
no spawned `pi` process.\n\n\
Examples:\n  \
lcgn -p \"say OK\" --model opencode-go/glm-5.3-flash --no-tools\n  \
lcgn --model opencode-go/glm-5.3-flash\n  \
lcgn --list-models glm\n  \
lcgn -c \"keep going\"\n  \
lcgn auth login opencode-go"
)]
pub struct Cli {
    /// Non-interactive: run the task, print the result, exit
    #[arg(short = 'p', long)]
    pub print: bool,
    /// Model as provider/id, e.g. opencode-go/glm-5.3-flash (bare id accepted when unambiguous)
    #[arg(long, value_name = "PROVIDER/MODEL")]
    pub model: Option<String>,
    /// Provider for --model (when the model id is given bare)
    #[arg(long, value_name = "NAME")]
    pub provider: Option<String>,
    /// Reasoning effort: off|minimal|low|medium|high|xhigh|max
    #[arg(
        long,
        value_name = "LEVEL",
        value_parser = ["off", "minimal", "low", "medium", "high", "xhigh", "max"]
    )]
    pub thinking: Option<String>,
    /// Output mode: text (default) or json (one JSON event per line)
    #[arg(long, value_name = "MODE", value_parser = ["text", "json"])]
    pub mode: Option<String>,
    /// Plan mode (read-only tools, update_plan allowed)
    #[arg(long)]
    pub plan: bool,
    /// Full access: do not prompt for permissions (mode "auto")
    #[arg(short = 'a', long)]
    pub approve: bool,
    /// Continue the most recent session for the current directory
    #[arg(short = 'c', long = "continue")]
    pub cont: bool,
    /// Pick a session to resume (interactive list)
    #[arg(short = 'r', long)]
    pub resume: bool,
    /// Open a session by (partial) id
    #[arg(long, value_name = "ID")]
    pub session: Option<String>,
    /// Ephemeral: do not persist the run
    #[arg(long)]
    pub no_session: bool,
    /// Chat only (mode "chat")
    #[arg(long)]
    pub no_tools: bool,
    /// Use the plain line-based REPL instead of the full-screen TUI
    /// (also: LCGN_NO_TUI=1)
    #[arg(long)]
    pub no_tui: bool,
    /// Replace the built-in system prompt
    #[arg(long, value_name = "TEXT")]
    pub system_prompt: Option<String>,
    /// Append text to the system prompt
    #[arg(long, value_name = "TEXT")]
    pub append_system_prompt: Option<String>,
    /// Project directory (default: current directory)
    #[arg(long, value_name = "DIR")]
    pub cwd: Option<String>,
    /// Print authenticated models as a table, optionally filtered
    #[arg(
        long,
        num_args = 0..=1,
        default_missing_value = "",
        value_name = "SEARCH"
    )]
    pub list_models: Option<String>,
    /// Config profile from config.toml `[profiles.<name>]`
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,
    /// Messages: `@path` attaches a file, the rest forms the prompt
    pub messages: Vec<String>,
    /// Defaults taken from config.toml (flags win); filled by
    /// [`Cli::apply_config_defaults`].
    #[arg(skip)]
    pub default_mode: Option<String>,
    #[arg(skip)]
    pub default_thinking: Option<String>,
    #[arg(skip)]
    pub hide_reasoning: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Manage provider credentials (Codex / Locagens Auth & Providers: ~/.locagens/auth.json, Codex-compatible with ~/.codex/auth.json + ~/.pi/agent fallback)
    Auth {
        /// Omit to list the providers you can sign in to
        #[command(subcommand)]
        action: Option<AuthAction>,
    },
    /// Show the effective config.toml (secrets masked)
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
    /// List sessions for the current directory
    Sessions {
        /// Print one JSON object per session
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum ConfigAction {
    /// Print the effective configuration (default)
    Show,
}

#[derive(Subcommand, Debug, Clone)]
pub enum AuthAction {
    /// Sign in to a provider: OAuth device login where supported (e.g. xai),
    /// otherwise store an API key
    Login {
        /// Provider id or number; omit to pick from the provider list
        provider: Option<String>,
        /// Store an API key instead of using OAuth (prompted with no echo
        /// when the key is omitted for providers without OAuth)
        #[arg(long, value_name = "KEY")]
        api_key: Option<String>,
    },
    /// Remove the stored credential for a provider
    Logout { provider: String },
    /// Check credential status (exit 0 ready / 1 not ready)
    Check {
        #[arg(long, value_name = "PROVIDER")]
        provider: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

impl Cli {
    /// The reasoning effort to request; `off` means none.
    pub fn thinking_level(&self) -> Option<String> {
        self.thinking
            .as_deref()
            .or(self.default_thinking.as_deref())
            .filter(|level| !level.is_empty() && *level != "off")
            .map(str::to_string)
    }

    /// Whether reasoning output should be rendered (dim on stderr).
    pub fn show_thinking(&self) -> bool {
        match self.thinking.as_deref() {
            Some("off") => false,
            Some(_) => true,
            None => !self.hide_reasoning && self.default_thinking.as_deref() != Some("off"),
        }
    }

    /// Fills unset flags from config.toml: flags override config, config
    /// overrides the built-in defaults.
    pub fn apply_config_defaults(&mut self, config: &locagens_config::config_toml::Config) {
        if self.model.is_none() {
            self.model = config.model.clone();
            if self.provider.is_none() {
                self.provider = config.model_provider.clone();
            }
        }
        self.default_thinking = config.reasoning_effort();
        self.default_mode = config.default_run_mode();
        self.hide_reasoning = !config.show_reasoning();
    }

    /// The effective Locagens run mode implied by the flags.
    pub fn effective_mode(&self) -> String {
        if self.plan {
            return "plan".to_string();
        }
        if self.no_tools {
            return "chat".to_string();
        }
        if self.approve {
            return "auto".to_string();
        }
        self.default_mode.clone().unwrap_or_else(|| "accept_edits".to_string())
    }

    /// The project directory: `--cwd` when given, else the current directory.
    pub fn project_dir(&self) -> Result<PathBuf, String> {
        match self.cwd.as_deref().filter(|dir| !dir.trim().is_empty()) {
            Some(dir) => crate::util::canonical_project_path(dir),
            None => std::env::current_dir()
                .map_err(|err| format!("Cannot resolve the current directory: {err}")),
        }
    }

    /// Whether the interactive session should use the full-screen TUI.
    pub fn use_tui(&self) -> bool {
        let env_off = std::env::var("LCGN_NO_TUI")
            .map(|value| env_flag_enabled(&value))
            .unwrap_or(false);
        !self.no_tui && !env_off && stdout_is_tty()
    }

    /// JSON output mode.
    pub fn json_mode(&self) -> bool {
        self.mode.as_deref() == Some("json")
    }
}

/// Is stdout an interactive terminal?
pub fn stdout_is_tty() -> bool {
    std::io::stdout().is_terminal()
}

/// `1`, `true`, `yes` and `on` (any case) enable a boolean env flag.
pub fn env_flag_enabled(value: &str) -> bool {
    matches!(value.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

/// Is stdin an interactive terminal?
pub fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}

/// Builds the run prompt from the message arguments: `@path` tokens become
/// file attachment blocks, the remaining tokens are the task text, and piped
/// stdin (when stdin is not a TTY) is appended.
pub fn assemble_prompt(messages: &[String], project: &Path, stdin: Option<&str>) -> Result<String, String> {
    let mut attachments = String::new();
    let mut texts: Vec<&str> = Vec::new();
    for message in messages {
        if let Some(relative) = message.strip_prefix('@') {
            if relative.trim().is_empty() {
                return Err("@file argument is empty.".to_string());
            }
            let path = project.join(relative.trim());
            let content = std::fs::read_to_string(&path)
                .map_err(|err| format!("Cannot read @file \"{}\": {err}", relative.trim()))?;
            attachments.push_str(&format!(
                "## Attached file: {}\n```\n{}\n```\n\n",
                relative.trim(),
                content
            ));
        } else {
            texts.push(message);
        }
    }
    let mut prompt = String::new();
    prompt.push_str(&attachments);
    prompt.push_str(&texts.join(" "));
    if let Some(piped) = stdin {
        let piped = piped.trim();
        if !piped.is_empty() {
            if !prompt.is_empty() {
                prompt.push_str("\n\n");
            }
            prompt.push_str(piped);
        }
    }
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        return Err(
            "No task given. Pass a message, `@file` attachments or piped stdin.".to_string(),
        );
    }
    Ok(prompt)
}

/// Reads piped stdin when it is not a terminal; `None` otherwise.
pub fn read_piped_stdin() -> Option<String> {
    if stdin_is_tty() {
        return None;
    }
    use std::io::Read;
    let mut buffer = String::new();
    match std::io::stdin().read_to_string(&mut buffer) {
        Ok(_) => Some(buffer),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("lcgn").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn parses_print_run_flags() {
        let parsed = cli(&["-p", "--model", "opencode-go/glm-5.3-flash", "say", "OK"]);
        assert!(parsed.print);
        assert_eq!(parsed.model.as_deref(), Some("opencode-go/glm-5.3-flash"));
        assert_eq!(parsed.messages, vec!["say", "OK"]);
        assert_eq!(parsed.effective_mode(), "accept_edits");
        assert_eq!(parsed.thinking_level(), None);
        assert!(parsed.show_thinking());
    }

    #[test]
    fn parses_mode_flags() {
        let plan = cli(&["--plan"]);
        assert_eq!(plan.effective_mode(), "plan");
        let chat = cli(&["--no-tools"]);
        assert_eq!(chat.effective_mode(), "chat");
        let auto = cli(&["-a"]);
        assert_eq!(auto.effective_mode(), "auto");
        // chat/plan win over --approve.
        let chat_auto = cli(&["-a", "--no-tools"]);
        assert_eq!(chat_auto.effective_mode(), "chat");
    }

    #[test]
    fn config_defaults_fill_unset_flags_only() {
        use locagens_config::config_toml::Config;
        let config = Config::from_toml_str(
            "model = \"m1\"
model_provider = \"p1\"
model_reasoning_effort = \"high\"
approval_policy = \"never\"
hide_agent_reasoning = true
",
        )
        .unwrap();
        let mut bare = cli(&["-p", "hi"]);
        bare.apply_config_defaults(&config);
        assert_eq!(bare.model.as_deref(), Some("m1"));
        assert_eq!(bare.provider.as_deref(), Some("p1"));
        assert_eq!(bare.thinking_level().as_deref(), Some("high"));
        assert_eq!(bare.effective_mode(), "auto");
        assert!(!bare.show_thinking());

        let mut flagged = cli(&["--model", "x/y", "--thinking", "low", "--plan"]);
        flagged.apply_config_defaults(&config);
        assert_eq!(flagged.model.as_deref(), Some("x/y"));
        assert_eq!(flagged.provider, None);
        assert_eq!(flagged.thinking_level().as_deref(), Some("low"));
        assert_eq!(flagged.effective_mode(), "plan");
        assert!(flagged.show_thinking());
    }

    #[test]
    fn parses_profile_flag_and_config_subcommand() {
        assert_eq!(cli(&["-p", "--profile", "fast", "hi"]).profile.as_deref(), Some("fast"));
        assert!(matches!(
            cli(&["config", "show"]).command,
            Some(Command::Config { action: Some(ConfigAction::Show) })
        ));
        assert!(matches!(cli(&["config"]).command, Some(Command::Config { action: None })));
    }

    #[test]
    fn parses_no_tui_flag_and_env_values() {
        assert!(cli(&["--no-tui"]).no_tui);
        assert!(!cli(&[]).no_tui);
        assert!(env_flag_enabled("1") && env_flag_enabled("TRUE") && env_flag_enabled(" on "));
        assert!(!env_flag_enabled("0") && !env_flag_enabled(""));
    }

    #[test]
    fn parses_thinking_levels() {
        let parsed = cli(&["--thinking", "high"]);
        assert_eq!(parsed.thinking_level().as_deref(), Some("high"));
        assert!(parsed.show_thinking());
        let off = cli(&["--thinking", "off"]);
        assert_eq!(off.thinking_level(), None);
        assert!(!off.show_thinking());
        assert!(Cli::try_parse_from(["lcgn", "--thinking", "ultra"]).is_err());
    }

    #[test]
    fn parses_list_models_optional_arg() {
        let bare = cli(&["--list-models"]);
        assert_eq!(bare.list_models.as_deref(), Some(""));
        let filtered = cli(&["--list-models", "glm"]);
        assert_eq!(filtered.list_models.as_deref(), Some("glm"));
    }

    #[test]
    fn parses_auth_and_sessions_subcommands() {
        let parsed = cli(&["auth", "login", "opencode-go"]);
        match parsed.command {
            Some(Command::Auth {
                action: Some(AuthAction::Login { provider, api_key }),
            }) => {
                assert_eq!(provider.as_deref(), Some("opencode-go"));
                assert!(api_key.is_none());
            }
            other => panic!("unexpected: {other:?}"),
        }
        let parsed = cli(&["auth", "check", "--json"]);
        assert!(matches!(
            parsed.command,
            Some(Command::Auth {
                action: Some(AuthAction::Check { json: true, .. })
            })
        ));
        let parsed = cli(&["auth"]);
        assert!(matches!(parsed.command, Some(Command::Auth { action: None })));
        let parsed = cli(&["auth", "login"]);
        assert!(matches!(
            parsed.command,
            Some(Command::Auth {
                action: Some(AuthAction::Login { provider: None, .. })
            })
        ));
        let parsed = cli(&["sessions"]);
        assert!(matches!(parsed.command, Some(Command::Sessions { json: false })));
    }

    #[test]
    fn assembles_prompt_with_attachment() {
        let dir = std::env::temp_dir().join(format!("lcgn-args-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("note.txt"), "hello file").unwrap();
        let prompt = assemble_prompt(
            &["@note.txt".to_string(), "summarize".to_string()],
            &dir,
            None,
        )
        .unwrap();
        assert!(prompt.contains("## Attached file: note.txt"));
        assert!(prompt.contains("hello file"));
        assert!(prompt.trim_end().ends_with("summarize"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn assembles_prompt_with_stdin() {
        let prompt =
            assemble_prompt(&["from stdin:".to_string()], Path::new("."), Some("piped body"))
                .unwrap();
        assert_eq!(prompt, "from stdin:

piped body");
        assert!(assemble_prompt(&[], Path::new("."), None).is_err());
        assert!(assemble_prompt(&["@missing.txt".to_string()], Path::new("."), None).is_err());
    }
}
