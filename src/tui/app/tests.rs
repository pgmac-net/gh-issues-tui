//! Tests for the application state machine.
//!
//! These stay together rather than following each item into its submodule:
//! nearly all of them drive a whole `App` through the shared `two_repo_app`
//! and `app_with` fixtures, so they exercise the state machine as a unit
//! rather than any one module's functions.

use super::*;
use crate::provider::types::{PrSummary, RepoIssues};
use chrono::TimeZone;

fn issue(number: u64, title: &str, state: IssueState) -> Issue {
    Issue {
        id: format!("I_{number}"),
        number,
        title: title.into(),
        body: String::new(),
        state,
        url: format!("https://github.com/o/r/issues/{number}"),
        author: "pgmac".into(),
        assignees: vec![],
        labels: vec![],
        comment_count: 0,
        created_at: Utc
            .with_ymd_and_hms(2026, 6, number as u32 % 28 + 1, 0, 0, 0)
            .unwrap(),
        updated_at: Utc
            .with_ymd_and_hms(2026, 7, number as u32 % 28 + 1, 0, 0, 0)
            .unwrap(),
        closed_at: None,
    }
}

fn app_with(repos: Vec<RepoIssues>) -> App {
    let mut app = App::new(
        "org".into(),
        None,
        false,
        false,
        "{owner}/{repo}#{number}".into(),
    );
    app.set_data(repos);
    app
}

fn two_repo_app() -> App {
    app_with(vec![
        RepoIssues {
            repo: "alpha".into(),
            repo_url: "u".into(),
            issues: vec![
                issue(1, "first bug", IssueState::Open),
                issue(2, "feature idea", IssueState::Open),
            ],
        },
        RepoIssues {
            repo: "beta".into(),
            repo_url: "u".into(),
            issues: vec![issue(3, "docs fix", IssueState::Open)],
        },
    ])
}

#[test]
fn rows_group_by_repo_with_headers() {
    let app = two_repo_app();
    assert_eq!(app.rows.len(), 5); // 2 headers + 3 issues
    assert!(matches!(app.rows[0], Row::RepoHeader { repo_idx: 0 }));
    assert!(matches!(app.rows[3], Row::RepoHeader { repo_idx: 1 }));
}

#[test]
fn collapse_hides_issue_rows_but_keeps_header() {
    let mut app = two_repo_app();
    app.selected = 0; // alpha header
    app.toggle_collapse();
    assert_eq!(app.rows.len(), 3); // alpha header + beta header + beta issue
    app.toggle_collapse();
    assert_eq!(app.rows.len(), 5);
}

#[test]
fn jump_to_number_selects_matching_issue() {
    let mut app = two_repo_app();
    app.selected = 0;
    assert!(app.jump_to_number(3));
    assert_eq!(app.selected_issue().map(|i| i.number), Some(3));
    // Filters untouched — the list still holds every issue.
    assert!(!app.filters.is_active());
}

#[test]
fn jump_to_number_expands_collapsed_group() {
    let mut app = two_repo_app();
    // Collapse beta (holds #3), then jump into it.
    app.collapsed.insert("beta".into());
    app.rebuild_rows();
    app.selected = 0; // sit on the alpha side
    assert!(app.jump_to_number(3));
    assert_eq!(app.selected_issue().map(|i| i.number), Some(3));
    assert!(!app.collapsed.contains("beta"));
}

#[test]
fn jump_to_number_absent_returns_false() {
    let mut app = two_repo_app();
    app.selected = 2;
    let before = app.selected;
    assert!(!app.jump_to_number(999));
    assert_eq!(app.selected, before);
    assert_eq!(app.status.as_deref(), Some("no issue #999 loaded"));
}

#[test]
fn jump_to_number_prefers_current_repo_on_collision() {
    // Both repos own an issue numbered 7.
    let mut app = app_with(vec![
        RepoIssues {
            repo: "alpha".into(),
            repo_url: "u".into(),
            issues: vec![issue(7, "alpha seven", IssueState::Open)],
        },
        RepoIssues {
            repo: "beta".into(),
            repo_url: "u".into(),
            issues: vec![issue(7, "beta seven", IssueState::Open)],
        },
    ]);
    // Selection sits on the beta group → beta's #7 should win.
    let beta_header = app
        .rows
        .iter()
        .position(|r| matches!(r, Row::RepoHeader { repo_idx: 1 }))
        .unwrap();
    app.selected = beta_header;
    assert!(app.jump_to_number(7));
    assert_eq!(
        app.selected_repo().map(|r| r.repo.clone()),
        Some("beta".into())
    );
    assert_eq!(
        app.selected_issue().map(|i| i.title.clone()),
        Some("beta seven".into())
    );
}

/// #129: following `o/r#7` must land in *that* repo. `jump_to_number` would
/// have preferred the group the selection already sits in, which is the wrong
/// answer when the reference named the other one.
#[test]
fn jump_to_ref_stays_in_the_named_repo_against_the_current_one() {
    let mut app = app_with(vec![
        RepoIssues {
            repo: "alpha".into(),
            repo_url: "u".into(),
            issues: vec![issue(7, "alpha seven", IssueState::Open)],
        },
        RepoIssues {
            repo: "beta".into(),
            repo_url: "u".into(),
            issues: vec![issue(7, "beta seven", IssueState::Open)],
        },
    ]);
    let beta_header = app
        .rows
        .iter()
        .position(|r| matches!(r, Row::RepoHeader { repo_idx: 1 }))
        .unwrap();
    app.selected = beta_header;

    assert!(app.jump_to_ref(Some("alpha"), 7));
    assert_eq!(
        app.selected_issue().map(|i| i.title.clone()),
        Some("alpha seven".into()),
        "the reference named alpha, so beta's #7 must not win"
    );
}

/// The browser fallback depends on this returning false quietly — a status
/// message here would overwrite the "opened …" one the caller sets.
#[test]
fn jump_to_ref_is_silent_when_the_named_repo_is_not_loaded() {
    let mut app = two_repo_app();
    app.selected = 1;
    let before = app.selected;
    app.status = None;

    assert!(!app.jump_to_ref(Some("gamma"), 1));
    assert_eq!(app.selected, before);
    assert_eq!(
        app.status, None,
        "the caller owns the message, not the jump"
    );
}

#[test]
fn jump_to_number_reveals_filtered_out_issue() {
    let mut app = app_with(vec![RepoIssues {
        repo: "alpha".into(),
        repo_url: "u".into(),
        issues: vec![
            issue(1, "open one", IssueState::Open),
            issue(2, "closed two", IssueState::Closed),
        ],
    }]);
    // State filter Open hides the closed issue; a text filter hides it too.
    app.state_filter = StateFilter::Open;
    app.filters.text = "one".into();
    app.rebuild_rows();
    assert!(app.jump_to_number(2));
    // Reveal widened the view: filters cleared and state relaxed to All.
    assert!(!app.filters.is_active());
    assert_eq!(app.state_filter, StateFilter::All);
    assert_eq!(app.selected_issue().map(|i| i.number), Some(2));
}

#[test]
fn default_collapsed_starts_all_groups_folded() {
    let mut app = App::new(
        "org".into(),
        None,
        false,
        true,
        "{owner}/{repo}#{number}".into(),
    );
    app.set_data(vec![
        RepoIssues {
            repo: "alpha".into(),
            repo_url: "u".into(),
            issues: vec![issue(1, "a", IssueState::Open)],
        },
        RepoIssues {
            repo: "beta".into(),
            repo_url: "u".into(),
            issues: vec![issue(2, "b", IssueState::Open)],
        },
    ]);
    assert_eq!(app.rows.len(), 2); // headers only
    assert_eq!(app.visible_issue_count(), 0);
}

#[test]
fn default_collapsed_preserves_manual_expand_across_reload() {
    let repos = || {
        vec![
            RepoIssues {
                repo: "alpha".into(),
                repo_url: "u".into(),
                issues: vec![issue(1, "a", IssueState::Open)],
            },
            RepoIssues {
                repo: "beta".into(),
                repo_url: "u".into(),
                issues: vec![issue(2, "b", IssueState::Open)],
            },
        ]
    };
    let mut app = App::new(
        "org".into(),
        None,
        false,
        true,
        "{owner}/{repo}#{number}".into(),
    );
    app.set_data(repos());
    assert_eq!(app.visible_issue_count(), 0);

    app.selected = 0;
    app.toggle_collapse(); // user expands alpha
    assert_eq!(app.visible_issue_count(), 1);

    app.set_data(repos()); // reload must not re-collapse it
    assert_eq!(app.visible_issue_count(), 1);
}

#[test]
fn default_collapsed_applies_to_new_repo_on_reload() {
    let alpha = RepoIssues {
        repo: "alpha".into(),
        repo_url: "u".into(),
        issues: vec![issue(1, "a", IssueState::Open)],
    };
    let beta = RepoIssues {
        repo: "beta".into(),
        repo_url: "u".into(),
        issues: vec![issue(2, "b", IssueState::Open)],
    };
    let mut app = App::new(
        "org".into(),
        None,
        false,
        true,
        "{owner}/{repo}#{number}".into(),
    );
    app.set_data(vec![alpha.clone()]);
    assert!(!app.collapsed.contains("alpha")); // single group auto-expands

    app.set_data(vec![alpha, beta]); // beta appears for the first time
    assert!(!app.collapsed.contains("alpha"));
    assert!(app.collapsed.contains("beta"));
    assert_eq!(app.visible_issue_count(), 1);
}

#[test]
fn default_collapsed_single_repo_starts_expanded() {
    let mut app = App::new(
        "org".into(),
        None,
        false,
        true,
        "{owner}/{repo}#{number}".into(),
    );
    app.set_data(vec![RepoIssues {
        repo: "solo".into(),
        repo_url: "u".into(),
        issues: vec![issue(1, "a", IssueState::Open)],
    }]);
    assert!(!app.collapsed.contains("solo"));
    assert_eq!(app.visible_issue_count(), 1);
}

