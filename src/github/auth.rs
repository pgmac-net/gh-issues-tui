use anyhow::{Result, bail};

/// Resolve a GitHub token: CLI flag → GITHUB_TOKEN → GH_TOKEN → `gh auth token`.
pub fn resolve_token(cli_token: Option<String>) -> Result<String> {
    resolve_token_inner(cli_token, &env_token, &gh_cli_token)
}

fn env_token(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn gh_cli_token() -> Option<String> {
    let out = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let token = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!token.is_empty()).then_some(token)
}

fn resolve_token_inner(
    cli_token: Option<String>,
    env: &dyn Fn(&str) -> Option<String>,
    gh: &dyn Fn() -> Option<String>,
) -> Result<String> {
    if let Some(t) = cli_token.filter(|t| !t.trim().is_empty()) {
        return Ok(t);
    }
    if let Some(t) = env("GITHUB_TOKEN") {
        return Ok(t);
    }
    if let Some(t) = env("GH_TOKEN") {
        return Ok(t);
    }
    if let Some(t) = gh() {
        return Ok(t);
    }
    bail!(
        "no GitHub token found: pass --token, set GITHUB_TOKEN or GH_TOKEN, \
         or log in with `gh auth login`"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_token_wins() {
        let t = resolve_token_inner(Some("cli-tok".into()), &|_| Some("env-tok".into()), &|| {
            Some("gh-tok".into())
        })
        .unwrap();
        assert_eq!(t, "cli-tok");
    }

    #[test]
    fn github_token_beats_gh_token() {
        let t = resolve_token_inner(
            None,
            &|name| match name {
                "GITHUB_TOKEN" => Some("primary".into()),
                _ => Some("secondary".into()),
            },
            &|| None,
        )
        .unwrap();
        assert_eq!(t, "primary");
    }

    #[test]
    fn falls_through_to_gh_cli() {
        let t = resolve_token_inner(None, &|_| None, &|| Some("gh-tok".into())).unwrap();
        assert_eq!(t, "gh-tok");
    }

    #[test]
    fn empty_cli_token_is_skipped() {
        let t =
            resolve_token_inner(Some("  ".into()), &|_| None, &|| Some("gh-tok".into())).unwrap();
        assert_eq!(t, "gh-tok");
    }

    #[test]
    fn errors_when_nothing_found() {
        assert!(resolve_token_inner(None, &|_| None, &|| None).is_err());
    }
}
