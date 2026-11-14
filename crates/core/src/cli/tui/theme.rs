//! One small colour theme for the whole TUI (UX-023).
//!
//! Dark and light presets, picked by `[tui] theme` in `config.toml` or, when
//! unset, by `COLORFGBG`. `NO_COLOR` keeps only bold, dim, reverse and the
//! glyphs. Secondary text uses an explicit grey instead of the `DIM`
//! modifier, which many terminals ignore.

use locagens_config::config_toml::TuiConfig;
use ratatui::style::{Color, Modifier, Style};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub accent: Color,
    pub muted: Color,
    pub warn: Color,
    pub ok: Color,
    pub err: Color,
    /// Plan mode (read-only) accent.
    pub plan: Color,
    pub agent: Color,
    /// Subtle full-width band behind user prompts.
    pub user_bg: Option<Color>,
    /// `NO_COLOR`: no hues at all.
    pub plain: bool,
}

impl Theme {
    pub const fn dark() -> Self {
        Self {
            accent: Color::Cyan,
            muted: Color::Indexed(245),
            warn: Color::Yellow,
            ok: Color::Green,
            err: Color::Red,
            plan: Color::LightBlue,
            agent: Color::Magenta,
            user_bg: Some(Color::Indexed(236)),
            plain: false,
        }
    }

    pub const fn light() -> Self {
        Self {
            accent: Color::Indexed(25),
            muted: Color::Indexed(241),
            warn: Color::Indexed(130),
            ok: Color::Indexed(28),
            err: Color::Indexed(124),
            plan: Color::Indexed(91),
            agent: Color::Indexed(90),
            user_bg: Some(Color::Indexed(254)),
            plain: false,
        }
    }

    pub const fn plain() -> Self {
        Self {
            accent: Color::Reset,
            muted: Color::Reset,
            warn: Color::Reset,
            ok: Color::Reset,
            err: Color::Reset,
            plan: Color::Reset,
            agent: Color::Reset,
            user_bg: None,
            plain: true,
        }
    }

    /// Picks the preset: `NO_COLOR` wins, then the configured name, then
    /// `COLORFGBG` (`fg;bg`, a light background is 7 or 15), else dark.
    pub fn choose(no_color: bool, configured: Option<&str>, colorfgbg: Option<&str>) -> Self {
        if no_color {
            return Self::plain();
        }
        match configured.map(|name| name.trim().to_lowercase()).as_deref() {
            Some("light") => Self::light(),
            Some("dark") => Self::dark(),
            _ if background_is_light(colorfgbg) => Self::light(),
            _ => Self::dark(),
        }
    }
}

fn background_is_light(colorfgbg: Option<&str>) -> bool {
    colorfgbg
        .and_then(|value| value.rsplit(';').next())
        .and_then(|bg| bg.trim().parse::<u8>().ok())
        .is_some_and(|bg| bg == 7 || bg == 15)
}

/// Whether the spinner and shimmer are off: `LCGN_REDUCED_MOTION=1` or
/// `[tui] animations = false`.
pub fn reduced_motion(env: Option<&str>, configured: Option<bool>) -> bool {
    let env_on = env.is_some_and(|v| !matches!(v.trim(), "" | "0" | "false"));
    env_on || configured == Some(false)
}

static THEME: OnceLock<Theme> = OnceLock::new();

/// Installs the theme from the environment and `[tui]`; first call wins.
pub fn init(config: Option<&TuiConfig>) {
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    let colorfgbg = std::env::var("COLORFGBG").ok();
    let theme = Theme::choose(no_color, config.and_then(|c| c.theme.as_deref()), colorfgbg.as_deref());
    let _ = THEME.set(theme);
}

pub fn current() -> &'static Theme {
    THEME.get_or_init(Theme::dark)
}

pub fn accent() -> Color {
    current().accent
}
pub fn warn() -> Color {
    current().warn
}
pub fn ok() -> Color {
    current().ok
}
pub fn err() -> Color {
    current().err
}
pub fn plan() -> Color {
    current().plan
}
pub fn agent() -> Color {
    current().agent
}

/// Secondary text.
pub fn dim() -> Style {
    let theme = current();
    if theme.plain {
        Style::default().add_modifier(Modifier::DIM)
    } else {
        Style::default().fg(theme.muted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_color_wins_then_config_then_colorfgbg() {
        assert!(Theme::choose(true, Some("light"), None).plain);
        assert_eq!(Theme::choose(false, Some("light"), Some("15;0")), Theme::light());
        assert_eq!(Theme::choose(false, Some("DARK"), Some("0;15")), Theme::dark());
        assert_eq!(Theme::choose(false, None, Some("0;15")), Theme::light());
        assert_eq!(Theme::choose(false, None, Some("15;0")), Theme::dark());
        assert_eq!(Theme::choose(false, None, None), Theme::dark());
        assert_eq!(Theme::choose(false, Some("nonsense"), Some("0;7")), Theme::light());
    }

    #[test]
    fn plain_theme_has_no_hues() {
        let plain = Theme::plain();
        assert_eq!(plain.accent, Color::Reset);
        assert!(plain.user_bg.is_none());
    }

    #[test]
    fn reduced_motion_from_env_or_config() {
        assert!(reduced_motion(Some("1"), None));
        assert!(!reduced_motion(Some("0"), None));
        assert!(!reduced_motion(Some(""), Some(true)));
        assert!(reduced_motion(None, Some(false)));
        assert!(!reduced_motion(None, None));
    }
}
