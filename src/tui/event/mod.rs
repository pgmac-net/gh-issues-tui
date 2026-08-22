//! The async event loop and every key handler.
//!
//! `mod.rs` owns the loop itself — the `tokio::select!` over terminal events,
//! the channel of `AppEvent`s from background tasks, and the auto-refresh
//! ticker. Work spawned onto tasks lives in `spawn`, and the per-mode key
//! handlers live under `keys/`.

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use crate::provider::Provider;
use crate::provider::error::RATE_LIMIT_MSG_PREFIX;
use crate::provider::types::{Comment, FormOptions, PrLookup, PrRef, RepoIssues, RepoLabel};

use super::app::{App, Mode, SessionId, priority_set_options};
use super::harness::{HarnessRegistry, HarnessSettings};
use super::theme::Theme;
use super::ui;

mod keys;
mod spawn;

use keys::{HarnessCtx, handle_key};
use spawn::{CommentRefresh, spawn_comments, spawn_fetch};

pub enum AppEvent {
    Data(Result<Vec<RepoIssues>, String>),
    Comments {
        issue_id: String,
        result: Result<Vec<Comment>, String>,
    },
    MutationDone {
        msg: String,
        comments: CommentRefresh,
    },
    MutationFailed(String),
    /// Per-repo picker options for the new-issue form.
    FormOptions {
        repo: String,
        result: Result<FormOptions, String>,
    },
    /// Repo labels fetched for the set-priority picker.
    PriorityOptions {
        issue_id: String,
        result: Result<Vec<RepoLabel>, String>,
    },
    /// Repo labels fetched for the edit-labels picker.
    LabelOptions {
        issue_id: String,
        result: Result<Vec<RepoLabel>, String>,
    },
    /// A resolved reference, fetched for the PR-summary popup. May be an issue
    /// rather than a PR — the shorthand forms cannot tell them apart.
    PrSummary {
        pr: PrRef,
        result: Box<Result<PrLookup, String>>,
    },
    /// A harness session produced output. Payload-free by design: the bytes
    /// went straight into that session's parser on its reader thread, so a
    /// noisy agent cannot flood or stall this channel.
    HarnessDirty(SessionId),
    /// A harness child finished.
    HarnessExited {
        id: SessionId,
        code: i32,
    },
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    client: Provider,
    org: String,
    initial_repo: Option<String>,
    include_closed: bool,
    default_collapsed: bool,
    refresh_interval: u64,
    hide_empty_repos: bool,
    copy_format: String,
    theme: Theme,
    harness: HarnessSettings,
) -> Result<()> {
    let terminal = ratatui::init();
    let result = event_loop(
        terminal,
        client,
        org,
        initial_repo,
        include_closed,
        default_collapsed,
        refresh_interval,
        hide_empty_repos,
        copy_format,
        theme,
        harness,
    )
    .await;
    ratatui::restore();
    result
}

