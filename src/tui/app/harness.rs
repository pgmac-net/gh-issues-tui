//! Harness session state — the pure half of the feature (#23).
//!
//! Everything here is metadata: which sessions exist, what they were launched
//! for, whether their child is still running, and which one is on screen. The
//! PTY handles, reader threads and `vt100` parsers live in `tui::harness`,
//! owned by the event loop, because `app/` has no I/O.
//!
//! That split is what makes the feature testable: every transition below
//! (launch, attach, detach, exit, kill) can be driven in a unit test without
//! spawning a single process.

use super::prelude::*;

/// Identifies a session for the lifetime of the process. Monotonic, never
/// reused — an id held by an in-flight event can therefore never be
/// mistaken for a different session that took its slot.
pub type SessionId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Running,
    /// The child exited with this code (`-1` when it was killed by a signal
    /// or the code could not be read).
    Exited(i32),
}

impl SessionStatus {
    pub fn is_running(self) -> bool {
        matches!(self, SessionStatus::Running)
    }

    /// Short word for the picker and status bar.
    pub fn label(self) -> String {
        match self {
            SessionStatus::Running => "running".to_string(),
            SessionStatus::Exited(code) => format!("exited {code}"),
        }
    }
}

/// One harness session, as far as the pure layer is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMeta {
    pub id: SessionId,
    /// Canonical `owner/repo#number` the session was launched for. This is
    /// deliberately *not* `copy_format` — that template is the user's
    /// clipboard preference and may not identify an issue at all.
    pub issue_ref: String,
    /// Key into the `[harnesses.*]` config table.
    pub harness: String,
    /// The issue's title at launch time, for the identity row (#132). Not
    /// refreshed if the issue is retitled later — it names the ticket the
    /// session was started on, which is the useful thing. May be empty.
    pub title: String,
    pub status: SessionStatus,
}

/// What pressing `A` on the current row should do. Computed purely so the
/// decision is testable; the event layer performs whichever action comes back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchAction {
    /// No issue selected (a repo header row, or an empty list).
    NoIssue,
    /// A live session already exists for this issue — show it rather than
    /// starting a second agent on the same ticket.
    Attach(SessionId),
    /// A session exists but its child has exited; relaunching discards the
    /// old screen, so ask first.
    ConfirmRelaunch(SessionId),
    /// Nothing running for this issue: spawn `harness` for `issue_ref`.
    Spawn { issue_ref: String, harness: String },
    /// No `default_harness` configured — let the user pick one.
    Pick { issue_ref: String },
}

/// Sessions and which one is on screen.
///
/// Grouped for the same reason as `DetailState`/`PrState`: these fields are
/// meaningless apart and reset together. Note there is no `Default`-shaped
/// reset — sessions deliberately survive `switch_org`, since an agent working
/// on `pgmac-net/foo#1` is unaffected by the list being pointed elsewhere.
#[derive(Debug, Default)]
pub struct HarnessState {
    pub sessions: Vec<SessionMeta>,
    /// The session currently rendered, if any. `None` means the normal
    /// list/detail view is on screen.
    pub active: Option<SessionId>,
    /// `F12` seen, waiting for the chord's second key.
    pub prefix_pending: bool,
    /// Rows scrolled back in an *exited* session's frozen screen. Reset on
    /// every attach so a session always opens at its final output.
    pub scrollback: usize,
    next_id: SessionId,
}

impl HarnessState {
    /// Record a new running session and return its id. The caller is
    /// responsible for actually spawning the child under that id.
    pub fn register(&mut self, issue_ref: String, harness: String, title: String) -> SessionId {
        let id = self.next_id;
        self.next_id += 1;
        self.sessions.push(SessionMeta {
            id,
            issue_ref,
            harness,
            title,
            status: SessionStatus::Running,
        });
        id
    }

    pub fn get(&self, id: SessionId) -> Option<&SessionMeta> {
        self.sessions.iter().find(|s| s.id == id)
    }

