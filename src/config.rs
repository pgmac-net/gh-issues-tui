use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::tui::theme::{ColorProfile, Theme};

/// TOML config at `~/.config/gh-issues/config.toml`.
///
/// Tokens are never stored here — they come from the environment or `gh`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Organisation used when `--org` is not given.
    pub default_org: Option<String>,

    /// Start with all repo groups collapsed (the default). They can still
    /// be expanded normally (Space / `]`), and a view showing a single
    /// repo group starts expanded regardless.
    #[serde(default = "default_collapsed_default")]
    pub default_collapsed: bool,

    /// Name of the colour profile to use, one of the `[color_profiles.*]`
    /// tables below. Unset → built-in colours.
    #[serde(default)]
    pub color_profile: Option<String>,

    /// User-defined colour profiles: `[color_profiles.<name>]` tables whose
    /// entries override individual UI colours (see `theme::ColorProfile`).
    #[serde(default, skip_serializing)]
    pub color_profiles: HashMap<String, ColorProfile>,
}

fn default_collapsed_default() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_org: None,
            default_collapsed: true,
            color_profile: None,
            color_profiles: HashMap::new(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("gh-issues")
            .join("config.toml")
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &std::path::Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
    }

    /// Resolve the active colour theme: the profile named by `color_profile`
    /// applied over the built-in defaults. Naming a missing profile is an
    /// error (likely a typo) rather than a silent fallback.
    pub fn resolve_theme(&self) -> Result<Theme> {
        let Some(name) = &self.color_profile else {
            return Ok(Theme::default());
        };
        match self.color_profiles.get(name) {
            Some(profile) => Ok(Theme::with_profile(profile)),
            None => {
                let mut known: Vec<&str> = self.color_profiles.keys().map(String::as_str).collect();
                known.sort_unstable();
                bail!(
                    "color_profile \"{name}\" has no [color_profiles.{name}] table in {} \
                     (defined profiles: {})",
                    Self::path().display(),
                    if known.is_empty() {
                        "none".to_string()
                    } else {
                        known.join(", ")
                    }
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_yields_default() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::load_from(&dir.path().join("nope.toml")).unwrap();
        assert!(cfg.default_org.is_none());
    }

    #[test]
    fn parses_default_org() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "default_org = \"pgmac-net\"\n").unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.default_org.as_deref(), Some("pgmac-net"));
        assert!(cfg.default_collapsed); // absent field defaults to true
    }

    #[test]
    fn parses_default_collapsed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "default_collapsed = false\n").unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert!(!cfg.default_collapsed);
    }

    #[test]
    fn parses_color_profiles_and_resolves_theme() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "color_profile = \"gruvbox\"\n\
             [color_profiles.gruvbox]\n\
             accent = \"#83a598\"\n\
             open = \"lightgreen\"\n\
             [color_profiles.mono]\n\
             accent = \"white\"\n",
        )
        .unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.color_profile.as_deref(), Some("gruvbox"));
        assert_eq!(cfg.color_profiles.len(), 2);

        let theme = cfg.resolve_theme().unwrap();
        assert_eq!(theme.accent, ratatui::style::Color::Rgb(0x83, 0xa5, 0x98));
        assert_eq!(theme.open, ratatui::style::Color::LightGreen);
        assert_eq!(theme.error, Theme::default().error); // unset field
    }

    #[test]
    fn no_profile_selected_uses_default_theme() {
        let cfg = Config::default();
        assert_eq!(cfg.resolve_theme().unwrap(), Theme::default());
    }

    #[test]
    fn unknown_profile_name_errors_with_known_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "color_profile = \"nope\"\n\
             [color_profiles.gruvbox]\n\
             accent = \"white\"\n",
        )
        .unwrap();
        let cfg = Config::load_from(&path).unwrap();
        let err = cfg.resolve_theme().unwrap_err().to_string();
        assert!(err.contains("nope"));
        assert!(err.contains("gruvbox"));
    }

    #[test]
    fn invalid_profile_color_fails_at_parse() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[color_profiles.bad]\n\
             accent = \"nonsense\"\n",
        )
        .unwrap();
        assert!(Config::load_from(&path).is_err());
    }

    #[test]
    fn rejects_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "default_org = [broken\n").unwrap();
        assert!(Config::load_from(&path).is_err());
    }
}
