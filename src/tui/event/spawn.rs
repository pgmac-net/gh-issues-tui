//! Work handed to background tasks; results return as `AppEvent`s.

use super::prelude::*;

/// Refetch the whole org: create and org switch, which change which issues
/// exist. At [`App::refresh_scope`], so it never fetches less than is loaded
/// (#226). Becomes the loaded scope when it lands.
pub(crate) fn spawn_fetch(client: &Provider, app: &mut App, tx: &mpsc::UnboundedSender<AppEvent>) {
    let scope = app.refresh_scope();
    app.begin_fetch(Loaded {
        scope: scope.clone(),
        repo: None,
    });
    send_org_fetch(client, app.org.clone(), scope, tx);
}

/// Refetch the filtered scope (#229, #230): the one repo an exact repo filter
/// names, otherwise the org, at [`App::refresh_scope`] (#226). Startup,
/// `Ctrl+R` and auto-widen land here; auto-refresh is [`spawn_org_refresh`].
pub(crate) fn spawn_filtered_fetch(
    client: &Provider,
    app: &mut App,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let wanted = app.wanted();
    let target = Loaded {
        scope: app.refresh_scope(),
        repo: wanted.repo,
    };
    begin(client, app, target, tx);
}

/// Auto-refresh (#226, ADR 0010): the whole org, at a scope covering both
/// what is loaded and what the filters want, so the snapshot stays fresh for
/// the next start even while you sit on one repo.
pub(crate) fn spawn_org_refresh(
    client: &Provider,
    app: &mut App,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let target = Loaded {
        scope: app.refresh_scope(),
        repo: None,
    };
    begin(client, app, target, tx);
}

fn begin(client: &Provider, app: &mut App, target: Loaded, tx: &mpsc::UnboundedSender<AppEvent>) {
    app.begin_fetch(target.clone());
    let org = app.org.clone();
    match target.repo {
        None => send_org_fetch(client, org, target.scope, tx),
        Some(repo) => send_repo_fetch(client, org, repo, target.scope, tx),
    }
}

/// Refetch one repo's issues (#229) for `r` on its header, at the *loaded*
/// scope so the group stays consistent with the rest of the list. Not a
/// change of loaded scope, so no `begin_fetch`.
pub(crate) fn spawn_repo_refresh(
    client: &Provider,
    app: &App,
    repo: String,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    send_repo_fetch(client, app.org.clone(), repo, app.loaded_scope(), tx);
}

/// Refetch the filtered scope when the filters have widened past the loaded
/// data (#230). Called after every key and every event; a no-op unless
/// [`App::widened`] says a fetch is due and nothing is in the way.
pub(crate) fn refetch_if_widened(
    client: &Provider,
    app: &mut App,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    if !app.widened() {
        return;
    }
    app.loading = true;
    // A load of data the list does not hold, not a refresh (#226).
    app.status = Some("filters widened, loading…".into());
    spawn_filtered_fetch(client, app, tx);
}

/// Run one provider call on a background task and send its answer back as
/// the event `wrap` builds (#232). Errors cross the channel as strings, as
/// every fetch event carries them. `call` owns what it needs; anything `wrap`
/// also needs is cloned for it by the caller.
fn spawn_call<T, Fut>(
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
    call: impl FnOnce(Provider) -> Fut + Send + 'static,
    wrap: impl FnOnce(Result<T, String>) -> AppEvent + Send + 'static,
) where
    T: Send + 'static,
    Fut: Future<Output = crate::provider::error::Result<T>> + Send + 'static,
{
    let client = client.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = call(client).await.map_err(|e| e.to_string());
        let _ = tx.send(wrap(result));
    });
}

/// After a refresh covering `scope` settles: fetch the outside sub-issues of
/// the parents there that are short of them (#261, ADR 0013). Nothing when
/// the backend can't, or no parent is short.
pub(crate) fn spawn_sub_issues(
    client: &Provider,
    app: &mut App,
    scope: SubIssueScope,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    if !client.supports_sub_issues() {
        return;
    }
    if let Some(batch) = app.sub_issue_batch(&scope) {
        spawn_sub_issue_batch(client, batch, tx);
    }
}

/// Fetch one batch of outside sub-issues: a first level, or the next one an
/// answer asked for.
pub(crate) fn spawn_sub_issue_batch(
    client: &Provider,
    batch: SubIssueBatch,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let parents = batch.parents.clone();
    spawn_call(
        client,
        tx,
        move |c| async move { c.sub_issues(&parents).await },
        move |result| AppEvent::SubIssues { batch, result },
    );
}