    /// The session launched for `issue_ref`, if any. At most one exists —
    /// `LaunchAction` attaches or asks rather than starting a second.
    pub fn find_by_issue(&self, issue_ref: &str) -> Option<&SessionMeta> {
        self.sessions.iter().find(|s| s.issue_ref == issue_ref)
    }

    pub fn active_meta(&self) -> Option<&SessionMeta> {
        self.active.and_then(|id| self.get(id))
    }

    /// Mark a child as finished. Unknown ids are ignored: an exit event can
    /// land after the session was killed and removed.
    pub fn mark_exited(&mut self, id: SessionId, code: i32) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.status = SessionStatus::Exited(code);
        }
    }

    /// Drop a session. Clears `active` when it was the one on screen, so the
    /// renderer can never be left pointing at a session that no longer exists.
    pub fn remove(&mut self, id: SessionId) {
        self.sessions.retain(|s| s.id != id);
        if self.active == Some(id) {
            self.active = None;
        }
    }

    /// Show a session. Opens at the newest output — an exited session's
    /// scrollback is only interesting from the bottom.
    pub fn attach(&mut self, id: SessionId) {
        if self.get(id).is_some() {
            self.active = Some(id);
            self.scrollback = 0;
            self.prefix_pending = false;
        }
    }

    /// Return to the list. The child keeps running: detach is not kill.
    pub fn detach(&mut self) {
        self.active = None;
        self.prefix_pending = false;
        self.scrollback = 0;
    }

    pub fn running(&self) -> impl Iterator<Item = &SessionMeta> {
        self.sessions.iter().filter(|s| s.status.is_running())
    }

    pub fn running_count(&self) -> usize {
        self.running().count()
    }

    pub fn exited_count(&self) -> usize {
        self.sessions.len() - self.running_count()
    }

    pub fn has_running(&self) -> bool {
        self.sessions.iter().any(|s| s.status.is_running())
    }

    /// Rows for the session picker, in launch order, newest last.
    pub fn picker_rows(&self) -> Vec<String> {
        self.sessions
            .iter()
            .map(|s| format!("{}  [{}]  {}", s.issue_ref, s.harness, s.status.label()))
            .collect()
    }

    /// Session id behind row `idx` of `picker_rows`.
    pub fn session_at(&self, idx: usize) -> Option<SessionId> {
        self.sessions.get(idx).map(|s| s.id)
    }

    /// The `n running, m exited` status-bar segment; `None` when there are
    /// no sessions at all, so the bar is unchanged for anyone not using the
    /// feature.
    pub fn status_segment(&self) -> Option<String> {
        if self.sessions.is_empty() {
            return None;
        }
        let (running, exited) = (self.running_count(), self.exited_count());
        Some(match (running, exited) {
            (r, 0) => format!("{r} running"),
            (0, e) => format!("{e} exited"),
            (r, e) => format!("{r} running, {e} exited"),
        })
    }
}

impl App {
    /// Canonical `owner/repo#number` for the selected issue, used to key
    /// sessions and to expand the `{ref}` placeholder.
    pub fn selected_issue_ref(&self) -> Option<String> {
        let issue = self.selected_issue()?;
        let repo = self.selected_repo()?;
        Some(format!("{}/{}#{}", self.org, repo.repo, issue.number))
    }

