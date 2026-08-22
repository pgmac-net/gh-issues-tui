use super::prelude::*;

/// The visible row list and everything that moves through it: rebuilding
/// rows from data + filters + sort + collapse, the selection, and the
/// per-repo collapse state.
impl App {
    /// Recompute the visible rows. Keeps the selection in range.
    pub fn rebuild_rows(&mut self) {
        for repo in &mut self.repos {
            sort_issues(&mut repo.issues, self.sort_key, self.sort_desc);
        }
        self.rows.clear();
        let repo_exact = self.repo_filter_exact();
        for (ri, repo) in self.repos.iter().enumerate() {
            if !self.filters.repo_matches(&repo.repo, repo_exact) {
                continue;
            }
            let visible: Vec<usize> = repo
                .issues
                .iter()
                .enumerate()
                .filter(|(_, i)| self.filters.matches(i, self.state_filter))
                .map(|(idx, _)| idx)
                .collect();
            if visible.is_empty() && self.filters.hide_empty {
                continue;
            }
            self.rows.push(Row::RepoHeader { repo_idx: ri });
            if !self.collapsed.contains(&repo.repo) {
                for ii in visible {
                    self.rows.push(Row::Issue {
                        repo_idx: ri,
                        issue_idx: ii,
                    });
                }
            }
        }
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
    }

    pub fn selected_issue(&self) -> Option<&Issue> {
        match self.rows.get(self.selected)? {
            Row::Issue {
                repo_idx,
                issue_idx,
            } => self.repos.get(*repo_idx)?.issues.get(*issue_idx),
            Row::RepoHeader { .. } => None,
        }
    }

    pub fn selected_repo(&self) -> Option<&RepoIssues> {
        match self.rows.get(self.selected)? {
            Row::Issue { repo_idx, .. } | Row::RepoHeader { repo_idx } => self.repos.get(*repo_idx),
        }
    }

    /// Short reference for the selected issue, rendered from
    /// `copy_format` (`{owner}`, `{repo}`, `{number}`). `None` when no
    /// issue is selected (e.g. a repo header row).
    pub fn selected_short_ref(&self) -> Option<String> {
        let issue = self.selected_issue()?;
        let repo = self.selected_repo()?;
        Some(
            self.copy_format
                .replace("{owner}", &self.org)
                .replace("{repo}", &repo.repo)
                .replace("{number}", &issue.number.to_string()),
        )
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

    pub fn set_all_collapsed(&mut self, collapsed: bool) {
        if collapsed {
            self.collapsed = self.repos.iter().map(|r| r.repo.clone()).collect();
        } else {
            self.collapsed.clear();
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
            Some(Row::Issue { repo_idx, .. }) | Some(Row::RepoHeader { repo_idx }) => *repo_idx,
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
        // Expand the group so the issue row exists.
        self.collapsed.remove(&repo_name);
        self.rebuild_rows();

        // Locate the row by repo name and number (number alone repeats across repos).
        let target = self.rows.iter().position(|row| match row {
            Row::Issue {
                repo_idx: r,
                issue_idx,
            } => self.repos.get(*r).is_some_and(|repo| {
                repo.repo == repo_name
                    && repo
                        .issues
                        .get(*issue_idx)
                        .is_some_and(|i| i.number == number)
            }),
            Row::RepoHeader { .. } => false,
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
