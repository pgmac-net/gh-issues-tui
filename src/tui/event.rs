use anyhow::Result;
use chrono::{Datelike, NaiveDate, TimeDelta};
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use crate::provider::Provider;
use crate::provider::error::RATE_LIMIT_MSG_PREFIX;
use crate::provider::types::{
    Comment, FormOptions, IssueState, PrRef, PrSummary, RepoIssues, RepoLabel,
};

use super::app::{
    App, BodyEditor, CommentFocus, ConfirmChoice, DetailSel, EditorTarget, Focus,
    ISSUE_FORM_CREATE_ROW, InputKind, IssueForm, Mode, StateFilter, body_popup_width,
    comment_pane_width, detail_split, priority_label_set, priority_set_options,
};
use super::theme::Theme;
use super::ui;

/// Messages from background tasks back into the UI loop.
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

fn spawn_fetch(client: &Provider, app: &App, tx: &mpsc::UnboundedSender<AppEvent>) {
    let client = client.clone();
    let org = app.org.clone();
    let include_closed = app.include_closed;
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = client
            .org_issues(&org, include_closed)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(AppEvent::Data(result));
    });
}

fn spawn_form_options(
    client: &Provider,
    org: String,
    repo: String,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let client = client.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = client
            .repo_form_options(&org, &repo)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(AppEvent::FormOptions { repo, result });
    });
}

fn spawn_priority_options(
    client: &Provider,
    org: String,
    repo: String,
    issue_id: String,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let client = client.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = client
            .repo_labels(&org, &repo)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(AppEvent::PriorityOptions { issue_id, result });
    });
}

fn spawn_label_options(
    client: &Provider,
    org: String,
    repo: String,
    issue_id: String,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let client = client.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = client
            .repo_labels(&org, &repo)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(AppEvent::LabelOptions { issue_id, result });
    });
}

fn spawn_comments(client: &Provider, issue_id: String, tx: &mpsc::UnboundedSender<AppEvent>) {
    let client = client.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = client.comments(&issue_id).await.map_err(|e| e.to_string());
        let _ = tx.send(AppEvent::Comments { issue_id, result });
    });
}

fn spawn_pr_summary(client: &Provider, pr: PrRef, tx: &mpsc::UnboundedSender<AppEvent>) {
    let client = client.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = client.pull_request(&pr).await.map_err(|e| e.to_string());
        let _ = tx.send(AppEvent::PrSummary {
            pr,
            result: Box::new(result),
        });
    });
}

/// Run a selection-changing action, then live-update the detail pane: when
/// the split is open and the selected issue changed, reset the pane and
/// fetch the new issue's comments (stale responses are dropped by id in
/// `handle_app_event`). Landing on a repo header just clears the pane.
fn nav(
    app: &mut App,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
    action: impl FnOnce(&mut App),
) {
    let prev = app.selected_issue().map(|i| i.id.clone());
    action(app);
    if !app.detail_open {
        return;
    }
    let current = app.selected_issue().map(|i| i.id.clone());
    if current == prev {
        return;
    }
    app.reset_detail_scroll();
    app.detail_comments = None;
    if let Some(id) = current {
        spawn_comments(client, id, tx);
    }
}

/// The issue id whose comment thread should be refetched after a mutation
/// completes, when the detail pane is open and showing an issue.
fn comments_refresh_target(app: &App) -> Option<String> {
    if !app.detail_open {
        return None;
    }
    app.selected_issue().map(|i| i.id.clone())
}

fn handle_app_event(
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
                        app.detail_comments = Some(c);
                        // Keep the selection valid within the new comment count.
                        app.clamp_detail_sel();
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
                || app.priority_pick_issue.as_deref() != Some(issue_id.as_str())
                || app.selected_issue().is_none_or(|i| i.id != issue_id)
            {
                if app.priority_pick_issue.as_deref() == Some(issue_id.as_str()) {
                    app.priority_pick_issue = None;
                }
                return;
            }
            match result {
                Ok(labels) => {
                    let options = priority_set_options(&labels);
                    if options.len() == 1 {
                        app.status = Some("no priority:* labels on this repo".into());
                        app.priority_pick_issue = None;
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
                        app.start_picker(options, idx);
                        app.mode = Mode::PrioritySet;
                    }
                }
                Err(e) => {
                    app.status = Some(format!("priorities failed: {e}"));
                    app.priority_pick_issue = None;
                }
            }
        }
        AppEvent::LabelOptions { issue_id, result } => {
            // Stale unless we are still in Normal mode waiting on this
            // issue's options with the selection unmoved.
            if app.mode != Mode::Normal
                || app.label_pick_issue.as_deref() != Some(issue_id.as_str())
                || app.selected_issue().is_none_or(|i| i.id != issue_id)
            {
                if app.label_pick_issue.as_deref() == Some(issue_id.as_str()) {
                    app.label_pick_issue = None;
                }
                return;
            }
            match result {
                Ok(labels) => {
                    if labels.is_empty() {
                        app.status = Some("no labels on this repo".into());
                        app.label_pick_issue = None;
                    } else {
                        let options: Vec<String> = labels.iter().map(|l| l.name.clone()).collect();
                        // Pre-check the issue's current labels.
                        app.multi_selected = app
                            .selected_issue()
                            .expect("checked above")
                            .labels
                            .iter()
                            .filter_map(|l| {
                                options.iter().position(|o| o.eq_ignore_ascii_case(&l.name))
                            })
                            .collect();
                        app.status = None;
                        app.start_picker(options, 0);
                        app.mode = Mode::LabelsSet;
                    }
                }
                Err(e) => {
                    app.status = Some(format!("labels failed: {e}"));
                    app.label_pick_issue = None;
                }
            }
        }
        AppEvent::PrSummary { pr, result } => {
            if let Err(e) = result.as_ref() {
                app.status = Some(format!("PR summary failed: {e}"));
            }
            app.set_pr_summary(&pr, *result);
        }
    }
}

fn handle_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    match app.mode {
        Mode::Normal => handle_normal_key(app, key, client, tx),
        Mode::Input(kind) => handle_input_key(app, key, kind, client, tx),
        Mode::FilterMenu => handle_filter_menu_key(app, key),
        Mode::SelectField(idx) => handle_select_field_key(app, key, idx),
        Mode::SelectFieldMulti(idx) => handle_select_field_multi_key(app, key, idx),
        Mode::Calendar(idx) => handle_calendar_key(app, key, idx),
        Mode::ConfirmState => handle_confirm_key(app, key, client, tx),
        Mode::IssueForm => handle_issue_form_key(app, key, client, tx),
        Mode::IssueFormSelect(idx) => handle_form_select_key(app, key, idx),
        Mode::IssueFormMulti(idx) => handle_form_multi_key(app, key, idx),
        Mode::IssueFormBody => handle_form_body_key(app, key),
        Mode::CommentEditor => handle_comment_editor_key(app, key, client, tx),
        Mode::PrioritySet => handle_priority_set_key(app, key, client, tx),
        Mode::LabelsSet => handle_labels_set_key(app, key, client, tx),
        Mode::PrPicker => handle_pr_picker_key(app, key, client, tx),
        Mode::PrSummary => handle_pr_summary_key(app, key),
        Mode::Help => app.mode = Mode::Normal,
    }
}

