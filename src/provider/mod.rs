pub mod error;
pub mod types;

use std::sync::Arc;

use async_trait::async_trait;

use error::{ProviderError, Result};
use types::{
    Comment, FormOptions, IssueState, NewIssueParams, PrRef, PrSummary, RateLimitData, RepoIssues,
    RepoLabel,
};

/// The backend-neutral surface the TUI talks to. One provider is selected at
/// startup (`--provider` flag / `provider` config key); everything the event
/// loop spawns goes through this trait, never a concrete client.
///
/// Core methods are required. Capability methods (`pull_request`) have
/// defaults that report `Unsupported` — providers opt in by overriding both
/// the method and its `supports_*` probe, and the UI hides the affordance
/// when the probe returns `false`.
#[async_trait]
pub trait IssueProvider: Send + Sync {
    /// Fetch all issues for every repository (or the provider's nearest
    /// grouping concept) owned by `org`.
    async fn org_issues(&self, org: &str, include_closed: bool) -> Result<Vec<RepoIssues>>;

    async fn comments(&self, issue_id: &str) -> Result<Vec<Comment>>;

    async fn add_comment(&self, issue_id: &str, body: &str) -> Result<()>;

    async fn set_state(&self, issue_id: &str, state: IssueState) -> Result<()>;

    async fn update_title(&self, issue_id: &str, title: &str) -> Result<()>;

    /// Replace the full assignee set with the users named in `logins`.
    async fn set_assignees(&self, issue_id: &str, logins: &[String]) -> Result<()>;

    /// Replace the full label set with labels named in `names` (must exist
    /// on the repo).
    async fn set_labels(
        &self,
        issue_id: &str,
        repo: &str,
        org: &str,
        names: &[String],
    ) -> Result<()>;

    async fn repo_labels(&self, org: &str, repo: &str) -> Result<Vec<RepoLabel>>;

    /// Everything the new-issue form's pickers need, fetched per repo.
    async fn repo_form_options(&self, org: &str, repo: &str) -> Result<FormOptions>;

    /// Create an issue; returns its number and node id.
    async fn create_issue(&self, p: &NewIssueParams) -> Result<(u64, String)>;

    /// The most recently observed rate limit state, if the provider tracks one.
    fn rate_limit(&self) -> Option<RateLimitData>;

    /// Capability probe for [`IssueProvider::pull_request`]. The UI checks
    /// this before offering the PR-summary popup.
    fn supports_pr_summary(&self) -> bool {
        false
    }

    /// Fetch the PR-summary popup's data. Capability method — only
    /// meaningful when [`IssueProvider::supports_pr_summary`] is `true`.
    async fn pull_request(&self, _pr: &PrRef) -> Result<PrSummary> {
        Err(ProviderError::Unsupported("pull request summaries"))
    }
}

/// The shared handle the event loop clones into spawned tasks.
pub type Provider = Arc<dyn IssueProvider>;

/// Provider names accepted by [`build`], in display order.
pub const SUPPORTED: &[&str] = &["github", "linear"];

/// Build the provider selected by `name`, resolving its credentials.
/// `token_flag` is the `--token` CLI value, meaningful per provider
/// (GitHub: flag → GITHUB_TOKEN → GH_TOKEN → `gh auth token`).
pub fn build(name: &str, token_flag: Option<String>) -> anyhow::Result<Provider> {
    match name {
        "github" => {
            let token = crate::github::auth::resolve_token(token_flag)?;
            Ok(Arc::new(crate::github::Client::new(token)?))
        }
        "linear" => {
            let key = crate::linear::auth::resolve_key(token_flag)?;
            Ok(Arc::new(crate::linear::Client::new(key)?))
        }
        other => anyhow::bail!(
            "unknown provider '{other}'; supported: {}",
            SUPPORTED.join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_rejects_unknown_provider() {
        let Err(err) = build("jira", None) else {
            panic!("expected build to fail");
        };
        let err = err.to_string();
        assert!(err.contains("unknown provider 'jira'"), "{err}");
        assert!(err.contains("github"), "{err}");
        assert!(err.contains("linear"), "{err}");
    }

    struct Minimal;

    #[async_trait]
    impl IssueProvider for Minimal {
        async fn org_issues(&self, _: &str, _: bool) -> Result<Vec<RepoIssues>> {
            Ok(vec![])
        }
        async fn comments(&self, _: &str) -> Result<Vec<Comment>> {
            Ok(vec![])
        }
        async fn add_comment(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        async fn set_state(&self, _: &str, _: IssueState) -> Result<()> {
            Ok(())
        }
        async fn update_title(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        async fn set_assignees(&self, _: &str, _: &[String]) -> Result<()> {
            Ok(())
        }
        async fn set_labels(&self, _: &str, _: &str, _: &str, _: &[String]) -> Result<()> {
            Ok(())
        }
        async fn repo_labels(&self, _: &str, _: &str) -> Result<Vec<RepoLabel>> {
            Ok(vec![])
        }
        async fn repo_form_options(&self, _: &str, _: &str) -> Result<FormOptions> {
            Ok(FormOptions::default())
        }
        async fn create_issue(&self, _: &NewIssueParams) -> Result<(u64, String)> {
            Ok((0, String::new()))
        }
        fn rate_limit(&self) -> Option<RateLimitData> {
            None
        }
        // Deliberately no capability overrides — exercises the defaults.
    }

    /// A trait impl that skips the capability methods gets the safe defaults:
    /// probe says unsupported, fetch errors instead of hanging or panicking.
    #[tokio::test]
    async fn capability_defaults_report_unsupported() {
        let p = Minimal;
        assert!(!p.supports_pr_summary());
        let err = p
            .pull_request(&PrRef {
                owner: "o".into(),
                repo: "r".into(),
                number: 1,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Unsupported(_)));
    }
}
