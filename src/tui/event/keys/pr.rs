use super::super::prelude::*;
use super::shared::*;

pub(crate) fn handle_pr_picker_key(
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
        KeyCode::Enter => match app.picker.selected_original() {
            Some(orig) => {
                let pr = app.pr.links[orig].clone();
                app.open_pr_summary(pr.clone());
                spawn_pr_summary(client, pr, tx);
            }
            None if app.picker.options.is_empty() => app.close_pr_picker(),
            None => {}
        },
        _ => {}
    }
}

/// The PR summary popup's navigable rows at the live terminal width. Read off
/// the same row model the popup draws, so a target's row index is exactly the
/// Hand a URL to the desktop browser, reporting either way on the status line.
fn open_url(app: &mut App, url: &str) {
    match open::that(url) {
        Ok(()) => app.status = Some(format!("opened {url}")),
        Err(e) => app.status = Some(format!("open failed: {e}")),
    }
}

pub(crate) fn handle_pr_summary_key(
    app: &mut App,
    key: KeyEvent,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.close_pr_summary(),
        KeyCode::Char('j') | KeyCode::Down => {
            let max = pr_scroll_max(app);
            app.pr.scroll_by(1, max);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            let max = pr_scroll_max(app);
            app.pr.scroll_by(-1, max);
        }
        KeyCode::PageDown => {
            let max = pr_scroll_max(app);
            app.pr.scroll_by(pr_page_rows(), max);
        }
        KeyCode::PageUp => {
            let max = pr_scroll_max(app);
            app.pr.scroll_by(-pr_page_rows(), max);
        }
        // Home/End move the viewport only — `sel` (the Tab selection) is
        // left alone, so jumping to the top/bottom can never change which
        // row `o`/Enter would open.
        KeyCode::Home | KeyCode::Char('g') => app.pr.scroll_to_top(),
        KeyCode::End | KeyCode::Char('G') => {
            let max = pr_scroll_max(app);
            app.pr.scroll_to_bottom(max);
        }
        // `PrState::select` snaps the scroll to the target's row; `app/`
        // computes no geometry, so the bound is applied here instead.
        KeyCode::Tab => {
            let targets = pr_targets(app);
            app.pr.select(1, &targets);
            let max = pr_scroll_max(app);
            app.pr.clamp_scroll(max);
        }
        KeyCode::BackTab => {
            let targets = pr_targets(app);
            app.pr.select(-1, &targets);
            let max = pr_scroll_max(app);
            app.pr.clamp_scroll(max);
        }
        KeyCode::Char('o') | KeyCode::Enter => {
            // #129: a reference that resolved to an issue moves the selector to
            // it instead of opening a browser — but only when it is actually in
            // the loaded data. Another org, an unloaded repo, or a closed issue
            // not yet fetched all fall back to opening it.
            if let Some(issue) = app.pr_issue_ref() {
                let mut jumped = false;
                if app.org.eq_ignore_ascii_case(&issue.owner) {
                    nav(app, client, tx, |a| {
                        jumped = a.jump_to_ref(Some(&issue.repo), issue.number);
                    });
                }
                if jumped {
                    app.close_pr_summary();
                } else {
                    open_url(app, &issue.url);
                }
                return;
            }
            if let Some(url) = app.pr.selected_url(&pr_targets(app)) {
                open_url(app, &url);
            }
        }
        KeyCode::Char('r') => {
            if let Some(pr) = app.pr.target.clone() {
                app.pr.refresh();
                spawn_pr_summary(client, pr, tx);
            }
        }
        _ => {}
    }
}
