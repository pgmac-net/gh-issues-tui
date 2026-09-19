//! Work handed to background tasks; results return as `AppEvent`s.

use super::prelude::*;

pub(crate) fn spawn_fetch(client: &Provider, app: &App, tx: &mpsc::UnboundedSender<AppEvent>) {
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

/// Rank any loaded labels that have not been ranked yet (#156). A no-op
/// without a `ranker` (feature off, or no key), when there is nothing new to
/// ask, when a request is already out, and after a failure this session.
pub(crate) fn spawn_label_ranks(
    app: &mut App,
    ranker: Option<&crate::typesafe::Client>,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let Some(ranker) = ranker else { return };
    let Some(labels) = app.begin_rank_inference() else {
        return;
    };
    let ranker = ranker.clone();
    let org = app.org.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = crate::typesafe::resolve(
            &ranker,
            &crate::typesafe::cache::default_path(),
            &org,
            labels,
        )
        .await
        .map_err(|e| e.to_string());
        let _ = tx.send(AppEvent::LabelRanks { org, result });
    });
}

pub(crate) fn spawn_form_options(
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

pub(crate) fn spawn_priority_options(
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

pub(crate) fn spawn_label_options(
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

pub(crate) fn spawn_comments(
    client: &Provider,
    issue_id: String,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let client = client.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = client.comments(&issue_id).await.map_err(|e| e.to_string());
        let _ = tx.send(AppEvent::Comments { issue_id, result });
    });
}

pub(crate) fn spawn_pr_summary(client: &Provider, pr: PrRef, tx: &mpsc::UnboundedSender<AppEvent>) {
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

/// Whether a mutation changed the selected issue's comment thread, and so
/// whether `MutationDone` needs to invalidate the cache and refetch it.
/// Most mutations (state, title, assignees, labels, description) leave the
/// thread untouched — only adding or editing a comment does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommentRefresh {
    Refetch,
    Skip,
}

/// Spawn a mutation against the selected issue; reports done/failed via `tx`.
/// `done_msg` takes `impl Into<String>` rather than `&'static str` so a
/// caller can report a destination or other runtime detail (the move
/// mutation's "moved to {target}") without every other call site needing to
/// change.
pub(crate) fn with_issue<F, Fut>(
    app: &mut App,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
    done_msg: impl Into<String> + Send + 'static,
    comments: CommentRefresh,
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
            Ok(()) => AppEvent::MutationDone {
                msg: done_msg.into(),
                comments,
            },
            Err(e) => AppEvent::MutationFailed(e.to_string()),
        };
        let _ = tx.send(msg);
    });
}
