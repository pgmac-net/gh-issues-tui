//! Outside sub-issues (#261, ADR 0013).
//!
//! A sub-issue the app has not loaded (another owner, a repo not fetched,
//! closed while showing open, outside a pushed-down filter) is fetched for its
//! parent, whole, and kept here: never in `App.repos`, never counted, never
//! deciding membership. The tree draws the ones that pass the filters as
//! context rows (`ContextRef::Outside`), read-only.
//!
//! A batch is the loaded parents a refresh covers whose `sub_issues.total` is
//! more than the sub-issues loaded for them. Its answer replaces those parents'
//! entries; until it lands the old ones stay drawn. A fetched child that is
//! itself short of children makes the next batch, down to [`MAX_DEPTH`].

use std::collections::HashSet;

use super::prelude::*;
use crate::provider::types::{OutsideIssue, SubIssues};

/// GitHub's sub-issue nesting limit: no batch goes deeper.
pub const MAX_DEPTH: u8 = 8;

/// Which loaded parents a refresh covers (decision 2 of ADR 0013).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubIssueScope {
    /// A load, `Ctrl+R` or auto-refresh: every loaded issue.
    All,
    /// `r` on a repo: that repo's issues.
    Repo(String),
    /// `r` on an issue: that issue.
    Issue(String),
}

/// One request: the parents to ask for, at which level of the tree, and the
/// generation that orders it against other answers for the same parents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubIssueBatch {
    pub parents: Vec<String>,
    pub generation: u64,
    pub depth: u8,
}

/// The outside sub-issues, by parent, as last fetched. An entry holds *every*
/// sub-issue the backend reported, loaded ones too: the tree skips those, so
/// a child that becomes loaded is drawn once, at home.
#[derive(Debug, Clone, Default)]
pub struct Outside {
    entries: Vec<SubIssues>,
    /// The newest generation applied per parent: an older answer that lands
    /// late is dropped.
    applied: HashMap<String, u64>,
    next_gen: u64,
}

impl Outside {
    /// Every parent's entry, in a stable order (row refs index into it).
    pub fn entries(&self) -> &[SubIssues] {
        &self.entries
    }

    /// The child at `entry`, `child`, as a row refers to it.
    pub fn child(&self, entry: usize, child: usize) -> Option<&OutsideIssue> {
        self.entries.get(entry)?.children.get(child)
    }

    fn position(&self, parent: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.parent == parent)
    }

    /// A new batch for `parents`, or `None` when there are none.
    fn batch(&mut self, mut parents: Vec<String>, depth: u8) -> Option<SubIssueBatch> {
        parents.sort();
        parents.dedup();
        if parents.is_empty() {
            return None;
        }
        self.next_gen += 1;
        Some(SubIssueBatch {
            parents,
            generation: self.next_gen,
            depth,
        })
    }
}

impl App {
    /// How many sub-issues of each issue are loaded: loaded issues whose
    /// nearest ancestor it is.
    fn loaded_children(&self) -> HashMap<String, u32> {
        let mut counts: HashMap<String, u32> = HashMap::new();
        for issue in self.repos.iter().flat_map(|r| r.issues.iter()) {
            if let Some(parent) = issue.ancestors.first() {
                *counts.entry(parent.id.clone()).or_default() += 1;
            }
        }
        counts
    }

    fn loaded_ids(&self) -> HashSet<String> {
        self.repos
            .iter()
            .flat_map(|r| r.issues.iter().map(|i| i.id.clone()))
            .collect()
    }

    /// Whether `issue` has more sub-issues than `counts` says are loaded.
    fn is_short(issue: &Issue, counts: &HashMap<String, u32>) -> bool {
        let total = issue.sub_issues.map_or(0, |s| s.total);
        total > counts.get(&issue.id).copied().unwrap_or(0)
    }

