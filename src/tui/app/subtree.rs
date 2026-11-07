//! The sub-issue tree inside each repo group (#234, ADR 0009).
//!
//! A group is a forest: the issues that pass the filters, nested under their
//! parents, plus every ancestor they need to keep their place. An ancestor
//! that is not itself a visible issue of the group (filtered out, in another
//! repo, or not loaded) is a *context row*: dim, never counted. When only its
//! stub is known (from a descendant's `ancestors`) it is read-only.
//!
//! Collapse state is keyed by issue id (`App.collapsed_issues`), like repo
//! collapse by name, so it survives refreshes and filter changes.

use std::collections::{HashMap, HashSet};

use super::prelude::*;
use super::rows::TreeRight;
use crate::provider::types::IssueStub;

/// A node's identity while a group's forest is built. A visible issue is keyed
/// by its position, so no two rows ever merge, even if ids repeat; an
/// ancestor that is not a visible issue is keyed by its id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Visible(usize),
    Ancestor(String),
}

/// One node of a group's forest while it is built.
struct Node {
    kind: NodeKind,
    /// Its issue id: what collapse state is keyed by.
    id: String,
    parent: Option<Key>,
    children: Vec<Key>,
    /// Sort position: the earliest visible issue at or below this node, so an
    /// ancestor sorts where its first descendant would have.
    order: usize,
}

enum NodeKind {
    /// A visible issue of this group (index into its issues).
    Issue(usize),
    Context(ContextRef),
}

/// Every loaded issue's position, by id.
pub(super) fn loaded_index(repos: &[RepoIssues]) -> HashMap<&str, (usize, usize)> {
    repos
        .iter()
        .enumerate()
        .flat_map(|(ri, r)| {
            r.issues
                .iter()
                .enumerate()
                .map(move |(ii, i)| (i.id.as_str(), (ri, ii)))
        })
        .collect()
}

/// A loaded sub-issue from another repo that hangs under a visible issue of a
/// group (#260, ADR 0012): the *anchor*, `anchor_level` steps up its
/// `ancestors`. Drawn there as a context row; its home row stays at home.
#[derive(Debug, Clone, Copy)]
pub(super) struct Foreign {
    repo_idx: usize,
    issue_idx: usize,
    anchor_level: usize,
}

/// Where every loaded issue sorts across all repos under the active sort,
/// indexed `[repo][issue]`. A cross-repo child is ranked on the same scale as
/// its parent's own children, so the two interleave. Ties keep repo order, and
/// so each repo's own order is unchanged.
pub(super) fn global_rank(repos: &[RepoIssues], key: SortKey, descending: bool) -> Vec<Vec<usize>> {
    let mut all: Vec<(usize, usize)> = repos
        .iter()
        .enumerate()
        .flat_map(|(ri, r)| (0..r.issues.len()).map(move |ii| (ri, ii)))
        .collect();
    all.sort_by(|a, b| {
        cmp_issues(
            &repos[a.0].issues[a.1],
            &repos[b.0].issues[b.1],
            key,
            descending,
        )
    });
    let mut rank: Vec<Vec<usize>> = repos.iter().map(|r| vec![0; r.issues.len()]).collect();
    for (pos, (ri, ii)) in all.into_iter().enumerate() {
        rank[ri][ii] = pos;
    }
    rank
}

/// For each repo, the other repos' passing issues that hang under one of its
/// passing issues. `passing[ri]` holds the indices of repo `ri`'s issues that
/// pass every filter except the repo filter, which is what makes a child
/// from a repo that is filtered out still show under its parent. Only a
/// passing issue anchors, so a context row never pulls in descendants.
pub(super) fn cross_repo_links(repos: &[RepoIssues], passing: &[Vec<usize>]) -> Vec<Vec<Foreign>> {
    let mut owner: HashMap<&str, usize> = HashMap::new();
    for (ri, idxs) in passing.iter().enumerate() {
        for &ii in idxs {
            owner.entry(repos[ri].issues[ii].id.as_str()).or_insert(ri);
        }
    }
    let mut out: Vec<Vec<Foreign>> = vec![Vec::new(); repos.len()];
    for (rj, idxs) in passing.iter().enumerate() {
        for &jj in idxs {
            let issue = &repos[rj].issues[jj];
            // An issue anchors once per group: at its nearest ancestor there.
            let mut anchored: Vec<usize> = Vec::new();
            for (level, anc) in issue.ancestors.iter().enumerate() {
                let Some(&ri) = owner.get(anc.id.as_str()) else {
                    continue;
                };
                if ri != rj && !anchored.contains(&ri) {
                    anchored.push(ri);
                    out[ri].push(Foreign {
                        repo_idx: rj,
                        issue_idx: jj,
                        anchor_level: level,
                    });
                }
            }
        }
    }
    out
}