#[test]
fn default_collapsed_expands_only_repo_matching_initial_filter() {
    let mut app = App::new(
        "org".into(),
        Some("beta".into()),
        false,
        true,
        "{owner}/{repo}#{number}".into(),
    );
    app.set_data(vec![
        RepoIssues {
            repo: "alpha".into(),
            repo_url: "u".into(),
            issues: vec![issue(1, "a", IssueState::Open)],
        },
        RepoIssues {
            repo: "beta".into(),
            repo_url: "u".into(),
            issues: vec![issue(2, "b", IssueState::Open)],
        },
    ]);
    // beta is the single visible group → expanded; alpha still defaults
    // collapsed and shows once the filter is cleared.
    assert!(!app.collapsed.contains("beta"));
    assert!(app.collapsed.contains("alpha"));
    assert_eq!(app.visible_issue_count(), 1);

    app.filters.clear();
    app.rebuild_rows();
    assert_eq!(app.visible_issue_count(), 1); // beta open, alpha folded
    assert_eq!(app.rows.len(), 3); // two headers + beta's issue
}

#[test]
fn manual_collapse_of_single_repo_survives_reload() {
    let repos = || {
        vec![RepoIssues {
            repo: "solo".into(),
            repo_url: "u".into(),
            issues: vec![issue(1, "a", IssueState::Open)],
        }]
    };
    let mut app = App::new(
        "org".into(),
        None,
        false,
        true,
        "{owner}/{repo}#{number}".into(),
    );
    app.set_data(repos());
    assert_eq!(app.visible_issue_count(), 1); // auto-expanded

    app.selected = 0;
    app.toggle_collapse(); // user folds it
    assert_eq!(app.visible_issue_count(), 0);

    app.set_data(repos()); // reload must not force it back open
    assert_eq!(app.visible_issue_count(), 0);
}

#[test]
fn without_default_collapsed_groups_start_expanded() {
    let app = two_repo_app(); // uses default_collapsed = false
    assert_eq!(app.visible_issue_count(), 3);
}

#[test]
fn filtering_to_single_repo_expands_it() {
    let mut app = two_repo_app();
    app.set_all_collapsed(true);
    assert_eq!(app.visible_issue_count(), 0);

    // Repo filter leaving one visible group expands it.
    app.apply_filter_input(InputKind::FilterField(1), "beta");
    assert_eq!(app.visible_issue_count(), 1);
    assert!(!app.collapsed.contains("beta"));

    // Text search narrowing to one group expands too.
    app.set_all_collapsed(true);
    app.apply_filter_input(InputKind::FilterField(1), "");
    app.apply_filter_input(InputKind::Search, "docs");
    assert_eq!(app.visible_issue_count(), 1); // beta's "docs fix"
}

#[test]
fn filtering_to_multiple_repos_keeps_them_folded() {
    let mut app = two_repo_app();
    app.set_all_collapsed(true);
    // "a" substring-matches both alpha and beta — no auto-expand.
    app.apply_filter_input(InputKind::FilterField(1), "a");
    assert_eq!(app.visible_issue_count(), 0);
    assert_eq!(app.rows.len(), 2); // two folded headers
}

#[test]
fn manual_collapse_sticks_until_filters_change_again() {
    let mut app = two_repo_app();
    app.set_all_collapsed(true);
    app.apply_filter_input(InputKind::FilterField(1), "beta");
    assert_eq!(app.visible_issue_count(), 1); // auto-expanded

    app.selected = 0;
    app.toggle_collapse(); // user folds it — must stay folded
    assert_eq!(app.visible_issue_count(), 0);

    app.apply_filter_input(InputKind::Search, "docs"); // filters change
    assert_eq!(app.visible_issue_count(), 1); // re-expanded
}

#[test]
fn detail_pane_open_close_and_focus_cycle() {
    let mut app = two_repo_app();
    assert!(!app.detail.open);
    app.cycle_focus(); // split closed → no-op
    assert_eq!(app.focus, Focus::List);

    app.open_detail();
    assert!(app.detail.open);
    assert_eq!(app.focus, Focus::Detail);

    app.cycle_focus();
    assert_eq!(app.focus, Focus::List);
    app.cycle_focus();
    assert_eq!(app.focus, Focus::Detail);

    app.close_detail();
    assert!(!app.detail.open);
    assert_eq!(app.focus, Focus::List);
}

#[test]
fn switch_org_closes_detail_pane() {
    let mut app = two_repo_app();
    app.open_detail();
    app.switch_org("other".into());
    assert!(!app.detail.open);
    assert_eq!(app.focus, Focus::List);
}

#[test]
fn filtered_issue_count_includes_collapsed_groups() {
    let mut app = two_repo_app(); // 3 issues across alpha + beta
    app.set_all_collapsed(true);
    assert_eq!(app.visible_issue_count(), 0);
    assert_eq!(app.filtered_issue_count(), 3);

    app.filters.repo = "beta".into();
    app.rebuild_rows();
    assert_eq!(app.filtered_issue_count(), 1);

    app.filters.clear();
    app.filters.text = "bug".into();
    app.rebuild_rows();
    assert_eq!(app.filtered_issue_count(), 1);
}

#[test]
fn collapse_all_and_expand_all() {
    let mut app = two_repo_app();
    app.set_all_collapsed(true);
    assert_eq!(app.rows.len(), 2);
    app.set_all_collapsed(false);
    assert_eq!(app.rows.len(), 5);
}

#[test]
fn text_filter_matches_title_and_number() {
    let mut app = two_repo_app();
    app.filters.text = "bug".into();
    app.rebuild_rows();
    assert_eq!(app.visible_issue_count(), 1);

    app.filters.text = "#3".into();
    app.rebuild_rows();
    assert_eq!(app.visible_issue_count(), 1);
    assert_eq!(app.rows.len(), 2); // beta header + issue 3
}

#[test]
fn repo_filter_is_exact_when_text_names_a_repo() {
    let mut app = app_with(vec![
        RepoIssues {
            repo: "api".into(),
            repo_url: "u".into(),
            issues: vec![issue(1, "a", IssueState::Open)],
        },
        RepoIssues {
            repo: "api-gateway".into(),
            repo_url: "u".into(),
            issues: vec![issue(2, "b", IssueState::Open)],
        },
    ]);
    app.filters.repo = "api".into();
    app.rebuild_rows();
    assert_eq!(app.visible_issue_count(), 1);
    assert!(matches!(app.rows[0], Row::RepoHeader { repo_idx: 0 }));

    // Case-insensitive exact match still wins over substring.
    app.filters.repo = "API".into();
    app.rebuild_rows();
    assert_eq!(app.visible_issue_count(), 1);

    // No exact match → substring behavior matches both.
    app.filters.repo = "ap".into();
    app.rebuild_rows();
    assert_eq!(app.visible_issue_count(), 2);
}

#[test]
fn initial_repo_filter_applies_on_first_load() {
    let mut app = App::new(
        "org".into(),
        Some("beta".into()),
        false,
        false,
        "{owner}/{repo}#{number}".into(),
    );
    app.set_data(vec![
        RepoIssues {
            repo: "alpha".into(),
            repo_url: "u".into(),
            issues: vec![issue(1, "a", IssueState::Open)],
        },
        RepoIssues {
            repo: "beta".into(),
            repo_url: "u".into(),
            issues: vec![issue(2, "b", IssueState::Open)],
        },
    ]);
    assert!(app.filters.is_active());
    assert_eq!(app.visible_issue_count(), 1);
    assert!(matches!(app.rows[0], Row::RepoHeader { repo_idx: 1 }));

    app.filters.clear();
    app.rebuild_rows();
    assert_eq!(app.visible_issue_count(), 2);
}

#[test]
fn switch_org_resets_view_state() {
    let mut app = two_repo_app();
    app.filters.repo = "alpha".into();
    app.collapsed.insert("beta".into());
    app.state_filter = StateFilter::All;
    app.selected = 2;
    app.rebuild_rows();

    app.switch_org("other".into());
    assert_eq!(app.org, "other");
    assert!(app.repos.is_empty());
    assert!(app.rows.is_empty());
    assert!(app.collapsed.is_empty());
    assert!(app.seen_repos.is_empty());
    assert!(!app.filters.is_active());
    assert_eq!(app.state_filter, StateFilter::Open);
    assert_eq!(app.selected, 0);
    assert!(app.loading);
}

#[test]
fn repo_filter_hides_whole_group() {
    let mut app = two_repo_app();
    app.filters.repo = "alph".into();
    app.rebuild_rows();
    assert_eq!(app.rows.len(), 3);
    assert!(matches!(app.rows[0], Row::RepoHeader { repo_idx: 0 }));
}

#[test]
fn state_filter_cycles_and_filters() {
    let mut app = app_with(vec![RepoIssues {
        repo: "r".into(),
        repo_url: "u".into(),
        issues: vec![
            issue(1, "open one", IssueState::Open),
            issue(2, "closed one", IssueState::Closed),
        ],
    }]);
    assert_eq!(app.visible_issue_count(), 1);
    app.state_filter = app.state_filter.next(); // closed
    app.rebuild_rows();
    assert_eq!(app.visible_issue_count(), 1);
    app.state_filter = app.state_filter.next(); // all
    app.rebuild_rows();
    assert_eq!(app.visible_issue_count(), 2);
}

