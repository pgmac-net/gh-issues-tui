//! Picker vocabulary (#230): every repo, assignee, author and label seen in
//! any fetch this session, for the filter pickers.
//!
//! Once filters push down, the loaded data holds only matching issues, so
//! options built from it would shrink to what is already selected: filter to
//! one assignee and the assignee picker offers only that one. The
//! vocabulary keeps growing across fetches and resets with the org.

use std::collections::BTreeSet;

use super::prelude::*;

#[derive(Debug, Default)]
pub struct Vocab {
    pub repos: BTreeSet<String>,
    pub assignees: BTreeSet<String>,
    pub authors: BTreeSet<String>,
    /// Label names, first spelling seen wins (compared case-insensitively).
    pub labels: Vec<String>,
}

impl Vocab {
    pub fn absorb_repos(&mut self, repos: &[RepoIssues]) {
        for repo in repos {
            self.repos.insert(repo.repo.clone());
            for issue in &repo.issues {
                self.absorb_issue(issue);
            }
        }
    }

    pub fn absorb_issue(&mut self, issue: &Issue) {
        self.assignees.extend(issue.assignees.iter().cloned());
        self.authors.insert(issue.author.clone());
        for label in &issue.labels {
            if !self
                .labels
                .iter()
                .any(|l| l.eq_ignore_ascii_case(&label.name))
            {
                self.labels.push(label.name.clone());
            }
        }
    }
}

impl App {
    /// Repo names for the repo-filter picker and move targets: the org's full
    /// list once fetched (`org_repo_names`), plus every repo seen loaded.
    pub fn known_repos(&self) -> Vec<String> {
        let mut names: BTreeSet<String> = self.vocab.repos.clone();
        names.extend(self.repos.iter().map(|r| r.repo.clone()));
        if let Some(org) = &self.org_repo_names {
            names.extend(org.iter().cloned());
        }
        names.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::types::Label;

    fn issue(author: &str, assignees: &[&str], labels: &[&str]) -> Issue {
        Issue {
            id: "I".into(),
            number: 1,
            title: String::new(),
            body: String::new(),
            state: IssueState::Open,
            url: String::new(),
            author: author.into(),
            assignees: assignees.iter().map(|s| (*s).to_string()).collect(),
            labels: labels
                .iter()
                .map(|n| Label {
                    name: (*n).to_string(),
                    ..Default::default()
                })
                .collect(),
            comment_count: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            closed_at: None,
        }
    }

    #[test]
    fn vocabulary_only_grows_and_dedups_labels_by_case() {
        let mut v = Vocab::default();
        v.absorb_issue(&issue("a", &["x"], &["priority:High"]));
        v.absorb_issue(&issue("b", &["y"], &["Priority:high", "bug"]));
        assert_eq!(v.authors.len(), 2);
        assert_eq!(v.assignees.len(), 2);
        assert_eq!(v.labels, vec!["priority:High", "bug"]);
    }

    fn repo(name: &str, issues: Vec<Issue>) -> RepoIssues {
        RepoIssues {
            repo: name.into(),
            repo_url: String::new(),
            issues,
        }
    }

    /// The regression the vocabulary exists for: after a fetch narrowed to
    /// one assignee, the pickers still offer everyone seen before.
    #[test]
    fn pickers_keep_options_after_the_loaded_data_narrows() {
        let mut app = App::new("org".into(), None, false, false, String::new());
        app.set_data(vec![repo(
            "r",
            vec![
                issue("ann", &["x"], &["priority:high"]),
                issue("bob", &["y"], &["priority:low"]),
            ],
        )]);
        app.set_data(vec![repo(
            "r",
            vec![issue("ann", &["x"], &["priority:high"])],
        )]);
        assert_eq!(app.compute_select_options(2), vec!["\u{2014}", "x", "y"]);
        assert_eq!(
            app.compute_select_options(3),
            vec!["\u{2014}", "ann", "bob"]
        );
        assert_eq!(app.compute_multi_options(4), vec!["low", "high"]);
    }

    #[test]
    fn switching_org_forgets_the_vocabulary() {
        let mut app = App::new("org".into(), None, false, false, String::new());
        app.set_data(vec![repo("r", vec![issue("ann", &["x"], &["bug"])])]);
        app.org_repo_names = Some(vec!["r".into()]);
        app.switch_org("other".into());
        assert!(app.vocab.assignees.is_empty());
        assert!(app.vocab.labels.is_empty());
        assert!(app.org_repo_names.is_none());
        assert!(app.loaded.is_none());
    }

    #[test]
    fn move_targets_reach_repos_that_are_not_loaded() {
        let mut app = App::new("org".into(), None, false, false, String::new());
        app.set_data(vec![repo("r", vec![issue("ann", &[], &[])])]);
        app.selected = 1;
        assert!(app.move_targets().is_empty());
        app.org_repo_names = Some(vec!["r".into(), "s".into()]);
        assert_eq!(app.move_targets(), vec!["s"]);
    }

    #[test]
    fn known_repos_merge_loaded_seen_and_org_names() {
        let mut app = App::new("org".into(), None, false, false, String::new());
        app.vocab.repos.insert("seen".into());
        app.org_repo_names = Some(vec!["listed".into(), "seen".into()]);
        app.set_data(vec![RepoIssues {
            repo: "loaded".into(),
            repo_url: String::new(),
            issues: vec![],
        }]);
        assert_eq!(app.known_repos(), vec!["listed", "loaded", "seen"]);
    }
}