/// What linking a node to its ancestors reads: the repos, which loaded issue
/// an id is, and which visible issue an id names (the first one wins).
struct Linker<'a> {
    repos: &'a [RepoIssues],
    loaded: &'a HashMap<&'a str, (usize, usize)>,
    by_id: HashMap<&'a str, Key>,
}

impl Linker<'_> {
    /// Link `start` to its first `levels` ancestors, adding any not already
    /// present as a context node. `from` is the issue `start` stands for, which
    /// an unloaded ancestor's stub is read back from. `order` is its sort
    /// rank: an ancestor sorts where its first descendant would.
    fn link(
        &self,
        nodes: &mut HashMap<Key, Node>,
        start: Key,
        from: (usize, usize),
        levels: usize,
        order: usize,
    ) {
        let ancestors = &self.repos[from.0].issues[from.1].ancestors;
        let mut child = start;
        for (level, stub) in ancestors.iter().enumerate().take(levels) {
            let parent_key = self
                .by_id
                .get(stub.id.as_str())
                .cloned()
                .unwrap_or_else(|| Key::Ancestor(stub.id.clone()));
            if let Some(node) = nodes.get_mut(&child) {
                node.parent.get_or_insert_with(|| parent_key.clone());
            }
            let parent = nodes.entry(parent_key.clone()).or_insert_with(|| Node {
                kind: NodeKind::Context(match self.loaded.get(stub.id.as_str()) {
                    Some(&(repo_idx, issue_idx)) => ContextRef::Loaded {
                        repo_idx,
                        issue_idx,
                    },
                    None => ContextRef::Stub {
                        repo_idx: from.0,
                        issue_idx: from.1,
                        level,
                    },
                }),
                id: stub.id.clone(),
                parent: None,
                children: Vec::new(),
                order,
            });
            parent.order = parent.order.min(order);
            if !parent.children.contains(&child) {
                parent.children.push(child.clone());
            }
            child = parent_key;
        }
    }
}

/// The rows below repo group `ri`'s header: its `visible` issues (in sort
/// order) as a forest, with collapsed nodes' descendants left out. `foreign`
/// are the other repos' sub-issues that hang under them (#260) and `rank`
/// puts both on one sort scale.
pub(super) fn group_rows(
    repos: &[RepoIssues],
    ri: usize,
    visible: &[usize],
    loaded: &HashMap<&str, (usize, usize)>,
    collapsed: &HashSet<String>,
    foreign: &[Foreign],
    rank: &[Vec<usize>],
) -> Vec<Row> {
    let issues = &repos[ri].issues;
    let mut nodes: HashMap<Key, Node> = HashMap::new();
    // Which visible issue an id names, for linking: the first one wins.
    let mut by_id: HashMap<&str, Key> = HashMap::new();
    for (pos, &ii) in visible.iter().enumerate() {
        by_id
            .entry(issues[ii].id.as_str())
            .or_insert(Key::Visible(pos));
        nodes.insert(
            Key::Visible(pos),
            Node {
                kind: NodeKind::Issue(ii),
                id: issues[ii].id.clone(),
                parent: None,
                children: Vec::new(),
                order: rank[ri][ii],
            },
        );
    }
    let linker = Linker {
        repos,
        loaded,
        by_id,
    };
    // Walk each visible issue's ancestry, linking child to parent and adding
    // any ancestor not already present as a context node.
    for (pos, &ii) in visible.iter().enumerate() {
        let levels = issues[ii].ancestors.len();
        linker.link(
            &mut nodes,
            Key::Visible(pos),
            (ri, ii),
            levels,
            rank[ri][ii],
        );
    }
    // Then each other-repo sub-issue, up to and including its anchor (#260).
    for f in foreign {
        let issue = &repos[f.repo_idx].issues[f.issue_idx];
        // An id that repeats a visible issue's would merge into it: skip.
        if linker.by_id.contains_key(issue.id.as_str()) {
            continue;
        }
        let start = Key::Ancestor(issue.id.clone());
        let order = rank[f.repo_idx][f.issue_idx];
        let node = nodes.entry(start.clone()).or_insert_with(|| Node {
            kind: NodeKind::Context(ContextRef::Loaded {
                repo_idx: f.repo_idx,
                issue_idx: f.issue_idx,
            }),
            id: issue.id.clone(),
            parent: None,
            children: Vec::new(),
            order,
        });
        node.order = node.order.min(order);
        linker.link(
            &mut nodes,
            start,
            (f.repo_idx, f.issue_idx),
            f.anchor_level + 1,
            order,
        );
    }

    let mut roots: Vec<&Key> = nodes
        .iter()
        .filter(|(_, n)| n.parent.is_none())
        .map(|(k, _)| k)
        .collect();
    roots.sort_by_key(|k| nodes[*k].order);

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for root in roots {
        emit(root, 0, ri, &nodes, collapsed, &mut seen, &mut out);
    }
    out
}

