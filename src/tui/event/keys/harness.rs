//! Keys for harness sessions and their popups (#23).
//!
//! In `Mode::Harness` the child owns the keyboard — arrows, `Esc`, `Ctrl+C`
//! and `Shift+Tab` all have to reach it, because agent CLIs bind them. The
//! TUI keeps back exactly one key, `F12`, as a tmux-style prefix; `F12 F12`
//! sends a literal `F12` through, so nothing is permanently lost.

use super::super::prelude::*;
use crate::tui::harness::{LaunchContext, keys::encode};

/// Everything the harness key handlers need that `App` deliberately cannot
/// hold: the live PTYs, the resolved config, and the channel their threads
/// report on.
pub(crate) struct HarnessCtx<'a> {
    pub registry: &'a mut HarnessRegistry,
    pub settings: &'a HarnessSettings,
    pub tx: &'a mpsc::UnboundedSender<AppEvent>,
}

impl HarnessCtx<'_> {
    /// Start `harness` for the issue `issue_ref` names, attaching on success.
    /// Every failure path lands in the status line rather than aborting.
    ///
    /// One session per issue is enforced *here* rather than in `launch_action`
    /// alone, so every entry point obeys it — `A`, the harness picker and
    /// `F12 n` all land in this function.
    pub(crate) fn launch(&mut self, app: &mut App, issue_ref: &str, harness: &str) {
        match app.harness.find_by_issue(issue_ref) {
            Some(existing) if existing.status.is_running() => {
                let id = existing.id;
                app.harness.attach(id);
                app.mode = Mode::Harness;
                app.status = Some(format!("{issue_ref} already has a session"));
                return;
            }
            // An exited session for this issue is replaced, not accumulated:
            // its screen is about to be superseded by the new run's.
            Some(existing) => {
                let id = existing.id;
                self.registry.remove(id);
                app.harness.remove(id);
            }
            None => {}
        }

        let Some(cfg) = self.settings.get(harness) else {
            app.status = Some(format!("unknown harness \"{harness}\""));
            return;
        };
        let Some(ctx) = launch_context(app) else {
            app.status = Some("no issue selected".into());
            return;
        };
        // The selection can only have produced this ref, but guard anyway —
        // a relaunch happens after a popup, and popups outlive selections.
        if ctx.issue_ref() != issue_ref {
            app.status = Some(format!("selection moved away from {issue_ref}"));
            return;
        }
        let cwd = match self.settings.workspace(harness, &ctx.owner, &ctx.repo) {
            Ok(dir) => dir,
            Err(e) => {
                app.status = Some(e);
                return;
            }
        };

        let areas = layout::harness_areas(layout::from_terminal_size());
        let id = app.harness.register(
            issue_ref.to_string(),
            harness.to_string(),
            ctx.title.clone(),
        );
        match self
            .registry
            .spawn(id, harness, cfg, &ctx, &cwd, areas.pane, self.tx)
        {
            Ok(()) => {
                app.harness.attach(id);
                app.mode = Mode::Harness;
                app.status = Some(format!("{harness} started in {}", cwd.display()));
            }
            Err(e) => {
                // Never leave a registered session with no process behind it.
                app.harness.remove(id);
                app.status = Some(e);
            }
        }
    }

    /// Kill a live session's child and drop its PTY.
    pub(crate) fn kill(&mut self, app: &mut App, id: SessionId) {
        self.registry.kill(id);
        self.registry.remove(id);
        app.harness.remove(id);
        if app.harness.active.is_none() {
            app.mode = Mode::Normal;
        }
    }
}

/// Build the placeholder context from the selected issue.
fn launch_context(app: &App) -> Option<LaunchContext> {
    let issue = app.selected_issue()?;
    let repo = app.selected_repo()?;
    Some(LaunchContext {
        owner: app.org.clone(),
        repo: repo.repo.clone(),
        number: issue.number,
        url: issue.url.clone(),
        title: issue.title.clone(),
    })
}