    /// What `A` should do on the current row, given the configured default
    /// harness. Pure — the event layer executes the result.
    pub fn launch_action(&self, default_harness: Option<&str>) -> LaunchAction {
        let Some(issue_ref) = self.selected_issue_ref() else {
            return LaunchAction::NoIssue;
        };
        if let Some(existing) = self.harness.find_by_issue(&issue_ref) {
            return if existing.status.is_running() {
                LaunchAction::Attach(existing.id)
            } else {
                LaunchAction::ConfirmRelaunch(existing.id)
            };
        }
        match default_harness {
            Some(h) => LaunchAction::Spawn {
                issue_ref,
                harness: h.to_string(),
            },
            None => LaunchAction::Pick { issue_ref },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(sessions: &[(&str, &str)]) -> HarnessState {
        let mut h = HarnessState::default();
        for (issue_ref, harness) in sessions {
            h.register(
                (*issue_ref).to_string(),
                (*harness).to_string(),
                String::new(),
            );
        }
        h
    }

    #[test]
    fn register_hands_out_unique_ids_and_starts_running() {
        let h = state_with(&[("o/r#1", "claude"), ("o/r#2", "codex")]);
        assert_eq!(h.sessions[0].id, 0);
        assert_eq!(h.sessions[1].id, 1);
        assert!(h.sessions.iter().all(|s| s.status.is_running()));
    }

    #[test]
    fn ids_are_never_reused_after_removal() {
        // A stale HarnessDirty/Exited event carrying a dead id must not be
        // able to address whichever session was created next.
        let mut h = state_with(&[("o/r#1", "claude")]);
        h.remove(0);
        let id = h.register("o/r#2".into(), "claude".into(), String::new());
        assert_eq!(id, 1, "the freed id must not come back");
        assert!(h.get(0).is_none());
    }

    #[test]
    fn removing_the_active_session_clears_active() {
        let mut h = state_with(&[("o/r#1", "claude")]);
        h.attach(0);
        h.remove(0);
        assert_eq!(h.active, None, "renderer must not point at a dead session");
    }

    #[test]
    fn attaching_an_unknown_id_is_ignored() {
        let mut h = HarnessState::default();
        h.attach(99);
        assert_eq!(h.active, None);
    }

    #[test]
    fn attach_resets_scrollback_and_any_half_typed_chord() {
        let mut h = state_with(&[("o/r#1", "claude"), ("o/r#2", "claude")]);
        h.attach(0);
        h.scrollback = 12;
        h.prefix_pending = true;
        h.attach(1);
        assert_eq!(h.scrollback, 0, "a session opens at its newest output");
        assert!(!h.prefix_pending);
    }

    #[test]
    fn detach_keeps_the_session_alive() {
        let mut h = state_with(&[("o/r#1", "claude")]);
        h.attach(0);
        h.detach();
        assert_eq!(h.active, None);
        assert_eq!(h.sessions.len(), 1, "detach is not kill");
        assert!(h.sessions[0].status.is_running());
    }

    #[test]
    fn mark_exited_records_the_code_and_keeps_the_session() {
        let mut h = state_with(&[("o/r#1", "claude")]);
        h.mark_exited(0, 1);
        assert_eq!(h.sessions[0].status, SessionStatus::Exited(1));
        assert_eq!(h.exited_count(), 1);
        assert_eq!(h.running_count(), 0);
    }

    #[test]
    fn mark_exited_for_an_unknown_id_is_ignored() {
        // The child can exit just as the user kills and removes the session.
        let mut h = state_with(&[("o/r#1", "claude")]);
        h.mark_exited(42, 0);
        assert!(h.sessions[0].status.is_running());
    }

    #[test]
    fn status_segment_counts_both_kinds() {
        let mut h = state_with(&[("o/r#1", "c"), ("o/r#2", "c"), ("o/r#3", "c")]);
        assert_eq!(h.status_segment().as_deref(), Some("3 running"));
        h.mark_exited(2, 0);
        assert_eq!(h.status_segment().as_deref(), Some("2 running, 1 exited"));
        h.mark_exited(0, 0);
        h.mark_exited(1, 0);
        assert_eq!(h.status_segment().as_deref(), Some("3 exited"));
    }

    #[test]
    fn status_segment_is_absent_without_sessions() {
        assert_eq!(HarnessState::default().status_segment(), None);
    }

    #[test]
    fn picker_rows_show_ref_harness_and_state() {
        let mut h = state_with(&[("o/r#1", "claude"), ("o/r#2", "codex")]);
        h.mark_exited(1, 130);
        assert_eq!(
            h.picker_rows(),
            vec!["o/r#1  [claude]  running", "o/r#2  [codex]  exited 130"]
        );
        assert_eq!(h.session_at(1), Some(1));
        assert_eq!(h.session_at(9), None);
    }

    #[test]
    fn find_by_issue_matches_the_canonical_ref() {
        let h = state_with(&[("pgmac-net/foo#12", "claude")]);
        assert!(h.find_by_issue("pgmac-net/foo#12").is_some());
        assert!(h.find_by_issue("pgmac-net/foo#1").is_none());
    }

    // --- `A`'s decision, driven through a real App -----------------------

    use crate::provider::types::{Issue, IssueState, Label, RepoIssues};

    fn app_with_one_issue() -> App {
        let issue = Issue {
            id: "I_1".into(),
            number: 7,
            title: "t".into(),
            body: String::new(),
            state: IssueState::Open,
            url: "https://github.com/org/r/issues/7".into(),
            author: "a".into(),
            assignees: vec![],
            labels: Vec::<Label>::new(),
            comment_count: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            closed_at: None,
        };
        let mut app = App::new(
            "org".into(),
            None,
            false,
            false,
            "{owner}/{repo}#{number}".into(),
        );
        app.set_data(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![issue],
        }]);
        app.selected = 1; // 0 is the repo header
        app
    }