/// Push `key` and, unless it is collapsed, its children in sort order. `seen`
/// guards against a malformed cycle in the data looping forever.
fn emit(
    key: &Key,
    depth: u8,
    ri: usize,
    nodes: &HashMap<Key, Node>,
    collapsed: &HashSet<String>,
    seen: &mut HashSet<Key>,
    out: &mut Vec<Row>,
) {
    if !seen.insert(key.clone()) {
        return;
    }
    let node = &nodes[key];
    let branch = !node.children.is_empty();
    out.push(match node.kind {
        NodeKind::Issue(issue_idx) => Row::Issue {
            repo_idx: ri,
            issue_idx,
            depth,
            branch,
        },
        NodeKind::Context(of) => Row::Context {
            repo_idx: ri,
            depth,
            branch,
            of,
        },
    });
    if branch && !collapsed.contains(&node.id) {
        let mut children: Vec<&Key> = node.children.iter().collect();
        children.sort_by_key(|c| nodes[*c].order);
        for child in children {
            emit(
                child,
                depth.saturating_add(1),
                ri,
                nodes,
                collapsed,
                seen,
                out,
            );
        }
    }
}

impl App {
    /// The issue a row stands for, when it is loaded: a visible issue, or a
    /// context row for a loaded one. `None` for a header or a stub.
    pub(super) fn row_issue(&self, row: &Row) -> Option<&Issue> {
        let (repo_idx, issue_idx) = match row {
            Row::Issue {
                repo_idx,
                issue_idx,
                ..
            } => (*repo_idx, *issue_idx),
            Row::Context {
                of:
                    ContextRef::Loaded {
                        repo_idx,
                        issue_idx,
                    },
                ..
            } => (*repo_idx, *issue_idx),
            _ => return None,
        };
        self.repos.get(repo_idx)?.issues.get(issue_idx)
    }

    /// The loaded issue a context row stands for (`None` for a stub, or a
    /// row that is not a context row).
    pub fn context_issue(&self, row: &Row) -> Option<&Issue> {
        match row {
            Row::Context { .. } => self.row_issue(row),
            _ => None,
        }
    }

    /// The repo group a loaded issue lives in.
    pub fn issue_repo(&self, id: &str) -> Option<&str> {
        self.repos
            .iter()
            .find(|r| r.issues.iter().any(|i| i.id == id))
            .map(|r| r.repo.as_str())
    }

    /// The stub a context row stands for, when its issue is not loaded.
    pub fn row_stub(&self, row: &Row) -> Option<&IssueStub> {
        match row {
            Row::Context {
                of:
                    ContextRef::Stub {
                        repo_idx,
                        issue_idx,
                        level,
                    },
                ..
            } => self
                .repos
                .get(*repo_idx)?
                .issues
                .get(*issue_idx)?
                .ancestors
                .get(*level),
            _ => None,
        }
    }