    /// After a refresh covering `scope` settles: the batch to fetch, if any
    /// parent there is short of sub-issues. A parent there that is not short
    /// any more has its entry dropped now, since every sub-issue it has is
    /// loaded.
    pub fn sub_issue_batch(&mut self, scope: &SubIssueScope) -> Option<SubIssueBatch> {
        let counts = self.loaded_children();
        let mut short = Vec::new();
        let mut complete = Vec::new();
        for repo in &self.repos {
            if matches!(scope, SubIssueScope::Repo(name) if *name != repo.repo) {
                continue;
            }
            for issue in &repo.issues {
                if matches!(scope, SubIssueScope::Issue(id) if *id != issue.id) {
                    continue;
                }
                if Self::is_short(issue, &counts) {
                    short.push(issue.id.clone());
                } else {
                    complete.push(issue.id.clone());
                }
            }
        }
        let before = self.outside.entries.len();
        self.outside
            .entries
            .retain(|e| !complete.contains(&e.parent));
        if self.prune_outside() || self.outside.entries.len() != before {
            self.outside_changed();
        }
        self.outside.batch(short, 0)
    }

    /// Fold an answer for `batch` in, and return the next level's batch: the
    /// fetched children that are themselves short of sub-issues.
    pub fn apply_sub_issues(
        &mut self,
        batch: &SubIssueBatch,
        answer: Vec<SubIssues>,
    ) -> Option<SubIssueBatch> {
        let mut answers: HashMap<String, SubIssues> =
            answer.into_iter().map(|s| (s.parent.clone(), s)).collect();
        let mut applied = Vec::new();
        for parent in &batch.parents {
            if self
                .outside
                .applied
                .get(parent)
                .is_some_and(|&g| g > batch.generation)
            {
                continue;
            }
            self.outside
                .applied
                .insert(parent.clone(), batch.generation);
            match (self.outside.position(parent), answers.remove(parent)) {
                (Some(i), Some(s)) => self.outside.entries[i] = s,
                (Some(i), None) => {
                    self.outside.entries.remove(i);
                }
                (None, Some(s)) => self.outside.entries.push(s),
                (None, None) => {}
            }
            applied.push(parent.clone());
        }
        if applied.is_empty() {
            return None;
        }
        self.prune_outside();
        self.outside_changed();

        if batch.depth + 1 >= MAX_DEPTH {
            return None;
        }
        let counts = self.loaded_children();
        let loaded = self.loaded_ids();
        let next: Vec<String> = self
            .outside
            .entries
            .iter()
            .filter(|e| applied.contains(&e.parent))
            .flat_map(|e| e.children.iter())
            .filter(|c| !loaded.contains(&c.issue.id) && !applied.contains(&c.issue.id))
            .filter(|c| Self::is_short(&c.issue, &counts))
            .map(|c| c.issue.id.clone())
            .collect();
        self.outside.batch(next, batch.depth + 1)
    }

    /// Drop entries no loaded issue leads to (a parent that is gone, and the
    /// levels below it). Returns whether anything was dropped.
    fn prune_outside(&mut self) -> bool {
        let loaded = self.loaded_ids();
        let mut keep: HashSet<String> = HashSet::new();
        let mut frontier: Vec<String> = self
            .outside
            .entries
            .iter()
            .filter(|e| loaded.contains(&e.parent))
            .map(|e| e.parent.clone())
            .collect();
        while let Some(parent) = frontier.pop() {
            if !keep.insert(parent.clone()) {
                continue;
            }
            if let Some(i) = self.outside.position(&parent) {
                frontier.extend(
                    self.outside.entries[i]
                        .children
                        .iter()
                        .map(|c| c.issue.id.clone()),
                );
            }
        }
        let before = self.outside.entries.len();
        self.outside.entries.retain(|e| keep.contains(&e.parent));
        self.outside.entries.len() != before
    }

    /// The outside store changed: rows index into it, so rebuild them, keep
    /// the selection where it was, and let the snapshot follow.
    fn outside_changed(&mut self) {
        let key = self.selected_key();
        self.data_gen += 1;
        self.rebuild_rows();
        self.reselect(key);
    }

    /// Take a snapshot's outside sub-issues, drawn before the first fetch.
    pub(super) fn restore_outside(&mut self, entries: Vec<SubIssues>) {
        if self.outside.entries.is_empty() {
            self.outside.entries = entries;
        }
    }

    /// The outside sub-issue a row stands for, when it is one.
    pub fn row_outside(&self, row: &Row) -> Option<&OutsideIssue> {
        match row {
            Row::Context {
                of: ContextRef::Outside { entry, child },
                ..
            } => self.outside.child(*entry, *child),
            _ => None,
        }
    }

