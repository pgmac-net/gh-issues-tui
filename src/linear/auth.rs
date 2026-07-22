use anyhow::{Result, bail};

/// Resolve a Linear API key: CLI flag → `LINEAR_API_KEY` → `LINEAR_TOKEN`.
///
/// Unlike GitHub there is no local-CLI fallback (Linear has no `gh`
/// equivalent installed on developer machines), so the chain is flag/env only.
pub fn resolve_key(cli_token: Option<String>) -> Result<String> {
    resolve_key_inner(cli_token, &env_key)
}

fn env_key(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn resolve_key_inner(
    cli_token: Option<String>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<String> {
    if let Some(t) = cli_token.filter(|t| !t.trim().is_empty()) {
        return Ok(t);
    }
    if let Some(t) = env("LINEAR_API_KEY") {
        return Ok(t);
    }
    if let Some(t) = env("LINEAR_TOKEN") {
        return Ok(t);
    }
    bail!("no Linear API key found: pass --token, or set LINEAR_API_KEY (or LINEAR_TOKEN)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_token_wins() {
        let t = resolve_key_inner(Some("cli-key".into()), &|_| Some("env-key".into())).unwrap();
        assert_eq!(t, "cli-key");
    }

    #[test]
    fn api_key_beats_token() {
        let t = resolve_key_inner(None, &|name| match name {
            "LINEAR_API_KEY" => Some("primary".into()),
            _ => Some("secondary".into()),
        })
        .unwrap();
        assert_eq!(t, "primary");
    }

    #[test]
    fn falls_through_to_linear_token() {
        let t = resolve_key_inner(None, &|name| (name == "LINEAR_TOKEN").then(|| "tok".into()))
            .unwrap();
        assert_eq!(t, "tok");
    }

    #[test]
    fn empty_cli_token_is_skipped() {
        let t = resolve_key_inner(Some("   ".into()), &|name| {
            (name == "LINEAR_API_KEY").then(|| "env".into())
        })
        .unwrap();
        assert_eq!(t, "env");
    }

    #[test]
    fn errors_when_nothing_found() {
        assert!(resolve_key_inner(None, &|_| None).is_err());
    }
}
