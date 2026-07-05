use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// TOML config at `~/.config/gh-issues/config.toml`.
///
/// Tokens are never stored here — they come from the environment or `gh`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Organisation used when `--org` is not given.
    pub default_org: Option<String>,

    /// Start with all repo groups collapsed. They can still be expanded
    /// normally (Space / `]`).
    #[serde(default)]
    pub default_collapsed: bool,
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
        assert!(!cfg.default_collapsed); // absent field defaults to false
    }

    #[test]
    fn parses_default_collapsed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "default_collapsed = true\n").unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert!(cfg.default_collapsed);
    }

    #[test]
    fn rejects_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "default_org = [broken\n").unwrap();
        assert!(Config::load_from(&path).is_err());
    }
}