    /// The id of whatever issue a row stands for, loaded or not.
    pub(super) fn row_issue_id(&self, row: &Row) -> Option<&str> {
        self.row_issue(row)
            .map(|i| i.id.as_str())
            .or_else(|| self.row_stub(row).map(|s| s.id.as_str()))
    }

    /// The selected row's stub: a context row whose issue is not loaded.
    /// Read-only: write keys report it instead of acting (#234).
    pub fn selected_stub(&self) -> Option<&IssueStub> {
        self.rows.get(self.selected).and_then(|r| self.row_stub(r))
    }

    /// Whether the selection is on a repo header (or nothing).
    pub fn selected_is_header(&self) -> bool {
        !matches!(
            self.rows.get(self.selected),
            Some(Row::Issue { .. } | Row::Context { .. })
        )
    }

    /// The selected tree node: its depth, whether it has children here, and
    /// its issue id.
    fn selected_node(&self) -> Option<(u8, bool, String)> {
        let row = self.rows.get(self.selected)?;
        let (depth, branch) = match row {
            Row::Issue { depth, branch, .. } | Row::Context { depth, branch, .. } => {
                (*depth, *branch)
            }
            Row::RepoHeader { .. } => return None,
        };
        Some((depth, branch, self.row_issue_id(row)?.to_string()))
    }

    /// `←` on an issue or context row: collapse an expanded branch, else go
    /// to the parent row, else (top level) to the repo header.
    pub(super) fn tree_left_node(&mut self) {
        let Some((depth, branch, id)) = self.selected_node() else {
            return;
        };
        if branch && !self.collapsed_issues.contains(&id) {
            self.collapsed_issues.insert(id.clone());
            self.rebuild_rows();
            self.reselect(Some(id));
        } else if depth > 0 {
            self.select_parent_row(depth);
        } else {
            self.select_repo_header();
        }
    }

    /// `→`/`Space`/`Enter` on an issue or context row: expand a collapsed
    /// branch and report it handled; anything else enters the detail pane.
    pub(super) fn tree_right_node(&mut self) -> TreeRight {
        match self.selected_node() {
            Some((_, true, id)) if self.collapsed_issues.contains(&id) => {
                self.collapsed_issues.remove(&id);
                self.rebuild_rows();
                self.reselect(Some(id));
                TreeRight::Handled
            }
            // A context row from another repo is a link: go to its home row
            // (#260, ADR 0012). The detail pane opens from there.
            Some(_) => match self.link_home() {
                Some((repo, number)) if self.jump_to_ref(Some(&repo), number) => TreeRight::Handled,
                _ => TreeRight::EnterDetail,
            },
            None => TreeRight::Handled,
        }
    }

    /// The repo and number of the home row the selected row links to: a
    /// loaded context row whose issue lives in another repo than the group it
    /// is drawn in. `None` for anything else, which keeps its own behaviour.
    fn link_home(&self) -> Option<(String, u64)> {
        let Row::Context {
            repo_idx,
            of:
                ContextRef::Loaded {
                    repo_idx: home,
                    issue_idx,
                },
            ..
        } = self.rows.get(self.selected)?
        else {
            return None;
        };
        if home == repo_idx {
            return None;
        }
        let repo = self.repos.get(*home)?;
        Some((repo.repo.clone(), repo.issues.get(*issue_idx)?.number))
    }

    /// Move up to the nearest row one level shallower: the parent.
    fn select_parent_row(&mut self, depth: u8) {
        if let Some(idx) = self.rows[..self.selected].iter().rposition(|r| {
            matches!(r, Row::Issue { depth: d, .. } | Row::Context { depth: d, .. } if *d + 1 == depth)
        }) {
            self.selected = idx;
        }
    }

    /// Every issue id that is some loaded issue's ancestor: the branches `[`
    /// collapses.
    pub(super) fn all_branch_ids(&self) -> HashSet<String> {
        self.repos
            .iter()
            .flat_map(|r| r.issues.iter())
            .flat_map(|i| i.ancestors.iter().map(|a| a.id.clone()))
            .collect()
    }