fn handle_pr_picker_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    if picker_common_key(app, key, true) {
        return;
    }
    match key.code {
        KeyCode::Esc => app.close_pr_picker(),
        KeyCode::Enter => match app.picker_selected_original() {
            Some(orig) => {
                let pr = app.pr_links[orig].clone();
                app.open_pr_summary(pr.clone());
                spawn_pr_summary(client, pr, tx);
            }
            None if app.select_options.is_empty() => app.close_pr_picker(),
            None => {}
        },
        _ => {}
    }
}

fn handle_pr_summary_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.close_pr_summary(),
        KeyCode::Char('j') | KeyCode::Down => {
            app.pr_scroll = app.pr_scroll.saturating_add(1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.pr_scroll = app.pr_scroll.saturating_sub(1);
        }
        _ => {}
    }
}

fn handle_priority_set_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    if picker_common_key(app, key, true) {
        return;
    }
    match key.code {
        KeyCode::Esc => {
            app.priority_pick_issue = None;
            app.mode = Mode::Normal;
        }
        KeyCode::Enter => {
            let Some(orig) = app.picker_selected_original() else {
                // Filter matches nothing: keep the picker open unless it is
                // truly empty (mirrors the select-field picker).
                if app.select_options.is_empty() {
                    app.priority_pick_issue = None;
                    app.mode = Mode::Normal;
                }
                return;
            };
            let pick = app.select_options[orig].clone();
            // The selection cannot move while the picker is open, but the
            // issue can vanish under a refetch that landed before it opened.
            let still_target = app
                .selected_issue()
                .is_some_and(|i| app.priority_pick_issue.as_deref() == Some(i.id.as_str()));
            app.priority_pick_issue = None;
            app.mode = Mode::Normal;
            if !still_target {
                app.status = Some("selection changed — priority not set".into());
                return;
            }
            let names = priority_label_set(
                app.selected_issue().expect("checked above"),
                (pick != "\u{2014}").then_some(pick.as_str()),
            );
            let (org, repo) = match app.selected_repo() {
                Some(r) => (app.org.clone(), r.repo.clone()),
                None => return,
            };
            with_issue(
                app,
                client,
                tx,
                "priority updated",
                move |c, id| async move { c.set_labels(&id, &repo, &org, &names).await },
            );
        }
        _ => {}
    }
}

fn handle_labels_set_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    if picker_common_key(app, key, false) {
        return;
    }
    match key.code {
        KeyCode::Esc => {
            app.label_pick_issue = None;
            app.mode = Mode::Normal;
        }
        KeyCode::Char(' ') => {
            if let Some(orig) = app.picker_selected_original()
                && !app.multi_selected.remove(&orig)
            {
                app.multi_selected.insert(orig);
            }
        }
        KeyCode::Enter => {
            // The selection cannot move while the picker is open, but the
            // issue can vanish under a refetch that landed before it opened.
            let still_target = app
                .selected_issue()
                .is_some_and(|i| app.label_pick_issue.as_deref() == Some(i.id.as_str()));
            let mut names: Vec<String> = app
                .multi_selected
                .iter()
                .filter_map(|&i| app.select_options.get(i).cloned())
                .collect();
            names.sort_unstable();
            app.label_pick_issue = None;
            app.mode = Mode::Normal;
            if !still_target {
                app.status = Some("selection changed — labels not set".into());
                return;
            }
            let (org, repo) = match app.selected_repo() {
                Some(r) => (app.org.clone(), r.repo.clone()),
                None => return,
            };
            with_issue(app, client, tx, "labels updated", move |c, id| async move {
                c.set_labels(&id, &repo, &org, &names).await
            });
        }
        _ => {}
    }
}

fn handle_issue_form_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let Some(form) = &mut app.issue_form else {
        app.mode = Mode::Normal;
        return;
    };
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.cancel_issue_form();
            app.status = Some("issue creation cancelled".into());
        }
        KeyCode::Char('j') | KeyCode::Down => {
            form.field_idx = (form.field_idx + 1).min(ISSUE_FORM_CREATE_ROW);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            form.field_idx = form.field_idx.saturating_sub(1);
        }
        KeyCode::Enter => {
            let idx = form.field_idx;
            match idx {
                0 => {
                    let current = form.title.clone();
                    app.input.start(&current);
                    app.mode = Mode::Input(InputKind::FormTitle);
                }
                1 => app.mode = Mode::IssueFormBody,
                _ if IssueForm::is_multi_field(idx) => {
                    let opts = form.field_options(idx);
                    app.multi_selected = form.multi_set(idx).clone();
                    app.start_picker(opts, 0);
                    app.mode = Mode::IssueFormMulti(idx);
                }
                _ if IssueForm::is_select_field(idx) => {
                    // "—" (none) is prepended; stored choices are offset by 1.
                    let mut opts = form.field_options(idx);
                    opts.insert(0, "\u{2014}".to_string());
                    let initial = form.get_single(idx).map_or(0, |i| i + 1);
                    app.start_picker(opts, initial);
                    app.mode = Mode::IssueFormSelect(idx);
                }
                _ => submit_issue_form(app, client, tx),
            }
        }
        _ => {}
    }
}

/// Keys shared by every option picker: ↑/↓ navigation over the filtered
/// view, Home/End, and type-ahead filter editing (chars append, Backspace
/// deletes, Ctrl+U clears). Space is passed through when `space_filters`
/// is false so the multi-select picker can use it to toggle. Returns true
/// when the key was consumed.
fn picker_common_key(app: &mut App, key: KeyEvent, space_filters: bool) -> bool {
    let visible = app.filtered_select().len();
    match key.code {
        KeyCode::Down => {
            if visible > 0 {
                app.select_idx = (app.select_idx + 1) % visible;
            }
            true
        }
        KeyCode::Up => {
            if visible > 0 {
                app.select_idx = (app.select_idx + visible - 1) % visible;
            }
            true
        }
        KeyCode::Home => {
            app.select_idx = 0;
            true
        }
        KeyCode::End => {
            app.select_idx = visible.saturating_sub(1);
            true
        }
        KeyCode::Backspace => {
            app.picker_filter_backspace();
            true
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.picker_filter_clear();
            true
        }
        KeyCode::Char(' ') if !space_filters => false,
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.picker_filter_push(c);
            true
        }
        _ => false,
    }
}

fn handle_form_select_key(app: &mut App, key: KeyEvent, idx: usize) {
    if picker_common_key(app, key, true) {
        return;
    }
    match key.code {
        KeyCode::Esc => app.mode = Mode::IssueForm,
        KeyCode::Enter => match app.picker_selected_original() {
            Some(orig) => {
                if let Some(form) = &mut app.issue_form {
                    // Index 0 is "—" (clear); real options are offset by 1.
                    form.set_single(idx, orig.checked_sub(1));
                }
                app.mode = Mode::IssueForm;
            }
            // No options at all → close; filter matching nothing → no-op
            // so the filter can be corrected.
            None if app.select_options.is_empty() => app.mode = Mode::IssueForm,
            None => {}
        },
        _ => {}
    }
}

fn handle_form_multi_key(app: &mut App, key: KeyEvent, idx: usize) {
    if picker_common_key(app, key, false) {
        return;
    }
    match key.code {
        KeyCode::Esc => app.mode = Mode::IssueForm, // discard toggles
        KeyCode::Char(' ') => {
            if let Some(orig) = app.picker_selected_original()
                && !app.multi_selected.remove(&orig)
            {
                app.multi_selected.insert(orig);
            }
        }
        KeyCode::Enter => {
            if let Some(form) = &mut app.issue_form {
                *form.multi_set_mut(idx) = app.multi_selected.clone();
            }
            app.mode = Mode::IssueForm;
        }
        _ => {}
    }
}