/// Fetch the org's repo names for the move picker (#230).
pub(crate) fn spawn_repo_names(
    client: &Provider,
    org: String,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let key = org.clone();
    spawn_call(
        client,
        tx,
        move |c| async move { c.repo_names(&key).await },
        move |result| AppEvent::RepoNames { org, result },
    );
}

fn send_org_fetch(
    client: &Provider,
    org: String,
    scope: FetchScope,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    spawn_call(
        client,
        tx,
        move |c| async move { c.org_issues(&org, &scope).await },
        AppEvent::Data,
    );
}

fn send_repo_fetch(
    client: &Provider,
    org: String,
    repo: String,
    scope: FetchScope,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let name = repo.clone();
    spawn_call(
        client,
        tx,
        move |c| async move { c.repo_issues(&org, &name, &scope).await },
        move |result| AppEvent::RepoRefreshed { repo, result },
    );
}

/// Refetch one issue (#229). `number` only labels the status line.
pub(crate) fn spawn_issue_refresh(
    client: &Provider,
    id: String,
    number: u64,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let key = id.clone();
    spawn_call(
        client,
        tx,
        move |c| async move { c.issue(&key).await.map(|i| i.map(Box::new)) },
        move |result| AppEvent::IssueRefreshed {
            issue_id: id,
            number,
            result,
        },
    );
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

/// Judge the selected ticket's readiness (#160), if there is anything to ask.
///
/// A no-op without the `send_issue_text` consent, before the comment thread has
/// settled, while a request for that ticket is out, once one has landed, and
/// after a failure this session. Never called from the launch path: the badge is
/// advisory and must not be able to delay or divert a launch.
pub(crate) fn spawn_readiness(
    app: &mut App,
    reader: Option<&crate::typesafe::Client>,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let Some(reader) = reader else { return };
    let Some((issue_id, state, hash)) = app.begin_readiness() else {
        return;
    };
    let reader = reader.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = crate::typesafe::readiness::assess(&reader, state)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(AppEvent::Readiness {
            issue_id,
            hash,
            result,
        });
    });
}

/// Run a semantic search (#158), if there is anything to ask.
///
/// Gated on the `send_issue_text` consent: `None` means `/` stays exactly the
/// substring match it always was. Beyond that, a no-op unless
/// `App::begin_semantic_search` finds unjudged candidates for the current query —
/// so it is safe to call after every key and every event, and it never fires per
/// keystroke while typing, because the text filter only changes on Enter.
pub(crate) fn spawn_semantic_search(
    app: &mut App,
    reader: Option<&crate::typesafe::Client>,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let Some(reader) = reader else { return };
    let Some((generation, query, candidates)) = app.begin_semantic_search() else {
        return;
    };
    let reader = reader.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = crate::typesafe::search::score(&reader, &query, &candidates)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(AppEvent::SemanticSearch { generation, result });
    });
}

pub(crate) fn spawn_form_options(
    client: &Provider,
    org: String,
    repo: String,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let name = repo.clone();
    spawn_call(
        client,
        tx,
        move |c| async move { c.repo_form_options(&org, &name).await },
        move |result| AppEvent::FormOptions { repo, result },
    );
}

/// Fetch a repo's labels for one of the two label pickers. `into` names the
/// event, which is the only difference between the priority and label paths.
fn spawn_repo_labels(
    client: &Provider,
    org: String,
    repo: String,
    issue_id: String,
    tx: &mpsc::UnboundedSender<AppEvent>,
    into: fn(String, Result<Vec<RepoLabel>, String>) -> AppEvent,
) {
    spawn_call(
        client,
        tx,
        move |c| async move { c.repo_labels(&org, &repo).await },
        move |result| into(issue_id, result),
    );
}

pub(crate) fn spawn_priority_options(
    client: &Provider,
    org: String,
    repo: String,
    issue_id: String,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    spawn_repo_labels(client, org, repo, issue_id, tx, |issue_id, result| {
        AppEvent::PriorityOptions { issue_id, result }
    });
}

pub(crate) fn spawn_label_options(
    client: &Provider,
    org: String,
    repo: String,
    issue_id: String,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    spawn_repo_labels(client, org, repo, issue_id, tx, |issue_id, result| {
        AppEvent::LabelOptions { issue_id, result }
    });
}

pub(crate) fn spawn_comments(
    client: &Provider,
    issue_id: String,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let key = issue_id.clone();
    spawn_call(
        client,
        tx,
        move |c| async move { c.comments(&key).await },
        move |result| AppEvent::Comments { issue_id, result },
    );
}

