//! Colour theme for the UI, overridable per profile from the config file.

use ratatui::style::Color;
use serde::Deserialize;

/// The resolved set of colours the UI draws with. Defaults reproduce the
/// original hard-coded scheme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    /// Repo headers, comment authors, prompts, help keys.
    pub accent: Color,
    /// Issue numbers, dates, secondary metadata.
    pub dim: Color,
    /// Background of the selected row in the list and popup pickers.
    pub selected_bg: Color,
    /// Open-issue state dot and label.
    pub open: Color,
    /// Closed-issue state dot and label.
    pub closed: Color,
    /// Assignee badges and the detail-view assignees/labels line.
    pub assignee: Color,
    /// Rate-limit warnings and transient statuses.
    pub warning: Color,
    /// Errors (rate-limit exhausted, failed operations).
    pub error: Color,
    /// Fallback for GitHub labels whose colour can't be parsed.
    pub label_fallback: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            accent: Color::Cyan,
            dim: Color::DarkGray,
            selected_bg: Color::Rgb(45, 90, 160),
            open: Color::Green,
            closed: Color::Magenta,
            assignee: Color::Yellow,
            warning: Color::Yellow,
            error: Color::Red,
            label_fallback: Color::Blue,
        }
    }
}

/// One `[color_profiles.<name>]` table from the config file. Every field is
/// optional; unset fields keep the built-in default. Colours parse from
/// ratatui's string forms: names ("cyan", "darkgray"), hex ("#2d5aa0"), or
/// ANSI indexes ("14").
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColorProfile {
    pub accent: Option<Color>,
    pub dim: Option<Color>,
    pub selected_bg: Option<Color>,
    pub open: Option<Color>,
    pub closed: Option<Color>,
    pub assignee: Option<Color>,
    pub warning: Option<Color>,
    pub error: Option<Color>,
    pub label_fallback: Option<Color>,
}

impl Theme {
    /// The default theme with a profile's overrides applied.
    pub fn with_profile(profile: &ColorProfile) -> Self {
        let d = Theme::default();
        Self {
            accent: profile.accent.unwrap_or(d.accent),
            dim: profile.dim.unwrap_or(d.dim),
            selected_bg: profile.selected_bg.unwrap_or(d.selected_bg),
            open: profile.open.unwrap_or(d.open),
            closed: profile.closed.unwrap_or(d.closed),
            assignee: profile.assignee.unwrap_or(d.assignee),
            warning: profile.warning.unwrap_or(d.warning),
            error: profile.error.unwrap_or(d.error),
            label_fallback: profile.label_fallback.unwrap_or(d.label_fallback),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_profile_yields_default_theme() {
        let theme = Theme::with_profile(&ColorProfile::default());
        assert_eq!(theme, Theme::default());
    }

    #[test]
    fn profile_overrides_only_set_fields() {
        let profile: ColorProfile = toml::from_str(
            "accent = \"#83a598\"\n\
             selected_bg = \"dark gray\"\n",
        )
        .unwrap();
        let theme = Theme::with_profile(&profile);
        assert_eq!(theme.accent, Color::Rgb(0x83, 0xa5, 0x98));
        assert_eq!(theme.selected_bg, Color::DarkGray);
        // Untouched fields keep defaults.
        assert_eq!(theme.open, Color::Green);
        assert_eq!(theme.error, Color::Red);
    }

    #[test]
    fn named_hex_and_indexed_colors_parse() {
        let profile: ColorProfile = toml::from_str(
            "open = \"lightgreen\"\n\
             closed = \"#ff00ff\"\n\
             dim = \"8\"\n",
        )
        .unwrap();
        assert_eq!(profile.open, Some(Color::LightGreen));
        assert_eq!(profile.closed, Some(Color::Rgb(0xff, 0x00, 0xff)));
        assert_eq!(profile.dim, Some(Color::Indexed(8)));
    }

    #[test]
    fn invalid_color_string_is_rejected() {
        assert!(toml::from_str::<ColorProfile>("accent = \"not-a-color\"\n").is_err());
    }

    #[test]
    fn unknown_field_is_rejected() {
        assert!(toml::from_str::<ColorProfile>("acent = \"red\"\n").is_err());
    }
}
