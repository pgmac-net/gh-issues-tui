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
    /// The id of the underlying `claude --bg` session, for a harness with
    /// `bg_dispatch` configured. `None` for a direct-exec harness (e.g.
    /// `opencode`), where the PTY child *is* the session.
    ///
    /// Set right after a fresh launch's dispatch step resolves it, or at
    /// startup by `reconcile` adopting a session from a previous run. Either
    /// way, this can be `Some` while the event loop's own `HarnessRegistry`
    /// has no live PTY for `id` yet — a reconciled session is metadata only
    /// until it is actually attached.
    pub bg_id: Option<String>,
    /// This session existed before the current run — `reconcile` adopted it
    /// from `claude agents`, matching only on its name being issue-ref
    /// shaped. It was not necessarily started by gh-issues-tui at all: any
    /// `--bg` session named `owner/repo#number` qualifies. Sticky — attaching
    /// to one does not make it ours (#148).
    pub adopted: bool,
}

/// The running sessions quitting will end (`terminated`) versus leave running
/// under `claude`'s supervisor (`survives`) — see `HarnessState::quit_summary`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuitSummary {
    pub terminated: Vec<SessionMeta>,
    pub survives: Vec<SessionMeta>,
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

/// Loose shape check for `owner/repo#number`, used to filter `reconcile`'s
/// input down to sessions this tool plausibly dispatched — not a full
/// validation, since a false positive here just means adopting a session
/// that then fails to attach cleanly, and a false negative silently drops
/// one that should have been adopted.
fn is_issue_ref(name: &str) -> bool {
    let Some((owner_repo, number)) = name.split_once('#') else {
        return false;
    };
    owner_repo.contains('/') && !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
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
            bg_id: None,
            adopted: false,
        });
        id
    }

    pub fn get(&self, id: SessionId) -> Option<&SessionMeta> {
        self.sessions.iter().find(|s| s.id == id)
    }

    /// Record the background session id a fresh dispatch resolved to. A
    /// no-op for an unknown id: the caller may have already removed the
    /// session on a spawn failure elsewhere in the same call.
    pub fn set_bg_id(&mut self, id: SessionId, bg_id: String) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.bg_id = Some(bg_id);
        }
    }

    /// Adopt background sessions dispatched by `harness` that are still
    /// running from a previous launch and aren't already tracked here —
    /// what lets a session outlive `gh-issues-tui` quitting.
    ///
    /// `bg_sessions` is `(bg_id, name)` pairs, already fetched by the caller
    /// (`harness::list_bg_sessions`) so this stays pure over already-I/O'd
    /// data. A name is adopted only when it parses as `owner/repo#number` —
    /// an unrelated `--bg` session sharing the same `claude agents` listing
    /// must not be mistaken for one of this tool's.
    ///
    /// Registered with metadata only, `status: Running` and no PTY — the
    /// event loop's `HarnessRegistry` only gets one the first time the
    /// session is actually attached (see `HarnessCtx::launch`).
    pub fn reconcile(&mut self, harness: &str, bg_sessions: &[(String, String)]) {
        for (bg_id, name) in bg_sessions {
            if !is_issue_ref(name) || self.find_by_issue(name).is_some() {
                continue;
            }
            let id = self.register(name.clone(), harness.to_string(), String::new());
            self.set_bg_id(id, bg_id.clone());
            if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
                s.adopted = true;
            }
        }
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

    /// Split the running sessions by what quitting actually does to them:
    /// a direct-exec harness's child dies with its PTY, while a
    /// `bg_dispatch` session (`bg_id: Some`) keeps running under `claude`'s
    /// own supervisor regardless — `HarnessRegistry::kill_all` only ever
    /// kills the local viewer for those (#148). Used to make the quit
    /// confirmation say what will really happen instead of claiming
    /// everything dies.
    pub fn quit_summary(&self) -> QuitSummary {
        let mut terminated = Vec::new();
        let mut survives = Vec::new();
        for s in self.running() {
            if s.bg_id.is_some() {
                survives.push(s.clone());
            } else {
                terminated.push(s.clone());
            }
        }
        QuitSummary {
            terminated,
            survives,
        }
    }

    /// Rows for the session picker, in launch order, newest last. An adopted
    /// session is marked with a leading `↗`; every row keeps the same prefix
    /// width so the columns stay aligned whether or not any row is adopted.
    pub fn picker_rows(&self) -> Vec<String> {
        self.sessions
            .iter()
            .map(|s| {
                let mark = if s.adopted { "\u{2197}" } else { " " };
                format!(
                    "{mark} {}  [{}]  {}",
                    s.issue_ref,
                    s.harness,
                    s.status.label()
                )
            })
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
    fn quit_summary_all_local_puts_everything_in_terminated() {
        let h = state_with(&[("o/r#1", "opencode"), ("o/r#2", "opencode")]);
        let s = h.quit_summary();
        assert_eq!(s.terminated.len(), 2);
        assert!(s.survives.is_empty());
    }

    #[test]
    fn quit_summary_all_background_puts_everything_in_survives() {
        let mut h = HarnessState::default();
        h.reconcile(
            "claude",
            &[("1".into(), "o/r#1".into()), ("2".into(), "o/r#2".into())],
        );
        let s = h.quit_summary();
        assert!(s.terminated.is_empty());
        assert_eq!(s.survives.len(), 2);
    }

    #[test]
    fn quit_summary_splits_a_mixed_set() {
        let mut h = state_with(&[("o/r#1", "opencode")]);
        h.reconcile("claude", &[("1".into(), "o/r#2".into())]);
        let s = h.quit_summary();
        assert_eq!(s.terminated.len(), 1);
        assert_eq!(s.terminated[0].issue_ref, "o/r#1");
        assert_eq!(s.survives.len(), 1);
        assert_eq!(s.survives[0].issue_ref, "o/r#2");
    }

    #[test]
    fn quit_summary_excludes_exited_sessions() {
        let mut h = state_with(&[("o/r#1", "opencode")]);
        h.mark_exited(0, 0);
        let s = h.quit_summary();
        assert!(s.terminated.is_empty());
        assert!(s.survives.is_empty());
    }

    #[test]
    fn picker_rows_show_ref_harness_and_state() {
        let mut h = state_with(&[("o/r#1", "claude"), ("o/r#2", "codex")]);
        h.mark_exited(1, 130);
        assert_eq!(
            h.picker_rows(),
            vec!["  o/r#1  [claude]  running", "  o/r#2  [codex]  exited 130"]
        );
        assert_eq!(h.session_at(1), Some(1));
        assert_eq!(h.session_at(9), None);
    }

    #[test]
    fn picker_rows_mark_adopted_sessions_and_keep_columns_aligned() {
        let mut h = HarnessState::default();
        h.reconcile("claude", &[("42".into(), "o/r#1".into())]);
        h.register("o/r#2".into(), "claude".into(), String::new());
        assert_eq!(
            h.picker_rows(),
            vec![
                "\u{2197} o/r#1  [claude]  running",
                "  o/r#2  [claude]  running"
            ]
        );
    }

    #[test]
    fn find_by_issue_matches_the_canonical_ref() {
        let h = state_with(&[("pgmac-net/foo#12", "claude")]);
        assert!(h.find_by_issue("pgmac-net/foo#12").is_some());
        assert!(h.find_by_issue("pgmac-net/foo#1").is_none());
    }

    #[test]
    fn a_fresh_session_has_no_bg_id() {
        let h = state_with(&[("o/r#1", "claude")]);
        assert_eq!(h.sessions[0].bg_id, None);
    }

    #[test]
    fn set_bg_id_records_it() {
        let mut h = state_with(&[("o/r#1", "claude")]);
        h.set_bg_id(0, "42".into());
        assert_eq!(h.sessions[0].bg_id.as_deref(), Some("42"));
    }

    #[test]
    fn set_bg_id_on_an_unknown_id_is_ignored() {
        let mut h = HarnessState::default();
        h.set_bg_id(99, "42".into()); // must not panic
        assert!(h.sessions.is_empty());
    }

    #[test]
    fn reconcile_adopts_a_matching_untracked_session() {
        let mut h = HarnessState::default();
        h.reconcile("claude", &[("42".into(), "pgmac-net/foo#12".into())]);
        assert_eq!(h.sessions.len(), 1);
        assert_eq!(h.sessions[0].issue_ref, "pgmac-net/foo#12");
        assert_eq!(h.sessions[0].harness, "claude");
        assert_eq!(h.sessions[0].bg_id.as_deref(), Some("42"));
        assert!(h.sessions[0].status.is_running());
        assert!(h.sessions[0].adopted, "reconcile must mark it adopted");
    }

    #[test]
    fn a_freshly_registered_session_is_not_adopted() {
        let h = state_with(&[("o/r#1", "claude")]);
        assert!(!h.sessions[0].adopted);
    }

    #[test]
    fn reconcile_skips_an_issue_already_tracked() {
        let mut h = state_with(&[("pgmac-net/foo#12", "claude")]);
        h.reconcile("claude", &[("42".into(), "pgmac-net/foo#12".into())]);
        assert_eq!(h.sessions.len(), 1, "must not duplicate a session");
        assert_eq!(
            h.sessions[0].bg_id, None,
            "the already-tracked session's own metadata must not be touched"
        );
        assert!(
            !h.sessions[0].adopted,
            "the already-tracked session must not become adopted"
        );
    }

    #[test]
    fn reconcile_ignores_a_name_that_is_not_an_issue_ref() {
        // An unrelated `--bg` session (some other tool, or a stray manual
        // one) sharing the same `claude agents` listing must not be adopted.
        let mut h = HarnessState::default();
        h.reconcile(
            "claude",
            &[
                ("1".into(), "my-other-task".into()),
                ("2".into(), "no-hash-here/repo".into()),
                ("3".into(), "owner/repo#not-a-number".into()),
            ],
        );
        assert!(h.sessions.is_empty());
    }

    #[test]
    fn reconcile_adopts_only_the_new_matches_among_a_mix() {
        let mut h = state_with(&[("pgmac-net/foo#1", "claude")]);
        h.reconcile(
            "claude",
            &[
                ("1".into(), "pgmac-net/foo#1".into()), // already tracked
                ("2".into(), "pgmac-net/foo#2".into()), // new
                ("3".into(), "junk".into()),            // not an issue ref
            ],
        );
        assert_eq!(h.sessions.len(), 2);
        assert!(h.find_by_issue("pgmac-net/foo#2").is_some());
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
