//! Fixtures shared by the key-handler tests.

use super::super::prelude::*;
use crate::provider::types::{RepoIssues, RepoLabel};

pub(crate) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

pub(crate) fn picker_test_app() -> App {
    let mut app = App::new(
        "org".into(),
        None,
        false,
        false,
        "{owner}/{repo}#{number}".into(),
    );
    app.picker
        .start(vec!["alpha".into(), "beta".into(), "gamma".into()], 0);
    app
}

pub(crate) fn issue_form_test_app() -> App {
    let mut app = App::new(
        "org".into(),
        None,
        false,
        false,
        "{owner}/{repo}#{number}".into(),
    );
    app.issue_form = Some(IssueForm::new("alpha".into()));
    app.mode = Mode::IssueForm;
    app
}

pub(crate) fn test_client() -> Provider {
    std::sync::Arc::new(crate::github::Client::new("test-token".into()).unwrap())
}

/// Single-repo app with one issue carrying `labels`, selected.
pub(crate) fn app_with_issue(labels: &[&str]) -> (App, String) {
    use crate::provider::types::{Issue, Label};

    let issue = Issue {
        labels: labels
            .iter()
            .map(|n| Label {
                name: (*n).to_string(),
                ..Default::default()
            })
            .collect(),
        ..Issue::fixture("I_1", 1)
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

pub(crate) fn repo_label(id: &str, name: &str) -> RepoLabel {
    RepoLabel {
        id: id.into(),
        name: name.into(),
    }
}

pub(crate) fn comment_editor_test_app() -> App {
    let (mut app, _issue_id) = app_with_issue(&[]);
    app.start_comment_editor();
    app
}

pub(crate) fn confirm_test_app() -> (App, Provider, mpsc::UnboundedSender<AppEvent>) {
    let (mut app, _issue_id) = app_with_issue(&[]);
    app.mode = Mode::ConfirmState;
    app.confirm_choice = ConfirmChoice::No;
    let client = test_client();
    let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();
    (app, client, tx)
}
