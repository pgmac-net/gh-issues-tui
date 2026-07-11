//! Detect the GitHub owner/repo of the git repository containing the cwd.
//!
//! Used at startup to scope the issue list to the repo the user is standing
//! in. Everything here is best-effort: any failure (no git, not a repo, no
//! `origin` remote, non-GitHub host) simply disables the feature.

/// Returns `(owner, repo)` for the `origin` remote of the cwd's git repo,
/// when that remote points at github.com.
pub fn detect() -> Option<(String, String)> {
    let out = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = String::from_utf8(out.stdout).ok()?;
    parse_remote_url(url.trim())
}

/// Parses the GitHub remote URL forms git produces:
/// `git@github.com:owner/repo(.git)`, `ssh://git@github.com/owner/repo(.git)`
/// and `https://github.com/owner/repo(.git)`. Non-GitHub hosts yield `None`.
fn parse_remote_url(url: &str) -> Option<(String, String)> {
    let path = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| url.strip_prefix("https://github.com/"))
        .or_else(|| url.strip_prefix("http://github.com/"))?;
    let path = path.strip_suffix(".git").unwrap_or(path);
    let path = path.strip_suffix('/').unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(url: &str) -> Option<(String, String)> {
        parse_remote_url(url)
    }

    #[test]
    fn parses_scp_style_ssh() {
        assert_eq!(
            parsed("git@github.com:pgmac-net/gh-issues-tui.git"),
            Some(("pgmac-net".into(), "gh-issues-tui".into()))
        );
        assert_eq!(
            parsed("git@github.com:pgmac/dotfiles"),
            Some(("pgmac".into(), "dotfiles".into()))
        );
    }

    #[test]
    fn parses_ssh_url() {
        assert_eq!(
            parsed("ssh://git@github.com/pgmac-net/homelabia.git"),
            Some(("pgmac-net".into(), "homelabia".into()))
        );
    }

    #[test]
    fn parses_https_with_and_without_git_suffix() {
        assert_eq!(
            parsed("https://github.com/pgmac-net/homelabia.git"),
            Some(("pgmac-net".into(), "homelabia".into()))
        );
        assert_eq!(
            parsed("https://github.com/pgmac-net/homelabia"),
            Some(("pgmac-net".into(), "homelabia".into()))
        );
        assert_eq!(
            parsed("https://github.com/pgmac-net/homelabia/"),
            Some(("pgmac-net".into(), "homelabia".into()))
        );
    }

    #[test]
    fn rejects_non_github_hosts() {
        assert_eq!(parsed("git@gitlab.com:owner/repo.git"), None);
        assert_eq!(parsed("https://gitlab.com/owner/repo.git"), None);
        assert_eq!(parsed("https://github.example.com/owner/repo"), None);
    }

    #[test]
    fn rejects_malformed_paths() {
        assert_eq!(parsed(""), None);
        assert_eq!(parsed("not a url"), None);
        assert_eq!(parsed("https://github.com/owner-only"), None);
        assert_eq!(parsed("https://github.com/owner/repo/extra"), None);
        assert_eq!(parsed("git@github.com:/repo.git"), None);
    }
}