/// Keys shared by every multi-line `BodyEditor`: readline-style editing plus
/// visual-row up/down. Enter and Esc are each caller's own since they mean
/// different things per mode (newline vs. keep/discard). Returns whether the
/// key was consumed.
fn apply_body_editor_key(body: &mut BodyEditor, key: KeyEvent, wrap_width: usize) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Enter => body.newline(),
        KeyCode::Backspace => body.backspace(),
        KeyCode::Delete => body.delete_char(),
        KeyCode::Left if ctrl => body.word_left(),
        KeyCode::Right if ctrl => body.word_right(),
        KeyCode::Left => body.left(),
        KeyCode::Right => body.right(),
        KeyCode::Up => body.up_visual(wrap_width),
        KeyCode::Down => body.down_visual(wrap_width),
        KeyCode::Home => body.home(),
        KeyCode::End => body.end(),
        KeyCode::Char('a') if ctrl => body.home(),
        KeyCode::Char('e') if ctrl => body.end(),
        KeyCode::Char('w') if ctrl => body.delete_word_back(),
        KeyCode::Char('u') if ctrl => body.kill_to_start(),
        KeyCode::Char('k') if ctrl => body.kill_to_end(),
        KeyCode::Char('d') if ctrl => body.delete_char(),
        KeyCode::Char(c) if !ctrl => body.insert(c),
        _ => return false,
    }
    true
}

fn handle_form_body_key(app: &mut App, key: KeyEvent) {
    let Some(form) = &mut app.issue_form else {
        app.mode = Mode::Normal;
        return;
    };
    if key.code == KeyCode::Esc {
        app.mode = Mode::IssueForm; // content kept
        return;
    }
    apply_body_editor_key(&mut form.body, key, body_wrap_width());
}

fn handle_comment_editor_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => cancel_comment(app),
        KeyCode::Char('s') if ctrl => submit_comment(app, client, tx),
        KeyCode::Tab => app.comment_focus = next_comment_focus(app.comment_focus),
        KeyCode::BackTab => app.comment_focus = prev_comment_focus(app.comment_focus),
        KeyCode::Enter | KeyCode::Char(' ') if app.comment_focus == CommentFocus::Save => {
            submit_comment(app, client, tx)
        }
        KeyCode::Enter | KeyCode::Char(' ') if app.comment_focus == CommentFocus::Cancel => {
            cancel_comment(app)
        }
        _ if app.comment_focus == CommentFocus::Editor => {
            apply_body_editor_key(&mut app.comment_editor, key, comment_wrap_width());
        }
        _ => {}
    }
}

fn cancel_comment(app: &mut App) {
    app.comment_editor = BodyEditor::default();
    app.comment_focus = CommentFocus::Editor;
    app.mode = Mode::Normal;
    app.status = Some("comment discarded".into());
}

fn next_comment_focus(focus: CommentFocus) -> CommentFocus {
    match focus {
        CommentFocus::Editor => CommentFocus::Save,
        CommentFocus::Save => CommentFocus::Cancel,
        CommentFocus::Cancel => CommentFocus::Editor,
    }
}

fn prev_comment_focus(focus: CommentFocus) -> CommentFocus {
    match focus {
        CommentFocus::Editor => CommentFocus::Cancel,
        CommentFocus::Save => CommentFocus::Editor,
        CommentFocus::Cancel => CommentFocus::Save,
    }
}

fn submit_comment(app: &mut App, client: &Provider, tx: &mpsc::UnboundedSender<AppEvent>) {
    let value = app.comment_editor.text();
    let target = app.editor_target.clone();
    app.comment_editor = BodyEditor::default();
    app.comment_focus = CommentFocus::Editor;
    app.editor_target = EditorTarget::NewComment;
    app.mode = Mode::Normal;
    // An empty comment is discarded; an empty description is a valid edit
    // (clearing the body).
    if value.trim().is_empty() && !matches!(target, EditorTarget::EditBody) {
        app.status = Some("empty — discarded".into());
        return;
    }
    match target {
        EditorTarget::NewComment => {
            with_issue(app, client, tx, "comment added", move |c, id| async move {
                c.add_comment(&id, &value).await
            });
        }
        EditorTarget::EditComment { comment_id } => {
            with_issue(
                app,
                client,
                tx,
                "comment updated",
                move |c, id| async move { c.update_comment(&id, &comment_id, &value).await },
            );
        }
        EditorTarget::EditBody => {
            with_issue(
                app,
                client,
                tx,
                "description updated",
                move |c, id| async move { c.update_body(&id, &value).await },
            );
        }
    }
}

/// The wrap width the body popup is currently rendered at.
fn body_wrap_width() -> usize {
    let cols = crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80);
    body_popup_width(cols) as usize
}

/// The wrap width the inline comment section is currently rendered at.
fn comment_wrap_width() -> usize {
    let cols = crossterm::terminal::size().map(|(w, _)| w).unwrap_or(80);
    comment_pane_width(cols) as usize
}

/// The detail pane's current inner width and the body/comments regions'
/// viewport heights, derived from the live terminal size. Mirrors `ui::draw`'s
/// 40/60 horizontal split and `detail_split`'s vertical split so the key
/// handler's scroll clamps agree with what is drawn.
fn detail_metrics() -> (u16, u16, u16) {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let inner_w = comment_pane_width(cols);
    // Main area = total rows minus the info + bottom status lines.
    let main_h = rows.saturating_sub(2);
    let (body_h, comments_h) = detail_split(main_h);
    (
        inner_w,
        body_h.saturating_sub(2),
        comments_h.saturating_sub(2),
    )
}

/// Scroll the selected detail region by `lines` (negative = up), clamped to
/// that region's extent: the body to its content, a comment to its own span.
fn detail_scroll(app: &mut App, lines: isize) {
    let (inner_w, body_view, comments_view) = detail_metrics();
    match app.detail_sel {
        DetailSel::Body => {
            let Some(issue) = app.selected_issue() else {
                return;
            };
            let content = ui::body_content_height(issue, inner_w);
            let max = content.saturating_sub(body_view);
            app.scroll_body(lines, max);
        }
        DetailSel::Comment(i) => {
            let bounds = app.detail_comments.as_ref().and_then(|comments| {
                let c = comments.get(i)?;
                let top = ui::comment_offset(comments, i, inner_w);
                let height = ui::comment_height(c, inner_w);
                Some((top, top + height.saturating_sub(comments_view)))
            });
            if let Some((lo, hi)) = bounds {
                app.scroll_comment(lines, lo, hi);
            }
        }
    }
}

/// The active region's viewport height, used as the PageUp/PageDown step.
fn detail_page_rows(app: &App) -> isize {
    let (_, body_view, comments_view) = detail_metrics();
    match app.detail_sel {
        DetailSel::Body => body_view as isize,
        DetailSel::Comment(_) => comments_view as isize,
    }
}

/// After `select_detail` lands on a comment, snap the comments viewport so
/// that comment's header sits at the top of the region.
fn snap_after_select(app: &mut App) {
    let DetailSel::Comment(i) = app.detail_sel else {
        return;
    };
    let (inner_w, _, _) = detail_metrics();
    let Some(comments) = app.detail_comments.as_ref() else {
        return;
    };
    let top = ui::comment_offset(comments, i, inner_w);
    app.snap_comment(top);
}