    #[test]
    fn the_issue_ref_is_canonical_regardless_of_copy_format() {
        // `copy_format` is a clipboard preference and may not even name an
        // issue; session keys and `{ref}` must not depend on it.
        let mut app = app_with_one_issue();
        app.copy_format = "{repo}".into();
        assert_eq!(app.selected_issue_ref().as_deref(), Some("org/r#7"));
    }

    #[test]
    fn a_repo_header_row_offers_nothing_to_launch() {
        let mut app = app_with_one_issue();
        app.selected = 0;
        assert_eq!(app.launch_action(Some("claude")), LaunchAction::NoIssue);
    }

    #[test]
    fn with_a_default_harness_a_fresh_issue_spawns() {
        let app = app_with_one_issue();
        assert_eq!(
            app.launch_action(Some("claude")),
            LaunchAction::Spawn {
                issue_ref: "org/r#7".into(),
                harness: "claude".into(),
            }
        );
    }

    #[test]
    fn without_a_default_harness_the_picker_opens() {
        let app = app_with_one_issue();
        assert_eq!(
            app.launch_action(None),
            LaunchAction::Pick {
                issue_ref: "org/r#7".into()
            }
        );
    }

    #[test]
    fn a_live_session_for_the_issue_is_attached_not_duplicated() {
        let mut app = app_with_one_issue();
        let id = app
            .harness
            .register("org/r#7".into(), "claude".into(), String::new());
        assert_eq!(
            app.launch_action(Some("claude")),
            LaunchAction::Attach(id),
            "pressing A twice must not start a second agent on one ticket"
        );
    }

    #[test]
    fn an_exited_session_asks_before_discarding_its_output() {
        let mut app = app_with_one_issue();
        let id = app
            .harness
            .register("org/r#7".into(), "claude".into(), String::new());
        app.harness.mark_exited(id, 0);
        assert_eq!(
            app.launch_action(Some("claude")),
            LaunchAction::ConfirmRelaunch(id)
        );
    }

    #[test]
    fn a_session_for_another_issue_does_not_block_this_one() {
        let mut app = app_with_one_issue();
        app.harness
            .register("org/r#99".into(), "claude".into(), String::new());
        assert!(matches!(
            app.launch_action(Some("claude")),
            LaunchAction::Spawn { .. }
        ));
    }

    #[test]
    fn sessions_survive_switching_org() {
        // An agent working a ticket is unaffected by the list being pointed
        // elsewhere — `switch_org` must not sweep the registry.
        let mut app = app_with_one_issue();
        app.harness
            .register("org/r#7".into(), "claude".into(), String::new());
        app.switch_org("other".into());
        assert_eq!(app.harness.sessions.len(), 1);
        assert!(app.harness.has_running());
    }
}
