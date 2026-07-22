use anyhow::{Result, bail};

/// Resolved Jira Cloud credentials. The site URL is not a secret but travels
/// with the credentials for convenience; the token still never touches the
/// config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JiraCreds {
    /// Site base URL, e.g. `https://acme.atlassian.net` (no trailing slash).
    pub base_url: String,
    pub email: String,
    pub token: String,
}

/// Resolve Jira Cloud credentials from the environment: `JIRA_BASE_URL`,
/// `JIRA_EMAIL`, and the token (`--token` flag → `JIRA_API_TOKEN`). Jira Cloud
/// authenticates with HTTP Basic `email:token`, so all three are required.
pub fn resolve(cli_token: Option<String>) -> Result<JiraCreds> {
    resolve_inner(cli_token, &env_var)
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn resolve_inner(
    cli_token: Option<String>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<JiraCreds> {
    let Some(base_url) = env("JIRA_BASE_URL") else {
        bail!("no Jira site: set JIRA_BASE_URL (e.g. https://your-site.atlassian.net)");
    };
    let Some(email) = env("JIRA_EMAIL") else {
        bail!("no Jira account: set JIRA_EMAIL");
    };
    let token = cli_token
        .filter(|t| !t.trim().is_empty())
        .or_else(|| env("JIRA_API_TOKEN"));
    let Some(token) = token else {
        bail!("no Jira API token: pass --token or set JIRA_API_TOKEN");
    };
    Ok(JiraCreds {
        base_url: base_url.trim_end_matches('/').to_string(),
        email,
        token,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn resolves_all_three_and_trims_trailing_slash() {
        let creds = resolve_inner(
            None,
            &env_from(&[
                ("JIRA_BASE_URL", "https://acme.atlassian.net/"),
                ("JIRA_EMAIL", "me@acme.com"),
                ("JIRA_API_TOKEN", "tok"),
            ]),
        )
        .unwrap();
        assert_eq!(creds.base_url, "https://acme.atlassian.net");
        assert_eq!(creds.email, "me@acme.com");
        assert_eq!(creds.token, "tok");
    }

    #[test]
    fn cli_token_overrides_env_token() {
        let creds = resolve_inner(
            Some("cli".into()),
            &env_from(&[
                ("JIRA_BASE_URL", "https://a.atlassian.net"),
                ("JIRA_EMAIL", "e"),
                ("JIRA_API_TOKEN", "env"),
            ]),
        )
        .unwrap();
        assert_eq!(creds.token, "cli");
    }

    #[test]
    fn missing_base_url_errors() {
        let err = resolve_inner(Some("t".into()), &env_from(&[("JIRA_EMAIL", "e")]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("JIRA_BASE_URL"), "{err}");
    }

    #[test]
    fn missing_email_errors() {
        let err = resolve_inner(
            Some("t".into()),
            &env_from(&[("JIRA_BASE_URL", "https://a.atlassian.net")]),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("JIRA_EMAIL"), "{err}");
    }

    #[test]
    fn missing_token_errors() {
        let err = resolve_inner(
            None,
            &env_from(&[
                ("JIRA_BASE_URL", "https://a.atlassian.net"),
                ("JIRA_EMAIL", "e"),
            ]),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("JIRA_API_TOKEN"), "{err}");
    }
}
