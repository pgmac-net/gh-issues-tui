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
                let bg_id = existing.bg_id.clone();
                // A session adopted from a previous run (`reconcile`) has
                // metadata but no PTY in *this* process's registry yet —
                // open the `claude attach` viewer onto it now, lazily.
                if !self.registry.has_pty(id)
                    && let Some(bg_id) = bg_id
                    && let Err(e) = self.attach_reconciled(app, id, &bg_id)
                {
                    app.harness.remove(id);
                    app.status = Some(e);
                    return;
                }
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
            Ok(finished_fast) => {
                if let Some(bg_id) = self.registry.bg_id(id) {
                    app.harness.set_bg_id(id, bg_id.to_string());
                }
                app.harness.attach(id);
                app.mode = Mode::Harness;
                app.status = Some(if finished_fast {
                    format!(
                        "{harness} started in {} — already finished; check the pane",
                        cwd.display()
                    )
                } else {
                    format!("{harness} started in {}", cwd.display())
                });
            }
            Err(e) => {
                // Never leave a registered session with no process behind it.
                app.harness.remove(id);
                app.status = Some(e);
            }
        }
    }

    /// Open a PTY viewer onto a session `reconcile` adopted from a previous
    /// run, resolving its workspace from `issue_ref` alone — a reconciled
    /// session has no cached `LaunchContext`, only its metadata.
    fn attach_reconciled(&mut self, app: &App, id: SessionId, bg_id: &str) -> Result<(), String> {
        let meta = app
            .harness
            .get(id)
            .ok_or_else(|| "session vanished".to_string())?;
        let harness = meta.harness.clone();
        let (owner, repo) = parse_owner_repo(&meta.issue_ref)
            .ok_or_else(|| format!("malformed issue ref {}", meta.issue_ref))?;
        let cfg = self
            .settings
            .get(&harness)
            .ok_or_else(|| format!("unknown harness \"{harness}\""))?;
        let cwd = self.settings.workspace(&harness, owner, repo)?;
        let areas = layout::harness_areas(layout::from_terminal_size());
        self.registry
            .attach_bg(id, bg_id, &cfg.command, &cwd, areas.pane, self.tx)
    }

    /// Kill a live session's child and drop its PTY.
    ///
    /// A reconciled session that was never attached this run has no
    /// `LiveSession` for `registry.kill` to find — `claude stop` is sent
    /// directly by id in that case, or the underlying background session
    /// would outlive being dismissed from the picker.
    pub(crate) fn kill(&mut self, app: &mut App, id: SessionId) {
        if !self.registry.has_pty(id)
            && let Some(bg_id) = app.harness.get(id).and_then(|m| m.bg_id.clone())
        {
            crate::tui::harness::stop_bg(&bg_id);
        }
        self.registry.kill(id);
        self.registry.remove(id);
        app.harness.remove(id);
        if app.harness.active.is_none() {
            app.mode = Mode::Normal;
        }
    }
}

/// Split `owner/repo#number` back into `(owner, repo)` — the inverse of
/// `App::selected_issue_ref`'s format, used by `attach_reconciled` to
/// resolve a workspace without a `LaunchContext`.
fn parse_owner_repo(issue_ref: &str) -> Option<(&str, &str)> {
    let (owner_repo, _number) = issue_ref.split_once('#')?;
    owner_repo.split_once('/')
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
        assert_eq!(
            app.picker.options,
            vec!["claude", "codex", "copilot", "opencode", "pi"]
        );
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

    // --- reconciled sessions (adopted from `claude agents` at startup) ---

    #[test]
    fn a_reconciled_session_opens_a_pty_lazily_on_first_attach() {
        // Mirrors what `reconcile` produces: metadata with a bg_id,
        // registered directly rather than through `spawn`, so the registry
        // starts with no `LiveSession` for it — the first `A`/attach must
        // open one, against `claude attach {bg_id}` (a real `sh` stand-in
        // here), not assume one already exists.
        let tmp = tempfile::tempdir().unwrap();
        let (mut app, _) = app_with_issue(&[]);
        let id = app
            .harness
            .register("org/r#1".into(), "probe".into(), String::new());
        app.harness.set_bg_id(id, "42".into());

        let mut harnesses = std::collections::HashMap::new();
        harnesses.insert(
            "probe".to_string(),
            crate::config::HarnessConfig {
                command: vec!["sh".into(), "-c".into(), "echo bg-{bg_id}".into()],
                workspace_roots: None,
                bg_dispatch: None,
            },
        );
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();
        let mut h = Harness {
            registry: HarnessRegistry::default(),
            settings: HarnessSettings {
                default_harness: None,
                harnesses,
                workspace_roots: Vec::new(),
                cwd_repo: Some(("org".into(), "r".into())),
                cwd: tmp.path().to_path_buf(),
            },
            tx,
            _rx,
        };

        h.ctx().launch(&mut app, "org/r#1", "probe");

        assert!(
            h.registry.has_pty(id),
            "the first attach must open a real pty for a reconciled session"
        );
        assert_eq!(app.harness.active, Some(id));
        assert_eq!(app.mode, Mode::Harness);
    }

    #[test]
    fn a_second_attach_does_not_reopen_the_pty() {
        // Once attached in this process, a reconciled session behaves like
        // any other running one — `has_pty` short-circuits `attach_reconciled`.
        let tmp = tempfile::tempdir().unwrap();
        let (mut app, _) = app_with_issue(&[]);
        let id = app
            .harness
            .register("org/r#1".into(), "probe".into(), String::new());
        app.harness.set_bg_id(id, "42".into());

        let mut harnesses = std::collections::HashMap::new();
        harnesses.insert(
            "probe".to_string(),
            crate::config::HarnessConfig {
                command: vec!["sh".into(), "-c".into(), "sleep 5".into()],
                workspace_roots: None,
                bg_dispatch: None,
            },
        );
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();
        let mut h = Harness {
            registry: HarnessRegistry::default(),
            settings: HarnessSettings {
                default_harness: None,
                harnesses,
                workspace_roots: Vec::new(),
                cwd_repo: Some(("org".into(), "r".into())),
                cwd: tmp.path().to_path_buf(),
            },
            tx,
            _rx,
        };

        h.ctx().launch(&mut app, "org/r#1", "probe");
        app.harness.detach();
        h.ctx().launch(&mut app, "org/r#1", "probe");

        assert_eq!(app.harness.active, Some(id));
        assert_eq!(app.status.as_deref(), Some("org/r#1 already has a session"));
    }

    #[test]
    fn killing_a_never_attached_reconciled_session_still_removes_it() {
        // `registry.kill` alone is a no-op with no `LiveSession` for `id` —
        // this pins that `HarnessCtx::kill` falls back to stopping the
        // background session directly rather than silently doing nothing.
        let (mut app, _) = app_with_issue(&[]);
        let id = app
            .harness
            .register("org/r#1".into(), "claude".into(), String::new());
        app.harness.set_bg_id(id, "not-a-real-session".into());
        let mut h = fixture(None);

        h.ctx().kill(&mut app, id);

        assert!(app.harness.sessions.is_empty());
        assert!(!h.registry.has_pty(id));
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
