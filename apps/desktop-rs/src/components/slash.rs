//! Desktop slash commands (QA-049): the composer parses with the shared
//! `locagens_core::commands` table and busy rule; this module only maps the
//! parsed command to what the desktop does, and feeds the `/` suggestions.

use crate::i18n::{self, Lang};
use locagens_core::commands::{self, CommandSpec, Frontend, SlashCommand};

/// What the composer does with a `/` prompt.
#[derive(Clone, Debug, PartialEq)]
pub enum SlashAction {
    /// `/new` (`/clear`): drop the open conversation.
    Reset,
    Fork,
    Undo,
    /// `/review [target]` with the raw argument.
    Review(String),
    /// `/plan` `/build` `/auto` `/chat`: the run mode to select.
    SetMode(&'static str),
    Help,
    /// Refused while a run is generating (the shared busy rule).
    Busy(String),
    /// A known command the desktop does not offer (`/diff`).
    Unavailable(String),
    Unknown(String),
}

/// Resolves a prompt; `None` when it is not a slash command.
/// `generating`: the open run is generating.
pub fn resolve(prompt: &str, generating: bool) -> Option<SlashAction> {
    let command = SlashCommand::parse(prompt)?;
    let slash_name = |command: &SlashCommand| command.spec().map(|spec| format!("/{}", spec.name)).unwrap_or_default();
    if let SlashCommand::Unknown(head) = &command {
        return Some(SlashAction::Unknown(head.clone()));
    }
    if !command.supported_by(Frontend::Desktop) {
        return Some(SlashAction::Unavailable(slash_name(&command)));
    }
    if generating && command.blocked_while_busy() {
        return Some(SlashAction::Busy(slash_name(&command)));
    }
    Some(match command {
        SlashCommand::New => SlashAction::Reset,
        SlashCommand::Fork => SlashAction::Fork,
        SlashCommand::Undo => SlashAction::Undo,
        SlashCommand::Review(target) => SlashAction::Review(target),
        SlashCommand::Plan => SlashAction::SetMode("plan"),
        SlashCommand::Build => SlashAction::SetMode("accept_edits"),
        SlashCommand::Auto => SlashAction::SetMode("auto"),
        SlashCommand::Chat => SlashAction::SetMode("chat"),
        SlashCommand::Help => SlashAction::Help,
        // Every other command is CLI-only (handled above).
        other => SlashAction::Unavailable(slash_name(&other)),
    })
}

/// Desktop commands to suggest while the prompt is a bare `/word`.
pub fn suggestions(prompt: &str) -> Vec<&'static CommandSpec> {
    match prompt.strip_prefix('/') {
        Some(query) if !query.contains(char::is_whitespace) => commands::matching(query, Frontend::Desktop),
        _ => Vec::new(),
    }
}

/// `/plan /build …`: the desktop commands, for the unknown-command toast.
pub fn command_names() -> String {
    commands::commands_for(Frontend::Desktop).map(|spec| format!("/{}", spec.name)).collect::<Vec<_>>().join(" ")
}

/// `/plan — switch to plan mode …; …`: the `/help` toast body.
pub fn command_help(lang: Lang) -> String {
    commands::commands_for(Frontend::Desktop)
        .map(|spec| format!("{} — {}", spec.usage(), i18n::command_description(lang, spec)))
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_not_a_command() {
        assert_eq!(resolve("fix the parser", false), None);
        assert_eq!(resolve("  ", true), None);
    }

    #[test]
    fn desktop_commands_map_to_actions() {
        assert_eq!(resolve("/new", false), Some(SlashAction::Reset));
        assert_eq!(resolve("/CLEAR", false), Some(SlashAction::Reset));
        assert_eq!(resolve("/fork", false), Some(SlashAction::Fork));
        assert_eq!(resolve("/undo", false), Some(SlashAction::Undo));
        assert_eq!(resolve("/review base main", false), Some(SlashAction::Review("base main".into())));
        assert_eq!(resolve("/plan", false), Some(SlashAction::SetMode("plan")));
        assert_eq!(resolve("/build", false), Some(SlashAction::SetMode("accept_edits")));
        assert_eq!(resolve("/auto", false), Some(SlashAction::SetMode("auto")));
        assert_eq!(resolve("/chat", false), Some(SlashAction::SetMode("chat")));
        assert_eq!(resolve("/help", false), Some(SlashAction::Help));
    }

    #[test]
    fn cli_only_and_unknown_commands_are_reported() {
        assert_eq!(resolve("/diff", false), Some(SlashAction::Unavailable("/diff".into())));
        assert_eq!(resolve("/model p/m", false), Some(SlashAction::Unavailable("/model".into())));
        assert_eq!(resolve("/quit", true), Some(SlashAction::Unavailable("/exit".into())));
        assert_eq!(resolve("/bogus x", false), Some(SlashAction::Unknown("/bogus".into())));
    }

    #[test]
    fn busy_commands_are_refused_while_generating() {
        for line in ["/new", "/clear", "/fork", "/undo", "/review", "/review base main"] {
            let name = format!("/{}", commands::find(line.split_whitespace().next().unwrap()).unwrap().name);
            assert_eq!(resolve(line, true), Some(SlashAction::Busy(name)), "{line}");
        }
        // Mode switches and help still work during a run.
        assert_eq!(resolve("/plan", true), Some(SlashAction::SetMode("plan")));
        assert_eq!(resolve("/help", true), Some(SlashAction::Help));
    }

    #[test]
    fn suggestions_follow_the_shared_table() {
        let names: Vec<&str> = suggestions("/").iter().map(|spec| spec.name).collect();
        assert_eq!(names, commands::commands_for(Frontend::Desktop).map(|spec| spec.name).collect::<Vec<_>>());
        let names: Vec<&str> = suggestions("/re").iter().map(|spec| spec.name).collect();
        assert_eq!(names, vec!["review"]);
        assert!(suggestions("/review ").is_empty(), "arguments close the list");
        assert!(suggestions("/diff").is_empty(), "CLI-only commands are not suggested");
        assert!(suggestions("hello").is_empty());
    }

    #[test]
    fn toast_texts_list_the_desktop_commands() {
        let names = command_names();
        assert!(names.starts_with("/plan /build") && names.contains("/undo") && !names.contains("/diff"), "{names}");
        let help = command_help(Lang::En);
        assert!(help.contains("/review [target] — code review"), "{help}");
        assert!(command_help(Lang::Tr).contains("/undo — "));
    }
}
