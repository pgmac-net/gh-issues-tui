use super::prelude::*;
use super::subtree::{Forest, cross_repo_links, global_rank, loaded_index};

/// The visible row list and everything that moves through it: rebuilding
/// rows from data + filters + sort + collapse, the selection, and the
/// per-repo collapse state.
impl App {
    /// Recompute the visible rows. Keeps the selection in range.
    pub fn rebuild_rows(&mut self) {
        for repo in &mut self.repos {
            sort_issues(&mut repo.issues, self.sort_key, self.sort_desc);
        }
        let repo_exact = self.repo_filter_exact();
        let loaded = loaded_index(&self.repos);
        // What passes every filter but the repo filter, in every repo: the
        // issues a group shows, and the sub-issues other repos show under
        // them (#260, ADR 0012).
        let passing: Vec<Vec<usize>> = self
            .repos
            .iter()
            .map(|repo| {
                repo.issues
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| self.filters.matches(i, self.state_filter))
                    .map(|(idx, _)| idx)
                    .collect()
            })
            .collect();
        // Outside sub-issues (#261): every one not also loaded (the loaded
        // copy wins) is a context row an ancestor can be drawn as, and the
        // ones that pass the filters hang under their visible parents.
        let mut outside_ids: HashMap<&str, (usize, usize)> = HashMap::new();
        let mut outside_passing = Vec::new();
        for (e, entry) in self.outside.entries().iter().enumerate() {
            for (c, child) in entry.children.iter().enumerate() {
                let id = child.issue.id.as_str();
                if loaded.contains_key(id) || outside_ids.contains_key(id) {
                    continue;
                }
                outside_ids.insert(id, (e, c));
                if self.filters.matches(&child.issue, self.state_filter) {
                    outside_passing.push((e, c));
                }
            }
        }
        let foreign = cross_repo_links(&self.repos, &passing, &self.outside, &outside_passing);
        let rank = global_rank(&self.repos, &self.outside, self.sort_key, self.sort_desc);
        let forest = Forest {
            repos: &self.repos,
            outside: &self.outside,
            loaded: &loaded,
            outside_ids: &outside_ids,
            collapsed: &self.collapsed_issues,
            rank: &rank,
        };
        let mut rows = Vec::new();
        for (ri, repo) in self.repos.iter().enumerate() {
            if !self.filters.repo_matches(&repo.repo, repo_exact) {
                continue;
            }
            let visible = &passing[ri];
            // An exact repo filter names the repo outright: keep its header
            // even when empty, or `n` has nothing to attach to (#198).
            if visible.is_empty() && self.filters.hide_empty && !repo_exact {
                continue;
            }
            rows.push(Row::RepoHeader { repo_idx: ri });
            if !self.collapsed.contains(&repo.repo) {
                // Issues nest under their sub-issue parents (#234).
                rows.extend(forest.group_rows(ri, visible, &foreign[ri]));
            }
        }
        self.rows = rows;
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
    }

    /// What to put the selection back on after the rows are rebuilt: the
    /// selected issue's key. A loaded row only; `None` on a header or stub.
    pub(super) fn selected_key(&self) -> Option<RowKey> {
        let id = self.row_issue(self.rows.get(self.selected)?)?.id.clone();
        Some(self.selected_key_for(id))
    }

    /// As [`Self::selected_key`] for `id`, which the selected row stands for
    /// (a stub's id too).
    pub(super) fn selected_key_for(&self, id: String) -> RowKey {
        let link_in = match self.rows.get(self.selected) {
            Some(Row::Context {
                repo_idx,
                of: ContextRef::Loaded { repo_idx: home, .. },
                ..
            }) if home != repo_idx => self.repos.get(*repo_idx).map(|r| r.repo.clone()),
            // An outside sub-issue (#261) can be drawn in several groups too.
            Some(Row::Context {
                repo_idx,
                of: ContextRef::Outside { .. },
                ..
            }) => self.repos.get(*repo_idx).map(|r| r.repo.clone()),
            _ => None,
        };
        RowKey { id, link_in }
    }

    /// Move the selection back onto the issue `key` names, when it is still
    /// in the rows. A vanished (or absent) id leaves the index where
    /// `rebuild_rows` clamped it. Takes an `Option<String>` id too.
    pub(super) fn reselect(&mut self, key: Option<impl Into<RowKey>>) {
        let Some(key) = key.map(Into::into) else {
            return;
        };
        let id = key.id;
        // A dim row for an issue from another repo stays on that dim row: its
        // home row is a different place, reached only by `Enter` (#260).
        if let Some(group) = &key.link_in
            && let Some(idx) = self.rows.iter().position(|row| {
                matches!(row, Row::Context { repo_idx, of: ContextRef::Loaded { .. } | ContextRef::Outside { .. }, .. }
                    if self.repos[*repo_idx].repo == *group)
                    && self.row_issue_id(row) == Some(id.as_str())
            })
        {
            self.selected = idx;
            return;
        }
        // A visible row first; failing that, the same issue as a context row
        // (it was filtered out, but its descendants keep it on screen).
        let at = |want_issue: bool| {
            self.rows.iter().position(|row| {
                matches!(row, Row::Issue { .. }) == want_issue
                    && self.row_issue_id(row) == Some(id.as_str())
            })
        };
        if let Some(idx) = at(true).or_else(|| at(false)) {
            self.selected = idx;
        }
    }

    /// The selected issue, when it is loaded: a visible issue, or a context
    /// row for a loaded one (#234). `None` on a header or a stub.
    pub fn selected_issue(&self) -> Option<&Issue> {
        self.row_issue(self.rows.get(self.selected)?)
    }

    /// The repo group the selection is in. For a context row from another
    /// repo this is the group it is drawn in, not its own repo.
    pub fn selected_repo(&self) -> Option<&RepoIssues> {
        match self.rows.get(self.selected)? {
            Row::Issue { repo_idx, .. }
            | Row::Context { repo_idx, .. }
            | Row::RepoHeader { repo_idx } => self.repos.get(*repo_idx),
        }
    }

    /// The repo the selected *issue* lives in. [`Self::selected_repo`] is the
    /// group it is drawn in, which differs for a context row from another
    /// repo (#260, ADR 0012). Issue-scoped keys (labels, priority, refs, PR
    /// shorthand, runs) use this one; group-scoped keys keep the group.
    pub fn selected_issue_repo(&self) -> Option<&RepoIssues> {
        match self.rows.get(self.selected)? {
            Row::Context {
                of: ContextRef::Loaded { repo_idx, .. },
                ..
            } => self.repos.get(*repo_idx),
            _ => self.selected_repo(),
        }
    }

    /// Short reference for the selected issue, rendered from
    /// `copy_format` (`{owner}`, `{repo}`, `{number}`). `None` when no
    /// issue is selected (e.g. a repo header row).
    pub fn selected_short_ref(&self) -> Option<String> {
        let issue = self.selected_issue()?;
        let (owner, repo) = self.selected_home()?;
        Some(
            self.copy_format
                .replace("{owner}", &owner)
                .replace("{repo}", &repo)
                .replace("{number}", &issue.number.to_string()),
        )
    }

    /// The owner and repo the selected issue lives in. An outside sub-issue
    /// (#261) knows its own, which may be another owner; anything else is in
    /// the browsed org, in [`Self::selected_issue_repo`].
    pub fn selected_home(&self) -> Option<(String, String)> {
        if let Some(o) = self.selected_outside() {
            return Some((o.owner.clone(), o.repo.clone()));
        }
        let repo = self.selected_issue_repo()?;
        Some((self.org.clone(), repo.repo.clone()))
    }

    pub fn toggle_collapse(&mut self) {
        if let Some(repo) = self.selected_repo().map(|r| r.repo.clone()) {
            if !self.collapsed.remove(&repo) {
                self.collapsed.insert(repo);
            }
            self.rebuild_rows();
        }
    }

    pub fn set_current_collapsed(&mut self, collapsed: bool) {
        if let Some(repo) = self.selected_repo().map(|r| r.repo.clone()) {
            if collapsed {
                self.collapsed.insert(repo.clone());
            } else {
                self.collapsed.remove(&repo);
            }
            self.rebuild_rows();
            if collapsed {
                // Collapsing from a child row would leave the selection index
                // pointing at an unrelated row — land on the group's header.
                let header = self.rows.iter().position(|r| {
                    matches!(r, Row::RepoHeader { repo_idx }
                        if self.repos.get(*repo_idx).is_some_and(|ri| ri.repo == repo))
                });
                if let Some(idx) = header {
                    self.selected = idx;
                }
            }
        }
    }

    /// `←` in the tree (#217) — "out one level". On an issue (#234): collapse
    /// it if it is an expanded parent, else go to its parent row, else (top
    /// level) to the repo header. On an expanded header, collapse it; on a
    /// collapsed header, nothing.
    pub fn tree_left(&mut self) {
        match self.rows.get(self.selected) {
            Some(Row::Issue { .. } | Row::Context { .. }) => self.tree_left_node(),
            Some(Row::RepoHeader { .. }) if !self.selected_group_collapsed() => {
                self.set_current_collapsed(true);
            }
            _ => {}
        }
    }

    /// `→` in the tree (#217) — "in one level": on a collapsed header, expand
    /// it; on an expanded one, select its first issue. On a collapsed parent
    /// issue, expand it (#234). On any other issue the next level is the
    /// detail pane, which the caller opens (it may need a comment fetch), so
    /// that case is only reported.
    pub fn tree_right(&mut self) -> TreeRight {
        match self.rows.get(self.selected) {
            Some(Row::Issue { .. } | Row::Context { .. }) => return self.tree_right_node(),
            Some(Row::RepoHeader { .. }) if self.selected_group_collapsed() => {
                self.set_current_collapsed(false);
            }
            Some(Row::RepoHeader { .. }) => self.select_first_issue(),
            None => {}
        }
        TreeRight::Handled
    }

    /// Whether the selected row's repo group is collapsed.
    fn selected_group_collapsed(&self) -> bool {
        self.selected_repo()
            .is_some_and(|r| self.collapsed.contains(&r.repo))
    }

    /// Move the selection from an issue up to its repo's header row.
    pub fn select_repo_header(&mut self) {
        if let Some(Row::Issue { repo_idx, .. } | Row::Context { repo_idx, .. }) =
            self.rows.get(self.selected)
        {
            let repo_idx = *repo_idx;
            if let Some(idx) = self.rows[..self.selected]
                .iter()
                .rposition(|r| matches!(r, Row::RepoHeader { repo_idx: h } if *h == repo_idx))
            {
                self.selected = idx;
            }
        }
    }

    /// Move the selection from an expanded header down to its first issue.
    /// An empty group has none, so the selection stays on the header.
    pub fn select_first_issue(&mut self) {
        if let Some(Row::RepoHeader { repo_idx }) = self.rows.get(self.selected)
            && matches!(self.rows.get(self.selected + 1),
                Some(Row::Issue { repo_idx: r, .. } | Row::Context { repo_idx: r, .. })
                    if r == repo_idx)
        {
            self.selected += 1;
        }
    }

    /// `[` / `]`: every repo group and every sub-issue branch (#234).
    pub fn set_all_collapsed(&mut self, collapsed: bool) {
        if collapsed {
            self.collapsed = self.repos.iter().map(|r| r.repo.clone()).collect();
            self.collapsed_issues = self.all_branch_ids();
        } else {
            self.collapsed.clear();
            self.collapsed_issues.clear();
        }
        self.rebuild_rows();
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let len = self.rows.len() as isize;
        let next = (self.selected as isize + delta).clamp(0, len - 1);
        self.selected = next as usize;
    }

    /// Move the selection to the loaded issue with `number`, expanding its
    /// repo group if collapsed. Searches the currently-selected repo group
    /// first, then the rest in order, so repeated numbers across repos stay
    /// unambiguous in the common single-repo/filtered view. Unlike `/`
    /// search this never filters the list *down*: if the target is loaded
    /// but hidden by the active filters or state filter, those are cleared
    /// (state → `All`) so the row becomes visible before jumping — widening,
    /// not narrowing. Returns false (selection unchanged, status set) when no
    /// loaded issue has that number; closed issues must be fetched (`f`/
    /// `--all`) before they can be jumped to.
    pub fn jump_to_number(&mut self, number: u64) -> bool {
        if self.jump_to_ref(None, number) {
            return true;
        }
        self.status = Some(format!("no issue #{number} loaded"));
        false
    }

    /// As [`Self::jump_to_number`], but optionally pinned to one repo.
    ///
    /// A reference names the repo it belongs to (`o/r#N`) and numbers repeat
    /// across repos, so following one must not land on a same-numbered issue
    /// somewhere else — with `repo` set, only that group is searched.
    ///
    /// Reports failure **without setting `status`**, unlike `jump_to_number`.
    /// A reference can point outside the loaded data entirely (another org, or
    /// a closed issue not yet fetched), and #129's caller answers that by
    /// opening it in a browser instead — so the message belongs to whichever
    /// caller actually gives up.
    pub fn jump_to_ref(&mut self, repo: Option<&str>, number: u64) -> bool {
        if self.repos.is_empty() {
            return false;
        }
        // Start the search at the currently-selected repo group.
        let start = match self.rows.get(self.selected) {
            Some(
                Row::Issue { repo_idx, .. }
                | Row::Context { repo_idx, .. }
                | Row::RepoHeader { repo_idx },
            ) => *repo_idx,
            None => 0,
        };
        let n = self.repos.len();
        let mut found: Option<usize> = None;
        for k in 0..n {
            let ri = (start + k) % n;
            if repo.is_some_and(|want| self.repos[ri].repo != want) {
                continue;
            }
            if self.repos[ri].issues.iter().any(|i| i.number == number) {
                found = Some(ri);
                break;
            }
        }
        let Some(repo_idx) = found else {
            return false;
        };
        let repo_name = self.repos[repo_idx].repo.clone();

        // Reveal the issue if the active filters currently hide it — clearing
        // widens the list, which is the opposite of `/` search's narrowing and
        // keeps the ticket's "mustn't filter the list" intent.
        let exact = self.repo_filter_exact();
        let hidden = !self.filters.repo_matches(&repo_name, exact)
            || self.repos[repo_idx]
                .issues
                .iter()
                .find(|i| i.number == number)
                .is_some_and(|i| !self.filters.matches(i, self.state_filter));
        if hidden {
            self.clear_filters();
            self.state_filter = StateFilter::All;
        }
        // Expand the group, and any collapsed sub-issue parents above the
        // issue (#234), so its row exists.
        self.collapsed.remove(&repo_name);
        let target_id = self.repos[repo_idx]
            .issues
            .iter()
            .find(|i| i.number == number)
            .map(|i| i.id.clone());
        if let Some(id) = &target_id {
            self.reveal_issue(id);
        }
        self.rebuild_rows();

        // Locate the issue's own row in that repo's group (number alone
        // repeats across repos).
        let target = self.rows.iter().position(|row| match row {
            Row::Issue {
                repo_idx: r,
                issue_idx,
                ..
            } => {
                *r == repo_idx
                    && self.repos[*r]
                        .issues
                        .get(*issue_idx)
                        .is_some_and(|i| i.number == number)
            }
            _ => false,
        });
        if let Some(idx) = target {
            self.selected = idx;
            self.status = Some(format!("jumped to #{number}"));
            true
        } else {
            // Shouldn't happen — the issue was found in loaded data and the
            // group was expanded. Reporting failure lets the caller decide the
            // message rather than leaving a misleading one here.
            false
        }
    }

    /// Count of issues in a given repo that pass the current filters (excluding repo filter).
    pub fn repo_visible_count(&self, repo_idx: usize) -> usize {
        self.repos
            .get(repo_idx)
            .map(|repo| {
                repo.issues
                    .iter()
                    .filter(|i| self.filters.matches(i, self.state_filter))
                    .count()
            })
            .unwrap_or(0)
    }

    /// Count of issues currently visible (excludes headers). Test helper —
    /// production code shows `filtered_issue_count` instead.
    #[cfg(test)]
    pub fn visible_issue_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| matches!(r, Row::Issue { .. }))
            .count()
    }

    /// Count of issues passing the current filters, including those hidden
    /// inside collapsed repo groups. Shown in the list title.
    pub fn filtered_issue_count(&self) -> usize {
        let exact = self.repo_filter_exact();
        self.repos
            .iter()
            .filter(|r| self.filters.repo_matches(&r.repo, exact))
            .flat_map(|r| r.issues.iter())
            .filter(|i| self.filters.matches(i, self.state_filter))
            .count()
    }
}

/// Which row to put the selection back on after a rebuild: an issue's id, and
/// when the selection was a dim row for an issue from another repo, the group
/// it was drawn in. Without the group the id alone names the issue's home row,
/// and a refresh or a collapse would carry the selection there (#260).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RowKey {
    pub(super) id: String,
    pub(super) link_in: Option<String>,
}

impl From<String> for RowKey {
    fn from(id: String) -> Self {
        RowKey { id, link_in: None }
    }
}

/// What `→` on a tree row did (#217). On an issue it moves into the detail
/// pane, which the key handler performs because it may spawn a comment fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeRight {
    Handled,
    EnterDetail,
}
