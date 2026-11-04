//! In-place issue updates after a mutation (#196).
//!
//! An issue-level mutation reports the issue as the backend holds it after
//! the write, and that one entry is replaced here — no org refetch. A
//! refetch straight after a write can read a stale replica (or an older
//! auto-refresh can land last), which is what left the list showing old
//! labels and an "open" marker on a just-closed issue. `set_data` closes the
//! same window from the other side via [`keep_newer_loaded`].

use std::collections::HashMap;
use std::time::Instant;

use super::App;
use super::pins::Pins;
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
    /// The moved issue is pinned (#249), so a refetch that cannot see it yet
    /// does not drop it.
    pub fn apply_move(&mut self, from_id: &str, target: &str, issue: Issue, now: Instant) -> bool {
        self.data_gen += 1;
        let prev_selected = self.selected_issue().map(|i| i.id.clone());
        for repo in &mut self.repos {
            repo.issues.retain(|i| i.id != from_id);
        }
        self.invalidate_comments(from_id);
        self.pins.release(from_id);
        let id = issue.id.clone();
        self.vocab.absorb_issue(&issue);
        self.place_issue(target, issue);
        self.pins.pin(id.clone(), now);
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

    /// An issue was created in `repo` (#249): show the copy the backend
    /// reported, with no refetch (ADR 0005). It is pinned until a refetch
    /// sees it (ADR 0011), its group is expanded and it is selected. Returns
    /// whether it is shown — the filters may hide it.
    pub fn insert_created(&mut self, repo: &str, issue: Issue, now: Instant) -> bool {
        self.data_gen += 1;
        let prev_selected = self.selected_issue().map(|i| i.id.clone());
        let id = issue.id.clone();
        self.vocab.absorb_issue(&issue);
        let slot = self.place_issue(repo, issue);
        self.pins.pin(id.clone(), now);
        // An explicit create in this repo beats a manual collapse, and a
        // group first seen here must not fold under `default_collapsed`.
        let group = self.repos[slot].repo.clone();
        self.collapsed.remove(&group);
        self.seen_repos.insert(group);
        self.stamp_label_ranks();
        self.rebuild_rows();
        let shown = self
            .rows
            .iter()
            .any(|row| self.row_issue_id(row) == Some(id.as_str()));
        self.reselect(if shown { Some(id) } else { prev_selected });
        shown
    }

    /// Put `issue` at the top of the `target` group, replacing any copy with
    /// the same id, and create the group in name order when it is not loaded.
    /// Returns the group's index. Callers rebuild rows.
    fn place_issue(&mut self, target: &str, issue: Issue) -> usize {
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
        let id = issue.id.clone();
        self.repos[slot].issues.retain(|i| i.id != id);
        self.repos[slot].issues.insert(0, issue);
        slot
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
/// stay absent — membership is the refetch's to decide — with one exception
/// (ADR 0011): a **pinned** issue (created or moved this session, #249) that
/// the refetch lacks is put back in its group, since the refetch may simply
/// not see it yet. A pinned issue the refetch does include is released.
pub(super) fn keep_newer_loaded(
    loaded: Vec<RepoIssues>,
    mut incoming: Vec<RepoIssues>,
    pins: &mut Pins,
) -> Vec<RepoIssues> {
    // Loaded issues by id, with the group each sat in and its URL.
    let mut newer: HashMap<String, (String, String, Issue)> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for repo in loaded {
        for issue in repo.issues {
            order.push(issue.id.clone());
            newer.insert(
                issue.id.clone(),
                (repo.repo.clone(), repo.repo_url.clone(), issue),
            );
        }
    }
    for issue in incoming.iter_mut().flat_map(|r| r.issues.iter_mut()) {
        pins.release(&issue.id);
        if let Some((_, _, local)) = newer.remove(&issue.id)
            && local.updated_at > issue.updated_at
        {
            *issue = local;
        }
    }
    // Put back pinned issues the refetch did not include, in loaded order.
    for id in order {
        if !pins.contains(&id) {
            continue;
        }
        let Some((group, url, issue)) = newer.remove(&id) else {
            continue;
        };
        let slot = match incoming
            .iter()
            .position(|r| r.repo.eq_ignore_ascii_case(&group))
        {
            Some(i) => i,
            None => {
                let at = incoming
                    .iter()
                    .position(|r| r.repo.to_lowercase() > group.to_lowercase())
                    .unwrap_or(incoming.len());
                incoming.insert(
                    at,
                    RepoIssues {
                        repo: group,
                        repo_url: url,
                        issues: Vec::new(),
                    },
                );
                at
            }
        };
        incoming[slot].issues.insert(0, issue);
    }
    incoming
}