    /// The selected row's outside sub-issue, when it is one. Read-only: write
    /// keys report it instead of acting, and `Enter` opens it.
    pub fn selected_outside(&self) -> Option<&OutsideIssue> {
        self.row_outside(self.rows.get(self.selected)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::types::{IssueStub, SubIssueProgress};
    use crate::tui::app::RefreshTarget;

    fn stub(id: &str, number: u64, repo: &str) -> IssueStub {
        IssueStub {
            id: id.into(),
            number,
            title: format!("t{number}"),
            state: IssueState::Open,
            url: format!("https://github.com/org/{repo}/issues/{number}"),
            repo: repo.into(),
            owner: "org".into(),
        }
    }

    /// Issue `number` with `ancestors` and `total` sub-issues. Higher numbers
    /// are newer, so the default sort lists them first.
    fn issue(number: u64, ancestors: Vec<IssueStub>, total: u32) -> Issue {
        let base: DateTime<Utc> = "2026-01-01T00:00:00Z".parse().unwrap();
        Issue {
            ancestors,
            updated_at: base + chrono::TimeDelta::seconds(number as i64),
            sub_issues: (total > 0).then_some(SubIssueProgress {
                completed: 0,
                total,
            }),
            url: format!("https://github.com/org/r/issues/{number}"),
            ..Issue::fixture(&format!("I_{number}"), number)
        }
    }

    fn outside(issue: Issue, repo: &str, owner: &str) -> OutsideIssue {
        OutsideIssue {
            issue: Issue {
                url: format!("https://github.com/{owner}/{repo}/issues/{}", issue.number),
                ..issue
            },
            repo: repo.into(),
            owner: owner.into(),
        }
    }

    fn repo(name: &str, issues: Vec<Issue>) -> RepoIssues {
        RepoIssues {
            repo: name.into(),
            repo_url: format!("https://github.com/org/{name}"),
            issues,
        }
    }

    fn app(repos: Vec<RepoIssues>) -> App {
        let mut app = App::new("org".into(), None, false, false, String::new());
        app.set_data(repos);
        app
    }

    /// The rows as `H repo`, `I<depth> #n`, `C<depth> #n` (loaded context),
    /// `O<depth> #n` (outside), `S<depth> #n` (stub); `+` on a branch.
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
                let plus = |b: bool| if b { "+" } else { "" };
                match row {
                    Row::RepoHeader { repo_idx } => format!("H {}", app.repos[*repo_idx].repo),
                    Row::Issue { depth, branch, .. } => {
                        format!("I{depth} #{}{}", num(row), plus(*branch))
                    }
                    Row::Context {
                        depth, branch, of, ..
                    } => {
                        let k = match of {
                            ContextRef::Stub { .. } => "S",
                            ContextRef::Outside { .. } => "O",
                            ContextRef::Loaded { .. } => "C",
                        };
                        format!("{k}{depth} #{}{}", num(row), plus(*branch))
                    }
                }
            })
            .collect()
    }

    /// `tf#32` (2 sub-issues, none loaded) and `tf#35` (none).
    fn parent_app() -> App {
        app(vec![repo(
            "tf",
            vec![issue(32, vec![], 2), issue(35, vec![], 0)],
        )])
    }

    /// The answer for #32: `homelabia#221` and `#205`.
    fn answer() -> Vec<SubIssues> {
        let p = stub("I_32", 32, "tf");
        vec![SubIssues {
            parent: "I_32".into(),
            children: vec![
                outside(issue(221, vec![p.clone()], 0), "homelabia", "org"),
                outside(issue(205, vec![p], 0), "homelabia", "org"),
            ],
        }]
    }

    fn fetched() -> App {
        let mut a = parent_app();
        let batch = a.sub_issue_batch(&SubIssueScope::All).unwrap();
        assert!(a.apply_sub_issues(&batch, answer()).is_none());
        a
    }

    #[test]
    fn only_a_parent_short_of_sub_issues_is_asked_for() {
        let mut a = parent_app();
        let batch = a.sub_issue_batch(&SubIssueScope::All).unwrap();
        assert_eq!(batch.parents, vec!["I_32".to_string()]);
        assert_eq!(batch.depth, 0);
        // One child loaded, one short: still asked for.
        let p = stub("I_32", 32, "tf");
        let mut a = app(vec![repo(
            "tf",
            vec![issue(32, vec![], 2), issue(40, vec![p.clone()], 0)],
        )]);
        assert!(a.sub_issue_batch(&SubIssueScope::All).is_some());
        // Both loaded: nothing to ask.
        let mut a = app(vec![repo(
            "tf",
            vec![
                issue(32, vec![], 2),
                issue(40, vec![p.clone()], 0),
                issue(41, vec![p], 0),
            ],
        )]);
        assert!(a.sub_issue_batch(&SubIssueScope::All).is_none());
    }

    #[test]
    fn a_scope_asks_only_for_its_parents() {
        let mut a = app(vec![
            repo("tf", vec![issue(32, vec![], 2)]),
            repo("ops", vec![issue(7, vec![], 1)]),
        ]);
        let batch = a
            .sub_issue_batch(&SubIssueScope::Repo("ops".into()))
            .unwrap();
        assert_eq!(batch.parents, vec!["I_7".to_string()]);
        let batch = a
            .sub_issue_batch(&SubIssueScope::Issue("I_32".into()))
            .unwrap();
        assert_eq!(batch.parents, vec!["I_32".to_string()]);
    }

    #[test]
    fn fetched_sub_issues_hang_dim_under_their_parent_and_are_not_counted() {
        let a = fetched();
        assert_eq!(
            shape(&a),
            vec!["H tf", "I0 #32+", "O1 #221", "O1 #205", "I0 #35"]
        );
        assert_eq!(a.filtered_issue_count(), 2, "outside rows are not counted");
        assert_eq!(a.repos.len(), 1, "they never join a repo group");
    }

    #[test]
    fn filters_apply_to_them_but_the_repo_filter_does_not() {
        let mut a = fetched();
        for e in &mut a.outside.entries {
            for c in &mut e.children {
                c.issue.title = if c.issue.number == 221 {
                    "keep"
                } else {
                    "drop"
                }
                .into();
            }
        }
        a.repos[0]
            .issues
            .iter_mut()
            .for_each(|i| i.title = "keep".into());
        a.filters.text = "keep".into();
        a.filters.repo = "tf".into();
        a.rebuild_rows();
        assert_eq!(shape(&a), vec!["H tf", "I0 #32+", "O1 #221", "I0 #35"]);
    }

    #[test]
    fn a_loaded_copy_wins_over_the_outside_one() {
        let mut a = fetched();
        // homelabia#221 gets loaded: drawn as a loaded context row, once.
        let p = stub("I_32", 32, "tf");
        let mut repos = a.repos.clone();
        repos.push(repo("homelabia", vec![issue(221, vec![p], 0)]));
        a.set_data(repos);
        assert_eq!(
            shape(&a),
            vec![
                "H tf",
                "I0 #32+",
                "C1 #221",
                "O1 #205",
                "I0 #35",
                "H homelabia",
                "C0 #32+",
                "I1 #221"
            ],
            "#221 is a loaded context row under #32 now, not an outside one"
        );
    }

    #[test]
    fn a_short_fetched_child_makes_the_next_level() {
        let mut a = parent_app();
        let batch = a.sub_issue_batch(&SubIssueScope::All).unwrap();
        let mut ans = answer();
        ans[0].children[0].issue.sub_issues = Some(SubIssueProgress {
            completed: 0,
            total: 1,
        });
        let next = a.apply_sub_issues(&batch, ans).unwrap();
        assert_eq!(next.parents, vec!["I_221".to_string()]);
        assert_eq!(next.depth, 1);
        let grand = vec![SubIssues {
            parent: "I_221".into(),
            children: vec![outside(
                issue(
                    7,
                    vec![stub("I_221", 221, "homelabia"), stub("I_32", 32, "tf")],
                    0,
                ),
                "ops",
                "other",
            )],
        }];
        assert!(a.apply_sub_issues(&next, grand).is_none());
        assert_eq!(
            shape(&a),
            vec!["H tf", "I0 #32+", "O1 #221+", "O2 #7", "O1 #205", "I0 #35"]
        );
    }

    #[test]
    fn no_level_is_asked_past_github_s_limit() {
        let mut a = parent_app();
        let mut ans = answer();
        ans[0].children[0].issue.sub_issues = Some(SubIssueProgress {
            completed: 0,
            total: 1,
        });
        let deep = SubIssueBatch {
            parents: vec!["I_32".into()],
            generation: 1,
            depth: MAX_DEPTH - 1,
        };
        assert!(a.apply_sub_issues(&deep, ans).is_none());
    }

    #[test]
    fn an_older_answer_that_lands_late_is_dropped() {
        let mut a = parent_app();
        let old = a.sub_issue_batch(&SubIssueScope::All).unwrap();
        let new = a.sub_issue_batch(&SubIssueScope::All).unwrap();
        a.apply_sub_issues(&new, answer());
        let mut stale = answer();
        stale[0].children.truncate(1);
        a.apply_sub_issues(&old, stale);
        assert_eq!(
            a.outside.entries()[0].children.len(),
            2,
            "the newer answer stands"
        );
    }

    #[test]
    fn a_refresh_replaces_a_parent_s_sub_issues_when_it_lands() {
        let mut a = fetched();
        let batch = a.sub_issue_batch(&SubIssueScope::All).unwrap();
        // Until the answer lands, the old ones stay drawn.
        assert!(shape(&a).contains(&"O1 #205".to_string()));
        let mut ans = answer();
        ans[0].children.truncate(1);
        a.apply_sub_issues(&batch, ans);
        assert!(!shape(&a).contains(&"O1 #205".to_string()));
    }

    #[test]
    fn a_parent_that_is_gone_takes_its_levels_with_it() {
        let mut a = fetched();
        let repos = vec![repo("tf", vec![issue(35, vec![], 0)])];
        a.set_data(repos);
        a.sub_issue_batch(&SubIssueScope::All);
        assert!(a.outside.entries().is_empty());
    }

    #[test]
    fn an_outside_row_is_read_only_and_knows_its_own_repo() {
        let mut a = fetched();
        a.copy_format = "{owner}/{repo}#{number}".into();
        a.selected = shape(&a).iter().position(|r| r == "O1 #221").unwrap();
        assert!(a.selected_outside().is_some());
        assert_eq!(a.selected_issue().map(|i| i.number), Some(221));
        assert_eq!(a.selected_short_ref().as_deref(), Some("org/homelabia#221"));
        assert_eq!(a.current_repo(), Some(("org".into(), "homelabia".into())));
        assert_eq!(a.refresh_target(), RefreshTarget::Repo("tf".into()));
    }

    #[test]
    fn another_owner_s_sub_issue_names_that_owner() {
        let mut a = parent_app();
        let batch = a.sub_issue_batch(&SubIssueScope::All).unwrap();
        let mut ans = answer();
        ans[0].children[0] = outside(ans[0].children[0].issue.clone(), "infra", "other");
        a.apply_sub_issues(&batch, ans);
        a.copy_format = "{owner}/{repo}#{number}".into();
        a.selected = shape(&a).iter().position(|r| r == "O1 #221").unwrap();
        assert_eq!(a.selected_short_ref().as_deref(), Some("other/infra#221"));
    }

    #[test]
    fn a_refresh_keeps_the_selection_on_an_outside_row() {
        let mut a = fetched();
        a.selected = shape(&a).iter().position(|r| r == "O1 #205").unwrap();
        let repos = a.repos.clone();
        a.set_data(repos);
        assert_eq!(a.selected_issue().map(|i| i.number), Some(205));
        let batch = a.sub_issue_batch(&SubIssueScope::All).unwrap();
        a.apply_sub_issues(&batch, answer());
        assert_eq!(a.selected_issue().map(|i| i.number), Some(205));
    }

    #[test]
    fn the_snapshot_carries_them_and_restores_them() {
        let mut a = fetched();
        a.fetched_at = Some(Utc::now());
        a.loaded = Some(crate::tui::app::Loaded {
            scope: crate::provider::scope::FetchScope::default(),
            repo: None,
        });
        let snap = a.snapshot().unwrap();
        assert_eq!(snap.outside.len(), 1);
        let mut b = App::new("org".into(), None, false, false, String::new());
        b.apply_snapshot(snap);
        assert_eq!(shape(&b), shape(&a));
    }

    #[test]
    fn switching_org_forgets_them() {
        let mut a = fetched();
        a.switch_org("other".into());
        assert!(a.outside.entries().is_empty());
    }
}