/// `Mode::Harness`: forward everything to the child except the `F12` chord.
pub(crate) fn handle_harness_key(app: &mut App, key: KeyEvent, hx: &mut HarnessCtx) {
    let Some(id) = app.harness.active else {
        app.mode = Mode::Normal;
        return;
    };
    let exited = app.harness.get(id).is_some_and(|s| !s.status.is_running());

    if app.harness.prefix_pending {
        app.harness.prefix_pending = false;
        handle_chord(app, key, id, hx);
        return;
    }
    if key.code == KeyCode::F(12) {
        app.harness.prefix_pending = true;
        return;
    }

    if exited {
        // Nothing is listening: the keys read the frozen screen instead.
        handle_exited_key(app, key, id, hx);
        return;
    }
    if let Some(bytes) = encode(key) {
        hx.registry.write(id, &bytes);
    }
}

/// The second key of an `F12` chord.
fn handle_chord(app: &mut App, key: KeyEvent, id: SessionId, hx: &mut HarnessCtx) {
    match key.code {
        KeyCode::Char('d') => {
            app.harness.detach();
            app.mode = Mode::Normal;
        }
        KeyCode::Char('s') => open_session_picker(app),
        KeyCode::Char('k') => {
            let running = app.harness.get(id).is_some_and(|s| s.status.is_running());
            if running {
                // Killing an agent mid-task is not undoable — confirm first.
                app.confirm_choice = ConfirmChoice::No;
                app.mode = Mode::ConfirmHarness(HarnessConfirm::Kill(id));
            } else {
                hx.kill(app, id);
                app.status = Some("session dismissed".into());
            }
        }
        KeyCode::Char('n') => open_harness_picker(app, hx.settings.names()),
        KeyCode::Char('?') => app.mode = Mode::Help,
        // F12 F12: the child gets the key the prefix ate.
        KeyCode::F(12) => {
            if let Some(bytes) = encode(key) {
                hx.registry.write(id, &bytes);
            }
        }
        _ => {
            app.status =
                Some("F12 d detach · s switch · k kill · n new · F12 F12 literal · ? help".into());
        }
    }
}

/// Keys for a session whose child has gone: scroll its frozen output, or leave.
fn handle_exited_key(app: &mut App, key: KeyEvent, id: SessionId, hx: &mut HarnessCtx) {
    // `k`/PageUp move *back* into history, so they raise the offset.
    match key.code {
        KeyCode::Char('k') | KeyCode::Up => app.harness.scrollback += 1,
        KeyCode::Char('j') | KeyCode::Down => {
            app.harness.scrollback = app.harness.scrollback.saturating_sub(1);
        }
        KeyCode::PageUp => app.harness.scrollback += 10,
        KeyCode::PageDown => app.harness.scrollback = app.harness.scrollback.saturating_sub(10),
        KeyCode::Char('G') | KeyCode::End => app.harness.scrollback = 0,
        KeyCode::Esc | KeyCode::Char('q') => {
            app.harness.detach();
            app.mode = Mode::Normal;
            return;
        }
        KeyCode::Char('x') => {
            hx.kill(app, id);
            app.status = Some("session dismissed".into());
            return;
        }
        _ => return,
    }
    hx.registry.set_scrollback(id, app.harness.scrollback);
}

/// Open the picker listing configured harnesses.
pub(crate) fn open_harness_picker(app: &mut App, names: Vec<String>) {
    if names.is_empty() {
        app.status = Some("no harnesses configured".into());
        return;
    }
    app.picker.start(names, 0);
    app.mode = Mode::HarnessPicker;
}

/// Open the picker listing existing sessions.
pub(crate) fn open_session_picker(app: &mut App) {
    let rows = app.harness.picker_rows();
    if rows.is_empty() {
        app.status = Some("no harness sessions".into());
        return;
    }
    app.picker.start(rows, 0);
    app.mode = Mode::SessionPicker;
}