fn submit_issue_form(app: &mut App, client: &Provider, tx: &mpsc::UnboundedSender<AppEvent>) {
    let Some(form) = &app.issue_form else { return };
    if form.options.is_none() {
        app.status = Some("still loading repo options — try again in a moment".into());
        return;
    }
    let Some(params) = form.build_params() else {
        app.status = Some("a title is required".into());
        return;
    };
    let repo = form.repo.clone();
    app.cancel_issue_form();
    app.status = Some(format!("creating issue in {repo}…"));
    let client = client.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let msg = match client.create_issue(&params).await {
            Ok((number, _url)) => AppEvent::MutationDone(format!("created #{number} in {repo}")),
            Err(e) => AppEvent::MutationFailed(e.to_string()),
        };
        let _ = tx.send(msg);
    });
}

/// Copy `text` to the system clipboard via an OSC 52 escape sequence
/// written straight to stdout. Terminal emulators intercept and consume
/// the sequence rather than displaying it, so this is safe to interleave
/// with ratatui's rendering. Works over SSH; requires terminal OSC 52
/// support. Transparently wrapped for tmux passthrough when `$TMUX` is set.
fn osc52_copy(text: &str) -> std::io::Result<()> {
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

fn handle_normal_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    match key.code {
        KeyCode::Char('q') => {
            if app.detail_open {
                app.close_detail();
            } else {
                app.should_quit = true;
            }
        }
        KeyCode::Esc if app.detail_open => app.close_detail(),
        // In the detail pane Tab/Shift+Tab move between comments; from the
        // list they switch into the pane.
        KeyCode::Tab => {
            if app.focus == Focus::Detail {
                app.select_detail(1);
                snap_after_select(app);
            } else {
                app.cycle_focus();
            }
        }
        KeyCode::BackTab => {
            if app.focus == Focus::Detail {
                app.select_detail(-1);
                snap_after_select(app);
            } else {
                app.cycle_focus();
            }
        }
        KeyCode::Char('?') => app.mode = Mode::Help,
        KeyCode::Char('r') => {
            app.loading = true;
            app.status = Some("reloading…".into());
            spawn_fetch(client, app, tx);
        }

        // navigation
        KeyCode::Char('j') | KeyCode::Down => {
            if app.focus == Focus::Detail {
                detail_scroll(app, 1);
            } else {
                nav(app, client, tx, |a| a.move_selection(1));
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.focus == Focus::Detail {
                detail_scroll(app, -1);
            } else {
                nav(app, client, tx, |a| a.move_selection(-1));
            }
        }
        KeyCode::PageDown => {
            if app.focus == Focus::Detail {
                let page = detail_page_rows(app);
                detail_scroll(app, page);
            } else {
                nav(app, client, tx, |a| a.move_selection(15));
            }
        }
        KeyCode::PageUp => {
            if app.focus == Focus::Detail {
                let page = detail_page_rows(app);
                detail_scroll(app, -page);
            } else {
                nav(app, client, tx, |a| a.move_selection(-15));
            }
        }
        KeyCode::Char('g') | KeyCode::Home => nav(app, client, tx, |a| a.selected = 0),
        KeyCode::Char('G') | KeyCode::End => {
            nav(app, client, tx, |a| {
                a.selected = a.rows.len().saturating_sub(1);
            });
        }

        // grouping (list focus only — in the detail pane ← focuses the list)
        KeyCode::Right if app.focus == Focus::List => {
            if app.selected_issue().is_some() {
                // Issue row: → goes deeper into the detail pane (mirror of
                // ← backing out), opening the split like Enter if needed.
                if let Some(issue_id) = app.enter_detail() {
                    spawn_comments(client, issue_id, tx);
                }
            } else {
                app.set_current_collapsed(false);
            }
        }
        KeyCode::Left => {
            if app.focus == Focus::Detail {
                app.focus = Focus::List;
            } else {
                nav(app, client, tx, |a| a.set_current_collapsed(true));
            }
        }
        KeyCode::Char(' ') => nav(app, client, tx, App::toggle_collapse),
        KeyCode::Char('[') => nav(app, client, tx, |a| a.set_all_collapsed(true)),
        KeyCode::Char(']') => nav(app, client, tx, |a| a.set_all_collapsed(false)),

        // filters & sort
        KeyCode::Char('/') => {
            app.input.start(&app.filters.text.clone());
            app.mode = Mode::Input(InputKind::Search);
        }
        // jump the selection to an issue by number (does not filter the list)
        KeyCode::Char('#') => {
            app.input.start("");
            app.mode = Mode::Input(InputKind::GotoNumber);
        }
        KeyCode::Char('f') => {
            app.state_filter = app.state_filter.next();
            if app.state_filter != StateFilter::Open && !app.include_closed {
                // Closed issues were never fetched; upgrade the dataset once.
                app.include_closed = true;
                app.loading = true;
                app.status = Some("fetching closed issues…".into());
                spawn_fetch(client, app, tx);
            }
            app.rebuild_rows();
            app.expand_single_visible();
        }
        KeyCode::Char('F') => {
            app.filter_menu_idx = 0;
            app.mode = Mode::FilterMenu;
        }
        KeyCode::Char('s') => {
            app.sort_key = app.sort_key.next();
            app.rebuild_rows();
        }
        KeyCode::Char('S') => {
            app.sort_desc = !app.sort_desc;
            app.rebuild_rows();
        }

        // switch org/owner
        KeyCode::Char('w') => {
            let current = app.org.clone();
            app.input.start(&current);
            app.mode = Mode::Input(InputKind::Org);
        }

        // open in browser
        KeyCode::Char('o') => {
            let url = app
                .selected_issue()
                .map(|i| i.url.clone())
                .or_else(|| app.selected_repo().map(|r| r.repo_url.clone()));
            if let Some(url) = url {
                match open::that(&url) {
                    Ok(()) => app.status = Some(format!("opened {url}")),
                    Err(e) => app.status = Some(format!("open failed: {e}")),
                }
            }
        }
        // copy short reference to clipboard (via OSC 52 — works over SSH,
        // no system clipboard dependency)
        KeyCode::Char('y') => {
            if let Some(r) = app.selected_short_ref() {
                match osc52_copy(&r) {
                    Ok(()) => app.status = Some(format!("copied {r}")),
                    Err(e) => app.status = Some(format!("copy failed: {e}")),
                }
            }
        }
        KeyCode::Char('O') => {
            if let Some(url) = app.selected_repo().map(|r| r.repo_url.clone()) {
                match open::that(&url) {
                    Ok(()) => app.status = Some(format!("opened {url}")),
                    Err(e) => app.status = Some(format!("open failed: {e}")),
                }
            }
        }

        // detail
        KeyCode::Enter => {
            if let Some(issue_id) = app.selected_issue().map(|i| i.id.clone()) {
                app.open_detail();
                spawn_comments(client, issue_id, tx);
            } else {
                app.toggle_collapse();
            }
        }

        // mutations
        KeyCode::Char('c') => {
            if let Some(issue_id) = app.start_comment_editor() {
                spawn_comments(client, issue_id, tx);
            }
        }
        // Edit the highlighted detail card: the issue body or a comment.
        KeyCode::Char('e') if app.detail_open => app.start_edit_selected_card(),
        KeyCode::Char('x') => {
            if app.selected_issue().is_some() {
                app.confirm_choice = ConfirmChoice::No;
                app.mode = Mode::ConfirmState;
            }
        }
        KeyCode::Char('a') => {
            if let Some(issue) = app.selected_issue() {
                let current = issue.assignees.join(", ");
                app.input.start(&current);
                app.mode = Mode::Input(InputKind::Assignees);
            }
        }
        KeyCode::Char('l') => {
            let target = app
                .selected_issue()
                .map(|i| i.id.clone())
                .zip(app.selected_repo().map(|r| r.repo.clone()));
            if let Some((issue_id, repo)) = target {
                app.label_pick_issue = Some(issue_id.clone());
                app.status = Some("loading labels…".into());
                spawn_label_options(client, app.org.clone(), repo, issue_id, tx);
            }
        }
        KeyCode::Char('t') => {
            if let Some(issue) = app.selected_issue() {
                let title = issue.title.clone();
                app.input.start(&title);
                app.mode = Mode::Input(InputKind::Title);
            }
        }
        KeyCode::Char('p') => {
            let target = app
                .selected_issue()
                .map(|i| i.id.clone())
                .zip(app.selected_repo().map(|r| r.repo.clone()));
            if let Some((issue_id, repo)) = target {
                app.priority_pick_issue = Some(issue_id.clone());
                app.status = Some("loading priorities…".into());
                spawn_priority_options(client, app.org.clone(), repo, issue_id, tx);
            }
        }
        KeyCode::Char('n') => {
            if let Some(repo) = app.selected_repo().map(|r| r.repo.clone()) {
                app.open_issue_form(repo.clone());
                spawn_form_options(client, app.org.clone(), repo, tx);
            }
        }
        KeyCode::Char('P') if app.detail_open => {
            if !client.supports_pr_summary() {
                app.status = Some("PR summaries not supported by this provider".into());
                return;
            }
            let links = app.collect_pr_links();
            match links.len() {
                0 => app.status = Some("no PR links found".into()),
                1 => {
                    let pr = links.into_iter().next().expect("checked len == 1");
                    app.open_pr_summary(pr.clone());
                    spawn_pr_summary(client, pr, tx);
                }
                _ => app.open_pr_picker(links),
            }
        }
        _ => {}
    }
}

fn handle_input_key(
    app: &mut App,
    key: KeyEvent,
    kind: InputKind,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    match key.code {
        KeyCode::Esc => {
            app.mode = match kind {
                InputKind::FilterField(_) => Mode::FilterMenu,
                InputKind::FormTitle => Mode::IssueForm,
                _ => Mode::Normal,
            };
        }
        KeyCode::Enter => {
            let value = app.input.buffer.clone();
            app.mode = Mode::Normal;
            submit_input(app, kind, value, client, tx);
        }
        KeyCode::Backspace => app.input.backspace(),
        KeyCode::Delete => app.input.delete_char(),
        KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => app.input.word_left(),
        KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => app.input.word_right(),
        KeyCode::Left => app.input.left(),
        KeyCode::Right => app.input.right(),
        KeyCode::Home => app.input.home(),
        KeyCode::End => app.input.end(),
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => app.input.home(),
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => app.input.end(),
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.input.delete_word_back();
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.input.kill_to_start();
        }
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.input.kill_to_end();
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.input.delete_char();
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.input.insert(c);
        }
        _ => {}
    }
}