pub(crate) fn spawn_pr_summary(client: &Provider, pr: PrRef, tx: &mpsc::UnboundedSender<AppEvent>) {
    let key = pr.clone();
    spawn_call(
        client,
        tx,
        move |c| async move { c.pull_request(&key).await },
        move |result| AppEvent::PrSummary {
            pr,
            result: Box::new(result),
        },
    );
}

/// Run the workflow-runs fetch the tab asked for (#222). Tagged with the
/// generation it was sent under, so a response overtaken by a selection change
/// or a refresh is dropped when it lands.
pub(crate) fn spawn_runs(
    client: &Provider,
    fetch: RunsFetch,
    tx: &mpsc::UnboundedSender<AppEvent>,
) {
    let client = client.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let RunsFetch {
            issue_id,
            generation,
            work,
            known,
        } = fetch;
        let result = match work {
            RunsWork::Load { text_refs } => {
                crate::runs::fetch::load(client.as_ref(), &issue_id, &text_refs, &known)
                    .await
                    .map_err(|e| e.to_string())
            }
            RunsWork::Poll { prs, hidden } => {
                Ok(crate::runs::fetch::poll(client.as_ref(), prs, hidden, &known).await)
            }
        };
        let _ = tx.send(AppEvent::Runs {
            issue_id,
            generation,
            result: Box::new(result),
        });
    });
}

/// What has to follow a mutation that does not report the changed issue
/// back. Issue-level mutations do report it and go through
/// [`with_issue_patch`] instead, which patches the list in place (#196).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AfterMutation {
    /// A comment was added: bump the list's count, refetch the thread.
    CommentAdded,
    /// A comment was edited: refetch the thread; the count is unchanged.
    CommentEdited,
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
    after: AfterMutation,
    op: F,
) where
    F: FnOnce(Provider, String) -> Fut + Send + 'static,
    Fut: Future<Output = crate::provider::error::Result<()>> + Send,
{
    mutate_selected(app, client, tx, op, move |id, ()| AppEvent::MutationDone {
        msg: done_msg.into(),
        issue_id: Some(id),
        after,
    });
}

/// Spawn an issue-level mutation (state, title, body, assignees, labels)
/// against the selected issue. The provider reports the issue as it stands
/// after the write, and `IssuePatched` replaces that one list entry — no org
/// refetch, which could read a replica that has not seen the write yet.
pub(crate) fn with_issue_patch<F, Fut>(
    app: &mut App,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
    done_msg: impl Into<String> + Send + 'static,
    op: F,
) where
    F: FnOnce(Provider, String) -> Fut + Send + 'static,
    Fut: Future<Output = crate::provider::error::Result<Issue>> + Send,
{
    mutate_selected(app, client, tx, op, move |_, issue| {
        AppEvent::IssuePatched {
            msg: done_msg.into(),
            issue: Box::new(issue),
        }
    });
}

/// Move the selected issue (#226): like [`with_issue_patch`], but the issue
/// that comes back may have a new id and lives in `target`, so it lands as
/// `IssueMoved` and is placed rather than patched. No org refetch.
pub(crate) fn with_issue_move<F, Fut>(
    app: &mut App,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
    done_msg: impl Into<String> + Send + 'static,
    target: String,
    op: F,
) where
    F: FnOnce(Provider, String) -> Fut + Send + 'static,
    Fut: Future<Output = crate::provider::error::Result<Issue>> + Send,
{
    mutate_selected(app, client, tx, op, move |from_id, issue| {
        AppEvent::IssueMoved {
            msg: done_msg.into(),
            from_id,
            target,
            issue: Box::new(issue),
        }
    });
}

/// The shared half of [`with_issue`] and [`with_issue_patch`] (#232): run
/// `op` against the selected issue's id, then report `done(id, value)` or
/// `MutationFailed`. A no-op with nothing selected.
fn mutate_selected<T, F, Fut>(
    app: &mut App,
    client: &Provider,
    tx: &mpsc::UnboundedSender<AppEvent>,
    op: F,
    done: impl FnOnce(String, T) -> AppEvent + Send + 'static,
) where
    T: Send + 'static,
    F: FnOnce(Provider, String) -> Fut + Send + 'static,
    Fut: Future<Output = crate::provider::error::Result<T>> + Send,
{
    let Some(issue) = app.selected_issue() else {
        return;
    };
    let id = issue.id.clone();
    let client = client.clone();
    let tx = tx.clone();
    app.status = Some("working…".into());
    tokio::spawn(async move {
        let msg = match op(client, id.clone()).await {
            Ok(value) => done(id, value),
            Err(e) => AppEvent::MutationFailed(e.to_string()),
        };
        let _ = tx.send(msg);
    });
}
