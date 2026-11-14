//! Frontend-neutral slash commands (QA-049): the command table (names,
//! aliases, arguments, descriptions and which frontends offer each), the
//! parser and the busy-blocking rule. The CLI executor and TUI popup
//! (`crate::cli`) and the desktop composer all use this one definition and
//! only own their execution.

/// A UI that accepts slash commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frontend {
    /// `lcgn`: the plain REPL and the TUI.
    Cli,
    /// The Dioxus desktop composer.
    Desktop,
}

const CLI: &[Frontend] = &[Frontend::Cli];
const ALL: &[Frontend] = &[Frontend::Cli, Frontend::Desktop];

/// One row of the command table.
pub struct CommandSpec {
    /// Canonical name without the slash (`model`).
    pub name: &'static str,
    /// Other spellings that parse to the same command (`quit` for `exit`).
    pub aliases: &'static [&'static str],
    /// Argument hint (`[provider/id]`), empty when there is none.
    pub args: &'static str,
    /// English one-liner (the desktop translates it by `name`).
    pub description: &'static str,
    /// Frontends that offer the command.
    pub frontends: &'static [Frontend],
}

impl CommandSpec {
    pub fn supports(&self, frontend: Frontend) -> bool {
        self.frontends.contains(&frontend)
    }

    /// `/name args` as shown in help lists and popups.
    pub fn usage(&self) -> String {
        if self.args.is_empty() {
            format!("/{}", self.name)
        } else {
            format!("/{} {}", self.name, self.args)
        }
    }
}

pub const COMMANDS: &[CommandSpec] = &[
    spec("model", &[], "[provider/id]", "show or switch models", CLI),
    spec("thinking", &[], "<level>", "reasoning effort (off..max)", CLI),
    spec("plan", &[], "", "switch to plan mode (read-only)", ALL),
    spec("build", &[], "", "switch to build mode (edits applied)", ALL),
    spec("auto", &[], "", "switch to full access (no permission prompts)", ALL),
    spec("chat", &[], "", "switch to chat mode (no tools)", ALL),
    spec("new", &["clear"], "", "start a fresh session on the next message", ALL),
    spec("fork", &[], "", "fork the current session", ALL),
    spec("undo", &[], "", "undo the last turn (git snapshot)", ALL),
    spec("diff", &[], "", "show the working-tree diff", CLI),
    spec("review", &[], "[target]", "code review: changes, a branch or a commit", ALL),
    spec("mcp", &[], "", "list MCP servers and status", CLI),
    spec("skills", &[], "", "list discovered skills", CLI),
    spec("resume", &["sessions"], "", "pick a session of this folder to continue", CLI),
    spec("login", &[], "[provider]", "connect a provider (API key or sign-in)", CLI),
    spec("logout", &[], "<provider>", "remove a provider's stored credentials", CLI),
    spec("copy", &[], "[code]", "copy the last answer (or its last code block)", CLI),
    spec("usage", &[], "", "usage and cost of this session", CLI),
    spec("help", &[], "", "show the available commands", ALL),
    spec("exit", &["quit"], "", "leave lcgn", CLI),
];

const fn spec(
    name: &'static str,
    aliases: &'static [&'static str],
    args: &'static str,
    description: &'static str,
    frontends: &'static [Frontend],
) -> CommandSpec {
    CommandSpec { name, aliases, args, description, frontends }
}

/// The table rows `frontend` offers, in table order.
pub fn commands_for(frontend: Frontend) -> impl Iterator<Item = &'static CommandSpec> {
    COMMANDS.iter().filter(move |spec| spec.supports(frontend))
}

/// Commands of `frontend` matching `query` (without the slash): prefix
/// matches first, then substring matches, each in table order.
pub fn matching(query: &str, frontend: Frontend) -> Vec<&'static CommandSpec> {
    let query = query.trim().trim_start_matches('/').to_lowercase();
    let prefix = commands_for(frontend).filter(|c| c.name.starts_with(&query));
    let contains = commands_for(frontend).filter(|c| !c.name.starts_with(&query) && c.name.contains(&query));
    prefix.chain(contains).collect()
}

