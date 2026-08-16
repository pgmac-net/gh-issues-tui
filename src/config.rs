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

    /// Issue backend used when `--provider` is not given. Unset → "github".
    /// See `provider::SUPPORTED` for valid names.
    #[serde(default)]
    pub provider: Option<String>,

    /// Start with all repo groups collapsed (the default). They can still
    /// be expanded normally (Space / `]`), and a view showing a single
    /// repo group starts expanded regardless.
    #[serde(default = "default_collapsed_default")]
    pub default_collapsed: bool,

    /// Seconds between automatic background refreshes of the issue list.
    /// `0` disables auto-refresh. Overridden by `--refresh`.
    #[serde(default = "refresh_interval_default")]
    pub refresh_interval: u64,

    /// Hide repo groups with no visible issues (the default). The filter
    /// editor can flip this per session; clearing filters restores it.
    #[serde(default = "hide_empty_repos_default")]
    pub hide_empty_repos: bool,

    /// Name of the colour profile to use, one of the `[color_profiles.*]`
    /// tables below. Unset → built-in colours.
    #[serde(default)]
    pub color_profile: Option<String>,

    /// Template for the short reference copied to the clipboard with `y`.
    /// Supports `{owner}`, `{repo}`, `{number}` placeholders.
    #[serde(default = "copy_format_default")]
    pub copy_format: String,

    /// User-defined colour profiles: `[color_profiles.<name>]` tables whose
    /// entries override individual UI colours (see `theme::ColorProfile`).
    #[serde(default, skip_serializing)]
    pub color_profiles: HashMap<String, ColorProfile>,

    /// Harness started by `A` when the issue has no session yet. Unset →
    /// `A` opens the harness picker instead of guessing.
    #[serde(default)]
    pub default_harness: Option<String>,

    /// Directories searched for a repo's clone when launching a harness, in
    /// order; the first `<root>/<repo>` that exists wins. `~` is expanded.
    /// Empty (the default) means only the cwd's own repo can be launched
    /// into — see `harness::workspace_dir`.
    #[serde(default)]
    pub workspace_roots: Vec<String>,

    /// Coding harnesses `A` can launch: `[harnesses.<name>]` tables.
    /// Built-in entries (see `builtin_harnesses`) are merged in for names
    /// the config does not define, so the common ones work with no config
    /// at all while staying fully overridable.
    #[serde(default)]
    pub harnesses: HashMap<String, HarnessConfig>,
}

/// One entry in the `[harnesses.*]` table.
///
/// `command` is an **argv array, never a shell string** — placeholders
/// expand into individual argv slots, so an issue title containing
/// `$(...)`, backticks or quotes is inert. Issue text is attacker-controlled
/// in a public org; routing it through `sh -c` would be a live injection hole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessConfig {
    /// Program and arguments. Supports the `{owner}`, `{repo}`, `{number}`,
    /// `{ref}` and `{url}` placeholders.
    pub command: Vec<String>,

    /// Overrides the top-level `workspace_roots` for this harness only.
    #[serde(default)]
    pub workspace_roots: Option<Vec<String>>,
}

/// Harnesses that ship working out of the box.
///
/// Only harnesses whose argument form has actually been verified are listed:
/// `claude [prompt]` starts an interactive session on a prompt, and
/// `opencode run [message..]` runs one non-interactively. `codex`, `copilot`
/// and `pi` are documented in the README as ready-to-paste snippets instead —
/// a shipped default built from a guessed argv fails at spawn time, which is
/// worse than no default at all.
pub fn builtin_harnesses() -> HashMap<String, HarnessConfig> {
    HashMap::from([
        (
            "claude".to_string(),
            HarnessConfig {
                command: vec![
                    "claude".into(),
                    "/pgmac-workflows:pickup-ticket {ref}".into(),
                ],
                workspace_roots: None,
            },
        ),
        (
            "opencode".to_string(),
            HarnessConfig {
                command: vec!["opencode".into(), "run".into(), "work on {url}".into()],
                workspace_roots: None,
            },
        ),
    ])
}

fn default_collapsed_default() -> bool {
    true
}

fn refresh_interval_default() -> u64 {
    300
}

fn hide_empty_repos_default() -> bool {
    true
}

