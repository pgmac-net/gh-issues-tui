//! Session state for the ticket-readiness badge (#160).
//!
//! Kept beside `comment_cache` and dropped by the same paths: a readiness
//! answer is derived from the body and thread, so it goes stale the moment
//! either changes. That is the opposite of an inferred label rank, whose
//! meaning does not drift and which is cached on disk without a TTL.
//!
//! Nothing here reaches a launch — see `typesafe::readiness`.

use super::prelude::*;

/// Judgements received, requests out, and whether asking has failed.
///
/// Shaped like [`RankState`](super::RankState) and for the same reasons: one
/// in-flight guard so navigating a list does not ask twice about the same
/// ticket, and one failure latch so a dead endpoint is not retried on every
/// keypress.
#[derive(Debug, Default)]
pub struct ReadinessState {
    answers: HashMap<String, Readiness>,
    in_flight: HashSet<String>,
    /// Asking failed. Off for the rest of the session: no retry storm, and the
    /// pane simply has no badge, which is how it behaved before this existed.
    failed: bool,
}

impl ReadinessState {
    /// The judgement for `issue_id`, if one has landed.
    pub fn get(&self, issue_id: &str) -> Option<&Readiness> {
        self.answers.get(issue_id)
    }

    /// Forget one judgement — paired with `invalidate_comments`, since the
    /// thread it was derived from is what just changed.
    pub fn invalidate(&mut self, issue_id: &str) {
        self.answers.remove(issue_id);
    }

    /// Drop everything, including the failure latch: fresh data or a new org
    /// deserves a fresh attempt.
    pub fn clear(&mut self) {
        self.answers.clear();
        self.in_flight.clear();
        self.failed = false;
    }

    /// Asking has failed this session, so nothing will ask again.
    #[cfg(test)]
    pub fn has_failed(&self) -> bool {
        self.failed
    }
}

impl App {
    /// The judgement for the issue the detail pane is showing.
    pub fn selected_readiness(&self) -> Option<&Readiness> {
        self.readiness.get(&self.selected_issue()?.id)
    }

    /// Record a judgement and clear its in-flight mark.
    pub fn apply_readiness(&mut self, issue_id: String, result: Result<Readiness, String>) {
        self.readiness.in_flight.remove(&issue_id);
        match result {
            Ok(readiness) => {
                self.readiness.answers.insert(issue_id, readiness);
            }
            // Deliberately silent: unlike the label-rank failure this sets no
            // status message. A missing badge is not something the user asked
            // for and cannot act on, and it must not displace a real message.
            Err(_) => self.readiness.failed = true,
        }
    }

    /// The state to send for the selected issue, marking a request in flight —
    /// or `None` when there is nothing to ask.
    ///
    /// Requires a *settled* thread. `blocked` and `duplicate` are answered from
    /// the comments, so asking before they arrive would judge the ticket on its
    /// body alone and then keep that answer.
    pub fn begin_readiness(&mut self) -> Option<(String, serde_json::Value)> {
        if self.readiness.failed {
            return None;
        }
        let issue = self.selected_issue()?;
        let id = issue.id.clone();
        if self.readiness.answers.contains_key(&id) || self.readiness.in_flight.contains(&id) {
            return None;
        }
        let comments = self.comment_cache.get(&id)?;
        let state = crate::typesafe::readiness::state(
            &issue.title,
            &issue.body,
            &comments.iter().map(|c| c.body.clone()).collect::<Vec<_>>(),
            issue.comment_count,
        );
        self.readiness.in_flight.insert(id.clone());
        Some((id, state))
    }

    /// Forget one judgement.
    pub fn invalidate_readiness(&mut self, issue_id: &str) {
        self.readiness.invalidate(issue_id);
    }
}