/// The table row named `name` (canonical name or alias, no slash).
pub fn find(name: &str) -> Option<&'static CommandSpec> {
    let name = name.trim_start_matches('/').to_lowercase();
    COMMANDS.iter().find(|spec| spec.name == name || spec.aliases.contains(&name.as_str()))
}

/// A parsed slash command.
#[derive(Clone, Debug, PartialEq)]
pub enum SlashCommand {
    Exit,
    Help,
    /// `/model` lists, `/model <id>` switches.
    Model(Option<String>),
    /// `/thinking` shows, `/thinking <level>` sets.
    Thinking(Option<String>),
    Plan,
    Build,
    Auto,
    Chat,
    New,
    Fork,
    Undo,
    Diff,
    /// `/review [target]`; the argument is the raw text after the command.
    Review(String),
    Mcp,
    Skills,
    /// `/resume` (alias `/sessions`).
    Sessions,
    /// `/login [provider]`.
    Login(Option<String>),
    /// `/logout <provider>`.
    Logout(Option<String>),
    /// `/copy` or `/copy code`.
    Copy(Option<String>),
    Usage,
    /// Not in the table; carries the lowercased head (`/nope`).
    Unknown(String),
}

impl SlashCommand {
    /// Parses a line starting with `/`; `None` for ordinary text.
    pub fn parse(line: &str) -> Option<SlashCommand> {
        let line = line.trim();
        if !line.starts_with('/') {
            return None;
        }
        let mut parts = line.split_whitespace();
        let raw_head = parts.next().unwrap_or("");
        let head = raw_head.to_lowercase();
        // `/usr/bin/foo fails` is a path in ordinary text, not a command.
        let name = &head[1..];
        if !name.is_empty() && !name.chars().all(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        let first_arg = parts.next().map(str::to_string);
        let Some(spec) = find(&head) else {
            return Some(Self::Unknown(head));
        };
        Some(match spec.name {
            "exit" => Self::Exit,
            "help" => Self::Help,
            "model" => Self::Model(first_arg),
            "thinking" => Self::Thinking(first_arg),
            "plan" => Self::Plan,
            "build" => Self::Build,
            "auto" => Self::Auto,
            "chat" => Self::Chat,
            "new" => Self::New,
            "fork" => Self::Fork,
            "undo" => Self::Undo,
            "diff" => Self::Diff,
            "review" => Self::Review(line[raw_head.len()..].trim().to_string()),
            "mcp" => Self::Mcp,
            "skills" => Self::Skills,
            "resume" => Self::Sessions,
            "login" => Self::Login(first_arg),
            "logout" => Self::Logout(first_arg),
            "copy" => Self::Copy(first_arg),
            "usage" => Self::Usage,
            _ => Self::Unknown(head),
        })
    }

    /// The command's table row; `None` for [`SlashCommand::Unknown`].
    pub fn spec(&self) -> Option<&'static CommandSpec> {
        let name = match self {
            Self::Exit => "exit",
            Self::Help => "help",
            Self::Model(_) => "model",
            Self::Thinking(_) => "thinking",
            Self::Plan => "plan",
            Self::Build => "build",
            Self::Auto => "auto",
            Self::Chat => "chat",
            Self::New => "new",
            Self::Fork => "fork",
            Self::Undo => "undo",
            Self::Diff => "diff",
            Self::Review(_) => "review",
            Self::Mcp => "mcp",
            Self::Skills => "skills",
            Self::Sessions => "resume",
            Self::Login(_) => "login",
            Self::Logout(_) => "logout",
            Self::Copy(_) => "copy",
            Self::Usage => "usage",
            Self::Unknown(_) => return None,
        };
        find(name)
    }

    /// Whether `frontend` offers this command (unknown commands: no).
    pub fn supported_by(&self, frontend: Frontend) -> bool {
        self.spec().is_some_and(|spec| spec.supports(frontend))
    }