fn copy_format_default() -> String {
    "{owner}/{repo}#{number}".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_org: None,
            provider: None,
            default_collapsed: true,
            refresh_interval: refresh_interval_default(),
            hide_empty_repos: true,
            color_profile: None,
            copy_format: copy_format_default(),
            color_profiles: HashMap::new(),
            default_harness: None,
            workspace_roots: Vec::new(),
            harnesses: builtin_harnesses(),
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
        let mut cfg: Self =
            toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        // Built-ins fill the gaps rather than replacing the parsed table:
        // defining `[harnesses.codex]` must not silently delete `claude`,
        // which a plain `#[serde(default = ...)]` on the field would do.
        for (name, harness) in builtin_harnesses() {
            cfg.harnesses.entry(name).or_insert(harness);
        }
        Ok(cfg)
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
    fn refresh_interval_defaults_to_five_minutes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "default_org = \"pgmac-net\"\n").unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.refresh_interval, 300);
    }

    #[test]
    fn parses_refresh_interval() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "refresh_interval = 60\n").unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.refresh_interval, 60);
    }

    #[test]
    fn hide_empty_repos_defaults_true_and_parses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "default_org = \"pgmac-net\"\n").unwrap();
        assert!(Config::load_from(&path).unwrap().hide_empty_repos);

        std::fs::write(&path, "hide_empty_repos = false\n").unwrap();
        assert!(!Config::load_from(&path).unwrap().hide_empty_repos);
    }

    #[test]
    fn copy_format_defaults_to_owner_repo_number() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "default_org = \"pgmac-net\"\n").unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.copy_format, "{owner}/{repo}#{number}");
    }

    #[test]
    fn parses_copy_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "copy_format = \"{repo}#{number}\"\n").unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.copy_format, "{repo}#{number}");
    }

    #[test]
    fn refresh_interval_zero_disables() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "refresh_interval = 0\n").unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.refresh_interval, 0);
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

    fn cfg_from(body: &str) -> Config {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, body).unwrap();
        Config::load_from(&path).unwrap()
    }

    fn harness_names(cfg: &Config) -> Vec<String> {
        let mut names: Vec<String> = cfg.harnesses.keys().cloned().collect();
        names.sort_unstable();
        names
    }

    #[test]
    fn builtin_harnesses_are_available_with_no_config() {
        let cfg = cfg_from("default_org = \"pgmac-net\"\n");
        assert_eq!(harness_names(&cfg), vec!["claude", "opencode"]);
        assert_eq!(cfg.harnesses["claude"].command[0], "claude");
    }

    #[test]
    fn a_user_harness_is_added_without_dropping_the_builtins() {
        // The bug this pins: `#[serde(default = "builtin_harnesses")]` would
        // replace the whole map, so defining one harness would delete claude.
        let cfg = cfg_from(
            "[harnesses.codex]\n\
             command = [\"codex\", \"work on {url}\"]\n",
        );
        assert_eq!(harness_names(&cfg), vec!["claude", "codex", "opencode"]);
    }

    #[test]
    fn a_user_harness_overrides_the_builtin_of_the_same_name() {
        let cfg = cfg_from(
            "[harnesses.claude]\n\
             command = [\"claude\", \"--resume\", \"{ref}\"]\n",
        );
        assert_eq!(
            cfg.harnesses["claude"].command,
            vec!["claude", "--resume", "{ref}"]
        );
        assert_eq!(harness_names(&cfg).len(), 2, "opencode still merged in");
    }

    #[test]
    fn parses_default_harness_and_workspace_roots() {
        let cfg = cfg_from(
            "default_harness = \"claude\"\n\
             workspace_roots = [\"~/pgmac\", \"~/projects\"]\n",
        );
        assert_eq!(cfg.default_harness.as_deref(), Some("claude"));
        assert_eq!(cfg.workspace_roots, vec!["~/pgmac", "~/projects"]);
    }

    #[test]
    fn workspace_roots_default_to_empty_and_harness_to_none() {
        let cfg = cfg_from("default_org = \"pgmac-net\"\n");
        assert!(cfg.workspace_roots.is_empty());
        assert!(cfg.default_harness.is_none());
    }

    #[test]
    fn a_harness_may_override_workspace_roots() {
        let cfg = cfg_from(
            "workspace_roots = [\"~/pgmac\"]\n\
             [harnesses.work]\n\
             command = [\"claude\"]\n\
             workspace_roots = [\"~/projects\"]\n",
        );
        assert_eq!(
            cfg.harnesses["work"].workspace_roots.as_deref(),
            Some(["~/projects".to_string()].as_slice())
        );
        assert!(cfg.harnesses["claude"].workspace_roots.is_none());
    }

    #[test]
    fn a_harness_without_a_command_is_a_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[harnesses.broken]\nworkspace_roots = []\n").unwrap();
        assert!(Config::load_from(&path).is_err());
    }
}
