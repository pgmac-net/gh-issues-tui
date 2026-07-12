use anyhow::Result;
use chrono::{Datelike, NaiveDate, TimeDelta};
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use crate::github::Client;
use crate::github::error::RATE_LIMIT_MSG_PREFIX;
use crate::github::types::{Comment, FormOptions, IssueState, RepoIssues};

use super::app::{App, Focus, ISSUE_FORM_CREATE_ROW, InputKind, IssueForm, Mode, StateFilter};
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
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    client: Client,
    org: String,
    initial_repo: Option<String>,
    include_closed: bool,
    default_collapsed: bool,
    refresh_interval: u64,
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
        theme,
    )
    .await;
    ratatui::restore();
    result
}

#[allow(clippy::too_many_arguments)]
async fn event_loop(
    mut terminal: DefaultTerminal,
    client: Client,
    org: String,
    initial_repo: Option<String>,
    include_closed: bool,
    default_collapsed: bool,
    refresh_interval: u64,
    theme: Theme,
) -> Result<()> {
    let mut app = App::new(org, initial_repo, include_closed, default_collapsed);
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

fn spawn_fetch(client: &Client, app: &App, tx: &mpsc::UnboundedSender<AppEvent>) {
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
    client: &Client,
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

fn spawn_comments(client: &Client, issue_id: String, tx: &mpsc::UnboundedSender<AppEvent>) {
    let client = client.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = client.comments(&issue_id).await.map_err(|e| e.to_string());
        let _ = tx.send(AppEvent::Comments { issue_id, result });
    });
}

/// Run a selection-changing action, then live-update the detail pane: when
/// the split is open and the selected issue changed, reset the pane and
/// fetch the new issue's comments (stale responses are dropped by id in
/// `handle_app_event`). Landing on a repo header just clears the pane.
fn nav(
    app: &mut App,
    client: &Client,
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
    app.detail_scroll = 0;
    app.detail_comments = None;
    if let Some(id) = current {
        spawn_comments(client, id, tx);
    }
}

fn handle_app_event(
    app: &mut App,
    msg: AppEvent,
    client: &Client,
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
                    Ok(c) => app.detail_comments = Some(c),
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
    }
}

fn handle_key(app: &mut App, key: KeyEvent, client: &Client, tx: &mpsc::UnboundedSender<AppEvent>) {
    match app.mode {
        Mode::Normal => handle_normal_key(app, key, client, tx),
        Mode::Input(kind) => handle_input_key(app, key, kind, client, tx),
        Mode::FilterMenu => handle_filter_menu_key(app, key),
        Mode::SelectField(idx) => handle_select_field_key(app, key, idx),
        Mode::Calendar(idx) => handle_calendar_key(app, key, idx),
        Mode::ConfirmState => handle_confirm_key(app, key, client, tx),
        Mode::IssueForm => handle_issue_form_key(app, key, client, tx),
        Mode::IssueFormSelect(idx) => handle_form_select_key(app, key, idx),
        Mode::IssueFormMulti(idx) => handle_form_multi_key(app, key, idx),
        Mode::IssueFormBody => handle_form_body_key(app, key),
        Mode::Help => app.mode = Mode::Normal,
    }
}

fn handle_issue_form_key(
    app: &mut App,
    key: KeyEvent,
    client: &Client,
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
                    app.select_options = form.field_options(idx);
                    app.multi_selected = form.multi_set(idx).clone();
                    app.select_idx = 0;
                    app.mode = Mode::IssueFormMulti(idx);
                }
                _ if IssueForm::is_select_field(idx) => {
                    // "—" (none) is prepended; stored choices are offset by 1.
                    let mut opts = form.field_options(idx);
                    opts.insert(0, "\u{2014}".to_string());
                    app.select_options = opts;
                    app.select_idx = form.get_single(idx).map_or(0, |i| i + 1);
                    app.mode = Mode::IssueFormSelect(idx);
                }
                _ => submit_issue_form(app, client, tx),
            }
        }
        _ => {}
    }
}

fn handle_form_select_key(app: &mut App, key: KeyEvent, idx: usize) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::IssueForm,
        KeyCode::Char('j') | KeyCode::Down => {
            if !app.select_options.is_empty() {
                app.select_idx = (app.select_idx + 1) % app.select_options.len();
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if !app.select_options.is_empty() {
                app.select_idx =
                    (app.select_idx + app.select_options.len() - 1) % app.select_options.len();
            }
        }
        KeyCode::Enter => {
            if let Some(form) = &mut app.issue_form
                && !app.select_options.is_empty()
            {
                // Index 0 is "—" (clear); real options are offset by 1.
                form.set_single(idx, app.select_idx.checked_sub(1));
            }
            app.mode = Mode::IssueForm;
        }
        _ => {}
    }
}

