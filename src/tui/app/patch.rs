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
        *slot = issue;
        // The judgement was derived from the old title/body/labels.
        self.invalidate_readiness(&id);
        self.stamp_label_ranks();
        self.rebuild_rows();
        self.reselect(prev_selected);
        true
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
