//! In-place issue updates after a mutation (#196).
//!
//! An issue-level mutation reports the issue as the backend holds it after
//! the write, and that one entry is replaced here — no org refetch. A
//! refetch straight after a write can read a stale replica (or an older
//! auto-refresh can land last), which is what left the list showing old
//! labels and an "open" marker on a just-closed issue. `set_data` closes the
//! same window from the other side via [`keep_newer_loaded`].

use std::collections::HashMap;

use super::App;
use crate::provider::types::{Issue, RepoIssues};

/// A repo's web URL from one of its issues' (`…/repo/issues/42` → `…/repo`),
/// for a group created by a move into a repo that was not loaded. Empty when
/// the URL has no `/issues/` part (Linear, Jira), as an unknown URL would be.
fn repo_url_of(issue_url: &str) -> String {
    issue_url
        .rsplit_once("/issues/")
        .map(|(repo, _)| repo.to_string())
        .unwrap_or_default()
}

impl App {
    /// Replace the loaded copy of `issue` (matched by id) with the one a
    /// mutation reported. Returns `false` when the issue is not loaded — it
    /// left the list under a refetch while the mutation was in flight.
    ///
    /// Filters re-apply as on any data change, so an issue that no longer
    /// matches (closed under the `open` filter, a filtered label removed)
    /// drops out and the selection clamps to its neighbour. Callers route
    /// through `nav()` so the detail pane follows.
    pub fn patch_issue(&mut self, issue: Issue) -> bool {
        let prev_selected = self.selected_issue().map(|i| i.id.clone());
        let Some(slot) = self
            .repos
            .iter_mut()
            .flat_map(|r| r.issues.iter_mut())
            .find(|i| i.id == issue.id)
        else {
            return false;
        };
        let id = issue.id.clone();
        self.vocab.absorb_issue(&issue);
        *slot = issue;
        self.data_gen += 1;
        // The judgement was derived from the old title/body/labels.
        self.invalidate_readiness(&id);
        self.stamp_label_ranks();
        self.rebuild_rows();
        self.reselect(prev_selected);
        true
    }

    /// An issue moved to `target` (#226): drop the copy under its old id and
    /// put the one the move reported into the target group, creating the
    /// group when it is not loaded. No org refetch, as for any patch (ADR
    /// 0005). On GitHub the moved issue has a new id and number. The
    /// selection follows it when it is still shown. Returns whether it is.
    pub fn apply_move(&mut self, from_id: &str, target: &str, issue: Issue) -> bool {
        self.data_gen += 1;
        let prev_selected = self.selected_issue().map(|i| i.id.clone());
        for repo in &mut self.repos {
            repo.issues.retain(|i| i.id != from_id);
        }
        self.invalidate_comments(from_id);
        let id = issue.id.clone();
        self.vocab.absorb_issue(&issue);
        let slot = match self
            .repos
            .iter()
            .position(|r| r.repo.eq_ignore_ascii_case(target))
        {
            Some(i) => i,
            None => {
                let at = self
                    .repos
                    .iter()
                    .position(|r| r.repo.to_lowercase() > target.to_lowercase())
                    .unwrap_or(self.repos.len());
                self.repos.insert(
                    at,
                    RepoIssues {
                        repo: target.to_string(),
                        repo_url: repo_url_of(&issue.url),
                        issues: Vec::new(),
                    },
                );
                at
            }
        };
        self.repos[slot].issues.retain(|i| i.id != id);
        self.repos[slot].issues.insert(0, issue);
        self.stamp_label_ranks();
        self.rebuild_rows();
        let follow = if prev_selected.as_deref() == Some(from_id) {
            Some(id.clone())
        } else {
            prev_selected
        };
        self.reselect(follow);
        self.rows
            .iter()
            .any(|row| self.row_issue_id(row) == Some(id.as_str()))
    }

    /// A comment was added to `issue_id`: the list shows the count, and no
    /// org refetch follows a comment mutation to correct it.
    pub fn bump_comment_count(&mut self, issue_id: &str) {
        if let Some(issue) = self
            .repos
            .iter_mut()
            .flat_map(|r| r.issues.iter_mut())
            .find(|i| i.id == issue_id)
        {
            issue.comment_count += 1;
        }
    }
}

/// Merge a refetch into the loaded data without regressing any issue: where
/// the loaded copy is strictly newer (`updated_at`) than the incoming one, the
/// loaded copy wins. That is a patch the refetch read too early to see —
/// from a lagging replica, or a fetch started before the mutation landed.
/// Equal timestamps take the incoming copy. Issues absent from `incoming`
/// stay absent: membership is always the refetch's to decide.
pub(super) fn keep_newer_loaded(
    loaded: Vec<RepoIssues>,
    mut incoming: Vec<RepoIssues>,
) -> Vec<RepoIssues> {
    let mut newer: HashMap<String, Issue> = loaded
        .into_iter()
        .flat_map(|r| r.issues)
        .map(|i| (i.id.clone(), i))
        .collect();
    for issue in incoming.iter_mut().flat_map(|r| r.issues.iter_mut()) {
        if let Some(local) = newer.remove(&issue.id)
            && local.updated_at > issue.updated_at
        {
            *issue = local;
        }
    }
    incoming
}