fn handle_form_multi_key(app: &mut App, key: KeyEvent, idx: usize) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::IssueForm, // discard toggles
        KeyCode::Char('j') | KeyCode::Down => {
            if !app.select_options.is_empty() {
                app.select_idx = (app.select_idx + 1) % app.select_options.len();
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if !app.select_options.is_empty() {
                app.select_idx =
                    (app.select_idx + app.select_options.len() - 1) % app.select_options.len();
            }
        }
        KeyCode::Char(' ') => {
            if !app.select_options.is_empty() && !app.multi_selected.remove(&app.select_idx) {
                app.multi_selected.insert(app.select_idx);
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

fn handle_form_body_key(app: &mut App, key: KeyEvent) {
    let Some(form) = &mut app.issue_form else {
        app.mode = Mode::Normal;
        return;
    };
    match key.code {
        KeyCode::Esc => app.mode = Mode::IssueForm, // content kept
        KeyCode::Enter => form.body.newline(),
        KeyCode::Backspace => form.body.backspace(),
        KeyCode::Left => form.body.left(),
        KeyCode::Right => form.body.right(),
        KeyCode::Up => form.body.up(),
        KeyCode::Down => form.body.down(),
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            form.body.insert(c);
        }
        _ => {}
    }
}

fn submit_issue_form(app: &mut App, client: &Client, tx: &mpsc::UnboundedSender<AppEvent>) {
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

fn handle_normal_key(
    app: &mut App,
    key: KeyEvent,
    client: &Client,
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
        KeyCode::Tab | KeyCode::BackTab => app.cycle_focus(),
        KeyCode::Char('?') => app.mode = Mode::Help,
        KeyCode::Char('r') => {
            app.loading = true;
            app.status = Some("reloading…".into());
            spawn_fetch(client, app, tx);
        }

        // navigation
        KeyCode::Char('j') | KeyCode::Down => {
            if app.focus == Focus::Detail {
                app.detail_scroll = app.detail_scroll.saturating_add(1);
            } else {
                nav(app, client, tx, |a| a.move_selection(1));
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if app.focus == Focus::Detail {
                app.detail_scroll = app.detail_scroll.saturating_sub(1);
            } else {
                nav(app, client, tx, |a| a.move_selection(-1));
            }
        }
        KeyCode::PageDown => nav(app, client, tx, |a| a.move_selection(15)),
        KeyCode::PageUp => nav(app, client, tx, |a| a.move_selection(-15)),
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
            if app.selected_issue().is_some() {
                app.input.start("");
                app.mode = Mode::Input(InputKind::Comment);
            }
        }
        KeyCode::Char('x') => {
            if app.selected_issue().is_some() {
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
            if let Some(issue) = app.selected_issue() {
                let current = issue
                    .labels
                    .iter()
                    .map(|l| l.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                app.input.start(&current);
                app.mode = Mode::Input(InputKind::Labels);
            }
        }
        KeyCode::Char('t') => {
            if let Some(issue) = app.selected_issue() {
                let title = issue.title.clone();
                app.input.start(&title);
                app.mode = Mode::Input(InputKind::Title);
            }
        }
        KeyCode::Char('n') => {
            if let Some(repo) = app.selected_repo().map(|r| r.repo.clone()) {
                app.open_issue_form(repo.clone());
                spawn_form_options(client, app.org.clone(), repo, tx);
            }
        }
        _ => {}
    }
}

fn handle_input_key(
    app: &mut App,
    key: KeyEvent,
    kind: InputKind,
    client: &Client,
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
        KeyCode::Left => app.input.left(),
        KeyCode::Right => app.input.right(),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.input.start("");
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
    client: &Client,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    match kind {
        InputKind::Search | InputKind::FilterField(_) => {
            app.apply_filter_input(kind, &value);
            if matches!(kind, InputKind::FilterField(_)) {
                app.mode = Mode::FilterMenu;
            }
        }
        InputKind::Comment => {
            if value.trim().is_empty() {
                app.status = Some("empty comment discarded".into());
                return;
            }
            with_issue(app, client, tx, "comment added", move |c, id| async move {
                c.add_comment(&id, &value).await
            });
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
        InputKind::Labels => {
            let names = split_csv(&value);
            let (org, repo) = match app.selected_repo() {
                Some(r) => (app.org.clone(), r.repo.clone()),
                None => return,
            };
            with_issue(app, client, tx, "labels updated", move |c, id| async move {
                c.set_labels(&id, &repo, &org, &names).await
            });
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
            app.filters.clear();
            app.rebuild_rows();
            app.expand_single_visible();
        }
        KeyCode::Enter => {
            let idx = app.filter_menu_idx;
            if App::is_select_field(idx) {
                app.select_options = app.compute_select_options(idx);
                let current = app.current_filter_value(idx);
                app.select_idx = app
                    .select_options
                    .iter()
                    .position(|v| v == &current)
                    .unwrap_or(0);
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
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::FilterMenu,
        KeyCode::Char('j') | KeyCode::Down => {
            if !app.select_options.is_empty() {
                app.select_idx = (app.select_idx + 1) % app.select_options.len();
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if !app.select_options.is_empty() {
                app.select_idx =
                    (app.select_idx + app.select_options.len() - 1) % app.select_options.len();
            }
        }
        KeyCode::Home | KeyCode::Char('g') => app.select_idx = 0,
        KeyCode::End | KeyCode::Char('G') => {
            app.select_idx = app.select_options.len().saturating_sub(1);
        }
        KeyCode::Enter => {
            if app.select_options.is_empty() {
                app.mode = Mode::FilterMenu;
                return;
            }
            let raw = app.select_options[app.select_idx].clone();
            let value = if raw == "\u{2014}" {
                String::new()
            } else {
                raw
            };
            app.apply_filter_input(InputKind::FilterField(idx), &value);
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
    client: &Client,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    app.mode = Mode::Normal;
    if key.code != KeyCode::Char('y') {
        app.status = Some("cancelled".into());
        return;
    }
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
    client: &Client,
    tx: &mpsc::UnboundedSender<AppEvent>,
    done_msg: &'static str,
    op: F,
) where
    F: FnOnce(Client, String) -> Fut + Send + 'static,
    Fut: Future<Output = crate::github::error::Result<()>> + Send,
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
}