    /// Commands that change which run is current or touch the working tree;
    /// every frontend refuses them while a run is generating.
    pub fn blocked_while_busy(&self) -> bool {
        matches!(self, Self::New | Self::Fork | Self::Undo | Self::Review(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_command_and_ignores_plain_text() {
        assert_eq!(SlashCommand::parse("hello"), None);
        assert_eq!(SlashCommand::parse("/usr/bin/foo fails"), None, "a path is text");
        assert_eq!(SlashCommand::parse("/bogus"), Some(SlashCommand::Unknown("/bogus".into())));
        assert_eq!(SlashCommand::parse("  /exit "), Some(SlashCommand::Exit));
        assert_eq!(SlashCommand::parse("/QUIT"), Some(SlashCommand::Exit));
        assert_eq!(SlashCommand::parse("/model"), Some(SlashCommand::Model(None)));
        assert_eq!(SlashCommand::parse("/model p/m"), Some(SlashCommand::Model(Some("p/m".into()))));
        assert_eq!(SlashCommand::parse("/thinking HIGH"), Some(SlashCommand::Thinking(Some("HIGH".into()))));
        assert_eq!(SlashCommand::parse("/auto"), Some(SlashCommand::Auto));
        assert_eq!(SlashCommand::parse("/chat"), Some(SlashCommand::Chat));
        assert_eq!(SlashCommand::parse("/clear"), Some(SlashCommand::New));
        assert_eq!(SlashCommand::parse("/review base main"), Some(SlashCommand::Review("base main".into())));
        assert_eq!(SlashCommand::parse("/REVIEW  commit abc"), Some(SlashCommand::Review("commit abc".into())));
        assert_eq!(SlashCommand::parse("/review"), Some(SlashCommand::Review(String::new())));
        assert_eq!(SlashCommand::parse("/nope x"), Some(SlashCommand::Unknown("/nope".into())));
    }

    #[test]
    fn every_table_entry_and_alias_parses_back_to_its_row() {
        for spec in COMMANDS {
            for spelling in std::iter::once(&spec.name).chain(spec.aliases) {
                let parsed = SlashCommand::parse(&format!("/{spelling}")).unwrap();
                assert_eq!(parsed.spec().map(|s| s.name), Some(spec.name), "/{spelling}");
            }
            assert!(!spec.frontends.is_empty(), "/{} is offered nowhere", spec.name);
        }
        assert_eq!(SlashCommand::Unknown("/x".into()).spec().map(|s| s.name), None);
    }

    #[test]
    fn state_changing_commands_are_blocked_while_busy() {
        for blocked in ["/new", "/clear", "/fork", "/undo", "/review", "/review base main"] {
            assert!(SlashCommand::parse(blocked).unwrap().blocked_while_busy(), "{blocked}");
        }
        for allowed in ["/plan", "/build", "/auto", "/chat", "/model", "/diff", "/usage", "/help", "/nope"] {
            assert!(!SlashCommand::parse(allowed).unwrap().blocked_while_busy(), "{allowed}");
        }
    }

    #[test]
    fn frontends_offer_their_own_subset() {
        let cli: Vec<&str> = commands_for(Frontend::Cli).map(|c| c.name).collect();
        assert_eq!(cli.len(), COMMANDS.len(), "the CLI offers every command");
        let desktop: Vec<&str> = commands_for(Frontend::Desktop).map(|c| c.name).collect();
        assert_eq!(desktop, ["plan", "build", "auto", "chat", "new", "fork", "undo", "review", "help"]);
        assert!(SlashCommand::Undo.supported_by(Frontend::Desktop));
        assert!(!SlashCommand::Diff.supported_by(Frontend::Desktop));
        assert!(!SlashCommand::Unknown("/x".into()).supported_by(Frontend::Cli));
    }

    #[test]
    fn matching_prefers_prefix_matches_per_frontend() {
        let names: Vec<&str> = matching("m", Frontend::Cli).iter().map(|c| c.name).collect();
        assert_eq!(names[0], "model");
        assert!(names.contains(&"mcp") && !names.contains(&"skills"));
        assert_eq!(matching("", Frontend::Cli).len(), COMMANDS.len());
        let names: Vec<&str> = matching("ess", Frontend::Cli).iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["sessions"]);
        assert!(matching("zzz", Frontend::Cli).is_empty());
        // The desktop never suggests CLI-only commands; a leading slash is fine.
        assert!(matching("m", Frontend::Desktop).is_empty());
        let names: Vec<&str> = matching("/u", Frontend::Desktop).iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["undo", "build", "auto"]);
        assert_eq!(COMMANDS[0].usage(), "/model [provider/id]");
        assert_eq!(find("PLAN").unwrap().usage(), "/plan");
    }
}
