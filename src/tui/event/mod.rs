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
use crate::provider::types::{Comment, FormOptions, PrRef, PrSummary, RepoIssues, RepoLabel};

use super::app::{App, Mode, priority_set_options};
use super::theme::Theme;
use super::ui;

mod keys;
mod spawn;

use keys::handle_key;
use spawn::{spawn_comments, spawn_fetch};

pub enum AppEvent {
    Data(Result<Vec<RepoIssues>, String>),
    Comments {
        issue_id: String,
        result: Result<Vec<Comment>, String>,
    },
    MutationDone(String),
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
    /// A linked PR's summary, fetched for the PR-summary popup.
    PrSummary {
        pr: PrRef,
        result: Box<Result<PrSummary, String>>,
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

    // Auto-refresh ticker. `interval` fires immediately on first tick, so
    // start one period out; a disabled (0) interval still needs a valid
    // ticker for `select!` — the branch is gated off instead.
    let refresh_enabled = refresh_interval > 0;
    let period = std::time::Duration::from_secs(refresh_interval.max(1));
    let mut refresh = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    spawn_fetch(&client, &app, &tx);

    loop {
        terminal.draw(|f| ui::draw(f, &app, &theme))?;

        tokio::select! {
            Some(Ok(ev)) = keys.next() => {
                if let Event::Key(key) = ev
                    && key.kind == KeyEventKind::Press
                {
                    handle_key(&mut app, key, &client, &tx);
                }
            }
            Some(msg) = rx.recv() => handle_app_event(&mut app, msg, &client, &tx),
            _ = refresh.tick(), if refresh_enabled => {
                if app.should_auto_refresh() {
                    app.loading = true;
                    app.auto_refreshing = true;
                    spawn_fetch(&client, &app, &tx);
                }
            }
        }

        if app.should_quit {
            return Ok(());
        }
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
    app.detail.comments = None;
    if let Some(id) = current {
        spawn_comments(client, id, tx);
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
    // Pull rate limit state from client after any API interaction.
    app.rate_limit = client.rate_limit();

    match msg {
        AppEvent::Data(Ok(repos)) => {
            app.rate_limit_error = None;
            app.set_data(repos);
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
            // Ignore stale responses for a previously selected issue.
            if app.selected_issue().map(|i| i.id.clone()) == Some(issue_id) {
                match result {
                    Ok(c) => {
                        app.detail.comments = Some(c);
                        // Keep the selection valid within the new comment count.
                        app.detail.clamp_sel();
                    }
                    Err(e) => app.status = Some(format!("comments failed: {e}")),
                }
            }
        }
        AppEvent::MutationDone(msg) => {
            app.status = Some(msg);
            // Only refetch if we have rate limit budget left.
            let should_fetch = app.rate_limit.is_none_or(|rl| rl.remaining > 0);
            if should_fetch {
                app.loading = true;
                spawn_fetch(client, app, tx);
                if let Some(id) = comments_refresh_target(app) {
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
        ISSUE_FORM_CANCEL_ROW, ISSUE_FORM_CREATE_ROW, ISSUE_FORM_LABEL_WIDTH, InputKind,
        InputState, IssueForm, Mode, StateFilter, issue_form_width, priority_label_set,
    };
    pub use crate::tui::{layout, ui};

    pub(crate) use super::spawn::*;
    pub(crate) use super::{AppEvent, nav, osc52_copy, split_csv};
}

#[cfg(test)]
mod tests {
    use super::*;
    use keys::testutil::app_with_issue;

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
