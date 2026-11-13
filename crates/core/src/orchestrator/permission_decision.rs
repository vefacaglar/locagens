//! The answer to a permission prompt, parsed once at the boundary.
//!
//! Frontends send strings (`allow_once`, `allow_project`, `allow_always`,
//! `allow_run`, `deny`). Anything else — a typo, other casing, an empty
//! string — is a denial: an unknown answer must never run a tool.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionDecision {
    AllowOnce,
    /// Standing grant for this project folder (`permissions.json`).
    AllowProject,
    /// Standing grant everywhere (`permissions.json`).
    AllowAlways,
    /// This tool for the rest of the run.
    AllowRun,
    Deny,
}

impl PermissionDecision {
    /// Parses a frontend answer; `None` for anything that is not exactly
    /// one of the five known values.
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "allow_once" => Self::AllowOnce,
            "allow_project" => Self::AllowProject,
            "allow_always" => Self::AllowAlways,
            "allow_run" => Self::AllowRun,
            "deny" => Self::Deny,
            _ => return None,
        })
    }

    /// Parses a frontend answer, treating unknown values as [`Self::Deny`].
    pub fn parse_or_deny(value: &str) -> Self {
        Self::parse(value).unwrap_or(Self::Deny)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AllowOnce => "allow_once",
            Self::AllowProject => "allow_project",
            Self::AllowAlways => "allow_always",
            Self::AllowRun => "allow_run",
            Self::Deny => "deny",
        }
    }

    pub fn allows(self) -> bool {
        self != Self::Deny
    }

    /// The `permissions.json` scope of a standing grant, if this is one.
    pub fn standing_scope(self) -> Option<&'static str> {
        match self {
            Self::AllowProject => Some("project"),
            Self::AllowAlways => Some("global"),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_known_answers_parse_and_everything_else_denies() {
        for decision in [
            PermissionDecision::AllowOnce,
            PermissionDecision::AllowProject,
            PermissionDecision::AllowAlways,
            PermissionDecision::AllowRun,
            PermissionDecision::Deny,
        ] {
            assert_eq!(PermissionDecision::parse(decision.as_str()), Some(decision));
        }
        for unknown in ["", "Deny", "denied", "ALLOW_ONCE", " allow_once", "yes", "allow"] {
            assert_eq!(PermissionDecision::parse(unknown), None, "{unknown:?}");
            assert_eq!(PermissionDecision::parse_or_deny(unknown), PermissionDecision::Deny);
            assert!(!PermissionDecision::parse_or_deny(unknown).allows());
        }
        assert_eq!(PermissionDecision::AllowProject.standing_scope(), Some("project"));
        assert_eq!(PermissionDecision::AllowRun.standing_scope(), None);
    }
}