#[allow(clippy::too_many_arguments)]
async fn event_loop(
    mut terminal: DefaultTerminal,
    client: Provider,
    org: String,
    initial_repo: Option<String>,
    include_closed: bool,
    default_collapsed: bool,
    refresh_interval: u64,
    hide_empty_repos: bool,
    copy_format: String,
    theme: Theme,
    harness_settings: HarnessSettings,
) -> Result<()> {
    let mut app = App::new(
        org,
        initial_repo,
        include_closed,
        default_collapsed,
        copy_format,
    );
    app.set_hide_empty_default(hide_empty_repos);
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
    let mut keys = EventStream::new();
    // The PTYs live here, not on `App` — see `tui::harness`.
    let mut registry = HarnessRegistry::default();

    // Auto-refresh ticker. `interval` fires immediately on first tick, so
    // start one period out; a disabled (0) interval still needs a valid
    // ticker for `select!` — the branch is gated off instead.
    let refresh_enabled = refresh_interval > 0;
    let period = std::time::Duration::from_secs(refresh_interval.max(1));
    let mut refresh = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    spawn_fetch(&client, &app, &tx);

    // A detached agent producing output must not force a re-render of the
    // issue list on every chunk it writes — its bytes are already in its
    // parser either way. Only a dirty *visible* session needs a frame.
    let mut redraw = true;

    // Restored by `Drop` on every exit path out of this function, and by the
    // panic hook in `main` on the ones that never unwind this far.
    let mut title = crate::tui::title::TerminalTitle::new();

    loop {
        // Driven off the drawn state rather than hooked into attach/detach/
        // kill/exit separately: one place to be right, and it cannot drift
        // out of step with what is actually on screen. `sync` is a no-op
        // unless the title really changed, so this costs nothing per frame.
        title.sync(app.harness.active_meta().map(|s| s.issue_ref.as_str()));

        if redraw {
            terminal.draw(|f| ui::draw(f, &app, &theme, &registry))?;
        }
        redraw = true;

        tokio::select! {
            Some(Ok(ev)) = keys.next() => {
                let mut hx = HarnessCtx {
                    registry: &mut registry,
                    settings: &harness_settings,
                    tx: &tx,
                };
                match ev {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        handle_key(&mut app, key, &client, &tx, &mut hx);
                    }
                    // Children need the new size too, or their own TUI keeps
                    // drawing to the old one.
                    Event::Resize(cols, rows) => {
                        resize_sessions(&app, &mut registry, cols, rows);
                    }
                    _ => {}
                }
            }
            Some(msg) = rx.recv() => {
                if let AppEvent::HarnessDirty(id) = &msg {
                    redraw = app.harness.active == Some(*id);
                }
                handle_app_event(&mut app, msg, &client, &tx);
            }
            _ = refresh.tick(), if refresh_enabled => {
                if app.should_auto_refresh() {
                    app.loading = true;
                    app.auto_refreshing = true;
                    spawn_fetch(&client, &app, &tx);
                }
            }
        }

        if app.should_quit {
            // Children die with the process anyway once their PTY closes;
            // doing it explicitly means the terminal is restored after they
            // are gone rather than racing them.
            registry.kill_all();
            return Ok(());
        }
    }
}

/// Keep every session's PTY the size of the pane it is drawn into. All
/// sessions render full-frame, so they all take the same size — an inactive
/// one resized now is correct the moment it is attached.
fn resize_sessions(app: &App, registry: &mut HarnessRegistry, cols: u16, rows: u16) {
    let areas = super::layout::harness_areas(ratatui::layout::Rect::new(0, 0, cols, rows));
    let ids: Vec<SessionId> = app.harness.sessions.iter().map(|s| s.id).collect();
    for id in ids {
        registry.resize(id, areas.pane.height, areas.pane.width);
    }
}

pub(crate) fn nav(
    app: &mut App,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
    action: impl FnOnce(&mut App),
) {
    let prev = app.selected_issue().map(|i| i.id.clone());
    action(app);
    if !app.detail.open {
        return;
    }
    let current = app.selected_issue().map(|i| i.id.clone());
    if current == prev {
        return;
    }
    app.detail.reset_scroll();
    // Holding j/k walks a row at a time; `load_comments` keeps that free for
    // issues already fetched this cycle and for issues with no comments (#107).
    match current {
        Some(id) => {
            if let Some(id) = app.load_comments(id) {
                spawn_comments(client, id, tx);
            }
        }
        None => app.detail.comments = None,
    }
}

/// The issue id whose comment thread should be refetched after a mutation
/// completes, when the detail pane is open and showing an issue.
pub(crate) fn comments_refresh_target(app: &App) -> Option<String> {
    if !app.detail.open {
        return None;
    }
    app.selected_issue().map(|i| i.id.clone())
}