/// `Mode::HarnessPicker`: choose which harness to start for the selected issue.
pub(crate) fn handle_harness_picker_key(app: &mut App, key: KeyEvent, hx: &mut HarnessCtx) {
    if picker_nav(app, key) {
        return;
    }
    if key.code == KeyCode::Enter {
        let chosen = app
            .picker
            .selected_original()
            .and_then(|i| app.picker.options.get(i).cloned());
        app.mode = Mode::Normal;
        if let (Some(harness), Some(issue_ref)) = (chosen, app.selected_issue_ref()) {
            hx.launch(app, &issue_ref, &harness);
        }
    }
}

/// `Mode::SessionPicker`: attach to an existing session.
pub(crate) fn handle_session_picker_key(app: &mut App, key: KeyEvent) {
    if picker_nav(app, key) {
        return;
    }
    if key.code == KeyCode::Enter {
        let id = app
            .picker
            .selected_original()
            .and_then(|i| app.harness.session_at(i));
        match id {
            Some(id) => {
                app.harness.attach(id);
                app.mode = Mode::Harness;
            }
            None => app.mode = Mode::Normal,
        }
    }
}

/// Movement, type-ahead and cancel, shared by both harness pickers. Returns
/// `true` when the key was consumed.
fn picker_nav(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => {
            // Cancelling from a session leaves you back in it, not on the list.
            app.mode = if app.harness.active.is_some() {
                Mode::Harness
            } else {
                Mode::Normal
            };
            true
        }
        KeyCode::Down => {
            let len = app.picker.filtered().len();
            if app.picker.idx + 1 < len {
                app.picker.idx += 1;
            }
            true
        }
        KeyCode::Up => {
            app.picker.idx = app.picker.idx.saturating_sub(1);
            true
        }
        KeyCode::Backspace => {
            app.picker.filter_backspace();
            true
        }
        KeyCode::Char(c) => {
            app.picker.filter_push(c);
            true
        }
        _ => false,
    }
}