fn submit_input(
    app: &mut App,
    kind: InputKind,
    value: String,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    match kind {
        InputKind::Search | InputKind::FilterField(_) => {
            app.apply_filter_input(kind, &value);
            if matches!(kind, InputKind::FilterField(_)) {
                app.mode = Mode::FilterMenu;
            }
        }
        InputKind::Assignees => {
            let logins = split_csv(&value);
            with_issue(
                app,
                client,
                tx,
                "assignees updated",
                move |c, id| async move { c.set_assignees(&id, &logins).await },
            );
        }
        InputKind::Title => {
            if value.trim().is_empty() {
                app.status = Some("empty title discarded".into());
                return;
            }
            with_issue(app, client, tx, "title updated", move |c, id| async move {
                c.update_title(&id, &value).await
            });
        }
        InputKind::Org => {
            let org = value.trim().to_string();
            if org.is_empty() || org.eq_ignore_ascii_case(&app.org) {
                app.status = Some("org unchanged".into());
                return;
            }
            app.status = Some(format!("switching to {org}…"));
            app.switch_org(org);
            spawn_fetch(client, app, tx);
        }
        InputKind::FormTitle => {
            if let Some(form) = &mut app.issue_form {
                form.title = value.trim().to_string();
            }
            app.mode = Mode::IssueForm;
        }
        InputKind::GotoNumber => {
            let trimmed = value.trim().trim_start_matches('#').trim();
            match trimmed.parse::<u64>() {
                Ok(number) => nav(app, client, tx, |a| {
                    a.jump_to_number(number);
                }),
                Err(_) => app.status = Some("not an issue number".into()),
            }
        }
    }
}

fn handle_filter_menu_key(app: &mut App, key: KeyEvent) {
    use super::app::FILTER_FIELDS;
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::Normal,
        KeyCode::Char('j') | KeyCode::Down => {
            app.filter_menu_idx = (app.filter_menu_idx + 1) % FILTER_FIELDS.len();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.filter_menu_idx =
                (app.filter_menu_idx + FILTER_FIELDS.len() - 1) % FILTER_FIELDS.len();
        }
        KeyCode::Char('c') => {
            app.clear_filters();
            app.rebuild_rows();
            app.expand_single_visible();
        }
        KeyCode::Enter => {
            let idx = app.filter_menu_idx;
            if idx == super::app::FILTER_HIDE_EMPTY_IDX {
                app.toggle_hide_empty();
            } else if App::is_multi_select_field(idx) {
                let options = app.compute_multi_options(idx);
                let current = if idx == 4 {
                    &app.filters.priority
                } else {
                    &app.filters.status
                };
                app.multi_selected = options
                    .iter()
                    .enumerate()
                    .filter(|(_, o)| current.iter().any(|c| c.eq_ignore_ascii_case(o)))
                    .map(|(i, _)| i)
                    .collect();
                app.start_picker(options, 0);
                app.mode = Mode::SelectFieldMulti(idx);
            } else if App::is_select_field(idx) {
                let options = app.compute_select_options(idx);
                let current = app.current_filter_value(idx);
                let initial = options.iter().position(|v| v == &current).unwrap_or(0);
                app.start_picker(options, initial);
                app.mode = Mode::SelectField(idx);
            } else if App::is_calendar_field(idx) {
                app.calendar_init(idx);
                app.mode = Mode::Calendar(idx);
            } else {
                let current = app.current_filter_value(idx);
                app.input.start(&current);
                app.mode = Mode::Input(InputKind::FilterField(idx));
            }
        }
        _ => {}
    }
}

fn handle_select_field_key(app: &mut App, key: KeyEvent, idx: usize) {
    if picker_common_key(app, key, true) {
        return;
    }
    match key.code {
        KeyCode::Esc => app.mode = Mode::FilterMenu,
        KeyCode::Enter => match app.picker_selected_original() {
            Some(orig) => {
                let raw = app.select_options[orig].clone();
                let value = if raw == "\u{2014}" {
                    String::new()
                } else {
                    raw
                };
                app.apply_filter_input(InputKind::FilterField(idx), &value);
                app.mode = Mode::FilterMenu;
            }
            // No options at all → close; filter matching nothing → no-op
            // so the filter can be corrected.
            None if app.select_options.is_empty() => app.mode = Mode::FilterMenu,
            None => {}
        },
        _ => {}
    }
}