pub(crate) fn handle_app_event(
    app: &mut App,
    msg: AppEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    // Harness events never touch the API, so they must not disturb the
    // rate-limit display or be counted as an interaction.
    match msg {
        // Output already went into the parser on the reader thread; the redraw
        // at the top of the loop is the whole response.
        AppEvent::HarnessDirty(_) => return,
        AppEvent::HarnessExited { id, code } => {
            app.harness.mark_exited(id, code);
            // The PTY stays until the session is dismissed — that frozen
            // screen is the agent's summary of what it did.
            if let Some(session) = app.harness.get(id) {
                app.status = Some(format!("{} exited ({code})", session.issue_ref));
            }
            return;
        }
        _ => {}
    }

    // Pull rate limit state from client after any API interaction.
    app.rate_limit = client.rate_limit();

    match msg {
        AppEvent::Data(Ok(repos)) => {
            app.rate_limit_error = None;
            nav(app, client, tx, |app| app.set_data(repos));
            let verb = if app.auto_refreshing {
                "auto-refreshed"
            } else {
                "loaded"
            };
            app.auto_refreshing = false;
            app.status = Some(format!(
                "{verb} {} issues across {} repos",
                app.repos.iter().map(|r| r.issues.len()).sum::<usize>(),
                app.repos.len()
            ));
        }
        AppEvent::Data(Err(e)) => {
            app.loading = false;
            app.auto_refreshing = false;
            if e.starts_with(RATE_LIMIT_MSG_PREFIX) {
                app.rate_limit_error = Some(e.clone());
                app.status = Some(format!("load failed — {e}"));
            } else {
                app.status = Some(format!("load failed: {e}"));
            }
        }
        AppEvent::Comments { issue_id, result } => {
            let is_selected = app.selected_issue().map(|i| i.id.clone()) == Some(issue_id.clone());
            match result {
                // Cached even when the selection has moved on: the thread is
                // still valid for `issue_id`, so navigating back to it should
                // not cost another request. Only the *display* is stale.
                Ok(c) => {
                    app.cache_comments(issue_id, c.clone());
                    if is_selected {
                        app.detail.comments = Some(c);
                        // Keep the selection valid within the new comment count.
                        app.detail.clamp_sel();
                    }
                }
                Err(e) if is_selected => app.status = Some(format!("comments failed: {e}")),
                Err(_) => {}
            }
        }
        AppEvent::MutationDone { msg, comments } => {
            app.status = Some(msg);
            // Only refetch if we have rate limit budget left.
            let should_fetch = app.rate_limit.is_none_or(|rl| rl.remaining > 0);
            if should_fetch {
                app.loading = true;
                spawn_fetch(client, app, tx);
                if comments == CommentRefresh::Refetch
                    && let Some(id) = comments_refresh_target(app)
                {
                    // The mutation changed the comment thread itself, so the
                    // cached copy is known-stale — drop it before refetching.
                    app.invalidate_comments(&id);
                    spawn_comments(client, id, tx);
                }
            } else {
                app.rate_limit_error = Some("rate limited — refetch skipped until reset".into());
            }
        }
        AppEvent::MutationFailed(e) => {
            if e.starts_with(RATE_LIMIT_MSG_PREFIX) {
                app.rate_limit_error = Some(e.clone());
            }
            app.status = Some(format!("failed: {e}"));
        }
        AppEvent::FormOptions { repo, result } => match result {
            Ok(options) => app.set_form_options(&repo, options),
            Err(e) => {
                // Without options there is no repo id, so the form cannot
                // submit — surface the error; the user can Esc out.
                if app.issue_form.as_ref().is_some_and(|f| f.repo == repo) {
                    app.status = Some(format!("form options failed: {e}"));
                }
            }
        },
        AppEvent::PriorityOptions { issue_id, result } => {
            // Stale unless we are still in Normal mode waiting on this
            // issue's options with the selection unmoved.
            if app.mode != Mode::Normal
                || app.picker.priority_issue.as_deref() != Some(issue_id.as_str())
                || app.selected_issue().is_none_or(|i| i.id != issue_id)
            {
                if app.picker.priority_issue.as_deref() == Some(issue_id.as_str()) {
                    app.picker.priority_issue = None;
                }
                return;
            }
            match result {
                Ok(labels) => {
                    let options = priority_set_options(&labels);
                    if options.len() == 1 {
                        app.status = Some("no priority:* labels on this repo".into());
                        app.picker.priority_issue = None;
                    } else {
                        // Highlight the issue's current priority when set.
                        let idx = app
                            .selected_issue()
                            .and_then(|i| i.priority_label())
                            .and_then(|l| {
                                options.iter().position(|o| o.eq_ignore_ascii_case(&l.name))
                            })
                            .unwrap_or(0);
                        app.status = None;
                        app.picker.start(options, idx);
                        app.mode = Mode::PrioritySet;
                    }
                }
                Err(e) => {
                    app.status = Some(format!("priorities failed: {e}"));
                    app.picker.priority_issue = None;
                }
            }
        }
        AppEvent::LabelOptions { issue_id, result } => {
            // Stale unless we are still in Normal mode waiting on this
            // issue's options with the selection unmoved.
            if app.mode != Mode::Normal
                || app.picker.label_issue.as_deref() != Some(issue_id.as_str())
                || app.selected_issue().is_none_or(|i| i.id != issue_id)
            {
                if app.picker.label_issue.as_deref() == Some(issue_id.as_str()) {
                    app.picker.label_issue = None;
                }
                return;
            }
            match result {
                Ok(labels) => {
                    if labels.is_empty() {
                        app.status = Some("no labels on this repo".into());
                        app.picker.label_issue = None;
                    } else {
                        let options: Vec<String> = labels.iter().map(|l| l.name.clone()).collect();
                        // Pre-check the issue's current labels.
                        app.picker.multi_selected = app
                            .selected_issue()
                            .expect("checked above")
                            .labels
                            .iter()
                            .filter_map(|l| {
                                options.iter().position(|o| o.eq_ignore_ascii_case(&l.name))
                            })
                            .collect();
                        app.status = None;
                        app.picker.start(options, 0);
                        app.mode = Mode::LabelsSet;
                    }
                }
                Err(e) => {
                    app.status = Some(format!("labels failed: {e}"));
                    app.picker.label_issue = None;
                }
            }
        }
        // Both are fully handled above; listed so a new event cannot be
        // added without the compiler pointing here.
        AppEvent::HarnessDirty(_) | AppEvent::HarnessExited { .. } => {}
        AppEvent::PrSummary { pr, result } => {
            if let Err(e) = result.as_ref() {
                app.status = Some(format!("PR summary failed: {e}"));
            }
            app.pr.set_summary(&pr, *result);
        }
    }
}