#[test]
fn assignee_and_author_filters() {
    let mut a = issue(1, "a", IssueState::Open);
    a.assignees = vec!["pgmac".into()];
    let mut b = issue(2, "b", IssueState::Open);
    b.author = "someone".into();
    let mut app = app_with(vec![RepoIssues {
        repo: "r".into(),
        repo_url: "u".into(),
        issues: vec![a, b],
    }]);

    app.filters.assignee = "PGMAC".into(); // case-insensitive
    app.rebuild_rows();
    assert_eq!(app.visible_issue_count(), 1);

    app.filters.clear();
    app.filters.author = "someone".into();
    app.rebuild_rows();
    assert_eq!(app.visible_issue_count(), 1);
}

#[test]
fn date_filters_bound_created() {
    let mut app = two_repo_app(); // created 2026-06-02, 06-03, 06-04
    app.filters.created_after = parse_date("2026-06-03");
    app.rebuild_rows();
    assert_eq!(app.visible_issue_count(), 2);
    app.filters.created_before = parse_date("2026-06-03");
    app.rebuild_rows();
    assert_eq!(app.visible_issue_count(), 1);
}

#[test]
fn closed_date_filter_excludes_never_closed() {
    let mut app = two_repo_app();
    app.filters.closed_after = parse_date("2020-01-01");
    app.rebuild_rows();
    assert_eq!(app.visible_issue_count(), 0);
}