fn handle_select_field_multi_key(app: &mut App, key: KeyEvent, idx: usize) {
    if picker_common_key(app, key, false) {
        return;
    }
    match key.code {
        KeyCode::Esc => app.mode = Mode::FilterMenu, // discard toggles
        KeyCode::Char(' ') => {
            if let Some(orig) = app.picker_selected_original()
                && !app.multi_selected.remove(&orig)
            {
                app.multi_selected.insert(orig);
            }
        }
        KeyCode::Enter => {
            let mut picked: Vec<usize> = app.multi_selected.iter().copied().collect();
            picked.sort();
            let values: Vec<String> = picked
                .into_iter()
                .filter_map(|i| app.select_options.get(i).cloned())
                .collect();
            app.apply_multi_filter(idx, values);
            app.mode = Mode::FilterMenu;
        }
        _ => {}
    }
}

fn handle_calendar_key(app: &mut App, key: KeyEvent, idx: usize) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::FilterMenu,
        KeyCode::Left => {
            app.calendar_cursor = app
                .calendar_cursor
                .pred_opt()
                .unwrap_or(app.calendar_cursor);
        }
        KeyCode::Right => {
            app.calendar_cursor = app
                .calendar_cursor
                .succ_opt()
                .unwrap_or(app.calendar_cursor);
        }
        KeyCode::Up => {
            app.calendar_cursor -= TimeDelta::days(7);
        }
        KeyCode::Down => {
            app.calendar_cursor += TimeDelta::days(7);
        }
        KeyCode::PageUp => {
            let first =
                NaiveDate::from_ymd_opt(app.calendar_cursor.year(), app.calendar_cursor.month(), 1)
                    .unwrap();
            app.calendar_cursor = first
                .pred_opt()
                .and_then(|d| d.with_day(1))
                .unwrap_or(first);
        }
        KeyCode::PageDown => {
            let first =
                NaiveDate::from_ymd_opt(app.calendar_cursor.year(), app.calendar_cursor.month(), 1)
                    .unwrap();
            let next_first = if first.month() == 12 {
                NaiveDate::from_ymd_opt(first.year() + 1, 1, 1).unwrap()
            } else {
                NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1).unwrap()
            };
            app.calendar_cursor = next_first;
        }
        KeyCode::Home => {
            app.calendar_cursor =
                NaiveDate::from_ymd_opt(app.calendar_cursor.year(), app.calendar_cursor.month(), 1)
                    .unwrap_or(app.calendar_cursor);
        }
        KeyCode::End => {
            let first =
                NaiveDate::from_ymd_opt(app.calendar_cursor.year(), app.calendar_cursor.month(), 1)
                    .unwrap();
            let next_first = if first.month() == 12 {
                NaiveDate::from_ymd_opt(first.year() + 1, 1, 1).unwrap()
            } else {
                NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1).unwrap()
            };
            app.calendar_cursor = next_first.pred_opt().unwrap_or(next_first);
        }
        KeyCode::Enter => {
            let value = app.calendar_cursor.format("%Y-%m-%d").to_string();
            app.apply_filter_input(InputKind::FilterField(idx), &value);
            app.mode = Mode::FilterMenu;
        }
        _ => {}
    }
}

fn handle_confirm_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    match key.code {
        KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::Char('h') | KeyCode::Char('l') => {
            app.confirm_choice = match app.confirm_choice {
                ConfirmChoice::Yes => ConfirmChoice::No,
                ConfirmChoice::No => ConfirmChoice::Yes,
            };
        }
        KeyCode::Char('y') => confirm_toggle_state(app, client, tx),
        KeyCode::Char('n') | KeyCode::Esc => {
            app.mode = Mode::Normal;
            app.status = Some("cancelled".into());
        }
        KeyCode::Enter => match app.confirm_choice {
            ConfirmChoice::Yes => confirm_toggle_state(app, client, tx),
            ConfirmChoice::No => {
                app.mode = Mode::Normal;
                app.status = Some("cancelled".into());
            }
        },
        _ => {}
    }
}

/// Applies the close/reopen mutation and returns to `Mode::Normal`. Shared
/// by the `y` shortcut and Enter-on-Yes in `handle_confirm_key`.
fn confirm_toggle_state(app: &mut App, client: &Provider, tx: &mpsc::UnboundedSender<AppEvent>) {
    app.mode = Mode::Normal;
    let target = match app.selected_issue() {
        Some(i) => match i.state {
            IssueState::Open => IssueState::Closed,
            IssueState::Closed => IssueState::Open,
        },
        None => return,
    };
    let msg = match target {
        IssueState::Closed => "issue closed",
        IssueState::Open => "issue reopened",
    };
    with_issue(app, client, tx, msg, move |c, id| async move {
        c.set_state(&id, target).await
    });
}

