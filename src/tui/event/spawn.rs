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

/// Rank the repo's own labels for the set-priority picker (#162).
///
/// Separate from `spawn_label_ranks` because it is keypress-driven and asks
/// about a different set: the repo's whole label list, including labels no
/// loaded issue carries. A repo that has just adopted `P0`/`P1`/`P2` has
/// nothing labelled yet, so the background pass has never seen those names
/// and the picker would have nothing to offer.
///
/// `labels` rides along so the answer can build the picker without refetching.
pub(crate) fn spawn_priority_ranks(
    ranker: &crate::typesafe::Client,
    org: String,
    issue_id: String,
    labels: Vec<RepoLabel>,
    ask: Vec<String>,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let ranker = ranker.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let result =
            crate::typesafe::resolve(&ranker, &crate::typesafe::cache::default_path(), &org, ask)
                .await
                .map_err(|e| e.to_string());
        let _ = tx.send(AppEvent::PriorityRanks {
            issue_id,
            labels,
            result,
        });
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
