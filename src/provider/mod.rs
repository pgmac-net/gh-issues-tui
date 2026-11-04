pub mod error;
pub mod http;
pub mod priority;
pub mod scope;
pub mod types;

use std::sync::Arc;

use async_trait::async_trait;

use error::{ProviderError, Result};
use scope::FetchScope;
use types::{
    Comment, Created, FormOptions, Issue, IssueState, NewIssueParams, PrLookup, PrRef,
    RateLimitData, RepoIssues, RepoLabel,
};

use crate::runs::{ActionsJob, ActionsRun, LinkedPr};

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
    /// grouping concept) owned by `org`, narrowed by `scope` (#230). Every
    /// group is returned even when the scope leaves it with no issues.
    async fn org_issues(&self, org: &str, scope: &FetchScope) -> Result<Vec<RepoIssues>>;
    /// One issue as the backend holds it now, by the id `org_issues` gave it
    /// (#229). `Ok(None)` when it no longer exists or can no longer be seen —
    /// deleted, transferred, access lost.
    async fn issue(&self, issue_id: &str) -> Result<Option<Issue>>;
    /// One group's issues, shaped exactly like the matching `org_issues`
    /// entry (#229). `Ok(None)` when the group is gone, archived, or cannot
    /// hold issues — the same groups `org_issues` leaves out.
    async fn repo_issues(
        &self,
        org: &str,
        repo: &str,
        scope: &FetchScope,
    ) -> Result<Option<RepoIssues>>;

    /// Names of every group `org_issues` would return, without their issues
    /// (#230). Feeds the move-target and repo-filter pickers when the loaded
    /// data covers fewer repos than the org has. The default reports
    /// `Unsupported`, and callers fall back to the loaded groups.
    async fn repo_names(&self, _org: &str) -> Result<Vec<String>> {
        Err(ProviderError::Unsupported("repo names"))
    }

    async fn comments(&self, issue_id: &str) -> Result<Vec<Comment>>;

    async fn add_comment(&self, issue_id: &str, body: &str) -> Result<()>;

    /// Edit an existing comment's body. `comment_id` is the backend node id
    /// carried on [`Comment::id`]; `issue_id` is its parent issue (some
    /// backends — Jira — address a comment by issue key + comment id).
    async fn update_comment(&self, issue_id: &str, comment_id: &str, body: &str) -> Result<()>;

    // Issue-level mutations return the issue as the backend holds it after
    // the write (#196), so the TUI patches that one entry in place instead
    // of refetching the org — a refetch straight after a write can read a
    // stale replica and leave the list showing the old state.

    async fn set_state(&self, issue_id: &str, state: IssueState) -> Result<Issue>;

    async fn update_title(&self, issue_id: &str, title: &str) -> Result<Issue>;

    /// Edit an issue's description (body).
    async fn update_body(&self, issue_id: &str, body: &str) -> Result<Issue>;

    /// Replace the full assignee set with the users named in `logins`.
    async fn set_assignees(&self, issue_id: &str, logins: &[String]) -> Result<Issue>;

    /// Replace the full label set with labels named in `names` (must exist
    /// on the repo).
    async fn set_labels(
        &self,
        issue_id: &str,
        repo: &str,
        org: &str,
        names: &[String],
    ) -> Result<Issue>;

    async fn repo_labels(&self, org: &str, repo: &str) -> Result<Vec<RepoLabel>>;

    /// Everything the new-issue form's pickers need, fetched per repo.
    async fn repo_form_options(&self, org: &str, repo: &str) -> Result<FormOptions>;

    /// Create an issue; returns its number and web URL.
    async fn create_issue(&self, p: &NewIssueParams) -> Result<Created>;

    /// The most recently observed rate limit state, if the provider tracks one.
    fn rate_limit(&self) -> Option<RateLimitData>;

    /// The host this backend talks to, which with the provider name and org
    /// keys the on-disk cache (#226): two Jira sites can share a project key.
    fn host(&self) -> String;

    /// Capability probe for [`IssueProvider::pull_request`]. The UI checks
    /// this before offering the PR-summary popup.
    fn supports_pr_summary(&self) -> bool {
        false
    }

    /// Resolve a reference found in an issue thread. Capability method — only
    /// meaningful when [`IssueProvider::supports_pr_summary`] is `true`.
    ///
    /// Returns [`PrLookup::Issue`] rather than an error when the number turns
    /// out to name an issue: the shorthand forms `parse_pr_links` matches
    /// cannot tell the two apart before fetching.
    async fn pull_request(&self, _pr: &PrRef) -> Result<PrLookup> {
        Err(ProviderError::Unsupported("pull request summaries"))
    }

    /// Capability probe for [`IssueProvider::move_issue`]. The UI checks
    /// this before offering the move-to-another-repo affordance.
    fn supports_move(&self) -> bool {
        false
    }

    /// Move an issue to another repo/team/project (`target`, the
    /// provider's nearest grouping concept — same meaning as
    /// [`RepoIssues::repo`]) within the same org. Capability method — only
    /// meaningful when [`IssueProvider::supports_move`] is `true`.
    /// Move an issue to another repo (team), returning it as it is after the
    /// move (#226): possibly a new id and number. Not every backend can.
    async fn move_issue(&self, _issue_id: &str, _org: &str, _target: &str) -> Result<Issue> {
        Err(ProviderError::Unsupported("moving issues between repos"))
    }

    /// Capability probe for the workflow-runs tab (#222) and every method
    /// below it. The UI checks this before offering the tab.
    fn supports_runs(&self) -> bool {
        false
    }

    /// The pull requests linked to an issue, in any state: those the backend
    /// knows reference it, plus `text_refs` found in its text. A reference
    /// that turns out to name an issue, or nothing, is dropped.
    async fn linked_prs(&self, _issue_id: &str, _text_refs: &[PrRef]) -> Result<Vec<LinkedPr>> {
        Err(ProviderError::Unsupported("workflow runs"))
    }

    /// The workflow runs of a commit, without their jobs.
    async fn head_runs(&self, _owner: &str, _repo: &str, _sha: &str) -> Result<Vec<ActionsRun>> {
        Err(ProviderError::Unsupported("workflow runs"))
    }

    /// The jobs of a run's latest attempt, with their steps.
    async fn run_jobs(&self, _owner: &str, _repo: &str, _run_id: i64) -> Result<Vec<ActionsJob>> {
        Err(ProviderError::Unsupported("workflow runs"))
    }

    /// Recent successful runs of a workflow for one trigger event, with jobs:
    /// what an ETA is learned from.
    async fn history_runs(
        &self,
        _owner: &str,
        _repo: &str,
        _workflow_id: i64,
        _event: &str,
    ) -> Result<Vec<ActionsRun>> {
        Err(ProviderError::Unsupported("workflow runs"))
    }

    /// Most recently observed rate limit for the workflow-runs calls, when
    /// they draw on a different budget from [`IssueProvider::rate_limit`].
    fn runs_rate_limit(&self) -> Option<RateLimitData> {
        None
    }
}