/// Spawn a mutation against the selected issue; reports done/failed via `tx`.
fn with_issue<F, Fut>(
    app: &mut App,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
    done_msg: &'static str,
    op: F,
) where
    F: FnOnce(Provider, String) -> Fut + Send + 'static,
    Fut: Future<Output = crate::provider::error::Result<()>> + Send,
{
    let Some(issue) = app.selected_issue() else {
        return;
    };
    let id = issue.id.clone();
    let client = client.clone();
    let tx = tx.clone();
    app.status = Some("working…".into());
    tokio::spawn(async move {
        let msg = match op(client, id).await {
            Ok(()) => AppEvent::MutationDone(done_msg.to_string()),
            Err(e) => AppEvent::MutationFailed(e.to_string()),
        };
        let _ = tx.send(msg);
    });
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_csv_trims_and_drops_empties() {
        assert_eq!(split_csv(" a , b ,, c "), vec!["a", "b", "c"]);
        assert!(split_csv("  ").is_empty());
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    fn picker_test_app() -> App {
        let mut app = App::new(
            "org".into(),
            None,
            false,
            false,
            "{owner}/{repo}#{number}".into(),
        );
        app.start_picker(vec!["alpha".into(), "beta".into(), "gamma".into()], 0);
        app
    }

    #[test]
    fn typing_filters_and_arrows_navigate_filtered_view() {
        let mut app = picker_test_app();
        assert!(picker_common_key(&mut app, key(KeyCode::Char('a')), true));
        // "a" matches alpha/beta/gamma... narrow further.
        assert!(picker_common_key(&mut app, key(KeyCode::Char('m')), true));
        assert_eq!(app.select_filter, "am");
        assert_eq!(app.picker_selected_original(), Some(2)); // gamma

        assert!(picker_common_key(&mut app, key(KeyCode::Backspace), true));
        assert!(picker_common_key(&mut app, key(KeyCode::Down), true));
        assert_eq!(app.select_filter, "a");
        // filter "a" matches all three; Down moved 0 → 1.
        assert_eq!(app.picker_selected_original(), Some(1));
    }

    #[test]
    fn multi_picker_space_toggles_original_index_through_filter() {
        let mut app = picker_test_app();
        app.issue_form = Some(IssueForm::new("alpha".into()));
        app.mode = Mode::IssueFormMulti(3);

        handle_form_multi_key(&mut app, key(KeyCode::Char('g')), 3); // filter → gamma only
        handle_form_multi_key(&mut app, key(KeyCode::Char(' ')), 3); // toggle it
        assert!(
            app.multi_selected.contains(&2),
            "toggle must hit gamma's original index, got {:?}",
            app.multi_selected
        );

        handle_form_multi_key(&mut app, key(KeyCode::Enter), 3);
        assert_eq!(app.mode, Mode::IssueForm);
        assert!(app.issue_form.unwrap().labels.contains(&2));
    }

    #[test]
    fn select_picker_enter_noop_on_no_matches_but_closes_when_empty() {
        let mut app = picker_test_app();
        app.mode = Mode::SelectField(1);
        handle_select_field_key(&mut app, key(KeyCode::Char('z')), 1); // no matches
        handle_select_field_key(&mut app, key(KeyCode::Enter), 1);
        assert_eq!(
            app.mode,
            Mode::SelectField(1),
            "Enter must not pick from nothing"
        );

        app.start_picker(Vec::new(), 0); // truly empty picker
        handle_select_field_key(&mut app, key(KeyCode::Enter), 1);
        assert_eq!(app.mode, Mode::FilterMenu);
    }

    #[test]
    fn select_picker_enter_applies_filtered_pick() {
        let mut app = picker_test_app();
        app.mode = Mode::SelectField(1); // repo filter field
        handle_select_field_key(&mut app, key(KeyCode::Char('b')), 1);
        handle_select_field_key(&mut app, key(KeyCode::Enter), 1);
        assert_eq!(app.filters.repo, "beta");
        assert_eq!(app.mode, Mode::FilterMenu);
    }

    #[test]
    fn multi_filter_picker_space_toggles_and_enter_applies() {
        let mut app = App::new(
            "org".into(),
            None,
            false,
            false,
            "{owner}/{repo}#{number}".into(),
        );
        app.start_picker(vec!["low".into(), "high".into(), "urgent".into()], 0);
        app.mode = Mode::SelectFieldMulti(4);

        handle_select_field_multi_key(&mut app, key(KeyCode::Char(' ')), 4); // low
        handle_select_field_multi_key(&mut app, key(KeyCode::Down), 4);
        handle_select_field_multi_key(&mut app, key(KeyCode::Down), 4);
        handle_select_field_multi_key(&mut app, key(KeyCode::Char(' ')), 4); // urgent
        handle_select_field_multi_key(&mut app, key(KeyCode::Enter), 4);

        assert_eq!(app.filters.priority, vec!["low", "urgent"]);
        assert_eq!(app.mode, Mode::FilterMenu);
    }

    #[test]
    fn multi_filter_picker_empty_selection_clears_filter() {
        let mut app = App::new(
            "org".into(),
            None,
            false,
            false,
            "{owner}/{repo}#{number}".into(),
        );
        app.filters.status = vec!["blocked".into()];
        app.start_picker(vec!["blocked".into(), "in-progress".into()], 0);
        app.multi_selected = [0].into_iter().collect();
        app.mode = Mode::SelectFieldMulti(5);

        handle_select_field_multi_key(&mut app, key(KeyCode::Char(' ')), 5); // untoggle blocked
        handle_select_field_multi_key(&mut app, key(KeyCode::Enter), 5);

        assert!(app.filters.status.is_empty());
        assert_eq!(app.mode, Mode::FilterMenu);
    }

    #[test]
    fn multi_filter_picker_esc_discards_toggles() {
        let mut app = App::new(
            "org".into(),
            None,
            false,
            false,
            "{owner}/{repo}#{number}".into(),
        );
        app.filters.priority = vec!["high".into()];
        app.start_picker(vec!["low".into(), "high".into()], 0);
        app.multi_selected = [1].into_iter().collect();
        app.mode = Mode::SelectFieldMulti(4);

        handle_select_field_multi_key(&mut app, key(KeyCode::Char(' ')), 4); // toggle low on
        handle_select_field_multi_key(&mut app, key(KeyCode::Esc), 4);

        assert_eq!(app.filters.priority, vec!["high"]);
        assert_eq!(app.mode, Mode::FilterMenu);
    }

    fn test_client() -> Provider {
        std::sync::Arc::new(crate::github::Client::new("test-token".into()).unwrap())
    }

    /// Single-repo app with one issue carrying `labels`, selected.
    fn app_with_issue(labels: &[&str]) -> (App, String) {
        use crate::provider::types::{Issue, Label};

        let issue = Issue {
            id: "I_1".into(),
            number: 1,
            title: "t".into(),
            body: String::new(),
            state: IssueState::Open,
            url: "u".into(),
            author: "a".into(),
            assignees: vec![],
            labels: labels
                .iter()
                .map(|n| Label {
                    name: (*n).to_string(),
                    color: String::new(),
                })
                .collect(),
            comment_count: 0,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            closed_at: None,
        };
        let id = issue.id.clone();
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
        app.selected = 1; // 0 = repo header, 1 = the issue
        (app, id)
    }

    fn repo_label(id: &str, name: &str) -> RepoLabel {
        RepoLabel {
            id: id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn label_options_prechecks_current_labels_and_opens_picker() {
        let (mut app, issue_id) = app_with_issue(&["bug", "priority:high"]);
        app.label_pick_issue = Some(issue_id.clone());
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_app_event(
            &mut app,
            AppEvent::LabelOptions {
                issue_id,
                result: Ok(vec![
                    repo_label("L1", "bug"),
                    repo_label("L2", "enhancement"),
                    repo_label("L3", "priority:high"),
                ]),
            },
            &client,
            &tx,
        );

        assert_eq!(app.mode, Mode::LabelsSet);
        assert_eq!(
            app.select_options,
            vec![
                "bug".to_string(),
                "enhancement".to_string(),
                "priority:high".to_string()
            ]
        );
        assert_eq!(app.multi_selected, [0, 2].into_iter().collect());
    }

    #[test]
    fn label_options_stale_when_selection_moved_on() {
        let (mut app, issue_id) = app_with_issue(&["bug"]);
        // Options land after the user already moved off this issue.
        app.label_pick_issue = Some(issue_id.clone());
        app.selected = 0; // header row: selected_issue() is now None
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_app_event(
            &mut app,
            AppEvent::LabelOptions {
                issue_id,
                result: Ok(vec![repo_label("L1", "bug")]),
            },
            &client,
            &tx,
        );

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.label_pick_issue.is_none());
    }

    #[test]
    fn label_options_empty_repo_labels_sets_status() {
        let (mut app, issue_id) = app_with_issue(&[]);
        app.label_pick_issue = Some(issue_id.clone());
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_app_event(
            &mut app,
            AppEvent::LabelOptions {
                issue_id,
                result: Ok(vec![]),
            },
            &client,
            &tx,
        );

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.label_pick_issue.is_none());
        assert_eq!(app.status.as_deref(), Some("no labels on this repo"));
    }

    #[test]
    fn labels_set_esc_discards_toggles() {
        let (mut app, issue_id) = app_with_issue(&["bug"]);
        app.label_pick_issue = Some(issue_id);
        app.start_picker(vec!["bug".into(), "enhancement".into()], 0);
        app.multi_selected = [0].into_iter().collect();
        app.mode = Mode::LabelsSet;
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_labels_set_key(&mut app, key(KeyCode::Char(' ')), &client, &tx); // toggle enhancement on
        handle_labels_set_key(&mut app, key(KeyCode::Esc), &client, &tx);

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.label_pick_issue.is_none());
    }

    #[test]
    fn labels_set_space_toggles_original_index_through_filter() {
        let (mut app, issue_id) = app_with_issue(&[]);
        app.label_pick_issue = Some(issue_id);
        app.start_picker(vec!["alpha".into(), "beta".into(), "gamma".into()], 0);
        app.mode = Mode::LabelsSet;
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_labels_set_key(&mut app, key(KeyCode::Char('g')), &client, &tx); // filter → gamma only
        handle_labels_set_key(&mut app, key(KeyCode::Char(' ')), &client, &tx); // toggle it

        assert!(
            app.multi_selected.contains(&2),
            "toggle must hit gamma's original index, got {:?}",
            app.multi_selected
        );
    }

    #[test]
    fn labels_set_enter_without_target_issue_reports_stale() {
        let mut app = App::new(
            "org".into(),
            None,
            false,
            false,
            "{owner}/{repo}#{number}".into(),
        ); // no data, no selected issue
        app.label_pick_issue = Some("I_ghost".into());
        app.start_picker(vec!["bug".into()], 0);
        app.multi_selected = [0].into_iter().collect();
        app.mode = Mode::LabelsSet;
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_labels_set_key(&mut app, key(KeyCode::Enter), &client, &tx);

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.label_pick_issue.is_none());
        assert_eq!(
            app.status.as_deref(),
            Some("selection changed — labels not set")
        );
    }

    #[test]
    fn comments_refresh_target_is_selected_issue_when_pane_open() {
        let (mut app, issue_id) = app_with_issue(&[]);
        app.detail_open = true;
        assert_eq!(comments_refresh_target(&app), Some(issue_id));
    }

    #[test]
    fn comments_refresh_target_none_when_pane_closed() {
        let (mut app, _issue_id) = app_with_issue(&[]);
        app.detail_open = false;
        assert_eq!(comments_refresh_target(&app), None);
    }

    #[test]
    fn comments_refresh_target_none_on_repo_header() {
        let (mut app, _issue_id) = app_with_issue(&[]);
        app.detail_open = true;
        app.selected = 0; // repo header row
        assert_eq!(comments_refresh_target(&app), None);
    }

    fn comment_editor_test_app() -> App {
        let (mut app, _issue_id) = app_with_issue(&[]);
        app.start_comment_editor();
        app
    }

    #[test]
    fn tab_cycles_comment_focus_editor_save_cancel() {
        let mut app = comment_editor_test_app();
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        assert_eq!(app.comment_focus, CommentFocus::Editor);
        handle_comment_editor_key(&mut app, key(KeyCode::Tab), &client, &tx);
        assert_eq!(app.comment_focus, CommentFocus::Save);
        handle_comment_editor_key(&mut app, key(KeyCode::Tab), &client, &tx);
        assert_eq!(app.comment_focus, CommentFocus::Cancel);
        handle_comment_editor_key(&mut app, key(KeyCode::Tab), &client, &tx);
        assert_eq!(app.comment_focus, CommentFocus::Editor);
    }

    #[test]
    fn back_tab_cycles_comment_focus_in_reverse() {
        let mut app = comment_editor_test_app();
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_comment_editor_key(&mut app, key(KeyCode::BackTab), &client, &tx);
        assert_eq!(app.comment_focus, CommentFocus::Cancel);
    }

    #[test]
    fn enter_on_cancel_focus_discards_and_returns_to_normal() {
        let mut app = comment_editor_test_app();
        app.comment_editor.insert('x');
        app.comment_focus = CommentFocus::Cancel;
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_comment_editor_key(&mut app, key(KeyCode::Enter), &client, &tx);

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.comment_editor.text(), "");
        assert_eq!(app.status.as_deref(), Some("comment discarded"));
    }

    #[test]
    fn typed_chars_only_reach_editor_when_editor_focused() {
        let mut app = comment_editor_test_app();
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        app.comment_focus = CommentFocus::Save;
        handle_comment_editor_key(&mut app, key(KeyCode::Char('x')), &client, &tx);
        assert_eq!(app.comment_editor.text(), "");

        app.comment_focus = CommentFocus::Editor;
        handle_comment_editor_key(&mut app, key(KeyCode::Char('x')), &client, &tx);
        assert_eq!(app.comment_editor.text(), "x");
    }

    #[test]
    fn esc_discards_regardless_of_focus() {
        let mut app = comment_editor_test_app();
        app.comment_editor.insert('x');
        app.comment_focus = CommentFocus::Save;
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();

        handle_comment_editor_key(&mut app, key(KeyCode::Esc), &client, &tx);

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.comment_editor.text(), "");
    }

    fn confirm_test_app() -> (App, Provider, mpsc::UnboundedSender<AppEvent>) {
        let (mut app, _issue_id) = app_with_issue(&[]);
        app.mode = Mode::ConfirmState;
        app.confirm_choice = ConfirmChoice::No;
        let client = test_client();
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();
        (app, client, tx)
    }

    #[test]
    fn confirm_arrow_and_tab_toggle_focus() {
        let (mut app, client, tx) = confirm_test_app();

        handle_confirm_key(&mut app, key(KeyCode::Right), &client, &tx);
        assert_eq!(app.confirm_choice, ConfirmChoice::Yes);
        assert_eq!(
            app.mode,
            Mode::ConfirmState,
            "toggling focus must not close the popup"
        );

        handle_confirm_key(&mut app, key(KeyCode::Left), &client, &tx);
        assert_eq!(app.confirm_choice, ConfirmChoice::No);

        handle_confirm_key(&mut app, key(KeyCode::Tab), &client, &tx);
        assert_eq!(app.confirm_choice, ConfirmChoice::Yes);

        handle_confirm_key(&mut app, key(KeyCode::Char('h')), &client, &tx);
        assert_eq!(app.confirm_choice, ConfirmChoice::No);

        handle_confirm_key(&mut app, key(KeyCode::Char('l')), &client, &tx);
        assert_eq!(app.confirm_choice, ConfirmChoice::Yes);
    }

    #[test]
    fn confirm_enter_on_no_cancels_without_mutating() {
        let (mut app, client, tx) = confirm_test_app();
        let original_state = app.selected_issue().unwrap().state;

        handle_confirm_key(&mut app, key(KeyCode::Enter), &client, &tx);

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.status.as_deref(), Some("cancelled"));
        assert_eq!(app.selected_issue().unwrap().state, original_state);
    }

    #[tokio::test]
    async fn confirm_enter_on_yes_triggers_mutation() {
        let (mut app, client, tx) = confirm_test_app();
        app.confirm_choice = ConfirmChoice::Yes;

        handle_confirm_key(&mut app, key(KeyCode::Enter), &client, &tx);

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.status.as_deref(), Some("working…"));
    }

    #[tokio::test]
    async fn confirm_y_shortcut_triggers_mutation_regardless_of_focus() {
        let (mut app, client, tx) = confirm_test_app();
        assert_eq!(app.confirm_choice, ConfirmChoice::No);

        handle_confirm_key(&mut app, key(KeyCode::Char('y')), &client, &tx);

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.status.as_deref(), Some("working…"));
    }

    #[test]
    fn confirm_n_and_esc_shortcuts_cancel_regardless_of_focus() {
        for code in [KeyCode::Char('n'), KeyCode::Esc] {
            let (mut app, client, tx) = confirm_test_app();
            app.confirm_choice = ConfirmChoice::Yes;

            handle_confirm_key(&mut app, key(code), &client, &tx);

            assert_eq!(app.mode, Mode::Normal);
            assert_eq!(app.status.as_deref(), Some("cancelled"));
        }
    }
}