pub(crate) fn osc52_copy(text: &str) -> std::io::Result<()> {
    use base64::Engine;
    use std::io::Write;

    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let seq = format!("\x1b]52;c;{encoded}\x07");
    let seq = if std::env::var_os("TMUX").is_some() {
        format!("\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b"))
    } else {
        seq
    };
    let mut stdout = std::io::stdout();
    stdout.write_all(seq.as_bytes())?;
    stdout.flush()
}

pub(crate) fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Items every key-handling and event submodule needs. One place for what
/// was a single import block at the top of the old `event.rs`.
pub(crate) mod prelude {
    pub use chrono::{Datelike, NaiveDate};
    pub use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    pub use tokio::sync::mpsc;

    pub use crate::provider::Provider;
    pub use crate::provider::types::{IssueState, PrRef};
    pub use crate::tui::app::{
        App, BodyEditor, CommentFocus, ConfirmChoice, DetailSel, EditorState, EditorTarget, Focus,
        HarnessConfirm, ISSUE_FORM_CANCEL_ROW, ISSUE_FORM_CREATE_ROW, ISSUE_FORM_LABEL_WIDTH,
        InputKind, InputState, IssueForm, LaunchAction, Mode, PendingMove, SessionId, StateFilter,
        issue_form_width, priority_label_set,
    };
    pub use crate::tui::harness::{HarnessRegistry, HarnessSettings};
    pub use crate::tui::{layout, ui};

    pub(crate) use super::spawn::*;
    pub(crate) use super::{AppEvent, nav, osc52_copy, split_csv};
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::types::{Comment, Issue, IssueState, Label, RepoIssues};
    use crate::tui::app::Row;
    use keys::testutil::{app_with_issue, test_client};

    fn stub_issue(id: &str, number: u64) -> Issue {
        Issue {
            id: id.into(),
            number,
            title: "t".into(),
            body: String::new(),
            state: IssueState::Open,
            url: "u".into(),
            author: "a".into(),
            assignees: vec![],
            labels: Vec::<Label>::new(),
            comment_count: 0,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            closed_at: None,
        }
    }

    fn stub_comment(body: &str) -> Comment {
        Comment {
            id: "C_1".into(),
            author: "a".into(),
            created_at: chrono::Utc::now(),
            body: body.into(),
        }
    }

