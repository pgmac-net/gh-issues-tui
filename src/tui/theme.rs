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
    /// Background fill for fenced and inline code.
    pub code_bg: Color,
    /// Foreground for fenced and inline code text, and for any token the
    /// syntax scanner doesn't classify.
    pub code_fg: Color,
    /// Language keywords in a fenced block, and the structural keys of a
    /// `json`/`yaml`/`toml` block.
    pub code_keyword: Color,
    /// String literals in a fenced block.
    pub code_string: Color,
    /// Comments in a fenced block.
    pub code_comment: Color,
    /// Numeric literals in a fenced block.
    pub code_number: Color,
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
            code_bg: Color::Rgb(38, 38, 38),
            code_fg: Color::Rgb(220, 220, 220),
            // One Dark derived, chosen for contrast against `code_bg` rather
            // than against the terminal's own palette — same reasoning as the
            // explicit RGB for `code_bg`/`code_fg`.
            code_keyword: Color::Rgb(0xc6, 0x78, 0xdd),
            code_string: Color::Rgb(0x98, 0xc3, 0x79),
            code_comment: Color::Rgb(0x7f, 0x84, 0x8e),
            code_number: Color::Rgb(0xd1, 0x9a, 0x66),
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
    pub code_bg: Option<Color>,
    pub code_fg: Option<Color>,
    pub code_keyword: Option<Color>,
    pub code_string: Option<Color>,
    pub code_comment: Option<Color>,
    pub code_number: Option<Color>,
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
            code_bg: profile.code_bg.unwrap_or(d.code_bg),
            code_fg: profile.code_fg.unwrap_or(d.code_fg),
            code_keyword: profile.code_keyword.unwrap_or(d.code_keyword),
            code_string: profile.code_string.unwrap_or(d.code_string),
            code_comment: profile.code_comment.unwrap_or(d.code_comment),
            code_number: profile.code_number.unwrap_or(d.code_number),
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
    fn token_colors_default_and_override_independently() {
        let d = Theme::default();
        // Every token colour must differ from `code_fg`, or highlighting is
        // invisible out of the box.
        for c in [d.code_keyword, d.code_string, d.code_comment, d.code_number] {
            assert_ne!(c, d.code_fg);
        }

        let profile: ColorProfile = toml::from_str(
            "code_keyword = \"#fb4934\"\n\
             code_string  = \"#b8bb26\"\n\
             code_comment = \"#928374\"\n\
             code_number  = \"#d3869b\"\n",
        )
        .unwrap();
        let theme = Theme::with_profile(&profile);
        assert_eq!(theme.code_keyword, Color::Rgb(0xfb, 0x49, 0x34));
        assert_eq!(theme.code_string, Color::Rgb(0xb8, 0xbb, 0x26));
        assert_eq!(theme.code_comment, Color::Rgb(0x92, 0x83, 0x74));
        assert_eq!(theme.code_number, Color::Rgb(0xd3, 0x86, 0x9b));
        // Untouched code keys keep their defaults.
        assert_eq!(theme.code_fg, d.code_fg);
        assert_eq!(theme.code_bg, d.code_bg);
    }

    #[test]
    fn flattening_every_token_colour_to_code_fg_is_expressible() {
        // The documented "no highlighting" recipe — there is no separate
        // on/off switch.
        let profile: ColorProfile = toml::from_str(
            "code_fg      = \"#dcdcdc\"\n\
             code_keyword = \"#dcdcdc\"\n\
             code_string  = \"#dcdcdc\"\n\
             code_comment = \"#dcdcdc\"\n\
             code_number  = \"#dcdcdc\"\n",
        )
        .unwrap();
        let t = Theme::with_profile(&profile);
        let flat = Color::Rgb(0xdc, 0xdc, 0xdc);
        assert!(
            [
                t.code_fg,
                t.code_keyword,
                t.code_string,
                t.code_comment,
                t.code_number
            ]
            .iter()
            .all(|c| *c == flat)
        );
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