/// `Mode::ConfirmHarness`: the Yes/No popup in front of an irreversible
/// harness action.
pub(crate) fn handle_confirm_harness_key(
    app: &mut App,
    key: KeyEvent,
    what: HarnessConfirm,
    hx: &mut HarnessCtx,
) {
    let confirmed = match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => true,
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => false,
        KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
            app.confirm_choice = match app.confirm_choice {
                ConfirmChoice::Yes => ConfirmChoice::No,
                ConfirmChoice::No => ConfirmChoice::Yes,
            };
            return;
        }
        KeyCode::Enter => app.confirm_choice == ConfirmChoice::Yes,
        _ => return,
    };

    // Where the popup returns to depends on what it was covering.
    app.mode = if app.harness.active.is_some() {
        Mode::Harness
    } else {
        Mode::Normal
    };
    if !confirmed {
        return;
    }
    match what {
        HarnessConfirm::Kill(id) => {
            hx.kill(app, id);
            app.status = Some("session terminated".into());
        }
        HarnessConfirm::Relaunch(id) => {
            let previous = app
                .harness
                .get(id)
                .map(|s| (s.issue_ref.clone(), s.harness.clone()));
            hx.kill(app, id);
            if let Some((issue_ref, harness)) = previous {
                hx.launch(app, &issue_ref, &harness);
            }
        }
        HarnessConfirm::Quit => {
            hx.registry.kill_all();
            app.should_quit = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::{app_with_issue, key};
    use super::*;
    use crate::config::builtin_harnesses;
    use crate::tui::harness::HarnessRegistry;

    struct Harness {
        registry: HarnessRegistry,
        settings: HarnessSettings,
        tx: mpsc::UnboundedSender<AppEvent>,
        _rx: mpsc::UnboundedReceiver<AppEvent>,
    }

    fn fixture(default_harness: Option<&str>) -> Harness {
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();
        Harness {
            registry: HarnessRegistry::default(),
            settings: HarnessSettings {
                default_harness: default_harness.map(str::to_string),
                harnesses: builtin_harnesses(),
                workspace_roots: Vec::new(),
                cwd_repo: None,
                cwd: std::path::PathBuf::from("/"),
            },
            tx,
            _rx,
        }
    }

    impl Harness {
        fn ctx(&mut self) -> HarnessCtx<'_> {
            HarnessCtx {
                registry: &mut self.registry,
                settings: &self.settings,
                tx: &self.tx,
            }
        }
    }

    /// An app sitting in an attached, running session. No process is spawned:
    /// the registry has no PTY for it, which every path here tolerates.
    fn attached_app() -> App {
        let (mut app, _) = app_with_issue(&[]);
        let id = app
            .harness
            .register("org/r#1".into(), "claude".into(), String::new());
        app.harness.attach(id);
        app.mode = Mode::Harness;
        app
    }

    #[test]
    fn f12_is_swallowed_and_arms_the_chord() {
        let mut app = attached_app();
        let mut h = fixture(None);
        handle_harness_key(&mut app, key(KeyCode::F(12)), &mut h.ctx());
        assert!(app.harness.prefix_pending, "F12 must not reach the child");
        assert_eq!(app.mode, Mode::Harness);
    }

    #[test]
    fn f12_d_detaches_without_killing() {
        let mut app = attached_app();
        let mut h = fixture(None);
        handle_harness_key(&mut app, key(KeyCode::F(12)), &mut h.ctx());
        handle_harness_key(&mut app, key(KeyCode::Char('d')), &mut h.ctx());
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.harness.active, None);
        assert_eq!(app.harness.sessions.len(), 1, "the child keeps running");
        assert!(!app.harness.prefix_pending);
    }

    #[test]
    fn a_bare_d_is_forwarded_not_treated_as_detach() {
        // The whole point of the prefix: unprefixed keys belong to the child.
        let mut app = attached_app();
        let mut h = fixture(None);
        handle_harness_key(&mut app, key(KeyCode::Char('d')), &mut h.ctx());
        assert_eq!(app.mode, Mode::Harness, "still attached");
        assert!(app.harness.active.is_some());
    }

    #[test]
    fn the_chord_disarms_after_one_key_whatever_it_was() {
        let mut app = attached_app();
        let mut h = fixture(None);
        handle_harness_key(&mut app, key(KeyCode::F(12)), &mut h.ctx());
        handle_harness_key(&mut app, key(KeyCode::Char('%')), &mut h.ctx());
        assert!(
            !app.harness.prefix_pending,
            "an unknown chord key resets it"
        );
        assert_eq!(app.mode, Mode::Harness);
    }

    #[test]
    fn f12_k_on_a_running_session_asks_first() {
        let mut app = attached_app();
        let mut h = fixture(None);
        handle_harness_key(&mut app, key(KeyCode::F(12)), &mut h.ctx());
        handle_harness_key(&mut app, key(KeyCode::Char('k')), &mut h.ctx());
        assert_eq!(app.mode, Mode::ConfirmHarness(HarnessConfirm::Kill(0)));
        assert_eq!(app.confirm_choice, ConfirmChoice::No, "safe default");
        assert_eq!(app.harness.sessions.len(), 1, "nothing killed yet");
    }

    #[test]
    fn f12_k_on_an_exited_session_dismisses_it_outright() {
        let mut app = attached_app();
        app.harness.mark_exited(0, 0);
        let mut h = fixture(None);
        handle_harness_key(&mut app, key(KeyCode::F(12)), &mut h.ctx());
        handle_harness_key(&mut app, key(KeyCode::Char('k')), &mut h.ctx());
        assert!(app.harness.sessions.is_empty(), "no confirm for a dead one");
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn f12_s_opens_the_session_picker() {
        let mut app = attached_app();
        let mut h = fixture(None);
        handle_harness_key(&mut app, key(KeyCode::F(12)), &mut h.ctx());
        handle_harness_key(&mut app, key(KeyCode::Char('s')), &mut h.ctx());
        assert_eq!(app.mode, Mode::SessionPicker);
        assert_eq!(app.picker.options.len(), 1);
    }

    #[test]
    fn f12_n_offers_the_configured_harnesses() {
        let mut app = attached_app();
        let mut h = fixture(None);
        handle_harness_key(&mut app, key(KeyCode::F(12)), &mut h.ctx());
        handle_harness_key(&mut app, key(KeyCode::Char('n')), &mut h.ctx());
        assert_eq!(app.mode, Mode::HarnessPicker);
        assert_eq!(app.picker.options, vec!["claude", "opencode"]);
    }

    #[test]
    fn an_exited_session_scrolls_instead_of_forwarding() {
        let mut app = attached_app();
        app.harness.mark_exited(0, 0);
        let mut h = fixture(None);
        handle_harness_key(&mut app, key(KeyCode::Char('k')), &mut h.ctx());
        handle_harness_key(&mut app, key(KeyCode::Char('k')), &mut h.ctx());
        assert_eq!(app.harness.scrollback, 2, "k walks back into history");
        handle_harness_key(&mut app, key(KeyCode::Char('j')), &mut h.ctx());
        assert_eq!(app.harness.scrollback, 1);
        handle_harness_key(&mut app, key(KeyCode::Char('G')), &mut h.ctx());
        assert_eq!(app.harness.scrollback, 0);
    }

    #[test]
    fn q_leaves_an_exited_session_without_dismissing_it() {
        let mut app = attached_app();
        app.harness.mark_exited(0, 0);
        let mut h = fixture(None);
        handle_harness_key(&mut app, key(KeyCode::Char('q')), &mut h.ctx());
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.harness.sessions.len(), 1, "output still readable via Z");
    }

    #[test]
    fn cancelling_a_picker_from_a_session_returns_to_it() {
        let mut app = attached_app();
        let mut h = fixture(None);
        handle_harness_key(&mut app, key(KeyCode::F(12)), &mut h.ctx());
        handle_harness_key(&mut app, key(KeyCode::Char('s')), &mut h.ctx());
        handle_session_picker_key(&mut app, key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Harness, "not back to the issue list");
    }

    #[test]
    fn the_session_picker_attaches_the_row_it_shows() {
        let (mut app, _) = app_with_issue(&[]);
        app.harness
            .register("org/r#1".into(), "claude".into(), String::new());
        let second = app
            .harness
            .register("org/r#2".into(), "codex".into(), String::new());
        open_session_picker(&mut app);
        app.picker.idx = 1;
        handle_session_picker_key(&mut app, key(KeyCode::Enter));
        assert_eq!(app.harness.active, Some(second));
        assert_eq!(app.mode, Mode::Harness);
    }

    #[test]
    fn the_session_picker_maps_back_through_its_type_ahead_filter() {
        // The picker's index is positional within the *filtered* view; this
        // is the bug class `picker_selected_original` exists to prevent.
        let (mut app, _) = app_with_issue(&[]);
        app.harness
            .register("org/r#1".into(), "claude".into(), String::new());
        let second = app
            .harness
            .register("org/r#2".into(), "codex".into(), String::new());
        open_session_picker(&mut app);
        for c in "codex".chars() {
            handle_session_picker_key(&mut app, key(KeyCode::Char(c)));
        }
        assert_eq!(app.picker.filtered().len(), 1);
        handle_session_picker_key(&mut app, key(KeyCode::Enter));
        assert_eq!(app.harness.active, Some(second));
    }

    #[test]
    fn confirming_a_kill_removes_the_session() {
        let mut app = attached_app();
        let mut h = fixture(None);
        app.confirm_choice = ConfirmChoice::Yes;
        handle_confirm_harness_key(
            &mut app,
            key(KeyCode::Enter),
            HarnessConfirm::Kill(0),
            &mut h.ctx(),
        );
        assert!(app.harness.sessions.is_empty());
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn declining_a_kill_leaves_the_session_alone() {
        let mut app = attached_app();
        let mut h = fixture(None);
        handle_confirm_harness_key(
            &mut app,
            key(KeyCode::Char('n')),
            HarnessConfirm::Kill(0),
            &mut h.ctx(),
        );
        assert_eq!(app.harness.sessions.len(), 1);
        assert_eq!(app.mode, Mode::Harness, "back into the session");
    }

    #[test]
    fn declining_the_quit_confirmation_does_not_quit() {
        let (mut app, _) = app_with_issue(&[]);
        app.harness
            .register("org/r#1".into(), "claude".into(), String::new());
        let mut h = fixture(None);
        handle_confirm_harness_key(
            &mut app,
            key(KeyCode::Esc),
            HarnessConfirm::Quit,
            &mut h.ctx(),
        );
        assert!(!app.should_quit);
        assert!(app.harness.has_running());
    }

    #[test]
    fn confirming_the_quit_confirmation_quits() {
        let (mut app, _) = app_with_issue(&[]);
        app.harness
            .register("org/r#1".into(), "claude".into(), String::new());
        let mut h = fixture(None);
        handle_confirm_harness_key(
            &mut app,
            key(KeyCode::Char('y')),
            HarnessConfirm::Quit,
            &mut h.ctx(),
        );
        assert!(app.should_quit);
    }

    #[test]
    fn launch_attaches_rather_than_duplicating_a_live_session() {
        // `F12 n` and the harness picker reach `launch` directly, bypassing
        // `launch_action` — the one-session-per-issue rule must hold anyway.
        let (mut app, _) = app_with_issue(&[]);
        let id = app
            .harness
            .register("org/r#1".into(), "claude".into(), String::new());
        let mut h = fixture(Some("claude"));
        h.ctx().launch(&mut app, "org/r#1", "opencode");
        assert_eq!(
            app.harness.sessions.len(),
            1,
            "no second agent on one ticket"
        );
        assert_eq!(app.harness.active, Some(id));
        assert_eq!(app.mode, Mode::Harness);
    }

    #[test]
    fn launch_replaces_an_exited_session_for_the_same_issue() {
        let (mut app, _) = app_with_issue(&[]);
        let id = app
            .harness
            .register("org/r#1".into(), "claude".into(), String::new());
        app.harness.mark_exited(id, 0);
        let mut h = fixture(Some("claude"));
        // The spawn itself fails (no clone configured), but the stale exited
        // session must already be gone — they must not accumulate.
        h.ctx().launch(&mut app, "org/r#1", "claude");
        assert!(app.harness.sessions.is_empty());
    }

    #[test]
    fn launching_an_unknown_harness_reports_it_and_registers_nothing() {
        let (mut app, _) = app_with_issue(&[]);
        let mut h = fixture(None);
        h.ctx().launch(&mut app, "org/r#1", "nope");
        assert!(app.harness.sessions.is_empty());
        assert!(app.status.as_deref().unwrap().contains("unknown harness"));
    }

    #[test]
    fn a_missing_clone_is_reported_and_registers_nothing() {
        let (mut app, _) = app_with_issue(&[]);
        let mut h = fixture(Some("claude"));
        h.ctx().launch(&mut app, "org/r#1", "claude");
        assert!(
            app.harness.sessions.is_empty(),
            "a failed launch must not leave a session with no process"
        );
        assert!(
            app.status.as_deref().unwrap().contains("no clone of org/r"),
            "got {:?}",
            app.status
        );
    }

    // --- the normal-mode entry points ------------------------------------

    use super::super::normal::handle_normal_key;
    use super::super::testutil::test_client;

    #[test]
    fn q_with_a_running_session_confirms_instead_of_quitting() {
        let (mut app, _) = app_with_issue(&[]);
        app.harness
            .register("org/r#1".into(), "claude".into(), String::new());
        let mut h = fixture(None);
        let client = test_client();
        let tx = h.tx.clone();
        handle_normal_key(
            &mut app,
            key(KeyCode::Char('q')),
            &client,
            &tx,
            &mut h.ctx(),
        );
        assert!(!app.should_quit, "a stray q must not kill an agent");
        assert_eq!(app.mode, Mode::ConfirmHarness(HarnessConfirm::Quit));
    }

    #[test]
    fn q_with_only_exited_sessions_quits_straight_away() {
        let (mut app, _) = app_with_issue(&[]);
        let id = app
            .harness
            .register("org/r#1".into(), "claude".into(), String::new());
        app.harness.mark_exited(id, 0);
        let mut h = fixture(None);
        let client = test_client();
        let tx = h.tx.clone();
        handle_normal_key(
            &mut app,
            key(KeyCode::Char('q')),
            &client,
            &tx,
            &mut h.ctx(),
        );
        assert!(app.should_quit, "nothing is running, no reason to ask");
    }

    #[test]
    fn capital_a_attaches_to_this_issue_s_live_session() {
        let (mut app, _) = app_with_issue(&[]);
        let id = app
            .harness
            .register("org/r#1".into(), "claude".into(), String::new());
        let mut h = fixture(Some("claude"));
        let client = test_client();
        let tx = h.tx.clone();
        handle_normal_key(
            &mut app,
            key(KeyCode::Char('A')),
            &client,
            &tx,
            &mut h.ctx(),
        );
        assert_eq!(app.harness.active, Some(id));
        assert_eq!(app.mode, Mode::Harness);
        assert_eq!(app.harness.sessions.len(), 1);
    }

    #[test]
    fn capital_a_without_a_default_harness_opens_the_picker() {
        let (mut app, _) = app_with_issue(&[]);
        let mut h = fixture(None);
        let client = test_client();
        let tx = h.tx.clone();
        handle_normal_key(
            &mut app,
            key(KeyCode::Char('A')),
            &client,
            &tx,
            &mut h.ctx(),
        );
        assert_eq!(app.mode, Mode::HarnessPicker);
    }

    #[test]
    fn capital_a_on_a_repo_header_says_so_rather_than_launching() {
        let (mut app, _) = app_with_issue(&[]);
        app.selected = 0;
        let mut h = fixture(Some("claude"));
        let client = test_client();
        let tx = h.tx.clone();
        handle_normal_key(
            &mut app,
            key(KeyCode::Char('A')),
            &client,
            &tx,
            &mut h.ctx(),
        );
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.status.as_deref().unwrap().contains("select an issue"));
    }

    #[test]
    fn capital_z_opens_the_session_picker_and_says_so_when_empty() {
        let (mut app, _) = app_with_issue(&[]);
        let mut h = fixture(None);
        let client = test_client();
        let tx = h.tx.clone();
        handle_normal_key(
            &mut app,
            key(KeyCode::Char('Z')),
            &client,
            &tx,
            &mut h.ctx(),
        );
        assert_eq!(app.mode, Mode::Normal);
        assert!(
            app.status
                .as_deref()
                .unwrap()
                .contains("no harness sessions")
        );

        app.harness
            .register("org/r#1".into(), "claude".into(), String::new());
        handle_normal_key(
            &mut app,
            key(KeyCode::Char('Z')),
            &client,
            &tx,
            &mut h.ctx(),
        );
        assert_eq!(app.mode, Mode::SessionPicker);
    }

    #[test]
    fn a_launch_for_a_ref_the_selection_moved_off_is_refused() {
        let (mut app, _) = app_with_issue(&[]);
        let mut h = fixture(Some("claude"));
        h.ctx().launch(&mut app, "org/r#999", "claude");
        assert!(app.harness.sessions.is_empty());
        assert!(app.status.as_deref().unwrap().contains("selection moved"));
    }
}