    /// Expand every collapsed ancestor of the issue `id`, so its row exists.
    pub(super) fn reveal_issue(&mut self, id: &str) {
        let ancestors: Vec<String> = self
            .repos
            .iter()
            .flat_map(|r| r.issues.iter())
            .find(|i| i.id == id)
            .map(|i| i.ancestors.iter().map(|a| a.id.clone()).collect())
            .unwrap_or_default();
        for a in ancestors {
            self.collapsed_issues.remove(&a);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stub(id: &str, number: u64, repo: &str) -> IssueStub {
        IssueStub {
            id: id.into(),
            number,
            title: format!("t{number}"),
            state: IssueState::Open,
            url: format!("https://x/{repo}/issues/{number}"),
            repo: repo.into(),
            owner: "org".into(),
        }
    }

    /// An issue with `ancestors` (nearest first). Higher numbers are newer, so
    /// the default updated-descending sort lists them first.
    fn issue(number: u64, ancestors: Vec<IssueStub>) -> Issue {
        let base: DateTime<Utc> = "2026-01-01T00:00:00Z".parse().unwrap();
        Issue {
            ancestors,
            updated_at: base + chrono::TimeDelta::seconds(number as i64),
            ..Issue::fixture(&format!("I_{number}"), number)
        }
    }

    fn repo(name: &str, issues: Vec<Issue>) -> RepoIssues {
        RepoIssues {
            repo: name.into(),
            repo_url: String::new(),
            issues,
        }
    }

    fn app(repos: Vec<RepoIssues>) -> App {
        let mut app = App::new("org".into(), None, false, false, String::new());
        app.set_data(repos);
        app
    }

    /// The rows as short strings: `H repo`, `I<depth> #n`, `C<depth> #n`
    /// (loaded context) or `S<depth> #n` (stub), with `+` on a branch.
    fn shape(app: &App) -> Vec<String> {
        app.rows
            .iter()
            .map(|row| {
                let num = |r: &Row| {
                    app.row_issue(r)
                        .map(|i| i.number)
                        .or_else(|| app.row_stub(r).map(|s| s.number))
                        .unwrap()
                };
                match row {
                    Row::RepoHeader { repo_idx } => format!("H {}", app.repos[*repo_idx].repo),
                    Row::Issue { depth, branch, .. } => {
                        format!("I{depth} #{}{}", num(row), if *branch { "+" } else { "" })
                    }
                    Row::Context {
                        depth, branch, of, ..
                    } => {
                        let k = if matches!(of, ContextRef::Stub { .. }) {
                            "S"
                        } else {
                            "C"
                        };
                        format!("{k}{depth} #{}{}", num(row), if *branch { "+" } else { "" })
                    }
                }
            })
            .collect()
    }

    fn select(app: &mut App, number: u64) {
        app.selected = app
            .rows
            .iter()
            .position(|r| {
                matches!(r, Row::Issue { .. }) && app.row_issue(r).unwrap().number == number
            })
            .unwrap();
    }

    /// #227 with three children, #63 with none: children sit under their parent.
    fn family() -> App {
        let p = stub("I_227", 227, "r");
        app(vec![repo(
            "r",
            vec![
                issue(227, vec![]),
                issue(229, vec![p.clone()]),
                issue(230, vec![p.clone()]),
                issue(231, vec![p]),
                issue(63, vec![]),
            ],
        )])
    }

    #[test]
    fn children_nest_under_their_parent_in_sort_order() {
        assert_eq!(
            shape(&family()),
            vec!["H r", "I0 #227+", "I1 #231", "I1 #230", "I1 #229", "I0 #63"]
        );
    }

    #[test]
    fn a_filtered_out_parent_stays_as_a_context_row_and_is_not_counted() {
        let mut a = family();
        // Every issue but the parent matches the text filter.
        for i in &mut a.repos[0].issues {
            i.title = if i.number == 227 { "parent" } else { "match" }.into();
        }
        a.filters.text = "match".into();
        a.rebuild_rows();
        assert_eq!(
            shape(&a),
            vec!["H r", "C0 #227+", "I1 #231", "I1 #230", "I1 #229", "I0 #63"]
        );
        assert_eq!(a.repo_visible_count(0), 4, "the context row is not counted");
        assert_eq!(a.filtered_issue_count(), 4);
    }

    #[test]
    fn an_unloaded_parent_is_a_read_only_stub() {
        let a = app(vec![repo("r", vec![issue(5, vec![stub("I_1", 1, "r")])])]);
        assert_eq!(shape(&a), vec!["H r", "S0 #1+", "I1 #5"]);
        let mut a = a;
        a.selected = 1;
        assert_eq!(a.selected_stub().map(|s| s.number), Some(1));
        assert!(a.selected_issue().is_none(), "nothing loaded to act on");
        assert!(!a.selected_is_header());
    }

    /// `api#10` with its sub-issue `web#20` in another repo.
    fn cross() -> App {
        app(vec![
            repo("api", vec![issue(10, vec![])]),
            repo("web", vec![issue(20, vec![stub("I_10", 10, "api")])]),
        ])
    }

    /// Select the context row for `number` in repo group `group`.
    fn select_ctx(app: &mut App, group: &str, number: u64) {
        app.selected = app
            .rows
            .iter()
            .position(|r| {
                matches!(r, Row::Context { repo_idx, .. } if app.repos[*repo_idx].repo == group)
                    && app.row_issue(r).is_some_and(|i| i.number == number)
            })
            .unwrap();
    }

    #[test]
    fn a_sub_issue_in_another_repo_shows_under_its_parent_and_at_home() {
        let a = cross();
        assert_eq!(
            shape(&a),
            vec!["H api", "I0 #10+", "C1 #20", "H web", "C0 #10+", "I1 #20"],
            "web#20 is dim under api#10 and a normal row in web"
        );
        assert_eq!(a.repo_visible_count(0), 1, "the dim child is not counted");
        assert_eq!(a.filtered_issue_count(), 2);
    }

    #[test]
    fn the_repo_filter_does_not_hide_the_dim_child() {
        let mut a = cross();
        a.filters.repo = "api".into();
        a.rebuild_rows();
        assert_eq!(shape(&a), vec!["H api", "I0 #10+", "C1 #20"]);
    }

    #[test]
    fn the_other_filters_hide_the_dim_child() {
        let mut a = cross();
        a.repos[0].issues[0].title = "keep".into();
        a.repos[1].issues[0].title = "drop".into();
        a.filters.text = "keep".into();
        a.rebuild_rows();
        assert_eq!(shape(&a), vec!["H api", "I0 #10"]);
    }

    #[test]
    fn a_context_row_does_not_anchor_children() {
        // An epic in api with a child in web and another in ops.
        let epic = stub("I_10", 10, "api");
        let a = app(vec![
            repo("api", vec![issue(10, vec![])]),
            repo("web", vec![issue(20, vec![epic.clone()])]),
            repo("ops", vec![issue(30, vec![epic])]),
        ]);
        assert_eq!(
            shape(&a),
            vec![
                "H api", "I0 #10+", "C1 #30", "C1 #20", "H web", "C0 #10+", "I1 #20", "H ops",
                "C0 #10+", "I1 #30"
            ],
            "ops#30 is not pulled into web through the epic's context row"
        );
    }

    #[test]
    fn a_filtered_out_parent_shows_no_other_repo_children() {
        let mut a = cross();
        a.repos[0].issues[0].title = "drop".into();
        a.repos[1].issues[0].title = "keep".into();
        a.filters.text = "keep".into();
        a.rebuild_rows();
        assert_eq!(
            shape(&a),
            vec!["H web", "C0 #10+", "I1 #20"],
            "api has nothing visible, so its group is not brought back"
        );
    }

    /// api#1 > web#2 > web#3 > ops#4, one repo per level change.
    fn chain() -> App {
        let s1 = stub("I_1", 1, "api");
        let s2 = stub("I_2", 2, "web");
        let s3 = stub("I_3", 3, "web");
        app(vec![
            repo("api", vec![issue(1, vec![])]),
            repo(
                "web",
                vec![
                    issue(2, vec![s1.clone()]),
                    issue(3, vec![s2.clone(), s1.clone()]),
                ],
            ),
            repo("ops", vec![issue(4, vec![s3, s2, s1])]),
        ])
    }

    #[test]
    fn the_whole_subtree_shows_at_its_real_depth() {
        assert_eq!(
            shape(&chain()),
            vec![
                "H api", "I0 #1+", "C1 #2+", "C2 #3+", "C3 #4", "H web", "C0 #1+", "I1 #2+",
                "I2 #3+", "C3 #4", "H ops", "C0 #1+", "C1 #2+", "C2 #3+", "I3 #4"
            ]
        );
    }

    #[test]
    fn a_hidden_middle_level_stays_as_context() {
        let mut a = chain();
        for i in a.repos.iter_mut().flat_map(|r| r.issues.iter_mut()) {
            i.title = if i.number == 3 { "drop" } else { "keep" }.into();
        }
        a.filters.text = "keep".into();
        a.rebuild_rows();
        let rows = shape(&a);
        assert_eq!(
            rows[..5],
            ["H api", "I0 #1+", "C1 #2+", "C2 #3+", "C3 #4"],
            "web#3 is filtered out but keeps ops#4 attached"
        );
    }

    #[test]
    fn other_repo_children_sort_with_the_parents_own() {
        let p = stub("I_100", 100, "api");
        let a = app(vec![
            repo(
                "api",
                vec![
                    issue(100, vec![]),
                    issue(30, vec![p.clone()]),
                    issue(5, vec![p.clone()]),
                ],
            ),
            repo("web", vec![issue(20, vec![p])]),
        ]);
        assert_eq!(
            shape(&a)[..5],
            ["H api", "I0 #100+", "I1 #30", "C1 #20", "I1 #5"],
            "newest first, whichever repo a child is in"
        );
    }

    #[test]
    fn collapsing_the_parent_collapses_it_in_every_group() {
        let mut a = cross();
        a.collapsed_issues.insert("I_10".into());
        a.rebuild_rows();
        assert_eq!(
            shape(&a),
            vec!["H api", "I0 #10+", "H web", "C0 #10+"],
            "api#10 is collapsed in api and as web's context row"
        );
    }

    #[test]
    fn enter_on_a_dim_child_jumps_to_its_home_row() {
        let mut a = cross();
        select_ctx(&mut a, "api", 20);
        assert_eq!(a.tree_right(), TreeRight::Handled);
        assert!(matches!(a.rows[a.selected], Row::Issue { .. }));
        assert_eq!(a.selected_issue().map(|i| i.number), Some(20));
        assert_eq!(a.selected_repo().map(|r| r.repo.as_str()), Some("web"));
    }

    #[test]
    fn enter_on_a_dim_parent_in_another_repo_jumps_to_its_home_row() {
        let mut a = cross();
        select_ctx(&mut a, "web", 10);
        // An expanded branch: not a collapse toggle, so it is a link.
        assert_eq!(a.tree_right(), TreeRight::Handled);
        assert!(matches!(a.rows[a.selected], Row::Issue { .. }));
        assert_eq!(a.selected_issue().map(|i| i.number), Some(10));
        assert_eq!(a.selected_repo().map(|r| r.repo.as_str()), Some("api"));
    }

    #[test]
    fn a_link_to_a_repo_the_filter_hides_clears_the_filter() {
        let mut a = cross();
        a.filters.repo = "api".into();
        a.rebuild_rows();
        select_ctx(&mut a, "api", 20);
        assert_eq!(a.tree_right(), TreeRight::Handled);
        assert_eq!(a.selected_issue().map(|i| i.number), Some(20));
        assert!(a.filters.repo.is_empty());
    }

    #[test]
    fn enter_on_a_context_row_in_its_own_repo_still_opens_detail() {
        let mut a = family();
        for i in &mut a.repos[0].issues {
            i.title = if i.number == 227 { "parent" } else { "match" }.into();
        }
        a.filters.text = "match".into();
        a.rebuild_rows();
        select_ctx(&mut a, "r", 227);
        assert_eq!(a.tree_right(), TreeRight::EnterDetail);
    }

    #[test]
    fn issue_scoped_keys_use_the_issues_own_repo_on_a_dim_row() {
        let mut a = cross();
        a.copy_format = "{repo}#{number}".into();
        select_ctx(&mut a, "api", 20);
        assert_eq!(a.selected_repo().map(|r| r.repo.as_str()), Some("api"));
        assert_eq!(
            a.selected_issue_repo().map(|r| r.repo.as_str()),
            Some("web"),
            "the group is api, the issue is web's"
        );
        assert_eq!(a.selected_short_ref().as_deref(), Some("web#20"));
        assert!(
            a.move_targets().iter().all(|r| r != "web"),
            "web is its current repo, not a move target"
        );
    }

    #[test]
    fn a_deep_chain_keeps_every_level_and_its_hidden_middle() {
        let root = stub("I_1", 1, "r");
        let mid = stub("I_2", 2, "r");
        let a = app(vec![repo(
            "r",
            vec![issue(1, vec![]), issue(3, vec![mid.clone(), root.clone()])],
        )]);
        // #2 is not loaded at all: drawn as a stub between its parent and child.
        assert_eq!(shape(&a), vec!["H r", "I0 #1+", "S1 #2+", "I2 #3"]);
    }

    #[test]
    fn left_and_right_collapse_expand_and_climb() {
        let mut a = family();
        select(&mut a, 230);
        a.tree_left();
        assert_eq!(a.selected_issue().unwrap().number, 227, "child → parent");
        a.tree_left();
        assert!(
            a.collapsed_issues.contains("I_227"),
            "expanded parent → collapse"
        );
        assert_eq!(shape(&a), vec!["H r", "I0 #227+", "I0 #63"]);
        assert_eq!(a.selected_issue().unwrap().number, 227);
        a.tree_left();
        assert!(
            matches!(a.rows[a.selected], Row::RepoHeader { .. }),
            "top level → header"
        );

        a.selected = 1;
        assert_eq!(
            a.tree_right(),
            TreeRight::Handled,
            "collapsed parent → expand"
        );
        assert!(!a.collapsed_issues.contains("I_227"));
        assert_eq!(
            a.tree_right(),
            TreeRight::EnterDetail,
            "expanded parent → detail"
        );
    }

    #[test]
    fn collapse_and_selection_survive_a_refresh() {
        let mut a = family();
        a.collapsed_issues.insert("I_227".into());
        a.rebuild_rows();
        select(&mut a, 63);
        let data = a.repos.clone();
        a.set_data(data);
        assert_eq!(shape(&a), vec!["H r", "I0 #227+", "I0 #63"]);
        assert_eq!(a.selected_issue().unwrap().number, 63);

        a.collapsed_issues.clear();
        a.rebuild_rows();
        select(&mut a, 230);
        let data = a.repos.clone();
        a.set_data(data);
        assert_eq!(
            a.selected_issue().unwrap().number,
            230,
            "a nested row is reselected by id"
        );
    }

    #[test]
    fn brackets_collapse_and_expand_every_branch() {
        let mut a = family();
        a.set_all_collapsed(true);
        assert!(a.collapsed_issues.contains("I_227"));
        a.collapsed.clear();
        a.rebuild_rows();
        assert_eq!(shape(&a), vec!["H r", "I0 #227+", "I0 #63"]);
        a.set_all_collapsed(false);
        assert!(a.collapsed_issues.is_empty());
    }

    #[test]
    fn jumping_to_a_hidden_child_expands_its_parent() {
        let mut a = family();
        a.collapsed_issues.insert("I_227".into());
        a.rebuild_rows();
        assert!(a.jump_to_number(230));
        assert_eq!(a.selected_issue().unwrap().number, 230);
        assert!(!a.collapsed_issues.contains("I_227"));
    }

    #[test]
    fn switching_org_forgets_collapsed_branches() {
        let mut a = family();
        a.collapsed_issues.insert("I_227".into());
        a.switch_org("other".into());
        assert!(a.collapsed_issues.is_empty());
    }
}