    #[test]
    fn data_event_resyncs_detail_pane_when_the_selected_issue_vanishes() {
        let (mut app, _id) = app_with_issue(&[]);
        // A second issue survives the "close" below.
        app.repos[0].issues.push(stub_issue("I_2", 2));
        app.rebuild_rows();
        app.selected = app
            .rows
            .iter()
            .position(|row| match row {
                Row::Issue {
                    repo_idx,
                    issue_idx,
                } => app.repos[*repo_idx].issues[*issue_idx].id == "I_1",
                Row::RepoHeader { .. } => false,
            })
            .expect("I_1 must be a row");
        app.detail.open = true;
        app.detail.body_scroll = 7;
        // Stale thread left over from I_1 — must not survive the resync.
        app.detail.comments = Some(vec![stub_comment("stale from I_1")]);

        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        // "Close" I_1: the refetch it triggers only ever returns I_2.
        handle_app_event(
            &mut app,
            AppEvent::Data(Ok(vec![RepoIssues {
                repo: "r".into(),
                repo_url: "u".into(),
                issues: vec![stub_issue("I_2", 2)],
            }])),
            &client,
            &tx,
        );

        assert_eq!(app.selected_issue().map(|i| i.id.as_str()), Some("I_2"));
        assert_eq!(
            app.detail.body_scroll, 0,
            "scroll must reset for the new issue"
        );
        // I_2 has comment_count 0, so load_comments settles it to an empty,
        // loaded thread rather than leaving the stale one in place.
        assert_eq!(app.detail.comments.as_ref().map(Vec::len), Some(0));
    }

    #[test]
    fn data_event_clears_detail_pane_when_no_issue_survives() {
        let (mut app, _id) = app_with_issue(&[]);
        app.detail.open = true;
        app.detail.comments = Some(vec![stub_comment("stale")]);

        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_app_event(&mut app, AppEvent::Data(Ok(vec![])), &client, &tx);

        assert!(app.selected_issue().is_none());
        assert!(app.detail.comments.is_none());
    }

    #[test]
    fn data_event_leaves_detail_pane_untouched_when_selection_is_unchanged() {
        let (mut app, _id) = app_with_issue(&[]);
        app.detail.open = true;
        app.detail.body_scroll = 3;
        app.detail.comments = Some(vec![stub_comment("still current")]);

        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        // Same issue comes back on refresh — just a title change.
        let mut refreshed = stub_issue("I_1", 1);
        refreshed.title = "updated".into();
        handle_app_event(
            &mut app,
            AppEvent::Data(Ok(vec![RepoIssues {
                repo: "r".into(),
                repo_url: "u".into(),
                issues: vec![refreshed],
            }])),
            &client,
            &tx,
        );

        assert_eq!(
            app.detail.body_scroll, 3,
            "unrelated selection must not reset scroll"
        );
        assert_eq!(
            app.detail.comments.as_ref().map(|c| c.len()),
            Some(1),
            "unrelated selection must not touch the loaded thread"
        );
        assert_eq!(
            app.detail.comments.as_ref().unwrap()[0].body,
            "still current"
        );
    }

    #[tokio::test]
    async fn mutation_done_skip_leaves_cached_comments_alone() {
        let (mut app, issue_id) = app_with_issue(&[]);
        app.detail.open = true;
        app.comment_cache
            .insert(issue_id.clone(), vec![stub_comment("cached")]);

        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_app_event(
            &mut app,
            AppEvent::MutationDone {
                msg: "issue closed".into(),
                comments: CommentRefresh::Skip,
            },
            &client,
            &tx,
        );

        assert!(app.comment_cache.contains_key(&issue_id));
    }

    #[tokio::test]
    async fn mutation_done_refetch_invalidates_cached_comments() {
        let (mut app, issue_id) = app_with_issue(&[]);
        app.detail.open = true;
        app.comment_cache
            .insert(issue_id.clone(), vec![stub_comment("cached")]);

        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_app_event(
            &mut app,
            AppEvent::MutationDone {
                msg: "comment added".into(),
                comments: CommentRefresh::Refetch,
            },
            &client,
            &tx,
        );

        assert!(!app.comment_cache.contains_key(&issue_id));
    }

    #[test]
    fn split_csv_trims_and_drops_empties() {
        assert_eq!(split_csv(" a , b ,, c "), vec!["a", "b", "c"]);
        assert!(split_csv("  ").is_empty());
    }

    #[test]
    fn comments_refresh_target_is_selected_issue_when_pane_open() {
        let (mut app, issue_id) = app_with_issue(&[]);
        app.detail.open = true;
        assert_eq!(comments_refresh_target(&app), Some(issue_id));
    }

    #[test]
    fn comments_refresh_target_none_when_pane_closed() {
        let (mut app, _issue_id) = app_with_issue(&[]);
        app.detail.open = false;
        assert_eq!(comments_refresh_target(&app), None);
    }

    #[test]
    fn comments_refresh_target_none_on_repo_header() {
        let (mut app, _issue_id) = app_with_issue(&[]);
        app.detail.open = true;
        app.selected = 0; // repo header row
        assert_eq!(comments_refresh_target(&app), None);
    }
}