#[test]
fn sort_by_created_ascending_and_descending() {
    let mut issues = vec![
        issue(3, "c", IssueState::Open),
        issue(1, "a", IssueState::Open),
        issue(2, "b", IssueState::Open),
    ];
    sort_issues(&mut issues, SortKey::Created, false);
    assert_eq!(
        issues.iter().map(|i| i.number).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    sort_issues(&mut issues, SortKey::Created, true);
    assert_eq!(
        issues.iter().map(|i| i.number).collect::<Vec<_>>(),
        vec![3, 2, 1]
    );
}

fn priority_issue(number: u64, priority: Option<&str>) -> Issue {
    let mut i = issue(number, "t", IssueState::Open);
    if let Some(p) = priority {
        i.labels = vec![crate::provider::types::Label {
            name: format!("priority:{p}"),
            color: String::new(),
        }];
    }
    i
}

#[test]
fn sort_by_priority_descending_and_ascending() {
    let mut issues = vec![
        priority_issue(1, Some("low")),
        priority_issue(2, Some("urgent")),
        priority_issue(3, Some("medium")),
        priority_issue(4, Some("high")),
        priority_issue(5, None),
    ];
    sort_issues(&mut issues, SortKey::Priority, true);
    assert_eq!(
        issues.iter().map(|i| i.number).collect::<Vec<_>>(),
        vec![2, 4, 3, 1, 5]
    );
    sort_issues(&mut issues, SortKey::Priority, false);
    assert_eq!(
        issues.iter().map(|i| i.number).collect::<Vec<_>>(),
        vec![5, 1, 3, 4, 2]
    );
}

#[test]
fn sort_by_priority_unknown_value_ranks_with_unsorted() {
    let mut issues = vec![
        priority_issue(1, Some("P1")),
        priority_issue(2, Some("low")),
    ];
    sort_issues(&mut issues, SortKey::Priority, true);
    assert_eq!(
        issues.iter().map(|i| i.number).collect::<Vec<_>>(),
        vec![2, 1]
    );
}

#[test]
fn priority_ties_break_by_updated_desc_in_both_directions() {
    // updated_at grows with the issue number in the test helper.
    let mut issues = vec![
        priority_issue(1, Some("high")),
        priority_issue(3, Some("high")),
        priority_issue(2, Some("high")),
    ];
    sort_issues(&mut issues, SortKey::Priority, true);
    assert_eq!(
        issues.iter().map(|i| i.number).collect::<Vec<_>>(),
        vec![3, 2, 1]
    );
    sort_issues(&mut issues, SortKey::Priority, false);
    assert_eq!(
        issues.iter().map(|i| i.number).collect::<Vec<_>>(),
        vec![3, 2, 1]
    );
}

#[test]
fn sort_key_cycle_covers_all_keys_and_wraps() {
    let mut key = SortKey::Updated;
    let mut seen = vec![key];
    loop {
        key = key.next();
        if key == SortKey::Updated {
            break;
        }
        seen.push(key);
    }
    assert_eq!(seen.len(), 7);
    assert!(seen.contains(&SortKey::Priority));
}

#[test]
fn sort_by_author() {
    let mut a = issue(1, "a", IssueState::Open);
    a.author = "zed".into();
    let mut b = issue(2, "b", IssueState::Open);
    b.author = "amy".into();
    let mut issues = vec![a, b];
    sort_issues(&mut issues, SortKey::Author, false);
    assert_eq!(issues[0].author, "amy");
}

#[test]
fn selection_clamps_after_filter_shrinks_rows() {
    let mut app = two_repo_app();
    app.selected = 4;
    app.filters.text = "docs".into();
    app.rebuild_rows();
    assert!(app.selected < app.rows.len());
}

#[test]
fn selected_issue_none_on_header() {
    let mut app = two_repo_app();
    app.selected = 0;
    assert!(app.selected_issue().is_none());
    app.selected = 1;
    assert_eq!(app.selected_issue().unwrap().number, 2); // sorted updated desc
}

#[test]
fn selected_short_ref_none_on_header() {
    let mut app = two_repo_app();
    app.selected = 0;
    assert!(app.selected_short_ref().is_none());
}

#[test]
fn selected_short_ref_default_format() {
    let mut app = two_repo_app();
    app.selected = 1; // "alpha" issue #2
    assert_eq!(app.selected_short_ref().unwrap(), "org/alpha#2");
}

#[test]
fn selected_short_ref_custom_format() {
    let mut app = two_repo_app();
    app.selected = 1;
    app.copy_format = "{repo}#{number} ({owner})".into();
    assert_eq!(app.selected_short_ref().unwrap(), "alpha#2 (org)");
}

#[test]
fn filter_input_round_trip() {
    let mut app = two_repo_app();
    app.apply_filter_input(InputKind::FilterField(4), "2026-06-03");
    assert_eq!(app.current_filter_value(4), "2026-06-03");
    app.apply_filter_input(InputKind::FilterField(4), "");
    assert_eq!(app.current_filter_value(4), "");
}

#[test]
fn filter_input_priority_parses_comma_list() {
    let mut app = two_repo_app();
    app.apply_filter_input(InputKind::FilterField(4), "high, urgent, ,");
    assert_eq!(app.filters.priority, vec!["high", "urgent"]);
    assert_eq!(app.current_filter_value(4), "high, urgent");
}

#[test]
fn apply_multi_filter_sets_and_clears() {
    let mut app = two_repo_app();
    app.apply_multi_filter(5, vec!["blocked".into(), "in-progress".into()]);
    assert_eq!(app.filters.status, vec!["blocked", "in-progress"]);
    assert!(app.filters.is_active());
    app.apply_multi_filter(5, Vec::new());
    assert!(app.filters.status.is_empty());
    assert!(!app.filters.is_active());
}

#[test]
fn input_scroll_skip_keeps_cursor_in_window() {
    // Cursor within the first window: no scroll.
    assert_eq!(input_scroll_skip(0, 10), 0);
    assert_eq!(input_scroll_skip(9, 10), 0);
    // Cursor past the window: skip advances to keep it on the last column.
    assert_eq!(input_scroll_skip(10, 10), 1);
    assert_eq!(input_scroll_skip(25, 10), 16);
    // Zero width is treated as one column wide.
    assert_eq!(input_scroll_skip(5, 0), 5);
}

#[test]
fn input_state_edits_utf8_safely() {
    let mut input = InputState::default();
    input.start("héllo");
    input.left();
    input.backspace(); // remove second 'l'
    assert_eq!(input.buffer, "hélo");
    input.insert('x'); // cursor sits before the final 'o'
    assert_eq!(input.buffer, "hélxo");
}

fn input(text: &str, cursor: usize) -> InputState {
    InputState {
        buffer: text.to_string(),
        cursor,
    }
}

#[test]
fn word_motion_is_whitespace_delimited() {
    let mut i = input("foo-bar  baz héllo", 18);
    i.word_left();
    assert_eq!(i.cursor, 13); // start of "héllo"
    i.word_left();
    assert_eq!(i.cursor, 9); // start of "baz"
    i.word_left();
    assert_eq!(i.cursor, 0); // "foo-bar" is one word
    i.word_right();
    assert_eq!(i.cursor, 7); // end of "foo-bar"
    i.word_right();
    assert_eq!(i.cursor, 12); // end of "baz"
}

#[test]
fn delete_word_back_removes_word_and_gap() {
    let mut i = input("one two  three", 14);
    i.delete_word_back();
    assert_eq!(i.buffer, "one two  ");
    assert_eq!(i.cursor, 9);
    i.delete_word_back();
    assert_eq!(i.buffer, "one ");
    i.delete_word_back();
    assert_eq!(i.buffer, "");
    i.delete_word_back(); // no-op at start
    assert_eq!(i.buffer, "");
}

#[test]
fn kill_to_start_and_end() {
    let mut i = input("héllo world", 6);
    i.kill_to_end();
    assert_eq!(i.buffer, "héllo ");
    assert_eq!(i.cursor, 6);
    i.cursor = 3;
    i.kill_to_start();
    assert_eq!(i.buffer, "lo ");
    assert_eq!(i.cursor, 0);
}

#[test]
fn delete_char_under_cursor() {
    let mut i = input("héllo", 1);
    i.delete_char();
    assert_eq!(i.buffer, "hllo");
    assert_eq!(i.cursor, 1);
    i.end();
    i.delete_char(); // no-op at end
    assert_eq!(i.buffer, "hllo");
}

#[test]
fn home_and_end() {
    let mut i = input("abc", 1);
    i.home();
    assert_eq!(i.cursor, 0);
    i.end();
    assert_eq!(i.cursor, 3);
}

#[test]
fn body_delete_char_merges_next_line_at_eol() {
    let mut b = BodyEditor::default();
    for c in "ab".chars() {
        b.insert(c);
    }
    b.newline();
    for c in "cd".chars() {
        b.insert(c);
    }
    b.line = 0;
    b.lines[0].end();
    b.delete_char();
    assert_eq!(b.text(), "abcd");
    assert_eq!(b.lines.len(), 1);
}

#[test]
fn wrap_lines_breaks_at_word_boundary() {
    let lines = vec![input("aaa bbb ccc", 0)];
    let rows = wrap_lines(&lines, 5);
    // "aaa bbb ccc" at width 5 → "aaa " / "bbb " / "ccc"
    assert_eq!(
        rows,
        vec![
            VisualRow {
                line: 0,
                start: 0,
                end: 4
            },
            VisualRow {
                line: 0,
                start: 4,
                end: 8
            },
            VisualRow {
                line: 0,
                start: 8,
                end: 11
            },
        ]
    );
}

#[test]
fn wrap_lines_hard_breaks_long_words_and_keeps_empty_lines() {
    let lines = vec![input("abcdefghij", 0), input("", 0)];
    let rows = wrap_lines(&lines, 4);
    assert_eq!(rows.len(), 4); // 3 hard-broken rows + 1 empty row
    assert_eq!(
        rows[0],
        VisualRow {
            line: 0,
            start: 0,
            end: 4
        }
    );
    assert_eq!(
        rows[2],
        VisualRow {
            line: 0,
            start: 8,
            end: 10
        }
    );
    assert_eq!(
        rows[3],
        VisualRow {
            line: 1,
            start: 0,
            end: 0
        }
    );
}

#[test]
fn wrap_lines_exact_width_does_not_split() {
    let lines = vec![input("abcd", 0)];
    assert_eq!(wrap_lines(&lines, 4).len(), 1);
}

#[test]
fn cursor_row_maps_wrap_boundary_to_next_row() {
    let lines = vec![input("aaa bbb", 0)];
    let rows = wrap_lines(&lines, 5); // rows: "aaa " / "bbb"
    assert_eq!(rows.len(), 2);
    assert_eq!(cursor_row(&rows, 0, 2), (0, 2));
    // Cursor at the boundary char index 4 belongs to the second row.
    assert_eq!(cursor_row(&rows, 0, 4), (1, 0));
    // End of the line stays on its final row.
    assert_eq!(cursor_row(&rows, 0, 7), (1, 3));
}

#[test]
fn visual_up_down_walk_wrapped_rows() {
    let mut b = BodyEditor::default();
    for c in "aaa bbb ccc".chars() {
        b.insert(c);
    }
    // width 5 → rows "aaa " / "bbb " / "ccc"; cursor at end (11) = row 2 col 3.
    b.up_visual(5);
    assert_eq!(b.lines[0].cursor, 7); // row 1 col 3 = char 4+3
    b.up_visual(5);
    assert_eq!(b.lines[0].cursor, 3); // row 0 col 3
    b.up_visual(5); // no-op on first row
    assert_eq!(b.lines[0].cursor, 3);
    b.down_visual(5);
    assert_eq!(b.lines[0].cursor, 7);
    b.down_visual(5);
    assert_eq!(b.lines[0].cursor, 11);
    b.down_visual(5); // no-op on last row
    assert_eq!(b.lines[0].cursor, 11);
}

#[test]
fn visual_down_crosses_logical_lines() {
    let mut b = BodyEditor::default();
    for c in "short".chars() {
        b.insert(c);
    }
    b.newline();
    for c in "aaaa bbbb".chars() {
        b.insert(c);
    }
    b.line = 0;
    b.lines[0].cursor = 5;
    b.down_visual(6); // into line 1's first row "aaaa "
    assert_eq!((b.line, b.lines[1].cursor), (1, 4)); // clamped to end-1 of non-final row
    b.down_visual(6); // into "bbbb"
    assert_eq!((b.line, b.lines[1].cursor), (1, 9));
}

fn repo_label(name: &str) -> RepoLabel {
    RepoLabel {
        id: format!("L_{name}"),
        name: name.into(),
    }
}

#[test]
fn priority_set_options_filters_sorts_and_prepends_clear() {
    let labels = vec![
        repo_label("bug"),
        repo_label("priority:urgent"),
        repo_label("priority:low"),
        repo_label("priority:aardvark"),
        repo_label("priority:high"),
        repo_label("status:blocked"),
    ];
    assert_eq!(
        priority_set_options(&labels),
        vec![
            "\u{2014}",
            "priority:low",
            "priority:high",
            "priority:urgent",
            "priority:aardvark",
        ]
    );
}

#[test]
fn priority_set_options_empty_repo_is_clear_only() {
    assert_eq!(priority_set_options(&[repo_label("bug")]), vec!["\u{2014}"]);
}

#[test]
fn priority_label_set_replaces_existing_priority() {
    let mut i = issue(1, "a", IssueState::Open);
    i.labels = vec![
        crate::provider::types::Label {
            name: "bug".into(),
            color: "".into(),
        },
        crate::provider::types::Label {
            name: "Priority:Low".into(),
            color: "".into(),
        },
    ];
    assert_eq!(
        priority_label_set(&i, Some("priority:high")),
        vec!["bug", "priority:high"]
    );
    // None clears the priority and keeps everything else.
    assert_eq!(priority_label_set(&i, None), vec!["bug"]);
}

#[test]
fn priority_label_set_adds_when_none_present() {
    let mut i = issue(1, "a", IssueState::Open);
    i.labels = vec![crate::provider::types::Label {
        name: "bug".into(),
        color: "".into(),
    }];
    assert_eq!(
        priority_label_set(&i, Some("priority:urgent")),
        vec!["bug", "priority:urgent"]
    );
}

/// Filter values for `label_filter_matches` tests.
fn fv(vals: &[&str]) -> Vec<String> {
    vals.iter().map(|s| s.to_string()).collect()
}

#[test]
fn label_filter_matches_bare_value() {
    let mut issue = issue(1, "a", IssueState::Open);
    issue.labels = vec![crate::provider::types::Label {
        name: "priority:high".into(),
        color: "".into(),
    }];
    assert!(super::label_filter_matches(
        &issue,
        "priority",
        &fv(&["high"])
    ));
    assert!(super::label_filter_matches(
        &issue,
        "priority",
        &fv(&["priority:high"])
    ));
    assert!(!super::label_filter_matches(
        &issue,
        "priority",
        &fv(&["low"])
    ));
    assert!(super::label_filter_matches(&issue, "priority", &[]));
}

#[test]
fn label_filter_matches_any_of_several_values() {
    let mut issue = issue(4, "d", IssueState::Open);
    issue.labels = vec![crate::provider::types::Label {
        name: "priority:urgent".into(),
        color: "".into(),
    }];
    assert!(super::label_filter_matches(
        &issue,
        "priority",
        &fv(&["high", "urgent"])
    ));
    assert!(!super::label_filter_matches(
        &issue,
        "priority",
        &fv(&["high", "medium"])
    ));
}

#[test]
fn label_filter_matches_status() {
    let mut issue = issue(2, "b", IssueState::Open);
    issue.labels = vec![crate::provider::types::Label {
        name: "status:needs-review".into(),
        color: "".into(),
    }];
    assert!(super::label_filter_matches(
        &issue,
        "status",
        &fv(&["needs-review"])
    ));
    assert!(super::label_filter_matches(
        &issue,
        "status",
        &fv(&["status:needs-review"])
    ));
    assert!(!super::label_filter_matches(
        &issue,
        "status",
        &fv(&["blocked"])
    ));
}

#[test]
fn label_filter_matches_is_case_insensitive() {
    let mut issue = issue(3, "c", IssueState::Open);
    issue.labels = vec![crate::provider::types::Label {
        name: "Priority:High".into(),
        color: "".into(),
    }];
    assert!(super::label_filter_matches(
        &issue,
        "priority",
        &fv(&["high"])
    ));
    assert!(super::label_filter_matches(
        &issue,
        "priority",
        &fv(&["HIGH"])
    ));
}

#[test]
fn compute_repo_options() {
    let app = two_repo_app();
    let opts = app.compute_select_options(1);
    assert_eq!(opts.len(), 3);
    assert_eq!(opts[0], "\u{2014}");
    assert!(opts.contains(&"alpha".to_string()));
    assert!(opts.contains(&"beta".to_string()));
}

#[test]
fn compute_assignee_options() {
    let mut a = issue(1, "a", IssueState::Open);
    a.assignees = vec!["bob".into(), "alice".into()];
    let mut b = issue(2, "b", IssueState::Open);
    b.assignees = vec!["bob".into()];
    let app = app_with(vec![RepoIssues {
        repo: "r".into(),
        repo_url: "u".into(),
        issues: vec![a, b],
    }]);
    let opts = app.compute_select_options(2);
    assert_eq!(opts[0], "\u{2014}");
    assert!(opts.contains(&"alice".to_string()));
    assert!(opts.contains(&"bob".to_string()));
    assert_eq!(opts.len(), 3);
}

#[test]
fn compute_author_options() {
    let mut a = issue(1, "a", IssueState::Open);
    a.author = "pgmac".into();
    let mut b = issue(2, "b", IssueState::Open);
    b.author = "someone".into();
    let app = app_with(vec![RepoIssues {
        repo: "r".into(),
        repo_url: "u".into(),
        issues: vec![a, b],
    }]);
    let opts = app.compute_select_options(3);
    assert_eq!(opts[0], "\u{2014}");
    assert!(opts.contains(&"pgmac".to_string()));
    assert!(opts.contains(&"someone".to_string()));
    assert_eq!(opts.len(), 3);
}

#[test]
fn compute_priority_options() {
    let mut a = issue(1, "a", IssueState::Open);
    a.labels = vec![crate::provider::types::Label {
        name: "priority:high".into(),
        color: "".into(),
    }];
    let mut b = issue(2, "b", IssueState::Open);
    b.labels = vec![crate::provider::types::Label {
        name: "priority:low".into(),
        color: "".into(),
    }];
    let app = app_with(vec![RepoIssues {
        repo: "r".into(),
        repo_url: "u".into(),
        issues: vec![a, b],
    }]);
    // Multi-select options: no "—" row, rank-ordered low → urgent.
    let opts = app.compute_multi_options(4);
    assert_eq!(opts, vec!["low".to_string(), "high".to_string()]);
}

#[test]
fn compute_priority_options_rank_order_unknown_last() {
    let mut a = issue(1, "a", IssueState::Open);
    a.labels = ["priority:urgent", "priority:medium", "priority:P1"]
        .iter()
        .map(|n| crate::provider::types::Label {
            name: n.to_string(),
            color: "".into(),
        })
        .collect();
    let app = app_with(vec![RepoIssues {
        repo: "r".into(),
        repo_url: "u".into(),
        issues: vec![a],
    }]);
    assert_eq!(
        app.compute_multi_options(4),
        vec!["medium".to_string(), "urgent".to_string(), "P1".to_string()]
    );
}

#[test]
fn compute_status_options() {
    let mut a = issue(1, "a", IssueState::Open);
    a.labels = vec![crate::provider::types::Label {
        name: "status:needs-review".into(),
        color: "".into(),
    }];
    let app = app_with(vec![RepoIssues {
        repo: "r".into(),
        repo_url: "u".into(),
        issues: vec![a],
    }]);
    let opts = app.compute_multi_options(5);
    assert_eq!(opts, vec!["needs-review".to_string()]);
}

#[test]
fn compute_multi_options_empty_when_no_label_match() {
    let app = two_repo_app();
    assert!(app.compute_multi_options(4).is_empty());
}

#[test]
fn collapse_from_child_row_selects_group_header() {
    let mut app = two_repo_app();
    app.selected = 2; // second issue inside alpha
    app.set_current_collapsed(true);
    assert_eq!(app.selected, 0); // alpha header
    assert!(matches!(
        app.rows[app.selected],
        Row::RepoHeader { repo_idx: 0 }
    ));
}

#[test]
fn expand_via_set_current_collapsed_keeps_selection() {
    let mut app = two_repo_app();
    app.selected = 0;
    app.set_current_collapsed(true);
    app.set_current_collapsed(false);
    assert_eq!(app.selected, 0);
    assert_eq!(app.visible_issue_count(), 3);
}

#[test]
fn label_values_handles_mixed_case_prefix() {
    let mut a = issue(1, "a", IssueState::Open);
    a.labels = vec![crate::provider::types::Label {
        name: "Priority:High".into(),
        color: "".into(),
    }];
    let app = app_with(vec![RepoIssues {
        repo: "r".into(),
        repo_url: "u".into(),
        issues: vec![a],
    }]);
    let opts = app.compute_multi_options(4);
    assert_eq!(opts, vec!["High".to_string()]);
}

#[test]
fn is_select_field_returns_correct_bool() {
    assert!(!App::is_select_field(0)); // text
    assert!(App::is_select_field(1)); // repo
    assert!(App::is_select_field(2)); // assignee
    assert!(App::is_select_field(3)); // author
    assert!(!App::is_select_field(4)); // priority is multi now
    assert!(!App::is_select_field(5)); // status is multi now
    assert!(!App::is_select_field(6)); // created after
    assert!(!App::is_multi_select_field(3)); // author
    assert!(App::is_multi_select_field(4)); // priority
    assert!(App::is_multi_select_field(5)); // status
    assert!(!App::is_multi_select_field(6)); // created after
}

#[test]
fn enter_detail_on_header_is_none_and_keeps_pane_closed() {
    let mut app = two_repo_app();
    app.selected = 0; // repo header
    assert_eq!(app.enter_detail(), None);
    assert!(!app.detail.open);
    assert_eq!(app.focus, Focus::List);
}

#[test]
fn enter_detail_opens_closed_pane_and_requests_comments() {
    let mut app = two_repo_app();
    app.selected = 1; // first issue row
    app.repos[0].issues[0].comment_count = 1; // otherwise the fetch is skipped
    let expected = app.selected_issue().unwrap().id.clone();
    assert_eq!(app.enter_detail(), Some(expected));
    assert!(app.detail.open);
    assert_eq!(app.focus, Focus::Detail);
}

#[test]
fn enter_detail_on_open_pane_just_moves_focus() {
    let mut app = two_repo_app();
    app.selected = 1;
    app.open_detail();
    app.focus = Focus::List; // as after ← backing out
    assert_eq!(app.enter_detail(), None); // no comment refetch
    assert!(app.detail.open);
    assert_eq!(app.focus, Focus::Detail);
}

#[test]
fn start_comment_editor_on_header_is_none_and_keeps_pane_closed() {
    let mut app = two_repo_app();
    app.selected = 0; // repo header
    assert_eq!(app.start_comment_editor(), None);
    assert!(!app.detail.open);
    assert_eq!(app.mode, Mode::Normal);
}

#[test]
fn start_comment_editor_opens_closed_pane_and_requests_comments() {
    let mut app = two_repo_app();
    app.selected = 1; // first issue row
    app.repos[0].issues[0].comment_count = 1; // otherwise the fetch is skipped
    let expected = app.selected_issue().unwrap().id.clone();
    assert_eq!(app.start_comment_editor(), Some(expected));
    assert!(app.detail.open);
    assert_eq!(app.focus, Focus::Detail);
    assert_eq!(app.mode, Mode::CommentEditor);
    assert_eq!(app.editor.focus, CommentFocus::Editor);
}

#[test]
fn start_comment_editor_on_open_pane_keeps_comments_and_skips_refetch() {
    let mut app = two_repo_app();
    app.selected = 1;
    app.open_detail();
    app.detail.comments = Some(vec![]);
    assert_eq!(app.start_comment_editor(), None); // no comment refetch
    assert!(app.detail.open);
    assert_eq!(app.mode, Mode::CommentEditor);
    assert_eq!(app.detail.comments.as_ref().map(Vec::len), Some(0));
}

#[test]
fn start_comment_editor_resets_stale_editor_content_and_focus() {
    let mut app = two_repo_app();
    app.selected = 1;
    app.editor.body.insert('x');
    app.editor.focus = CommentFocus::Save;
    app.start_comment_editor();
    assert_eq!(app.editor.body.text(), "");
    assert_eq!(app.editor.focus, CommentFocus::Editor);
}

fn comment(id: &str, body: &str) -> Comment {
    Comment {
        id: id.into(),
        author: "octocat".into(),
        created_at: Utc.with_ymd_and_hms(2026, 7, 22, 13, 6, 0).unwrap(),
        body: body.into(),
    }
}

/// Detail pane open on issue #1 (empty body) with two comments loaded.
fn detail_app_with_comments() -> App {
    let mut app = two_repo_app();
    app.selected = 1; // issue #1
    app.open_detail();
    app.detail.comments = Some(vec![
        comment("c1", "first\nsecond"),
        comment("c2", "only one line"),
    ]);
    app
}

#[test]
fn body_editor_from_text_splits_lines_and_ends_cursor() {
    let b = BodyEditor::from_text("hello\nworld");
    assert_eq!(b.text(), "hello\nworld");
    assert_eq!(b.line, 1);
    assert_eq!(b.lines[1].cursor, 5); // end of "world"
}

#[test]
fn body_editor_from_empty_text_is_default() {
    let b = BodyEditor::from_text("");
    assert_eq!(b.lines.len(), 1);
    assert_eq!(b.text(), "");
}

#[test]
fn detail_comment_count_reflects_loaded_thread() {
    let app = detail_app_with_comments();
    assert_eq!(app.detail.comment_count(), 2);
    let mut none = two_repo_app();
    none.selected = 1;
    none.open_detail();
    assert_eq!(none.detail.comment_count(), 0);
}

#[test]
fn select_detail_cycles_body_through_comments_and_wraps() {
    let mut app = detail_app_with_comments(); // body + 2 comments
    assert_eq!(app.detail.sel, DetailSel::Body);

    app.detail.select(1);
    assert_eq!(app.detail.sel, DetailSel::Comment(0));
    app.detail.select(1);
    assert_eq!(app.detail.sel, DetailSel::Comment(1));
    app.detail.select(1); // wraps back to body
    assert_eq!(app.detail.sel, DetailSel::Body);

    app.detail.select(-1); // wraps to last comment
    assert_eq!(app.detail.sel, DetailSel::Comment(1));
}

#[test]
fn select_detail_with_no_comments_stays_on_body() {
    let mut app = two_repo_app();
    app.selected = 1;
    app.open_detail();
    app.detail.comments = Some(vec![]);
    app.detail.select(1);
    assert_eq!(app.detail.sel, DetailSel::Body);
    app.detail.select(-1);
    assert_eq!(app.detail.sel, DetailSel::Body);
}

#[test]
fn scroll_body_clamps_to_zero_and_max() {
    let mut app = detail_app_with_comments();
    app.detail.scroll_body(-1, 10); // can't go above the top
    assert_eq!(app.detail.body_scroll, 0);
    app.detail.scroll_body(4, 10);
    assert_eq!(app.detail.body_scroll, 4);
    app.detail.scroll_body(100, 10); // clamped at max
    assert_eq!(app.detail.body_scroll, 10);
}

#[test]
fn scroll_comment_clamps_within_its_span() {
    let mut app = detail_app_with_comments();
    app.detail.sel = DetailSel::Comment(1);
    app.detail.snap_comment(20); // comment top offset
    // Span is [20, 25]; scrolling can't rise above the header.
    app.detail.scroll_comment(-5, 20, 25);
    assert_eq!(app.detail.comments_scroll, 20);
    app.detail.scroll_comment(3, 20, 25);
    assert_eq!(app.detail.comments_scroll, 23);
    app.detail.scroll_comment(100, 20, 25); // clamped at the bottom
    assert_eq!(app.detail.comments_scroll, 25);
}

#[test]
fn scroll_comment_that_fits_does_not_move() {
    let mut app = detail_app_with_comments();
    app.detail.snap_comment(8);
    // hi < lo (comment shorter than viewport): stays pinned to the top.
    app.detail.scroll_comment(5, 8, 3);
    assert_eq!(app.detail.comments_scroll, 8);
}

#[test]
fn clamp_detail_sel_falls_back_when_thread_shrinks() {
    let mut app = detail_app_with_comments();
    app.detail.sel = DetailSel::Comment(1);
    app.detail.comments = Some(vec![comment("c1", "only one now")]);
    app.detail.clamp_sel();
    assert_eq!(app.detail.sel, DetailSel::Comment(0));

    app.detail.comments = Some(vec![]);
    app.detail.clamp_sel();
    assert_eq!(app.detail.sel, DetailSel::Body);
}

#[test]
fn reset_detail_scroll_returns_to_body_top() {
    let mut app = detail_app_with_comments();
    app.detail.sel = DetailSel::Comment(1);
    app.detail.body_scroll = 4;
    app.detail.comments_scroll = 12;
    app.detail.reset_scroll();
    assert_eq!(app.detail.sel, DetailSel::Body);
    assert_eq!(app.detail.body_scroll, 0);
    assert_eq!(app.detail.comments_scroll, 0);
}

#[test]
fn start_edit_body_card_prefills_and_targets_body() {
    let mut app = detail_app_with_comments();
    if let Some(&Row::Issue {
        repo_idx,
        issue_idx,
    }) = app.rows.get(app.selected)
    {
        app.repos[repo_idx].issues[issue_idx].body = "current description".into();
    }
    app.detail.sel = DetailSel::Body;
    app.start_edit_selected_card();
    assert_eq!(app.mode, Mode::CommentEditor);
    assert_eq!(app.editor.target, EditorTarget::EditBody);
    assert_eq!(app.editor.body.text(), "current description");
}

#[test]
fn start_edit_comment_card_prefills_and_targets_comment_id() {
    let mut app = detail_app_with_comments();
    app.detail.sel = DetailSel::Comment(0); // first comment
    app.start_edit_selected_card();
    assert_eq!(app.mode, Mode::CommentEditor);
    assert_eq!(
        app.editor.target,
        EditorTarget::EditComment {
            comment_id: "c1".into()
        }
    );
    assert_eq!(app.editor.body.text(), "first\nsecond");
}

// `detail_split` moved to `tui::layout`, which owns all screen geometry;
// its tests moved with it.

#[test]
fn start_edit_selected_card_noop_when_pane_closed() {
    let mut app = two_repo_app();
    app.selected = 1;
    app.start_edit_selected_card();
    assert_eq!(app.mode, Mode::Normal);
}

#[test]
fn parse_date_rejects_garbage() {
    assert!(parse_date("not-a-date").is_none());
    assert!(parse_date("").is_none());
    assert_eq!(
        parse_date("2026-07-05"),
        NaiveDate::from_ymd_opt(2026, 7, 5)
    );
}

fn select_issue(app: &mut App, id: &str) {
    let idx = app
        .rows
        .iter()
        .position(|row| match row {
            Row::Issue {
                repo_idx,
                issue_idx,
            } => app.repos[*repo_idx].issues[*issue_idx].id == id,
            Row::RepoHeader { .. } => false,
        })
        .expect("issue row present");
    app.selected = idx;
}

#[test]
fn set_data_keeps_selection_on_same_issue() {
    let mut app = two_repo_app();
    select_issue(&mut app, "I_1");

    // Refresh delivers a new issue that sorts above the selected one.
    app.set_data(vec![
        RepoIssues {
            repo: "alpha".into(),
            repo_url: "u".into(),
            issues: vec![
                issue(1, "first bug", IssueState::Open),
                issue(2, "feature idea", IssueState::Open),
                issue(5, "brand new", IssueState::Open),
            ],
        },
        RepoIssues {
            repo: "beta".into(),
            repo_url: "u".into(),
            issues: vec![issue(3, "docs fix", IssueState::Open)],
        },
    ]);

    assert_eq!(app.selected_issue().map(|i| i.id.as_str()), Some("I_1"));
}

#[test]
fn set_data_clamps_when_selected_issue_vanishes() {
    let mut app = two_repo_app();
    select_issue(&mut app, "I_3"); // last row (beta's only issue)

    app.set_data(vec![RepoIssues {
        repo: "alpha".into(),
        repo_url: "u".into(),
        issues: vec![issue(1, "first bug", IssueState::Open)],
    }]);

    assert!(app.selected < app.rows.len());
    assert!(app.selected_issue().is_none_or(|i| i.id != "I_3"));
}

fn form_options() -> FormOptions {
    let id_name = |id: &str, name: &str| IdName {
        id: id.into(),
        name: name.into(),
    };
    FormOptions {
        repo_id: "R_repo".into(),
        labels: vec![
            id_name("L_bug", "bug"),
            id_name("L_enh", "enhancement"),
            id_name("L_ph", "priority:high"),
            id_name("L_pl", "priority:low"),
        ],
        users: vec![id_name("U_pgmac", "pgmac"), id_name("U_bot", "bot")],
        milestones: vec![id_name("M_1", "v1.0")],
        projects: vec![id_name("P_1", "Homelab")],
        issue_types: vec![id_name("T_bug", "Bug"), id_name("T_feat", "Feature")],
    }
}

#[test]
fn issue_form_opens_and_options_land() {
    let mut app = two_repo_app();
    app.open_issue_form("alpha".into());
    assert_eq!(app.mode, Mode::IssueForm);
    let form = app.issue_form.as_ref().unwrap();
    assert_eq!(form.repo, "alpha");
    assert!(form.options.is_none());
    assert!(form.field_options(3).is_empty()); // loading → empty

    app.set_form_options("alpha", form_options());
    let form = app.issue_form.as_ref().unwrap();
    assert_eq!(form.field_options(2), vec!["pgmac", "bot"]);
    assert_eq!(form.field_options(5), vec!["priority:high", "priority:low"]);
}

#[test]
fn stale_form_options_are_dropped() {
    let mut app = two_repo_app();
    app.open_issue_form("alpha".into());
    app.set_form_options("beta", form_options()); // stale: other repo
    assert!(app.issue_form.as_ref().unwrap().options.is_none());

    app.cancel_issue_form();
    app.set_form_options("alpha", form_options()); // stale: form closed
    assert!(app.issue_form.is_none());
    assert_eq!(app.mode, Mode::Normal);
}

#[test]
fn build_params_requires_options_and_title() {
    let mut form = IssueForm::new("alpha".into());
    form.title.start("hello");
    assert!(form.build_params().is_none()); // options not loaded

    form.options = Some(form_options());
    form.title.start("   ");
    assert!(form.build_params().is_none()); // blank title

    form.title.start("hello");
    let p = form.build_params().unwrap();
    assert_eq!(p.repo_id, "R_repo");
    assert_eq!(p.title, "hello");
    assert!(p.label_ids.is_empty() && p.assignee_ids.is_empty());
    assert!(p.milestone_id.is_none() && p.issue_type_id.is_none() && p.project_id.is_none());
}

#[test]
fn build_params_assembles_ids_and_merges_priority() {
    let mut form = IssueForm::new("alpha".into());
    form.options = Some(form_options());
    form.title.start("t");
    form.assignees.insert(0); // pgmac
    form.labels.insert(0); // bug
    form.priority = Some(0); // priority:high → L_ph
    form.issue_type = Some(1); // Feature
    form.project = Some(0);
    form.milestone = Some(0);

    let p = form.build_params().unwrap();
    assert_eq!(p.assignee_ids, vec!["U_pgmac"]);
    assert_eq!(p.label_ids, vec!["L_bug", "L_ph"]);
    assert_eq!(p.issue_type_id.as_deref(), Some("T_feat"));
    assert_eq!(p.project_id.as_deref(), Some("P_1"));
    assert_eq!(p.milestone_id.as_deref(), Some("M_1"));

    // Picking the same priority label in the labels field must not
    // duplicate its id.
    form.labels.insert(2); // priority:high via labels
    let p = form.build_params().unwrap();
    assert_eq!(
        p.label_ids.iter().filter(|i| *i == "L_ph").count(),
        1,
        "priority label id duplicated"
    );
}

#[test]
fn form_field_display_joins_multi_selections() {
    let mut form = IssueForm::new("alpha".into());
    form.options = Some(form_options());
    form.labels.insert(1);
    form.labels.insert(0);
    assert_eq!(form.field_display(3), "bug, enhancement");
    form.priority = Some(1);
    assert_eq!(form.field_display(5), "priority:low");
}

#[test]
fn body_editor_splits_merges_and_clamps() {
    let mut b = BodyEditor::default();
    for c in "hello".chars() {
        b.insert(c);
    }
    b.left();
    b.left(); // cursor after "hel"
    b.newline();
    assert_eq!(b.text(), "hel\nlo");
    assert_eq!(b.line, 1);
    assert_eq!(b.lines[1].cursor, 0);

    b.backspace(); // col 0 → merge back
    assert_eq!(b.text(), "hello");
    assert_eq!(b.line, 0);
    assert_eq!(b.lines[0].cursor, 3); // at the old split point

    b.newline();
    b.insert('x');
    b.up_visual(80); // wide enough that visual rows == logical lines
    assert_eq!(b.line, 0);
    b.down_visual(80);
    assert_eq!(b.line, 1);
    assert_eq!(b.text(), "hel\nxlo");
    assert_eq!(b.summary(), "hel (+1 more lines)");
}

#[test]
fn body_editor_handles_multibyte() {
    let mut b = BodyEditor::default();
    for c in "héllo".chars() {
        b.insert(c);
    }
    b.left();
    b.left();
    b.left(); // after "hé"
    b.newline();
    assert_eq!(b.text(), "hé\nllo");
    b.backspace();
    assert_eq!(b.text(), "héllo");
}

fn picker_app(options: &[&str]) -> App {
    let mut app = two_repo_app();
    app.picker
        .start(options.iter().map(|s| s.to_string()).collect(), 0);
    app
}

#[test]
fn picker_filter_narrows_and_maps_to_original_indices() {
    let mut app = picker_app(&["\u{2014}", "ansible", "budgeteer", "gh-issues-tui", "ghar"]);
    app.picker.filter_push('g');
    app.picker.filter_push('h');
    let filtered = app.picker.filtered();
    assert_eq!(
        filtered,
        vec![(3, "gh-issues-tui"), (4, "ghar")],
        "case-insensitive substring over original indices"
    );
    assert_eq!(app.picker.idx, 0); // reset to first match
    assert_eq!(app.picker.selected_original(), Some(3));

    app.picker.idx = 1;
    assert_eq!(app.picker.selected_original(), Some(4));
}

#[test]
fn picker_filter_matches_case_insensitively() {
    let mut app = picker_app(&["Docker-Nagios", "homelabia"]);
    app.picker.filter_push('N');
    app.picker.filter_push('A');
    assert_eq!(app.picker.filtered(), vec![(0, "Docker-Nagios")]);
}

#[test]
fn picker_backspace_and_clear_restore_and_clamp() {
    let mut app = picker_app(&["alpha", "beta"]);
    app.picker.idx = 1; // beta
    app.picker.filter_push('x'); // no matches
    assert!(app.picker.filtered().is_empty());
    assert_eq!(app.picker.selected_original(), None);

    app.picker.filter_backspace();
    assert_eq!(app.picker.filtered().len(), 2);
    assert!(app.picker.idx < 2); // clamped into range

    app.picker.filter_push('b');
    app.picker.filter_clear();
    assert_eq!(app.picker.filter, "");
    assert_eq!(app.picker.filtered().len(), 2);
}

#[test]
fn start_picker_resets_filter() {
    let mut app = picker_app(&["alpha"]);
    app.picker.filter_push('z');
    app.picker.start(vec!["beta".into()], 0);
    assert_eq!(app.picker.filter, "");
    assert_eq!(app.picker.filtered(), vec![(0, "beta")]);
}

fn app_with_empty_repo() -> App {
    app_with(vec![
        RepoIssues {
            repo: "alpha".into(),
            repo_url: "u".into(),
            issues: vec![issue(1, "first bug", IssueState::Open)],
        },
        RepoIssues {
            repo: "empty-repo".into(),
            repo_url: "u".into(),
            issues: vec![],
        },
    ])
}

#[test]
fn hide_empty_hides_and_toggle_reveals_zero_issue_repos() {
    let mut app = app_with_empty_repo();
    // Default: hidden — only alpha's header + issue.
    assert_eq!(app.rows.len(), 2);

    app.toggle_hide_empty();
    assert_eq!(app.rows.len(), 3); // + empty-repo header
    assert!(matches!(app.rows[2], Row::RepoHeader { repo_idx: 1 }));
    assert_eq!(app.repo_visible_count(1), 0);

    app.toggle_hide_empty();
    assert_eq!(app.rows.len(), 2);
}

#[test]
fn hide_empty_off_also_reveals_filtered_to_zero_groups() {
    let mut app = two_repo_app();
    app.filters.text = "docs".into(); // matches only beta's issue
    app.rebuild_rows();
    assert_eq!(app.rows.len(), 2); // beta header + its issue

    app.toggle_hide_empty();
    // alpha reappears as an empty group under the same rule.
    assert!(
        app.rows
            .iter()
            .any(|r| matches!(r, Row::RepoHeader { repo_idx: 0 }))
    );
    assert_eq!(app.repo_visible_count(0), 0);
}

#[test]
fn clear_filters_restores_config_default_not_false() {
    let mut app = app_with_empty_repo();
    app.set_hide_empty_default(false); // config says show empties
    app.rebuild_rows();
    assert_eq!(app.rows.len(), 3);
    assert!(!app.filters_active(), "config default is not 'active'");

    app.toggle_hide_empty(); // user hides them this session
    assert!(app.filters_active());

    app.clear_filters();
    app.rebuild_rows();
    assert!(!app.filters.hide_empty); // back to config default
    assert!(!app.filters_active());
    assert_eq!(app.rows.len(), 3);
}

#[test]
fn switch_org_restores_hide_empty_default() {
    let mut app = app_with_empty_repo();
    app.toggle_hide_empty();
    assert!(!app.filters.hide_empty);
    app.switch_org("other".into());
    assert!(app.filters.hide_empty); // default true restored
}

#[test]
fn filters_active_only_on_hide_empty_deviation() {
    let mut app = two_repo_app();
    assert!(!app.filters_active());
    app.toggle_hide_empty();
    assert!(app.filters_active());
    app.toggle_hide_empty();
    assert!(!app.filters_active());
}

#[test]
fn hide_empty_row_shows_yes_no_in_filter_menu() {
    let mut app = two_repo_app();
    assert_eq!(app.current_filter_value(FILTER_HIDE_EMPTY_IDX), "yes");
    app.toggle_hide_empty();
    assert_eq!(app.current_filter_value(FILTER_HIDE_EMPTY_IDX), "no");
}

#[test]
fn auto_refresh_blocked_in_form_modes() {
    let mut app = two_repo_app();
    assert!(app.should_auto_refresh());
    for mode in [
        Mode::IssueForm,
        Mode::IssueFormSelect(4),
        Mode::IssueFormMulti(2),
        Mode::CommentEditor,
    ] {
        app.mode = mode;
        assert!(!app.should_auto_refresh(), "{mode:?} must block refresh");
    }
}

#[test]
fn auto_refresh_gated_by_loading_rate_limit_and_mode() {
    let mut app = two_repo_app(); // set_data cleared `loading`
    assert!(app.should_auto_refresh());

    app.loading = true;
    assert!(!app.should_auto_refresh());
    app.loading = false;

    app.rate_limit_error = Some("rate limited".into());
    assert!(!app.should_auto_refresh());
    app.rate_limit_error = None;

    app.mode = Mode::Input(InputKind::Search);
    assert!(!app.should_auto_refresh());
    app.mode = Mode::ConfirmState;
    assert!(!app.should_auto_refresh());
    app.mode = Mode::Help;
    assert!(app.should_auto_refresh());
    app.mode = Mode::Normal;
    assert!(app.should_auto_refresh());
}

/// A resolved reference that is a PR — what most of these tests mean when
/// they say "the fetch landed".
fn sample_pr_lookup(pr: PrRef) -> PrLookup {
    PrLookup::Pr(Box::new(sample_pr_summary(pr)))
}

fn sample_pr_summary(pr: PrRef) -> PrSummary {
    PrSummary {
        pr,
        title: "t".into(),
        body: String::new(),
        state: crate::provider::types::PrState::Open,
        is_draft: false,
        base_ref: "main".into(),
        head_ref: "feature".into(),
        additions: 0,
        deletions: 0,
        changed_files: 0,
        comment_count: 0,
        review_thread_count: 0,
        reviews: Default::default(),
        checks: Default::default(),
        pr_runs: vec![],
        default_branch_name: "main".into(),
        default_branch_runs: vec![],
    }
}

/// #129: a bare `#N` means "this repo", so it resolves against the repo the
/// selected issue belongs to — not against whatever repo was mentioned last.
#[test]
fn collect_pr_links_resolves_bare_shorthand_against_the_selected_repo() {
    let mut app = two_repo_app();
    app.selected = 1; // first issue in alpha
    app.repos[0].issues[0].body = "closes #7 and pgmac-net/other#8".into();

    assert_eq!(
        app.collect_pr_links(),
        vec![
            PrRef {
                owner: "org".into(),
                repo: "alpha".into(),
                number: 7
            },
            PrRef {
                owner: "pgmac-net".into(),
                repo: "other".into(),
                number: 8
            },
        ]
    );
}

/// The repo's own URL is the source of truth for the owner, so a list that
/// mixes owners still resolves each thread's bare `#N` correctly.
#[test]
fn current_repo_takes_the_owner_from_the_repo_url() {
    let mut app = app_with(vec![RepoIssues {
        repo: "gh-issues-tui".into(),
        repo_url: "https://github.com/pgmac-net/gh-issues-tui".into(),
        issues: vec![issue(1, "a", IssueState::Open)],
    }]);
    app.selected = 1;
    assert_eq!(
        app.current_repo(),
        Some(("pgmac-net".into(), "gh-issues-tui".into()))
    );
}

#[test]
fn collect_pr_links_scans_body_then_comments_in_order() {
    let mut app = two_repo_app();
    app.selected = 1; // first issue in alpha
    {
        let issue = &mut app.repos[0].issues[0];
        issue.body = "see https://github.com/o/r/pull/1".into();
    }
    app.detail.comments = Some(vec![Comment {
        id: "c1".into(),
        author: "x".into(),
        created_at: Utc::now(),
        body: "also https://github.com/o/r2/pull/2".into(),
    }]);
    let links = app.collect_pr_links();
    assert_eq!(
        links,
        vec![
            PrRef {
                owner: "o".into(),
                repo: "r".into(),
                number: 1
            },
            PrRef {
                owner: "o".into(),
                repo: "r2".into(),
                number: 2
            },
        ]
    );
}

#[test]
fn open_pr_summary_sets_target_and_loading_state() {
    let mut app = two_repo_app();
    let pr = PrRef {
        owner: "o".into(),
        repo: "r".into(),
        number: 1,
    };
    app.open_pr_summary(pr.clone());
    assert_eq!(app.mode, Mode::PrSummary);
    assert_eq!(app.pr.target, Some(pr));
    assert!(app.pr.summary.is_none());
}

#[test]
fn open_pr_picker_populates_options_from_links() {
    let mut app = two_repo_app();
    let links = vec![
        PrRef {
            owner: "o".into(),
            repo: "r".into(),
            number: 1,
        },
        PrRef {
            owner: "o".into(),
            repo: "r".into(),
            number: 2,
        },
    ];
    app.open_pr_picker(links);
    assert_eq!(app.mode, Mode::PrPicker);
    assert_eq!(app.picker.options, vec!["o/r#1", "o/r#2"]);
}

#[test]
fn set_pr_summary_applies_only_to_current_target() {
    let mut app = two_repo_app();
    let pr1 = PrRef {
        owner: "o".into(),
        repo: "r".into(),
        number: 1,
    };
    let pr2 = PrRef {
        owner: "o".into(),
        repo: "r".into(),
        number: 2,
    };
    app.open_pr_summary(pr1.clone());
    // A response for a different PR (the popup retargeted before this
    // landed) must not overwrite the current summary.
    app.pr.set_summary(&pr2, Ok(sample_pr_lookup(pr2.clone())));
    assert!(app.pr.summary.is_none());

    app.pr.set_summary(&pr1, Ok(sample_pr_lookup(pr1.clone())));
    assert!(app.pr.summary.is_some());
}

#[test]
fn close_detail_clears_pr_state() {
    let mut app = two_repo_app();
    app.selected = 1;
    app.open_detail();
    let pr = PrRef {
        owner: "o".into(),
        repo: "r".into(),
        number: 1,
    };
    app.open_pr_summary(pr.clone());
    app.pr.set_summary(&pr.clone(), Ok(sample_pr_lookup(pr)));
    app.close_detail();
    assert!(app.pr.target.is_none());
    assert!(app.pr.summary.is_none());
}

/// A `sample_pr_summary` with two checks, one PR run, and one
/// default-branch run, so `pr_targets()` has more than the header row.
fn sample_pr_summary_with_checks(pr: PrRef) -> PrLookup {
    use crate::provider::types::{CheckContextInfo, CheckRollup, WorkflowRunInfo};
    PrLookup::Pr(Box::new(PrSummary {
        checks: CheckRollup {
            state: Some("FAILURE".into()),
            contexts: vec![
                CheckContextInfo {
                    name: "build".into(),
                    conclusion: "FAILURE".into(),
                    url: "https://github.com/o/r/runs/1".into(),
                },
                CheckContextInfo {
                    name: "test".into(),
                    conclusion: "SUCCESS".into(),
                    url: "https://github.com/o/r/runs/2".into(),
                },
            ],
        },
        pr_runs: vec![WorkflowRunInfo {
            workflow: "ci.yml".into(),
            run_number: 42,
            event: "push".into(),
            conclusion: Some("FAILURE".into()),
            created_at: Utc::now(),
            url: "https://github.com/o/r/actions/runs/42".into(),
        }],
        default_branch_runs: vec![WorkflowRunInfo {
            workflow: "release.yml".into(),
            run_number: 7,
            event: "push".into(),
            conclusion: Some("SUCCESS".into()),
            created_at: Utc::now(),
            url: "https://github.com/o/r/actions/runs/7".into(),
        }],
        ..sample_pr_summary(pr)
    }))
}

/// Targets as `ui::pr_targets` would report them: the PR header at row 0,
/// then rows further down the popup. Selection arithmetic only cares
/// about the sequence, so it is tested against plain data — the mapping
/// from a summary to these rows is `ui`'s to verify.
fn sample_targets() -> Vec<PrTarget> {
    [
        ("https://github.com/o/r/pull/1", 0u16),
        ("https://github.com/o/r/runs/1", 12),
        ("https://github.com/o/r/runs/2", 13),
        ("https://github.com/o/r/actions/runs/42", 16),
        ("https://github.com/o/r/actions/runs/7", 19),
    ]
    .into_iter()
    .map(|(url, line)| PrTarget {
        url: url.into(),
        line,
    })
    .collect()
}

#[test]
fn select_pr_target_wraps_and_snaps_scroll() {
    let mut app = two_repo_app();
    let targets = sample_targets();

    assert_eq!(app.pr.sel, 0);
    app.pr.select(1, &targets);
    assert_eq!(app.pr.sel, 1);
    assert_eq!(app.pr.scroll, targets[1].line);

    // Shift+Tab from the first row wraps to the last.
    app.pr.sel = 0;
    app.pr.select(-1, &targets);
    assert_eq!(app.pr.sel, targets.len() - 1);
    assert_eq!(app.pr.scroll, targets.last().unwrap().line);
}

#[test]
fn select_pr_target_is_a_noop_without_targets() {
    let mut app = two_repo_app();
    app.pr.select(1, &[]);
    assert_eq!(app.pr.sel, 0);
    assert_eq!(app.pr.scroll, 0);
}

#[test]
fn pr_selected_url_tracks_selection() {
    let mut app = two_repo_app();
    let targets = sample_targets();

    assert_eq!(
        app.pr.selected_url(&targets),
        Some("https://github.com/o/r/pull/1".to_string())
    );
    app.pr.select(1, &targets);
    assert_eq!(
        app.pr.selected_url(&targets),
        Some("https://github.com/o/r/runs/1".to_string())
    );
}

#[test]
fn pr_sel_resets_on_open_close_and_refresh() {
    let mut app = two_repo_app();
    let pr1 = PrRef {
        owner: "o".into(),
        repo: "r".into(),
        number: 1,
    };
    app.open_pr_summary(pr1.clone());
    app.pr
        .set_summary(&pr1, Ok(sample_pr_summary_with_checks(pr1.clone())));
    app.pr.select(1, &sample_targets());
    assert_eq!(app.pr.sel, 1);

    app.pr.refresh();
    assert_eq!(app.pr.sel, 0);
    assert!(app.pr.summary.is_none());

    app.pr
        .set_summary(&pr1, Ok(sample_pr_summary_with_checks(pr1.clone())));
    app.pr.select(1, &sample_targets());
    assert_eq!(app.pr.sel, 1);
    app.close_pr_summary();
    assert_eq!(app.pr.sel, 0);

    app.open_pr_summary(pr1.clone());
    app.pr
        .set_summary(&pr1.clone(), Ok(sample_pr_summary_with_checks(pr1)));
    app.pr.select(1, &sample_targets());
    assert_eq!(app.pr.sel, 1);
    let pr2 = PrRef {
        owner: "o".into(),
        repo: "r".into(),
        number: 2,
    };
    app.open_pr_summary(pr2);
    assert_eq!(app.pr.sel, 0);
}

// ---- comment thread cache (#107) ----

#[test]
fn load_comments_skips_the_fetch_when_the_issue_has_none() {
    let mut app = two_repo_app();
    app.selected = 1;
    let id = app.selected_issue().unwrap().id.clone();
    assert_eq!(app.repos[0].issues[0].comment_count, 0);

    // No request, and the pane shows a loaded-empty thread rather than
    // sitting on `None` waiting for a response that would never come.
    assert_eq!(app.load_comments(id), None);
    assert!(app.detail.comments.is_some());
    assert_eq!(app.detail.comment_count(), 0);
}

#[test]
fn load_comments_fetches_once_then_serves_from_cache() {
    let mut app = two_repo_app();
    app.selected = 1;
    app.repos[0].issues[0].comment_count = 2;
    let id = app.selected_issue().unwrap().id.clone();

    // First visit: a fetch is required and the pane waits.
    assert_eq!(app.load_comments(id.clone()), Some(id.clone()));
    assert!(app.detail.comments.is_none());

    app.cache_comments(id.clone(), vec![comment("c1", "one")]);

    // Second visit: served from cache, no request.
    assert_eq!(app.load_comments(id), None);
    assert_eq!(app.detail.comment_count(), 1);
}

#[test]
fn invalidate_comments_forces_the_next_load_to_refetch() {
    let mut app = two_repo_app();
    app.selected = 1;
    app.repos[0].issues[0].comment_count = 1;
    let id = app.selected_issue().unwrap().id.clone();
    app.cache_comments(id.clone(), vec![comment("c1", "one")]);
    assert_eq!(app.load_comments(id.clone()), None);

    app.invalidate_comments(&id);
    assert_eq!(app.load_comments(id.clone()), Some(id));
}

#[test]
fn set_data_clears_the_comment_cache() {
    let mut app = two_repo_app();
    app.selected = 1;
    app.repos[0].issues[0].comment_count = 1;
    let id = app.selected_issue().unwrap().id.clone();
    app.cache_comments(id.clone(), vec![comment("c1", "one")]);

    // A refetch can carry comments added elsewhere, so the cache goes.
    let repos = app.repos.clone();
    app.set_data(repos);
    assert!(app.comment_cache.is_empty());
    assert_eq!(app.load_comments(id.clone()), Some(id));
}

#[test]
fn switch_org_clears_the_comment_cache() {
    let mut app = two_repo_app();
    app.cache_comments("I_1".into(), vec![comment("c1", "one")]);
    app.switch_org("other-org".into());
    assert!(app.comment_cache.is_empty());
}