/// The shared handle the event loop clones into spawned tasks.
pub type Provider = Arc<dyn IssueProvider>;

/// Provider names accepted by [`build`], in display order.
pub const SUPPORTED: &[&str] = &["github", "linear", "jira"];

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
        "jira" => {
            let creds = crate::jira::auth::resolve(token_flag)?;
            Ok(Arc::new(crate::jira::Client::new(creds)?))
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
        let Err(err) = build("asana", None) else {
            panic!("expected build to fail");
        };
        let err = err.to_string();
        assert!(err.contains("unknown provider 'asana'"), "{err}");
        assert!(err.contains("github"), "{err}");
        assert!(err.contains("linear"), "{err}");
        assert!(err.contains("jira"), "{err}");
    }

    struct Minimal;

    #[async_trait]
    impl IssueProvider for Minimal {
        async fn org_issues(&self, _: &str, _: &FetchScope) -> Result<Vec<RepoIssues>> {
            Ok(vec![])
        }
        async fn issue(&self, _: &str) -> Result<Option<Issue>> {
            Ok(None)
        }
        async fn repo_issues(
            &self,
            _: &str,
            _: &str,
            _: &FetchScope,
        ) -> Result<Option<RepoIssues>> {
            Ok(None)
        }
        async fn comments(&self, _: &str) -> Result<Vec<Comment>> {
            Ok(vec![])
        }
        async fn add_comment(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        async fn update_comment(&self, _: &str, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        async fn set_state(&self, _: &str, _: IssueState) -> Result<Issue> {
            Err(ProviderError::Unsupported("stub"))
        }
        async fn update_title(&self, _: &str, _: &str) -> Result<Issue> {
            Err(ProviderError::Unsupported("stub"))
        }
        async fn update_body(&self, _: &str, _: &str) -> Result<Issue> {
            Err(ProviderError::Unsupported("stub"))
        }
        async fn set_assignees(&self, _: &str, _: &[String]) -> Result<Issue> {
            Err(ProviderError::Unsupported("stub"))
        }
        async fn set_labels(&self, _: &str, _: &str, _: &str, _: &[String]) -> Result<Issue> {
            Err(ProviderError::Unsupported("stub"))
        }
        async fn repo_labels(&self, _: &str, _: &str) -> Result<Vec<RepoLabel>> {
            Ok(vec![])
        }
        async fn repo_form_options(&self, _: &str, _: &str) -> Result<FormOptions> {
            Ok(FormOptions::default())
        }
        async fn create_issue(&self, _: &NewIssueParams) -> Result<Created> {
            Ok(Created::new(Issue::fixture("I_0", 0)))
        }
        fn host(&self) -> String {
            String::new()
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

        assert!(!p.supports_move());
        let err = p.move_issue("I_1", "org", "target").await.unwrap_err();
        assert!(matches!(err, ProviderError::Unsupported(_)));
    }
}
