use super::super::prelude::*;
use super::detail::{
    detail_jump_end, detail_jump_top, detail_key_fold_all, detail_key_left, detail_key_right,
    detail_key_toggle, detail_page, detail_select, detail_step,
};
use super::harness::{HarnessCtx, open_harness_picker, open_session_picker};

pub(crate) fn handle_normal_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
    hx: &mut HarnessCtx,
) {
    // A stub is an ancestor known only from a descendant (#234, ADR 0009):
    // nothing is loaded to write to, so every write key says so here, once,
    // instead of each arm failing silently.
    if let Some(stub) = app.selected_stub()
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(
            key.code,
            KeyCode::Char('x' | 'e' | 'c' | 'a' | 'l' | 't' | 'p' | 'm' | 'A')
        )
    {
        app.status = Some(format!(
            "#{} isn't loaded, so it's read-only (o opens it)",
            stub.number
        ));
        return;
    }
    match key.code {
        KeyCode::Char('q') => {
            if app.detail.open {
                app.close_detail();
            } else if app.harness.has_running() {
                // Quitting closes the PTYs, which SIGHUPs the children — so
                // say what is about to be lost rather than doing it silently.
                app.confirm_choice = ConfirmChoice::No;
                app.mode = Mode::ConfirmHarness(HarnessConfirm::Quit);
            } else {
                app.should_quit = true;
            }
        }
        // launch a coding harness for the selected issue (#23)
        KeyCode::Char('A') => match app.launch_action(hx.settings.default_harness.as_deref()) {
            LaunchAction::NoIssue => {
                app.status = Some("select an issue to send to a harness".into());
            }
            LaunchAction::Attach(id) => {
                app.harness.attach(id);
                app.mode = Mode::Harness;
            }
            LaunchAction::ConfirmRelaunch(id) => {
                app.confirm_choice = ConfirmChoice::No;
                app.mode = Mode::ConfirmHarness(HarnessConfirm::Relaunch(id));
            }
            LaunchAction::Spawn { issue_ref, harness } => hx.launch(app, &issue_ref, &harness),
            LaunchAction::Pick { .. } => open_harness_picker(app, hx.settings.names()),
        },
        // reattach to a detached session
        KeyCode::Char('Z') => open_session_picker(app),
        KeyCode::Esc if app.detail.open => app.close_detail(),
        // Every key below that has a meaning in both panes acts on the focused
        // pane only (#217, ADR 0006): one branch per key, split on focus, so
        // nothing reaches the pane without focus. In the detail pane Tab /
        // Shift+Tab move header to header; from the list they switch panes.
        KeyCode::Tab | KeyCode::BackTab if app.focus == Focus::Detail => {
            detail_select(app, if key.code == KeyCode::Tab { 1 } else { -1 });
        }
        KeyCode::Tab | KeyCode::BackTab => app.cycle_focus(),
        KeyCode::Char('?') => app.open_help(app.context_topic()),
        // Ctrl+R: the filtered scope — the exact-filtered repo, else the org
        // (#229). A different key from `r`, not a focus guard (ADR 0006).
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if refresh_busy(app) {
                return;
            }
            app.loading = true;
            app.status = Some(match app.filtered_target() {
                FetchTarget::Org => "refreshing org…".into(),
                FetchTarget::Repo(repo) => format!("refreshing repo {repo}…"),
            });
            spawn_filtered_fetch(client, app, tx);
        }
        // r: whatever is selected — the issue, or the repo group (#229).
        KeyCode::Char('r') => match app.focus {
            Focus::Detail if app.runs_showing() => app.runs_refresh(std::time::Instant::now()),
            _ => {
                if refresh_busy(app) {
                    return;
                }
                app.loading = true;
                match app.refresh_target() {
                    RefreshTarget::Issue { id, number } => {
                        app.status = Some(format!("refreshing #{number}…"));
                        spawn_issue_refresh(client, id, number, tx);
                    }
                    RefreshTarget::Repo(repo) => {
                        app.status = Some(format!("refreshing repo {repo}…"));
                        spawn_repo_refresh(client, app, repo, tx);
                    }
                    RefreshTarget::Filtered => {
                        app.status = Some("refreshing…".into());
                        spawn_filtered_fetch(client, app, tx);
                    }
                }
            }
        },
        // flip the lower detail region between comments and runs (#222)
        KeyCode::Char('R') => {
            if !app.runs.supported {
                app.status = Some("workflow runs not supported by this provider".into());
                return;
            }
            if !app.detail.open {
                if let Some(issue_id) = app.enter_detail() {
                    spawn_comments(client, issue_id, tx);
                }
                if !app.detail.open {
                    app.status = Some("select an issue to see its runs".into());
                    return;
                }
            }
            app.toggle_runs_tab(std::time::Instant::now());
        }

        // navigation: ↑/↓ move a row cursor through whichever pane has focus
        // — tree rows, or the body and comments of the detail pane (#217).
        KeyCode::Char('j') | KeyCode::Down => match app.focus {
            Focus::Detail => detail_step(app, 1),
            Focus::List => nav(app, client, tx, |a| a.move_selection(1)),
        },
        KeyCode::Char('k') | KeyCode::Up => match app.focus {
            Focus::Detail => detail_step(app, -1),
            Focus::List => nav(app, client, tx, |a| a.move_selection(-1)),
        },
        KeyCode::PageDown => match app.focus {
            Focus::Detail => detail_page(app, 1),
            Focus::List => nav(app, client, tx, |a| a.move_selection(15)),
        },
        KeyCode::PageUp => match app.focus {
            Focus::Detail => detail_page(app, -1),
            Focus::List => nav(app, client, tx, |a| a.move_selection(-15)),
        },
        KeyCode::Char('g') | KeyCode::Home => match app.focus {
            Focus::Detail => detail_jump_top(app),
            Focus::List => nav(app, client, tx, |a| a.selected = 0),
        },
        KeyCode::Char('G') | KeyCode::End => match app.focus {
            Focus::Detail => detail_jump_end(app),
            Focus::List => nav(app, client, tx, |a| {
                a.selected = a.rows.len().saturating_sub(1);
            }),
        },

        // nesting: ← out one level, → in one level, Space/Enter toggle — the
        // same meaning in the tree and the detail pane (#217, ADR 0006).
        KeyCode::Right => match app.focus {
            Focus::Detail => detail_key_right(app),
            Focus::List => tree_right(app, client, tx),
        },
        KeyCode::Left => match app.focus {
            Focus::Detail => detail_key_left(app),
            Focus::List => nav(app, client, tx, App::tree_left),
        },
        KeyCode::Char(' ') | KeyCode::Enter => match app.focus {
            Focus::Detail => detail_key_toggle(app),
            // On an issue (or a context row, #234) the toggle is `→`; on a
            // header it opens or closes the group.
            Focus::List if !app.selected_is_header() => tree_right(app, client, tx),
            Focus::List => nav(app, client, tx, App::toggle_collapse),
        },
        // `[`/`]`: fold or unfold everything in the focused pane.
        KeyCode::Char('[') | KeyCode::Char(']') => {
            let folded = key.code == KeyCode::Char('[');
            match app.focus {
                Focus::Detail => detail_key_fold_all(app, folded),
                Focus::List => nav(app, client, tx, |a| a.set_all_collapsed(folded)),
            }
        }

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
            // Showing closed issues widens the wanted scope; the auto-widen
            // after this key fetches them when they were never loaded (#230).
            app.state_filter = app.state_filter.next();
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
                .runs_selected_url()
                .or_else(|| app.selected_issue().map(|i| i.url.clone()))
                // A stub's own url (#234); empty for Jira, which falls through.
                .or_else(|| {
                    app.selected_stub()
                        .map(|s| s.url.clone())
                        .filter(|u| !u.is_empty())
                })
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

        // mutations
        KeyCode::Char('c') => {
            // The editor sits under the thread it adds to.
            if app.runs_showing() {
                app.toggle_runs_tab(std::time::Instant::now());
            }
            if let Some(issue_id) = app.start_comment_editor() {
                spawn_comments(client, issue_id, tx);
            }
        }
        // Edit the highlighted detail card: the issue body or a comment. The
        // runs tab highlights neither.
        KeyCode::Char('e') if app.detail.open && !app.runs_showing() => {
            app.start_edit_selected_card();
        }
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
                .zip(app.selected_issue_repo().map(|r| r.repo.clone()));
            if let Some((issue_id, repo)) = target {
                app.picker.label_issue = Some(issue_id.clone());
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
                .zip(app.selected_issue_repo().map(|r| r.repo.clone()));
            if let Some((issue_id, repo)) = target {
                app.picker.priority_issue = Some(issue_id.clone());
                app.status = Some("loading priorities…".into());
                spawn_priority_options(client, app.org.clone(), repo, issue_id, tx);
            }
        }
        KeyCode::Char('n') => {
            if let Some(repo) = app.selected_repo().map(|r| r.repo.clone()) {
                app.open_issue_form(repo.clone());
                spawn_form_options(client, app.org.clone(), repo, tx);
            } else {
                app.status = Some(
                    "no repo selected — filter to a repo or set \"hide empty repos\" to no".into(),
                );
            }
        }
        // move the selected issue to another repo (#135)
        KeyCode::Char('m') => {
            if app.selected_issue().is_some() {
                if !client.supports_move() {
                    app.status = Some("moving issues not supported by this provider".into());
                } else if app.org_repo_names.is_none() {
                    // The loaded repos may be only the filtered one (#230):
                    // ask for the org's list once, then open the picker.
                    app.picker.move_issue = app.selected_issue().map(|i| i.id.clone());
                    app.status = Some("loading repos…".into());
                    spawn_repo_names(client, app.org.clone(), tx);
                } else {
                    open_move_targets(app);
                }
            }
        }
        KeyCode::Char('P') if app.detail.open => {
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

/// `→` (and `Space`/`Enter` on an issue) in the tree (#217): one level in. On
/// an issue that is the detail pane — opened, or just focused when already
/// open so its place is kept — with the comment fetch it may need.
fn tree_right(app: &mut App, client: &Provider, tx: &mpsc::UnboundedSender<AppEvent>) {
    let mut step = TreeRight::Handled;
    nav(app, client, tx, |a| step = a.tree_right());
    if step == TreeRight::EnterDetail
        && let Some(issue_id) = app.enter_detail()
    {
        spawn_comments(client, issue_id, tx);
    }
}

/// A refresh of any scope is already out: say so and start nothing, so two
/// answers cannot race to settle the list (#229).
fn refresh_busy(app: &mut App) -> bool {
    if app.loading {
        app.status = Some("a refresh is already running".into());
    }
    app.loading
}

#[cfg(test)]
mod tests {
    use super::super::testutil::{app_with_issue, key, test_client};
    use super::*;
    use crate::config::builtin_harnesses;

    /// Press `key` in Normal mode. Spawned fetches go nowhere: the receiver
    /// is dropped and the runtime ends with the test.
    fn press(app: &mut App, key: KeyEvent) {
        let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();
        let mut registry = HarnessRegistry::default();
        let settings = HarnessSettings {
            default_harness: None,
            harnesses: builtin_harnesses(),
            workspace_roots: Vec::new(),
            cwd_repo: None,
            cwd: std::path::PathBuf::from("/"),
        };
        let mut hx = HarnessCtx {
            registry: &mut registry,
            settings: &settings,
            tx: &tx,
        };
        handle_normal_key(app, key, &test_client(), &tx, &mut hx);
    }

    fn ctrl_r() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)
    }

    /// One issue whose parent #1 is not loaded, so it is drawn as a stub;
    /// the stub row is selected.
    fn stub_app() -> App {
        use crate::provider::types::{IssueStub, RepoIssues};
        let parent = IssueStub {
            id: "I_1".into(),
            number: 1,
            title: "parent".into(),
            state: IssueState::Open,
            url: "https://github.com/org/r/issues/1".into(),
            repo: "r".into(),
            owner: "org".into(),
        };
        let mut app = App::new("org".into(), None, false, false, String::new());
        app.set_data(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![Issue {
                ancestors: vec![parent],
                ..Issue::fixture("I_5", 5)
            }],
        }]);
        app.selected = 1; // 0 = header, 1 = the stub, 2 = the child
        assert!(app.selected_stub().is_some());
        app
    }

    /// #234: a stub is read-only; every write key says so instead of acting.
    #[tokio::test]
    async fn write_keys_on_a_stub_say_it_is_not_loaded() {
        for c in ['x', 'e', 'c', 'a', 'l', 't', 'p', 'm', 'A'] {
            let mut app = stub_app();
            press(&mut app, key(KeyCode::Char(c)));
            assert_eq!(app.mode, Mode::Normal, "{c} opened something on a stub");
            assert_eq!(
                app.status.as_deref(),
                Some("#1 isn't loaded, so it's read-only (o opens it)"),
                "{c}"
            );
        }
    }

    /// #234: Space on a stub acts on the tree node, never the repo group.
    #[tokio::test]
    async fn space_on_a_stub_never_folds_the_repo_group() {
        let mut app = stub_app();
        press(&mut app, key(KeyCode::Char(' ')));
        assert!(app.collapsed.is_empty(), "the repo group stayed open");
    }

    #[tokio::test]
    async fn r_on_an_issue_refreshes_that_issue() {
        let (mut app, _) = app_with_issue(&[]);
        press(&mut app, key(KeyCode::Char('r')));
        assert!(app.loading);
        assert_eq!(app.status.as_deref(), Some("refreshing #1…"));
    }

    #[tokio::test]
    async fn r_on_a_repo_header_refreshes_that_repo() {
        let (mut app, _) = app_with_issue(&[]);
        app.selected = 0;
        press(&mut app, key(KeyCode::Char('r')));
        assert!(app.loading);
        assert_eq!(app.status.as_deref(), Some("refreshing repo r…"));
    }

    #[tokio::test]
    async fn ctrl_r_refreshes_the_org_unless_a_repo_is_named_exactly() {
        let (mut app, _) = app_with_issue(&[]);
        press(&mut app, ctrl_r());
        assert_eq!(app.status.as_deref(), Some("refreshing org…"));

        app.loading = false;
        app.filters.repo = "R".into();
        press(&mut app, ctrl_r());
        assert_eq!(app.status.as_deref(), Some("refreshing repo r…"));
    }

    #[tokio::test]
    async fn no_refresh_starts_while_one_is_running() {
        let (mut app, _) = app_with_issue(&[]);
        app.loading = true;
        press(&mut app, key(KeyCode::Char('r')));
        assert_eq!(app.status.as_deref(), Some("a refresh is already running"));
        press(&mut app, ctrl_r());
        assert_eq!(app.status.as_deref(), Some("a refresh is already running"));
    }
}
