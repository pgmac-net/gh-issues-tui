use super::prelude::*;
use super::widgets::centered;
use crate::provider::types::{PrState, PrSummary, WorkflowRunInfo};
use crate::tui::app::PrTarget;

pub(super) fn conclusion_style(conclusion: Option<&str>, t: &Theme) -> (&'static str, Color) {
    match conclusion.unwrap_or("PENDING") {
        "SUCCESS" => ("✔", t.open),
        "FAILURE" | "ERROR" | "TIMED_OUT" | "STARTUP_FAILURE" => ("✘", t.error),
        "CANCELLED" | "SKIPPED" | "NEUTRAL" | "STALE" => ("-", t.dim),
        _ => ("…", t.warning),
    }
}

/// Outer width of the PR summary popup, before its borders.
pub(super) const PR_SUMMARY_WIDTH: u16 = 76;

/// The PR summary popup's inner text width for a frame `frame_width` wide.
/// Shared by the renderer and the key handler so both measure the same rows.
pub fn pr_summary_inner_width(frame_width: u16) -> u16 {
    PR_SUMMARY_WIDTH.min(frame_width).saturating_sub(2)
}

/// The PR summary popup's outer area within `frame`.
pub(super) fn pr_summary_area(frame: Rect) -> Rect {
    centered(frame, PR_SUMMARY_WIDTH, (frame.height * 3 / 4).max(12))
}

/// One drawn row of the PR summary popup: the line as rendered, plus the URL
/// it opens when it is a navigable row.
///
/// This is the popup's single source of truth. The renderer draws
/// `rows[i].line`; [`pr_targets`] reports position `i` for every row carrying
/// a URL. The two cannot disagree, because there is only one sequence.
pub struct PrRow {
    pub line: Line<'static>,
    pub url: Option<String>,
}

/// Build the popup's rows, already wrapped to `width`.
///
/// Wrapping happens here rather than in the `Paragraph` so that a row index
/// means the same thing to the renderer and to the key handler — the house
/// rule the detail pane already follows (see [`super::linkmap`]). A logical
/// line that wraps contributes several rows; only its first carries the URL,
/// so selecting it scrolls to where the item starts.
pub fn pr_summary_rows(
    summary: Option<&Result<PrSummary, String>>,
    t: &Theme,
    width: u16,
) -> Vec<PrRow> {
    let tagged = pr_summary_logical_rows(summary, t);
    let mut out = Vec::new();
    for (line, url) in tagged {
        let (wrapped, _) = linkmap::wrap(&[line], &[], width as usize);
        for (i, line) in wrapped.into_iter().enumerate() {
            out.push(PrRow {
                line,
                // Only the first wrapped row of an item is its target.
                url: if i == 0 { url.clone() } else { None },
            });
        }
    }
    out
}

/// The navigable rows of the PR summary popup, as positions in
/// [`pr_summary_rows`]' output. Derived, never separately computed.
pub fn pr_targets(summary: Option<&Result<PrSummary, String>>, width: u16) -> Vec<PrTarget> {
    // Styling does not affect row count or URLs, so the default theme is a
    // sound basis for measurement — the same convention `body_content_height`
    // uses.
    pr_summary_rows(summary, &Theme::default(), width)
        .into_iter()
        .enumerate()
        .filter_map(|(i, row)| {
            row.url.map(|url| PrTarget {
                url,
                line: i as u16,
            })
        })
        .collect()
}

/// The popup's unwrapped lines, each tagged with the URL it opens (if any).
pub(super) fn pr_summary_logical_rows(
    summary: Option<&Result<PrSummary, String>>,
    t: &Theme,
) -> Vec<(Line<'static>, Option<String>)> {
    let plain = |line: Line<'static>| (line, None);

    let s = match summary {
        None => {
            return vec![plain(Line::styled(
                "loading PR summary…",
                Style::default().fg(t.dim),
            ))];
        }
        Some(Err(e)) => {
            return vec![plain(Line::styled(
                format!("failed: {e}"),
                Style::default().fg(t.error),
            ))];
        }
        Some(Ok(s)) => s,
    };

    let mut rows: Vec<(Line<'static>, Option<String>)> = Vec::new();

    // The PR header itself is the first navigable row.
    rows.push((
        Line::from(vec![
            Span::styled(format!("{} ", s.pr.label()), Style::default().fg(t.dim)),
            Span::styled(
                s.title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]),
        Some(s.pr.url()),
    ));
    rows.push(plain(Line::from(vec![
        Span::styled(
            if s.is_draft {
                "draft ".to_string()
            } else {
                format!("{} ", s.state)
            },
            Style::default().fg(match s.state {
                PrState::Merged => t.assignee,
                PrState::Open => t.open,
                PrState::Closed => t.closed,
            }),
        ),
        Span::styled(
            format!(
                "{} ← {}   +{}/-{} · {} files",
                s.base_ref, s.head_ref, s.additions, s.deletions, s.changed_files
            ),
            Style::default().fg(t.dim),
        ),
    ])));
    rows.push(plain(Line::default()));

    for l in s.body.lines() {
        rows.push(plain(Line::raw(l.to_string())));
    }
    rows.push(plain(Line::default()));

    let review_line = match s.reviews.decision {
        Some(d) => format!("{d}"),
        None => "no reviews yet".to_string(),
    };
    rows.push(plain(Line::from(vec![
        Span::styled("reviews: ", Style::default().fg(t.accent)),
        Span::raw(format!(
            "{review_line} · {} approved, {} changes requested, {} commented",
            s.reviews.approved, s.reviews.changes_requested, s.reviews.commented
        )),
    ])));
    rows.push(plain(Line::from(vec![
        Span::styled("comments: ", Style::default().fg(t.accent)),
        Span::raw(format!(
            "{} · {} review threads",
            s.comment_count, s.review_thread_count
        )),
    ])));
    rows.push(plain(Line::default()));

    rows.push(plain(Line::from(vec![
        Span::styled("checks: ", Style::default().fg(t.accent)),
        Span::raw(s.checks.state.clone().unwrap_or_else(|| "none".into())),
    ])));
    for c in &s.checks.contexts {
        let (sym, color) = conclusion_style(Some(c.conclusion.as_str()), t);
        rows.push((
            Line::from(vec![
                Span::styled(format!("  {sym} "), Style::default().fg(color)),
                Span::raw(c.name.clone()),
            ]),
            Some(c.url.clone()),
        ));
    }

    let mut run_section = |heading: Line<'static>, runs: &[WorkflowRunInfo]| {
        if runs.is_empty() {
            return;
        }
        rows.push(plain(Line::default()));
        rows.push(plain(heading));
        for r in runs {
            let (sym, color) = conclusion_style(r.conclusion.as_deref(), t);
            rows.push((
                Line::from(vec![
                    Span::styled(format!("  {sym} "), Style::default().fg(color)),
                    Span::raw(format!("{} #{} ({})", r.workflow, r.run_number, r.event)),
                ]),
                Some(r.url.clone()),
            ));
        }
    };

    run_section(
        Line::styled("PR workflow runs:", Style::default().fg(t.accent)),
        &s.pr_runs,
    );
    run_section(
        Line::styled(
            format!("── default branch ({}) ──", s.default_branch_name),
            Style::default().fg(t.accent),
        ),
        &s.default_branch_runs,
    );

    rows
}

pub(super) fn draw_pr_summary_popup(f: &mut Frame, app: &App, t: &Theme) {
    let area = pr_summary_area(f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.accent))
        .title(" PR summary (j/k scroll · Tab select · o open · r refresh · Esc close) ");

    let width = pr_summary_inner_width(f.area().width);
    let rows = pr_summary_rows(app.pr.summary.as_ref(), t, width);
    let mut lines: Vec<Line> = rows.into_iter().map(|r| r.line).collect();

    // Highlight the selected row (`Tab`/`Shift+Tab`) by patching a
    // background onto each of its spans' existing styles, preserving their
    // foreground colours and modifiers.
    if let Some(sel_line) = pr_targets(app.pr.summary.as_ref(), width)
        .get(app.pr.sel)
        .map(|target| target.line as usize)
        && let Some(l) = lines.get_mut(sel_line)
    {
        let old = std::mem::take(l);
        *l = Line::from(
            old.spans
                .into_iter()
                .map(|s| Span::styled(s.content, s.style.bg(t.selected_bg)))
                .collect::<Vec<_>>(),
        );
    }

    // Wrapping is already applied by `pr_summary_rows`, so the Paragraph must
    // not wrap again — otherwise a drawn row would stop matching its index.
    let para = Paragraph::new(lines)
        .block(block)
        .scroll((app.pr.scroll, 0));
    f.render_widget(para, area);
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;
    use crate::tui::app::Mode;

    fn pr_summary_fixture(body: &str) -> crate::provider::types::PrSummary {
        use crate::provider::types::{
            CheckContextInfo, CheckRollup, PrRef, PrState, PrSummary, ReviewDecision,
            ReviewSummary, WorkflowRunInfo,
        };

        let run = |workflow: &str, n: u64| WorkflowRunInfo {
            workflow: workflow.into(),
            run_number: n,
            event: "push".into(),
            conclusion: Some("SUCCESS".into()),
            created_at: chrono::Utc::now(),
            url: format!("https://example.test/run/{n}"),
        };

        PrSummary {
            pr: PrRef {
                owner: "o".into(),
                repo: "r".into(),
                number: 7,
            },
            title: "Add a thing".into(),
            body: body.into(),
            state: PrState::Open,
            is_draft: false,
            base_ref: "main".into(),
            head_ref: "feature".into(),
            additions: 10,
            deletions: 2,
            changed_files: 3,
            comment_count: 4,
            review_thread_count: 1,
            reviews: ReviewSummary {
                decision: Some(ReviewDecision::Approved),
                approved: 1,
                changes_requested: 0,
                commented: 2,
            },
            checks: CheckRollup {
                state: Some("SUCCESS".into()),
                contexts: vec![
                    CheckContextInfo {
                        name: "check-one".into(),
                        conclusion: "SUCCESS".into(),
                        url: "https://example.test/check/1".into(),
                    },
                    CheckContextInfo {
                        name: "check-two".into(),
                        conclusion: "FAILURE".into(),
                        url: "https://example.test/check/2".into(),
                    },
                ],
            },
            pr_runs: vec![run("pr-workflow", 11)],
            default_branch_name: "main".into(),
            default_branch_runs: vec![run("main-workflow", 22)],
        }
    }

    /// An app showing the PR summary popup for a PR with `body`.
    fn pr_summary_app(body: &str) -> App {
        use crate::provider::types::PrRef;

        let mut app = test_app();
        let pr = PrRef {
            owner: "o".into(),
            repo: "r".into(),
            number: 7,
        };
        app.pr.target = Some(pr);
        app.pr.summary = Some(Ok(pr_summary_fixture(body)));
        app.mode = Mode::PrSummary;
        app
    }

    /// The popup's navigable rows for an app rendered into a `frame_width`
    /// wide terminal — the same call the key handler makes.
    fn app_pr_targets(app: &App, frame_width: u16) -> Vec<PrTarget> {
        pr_targets(app.pr.summary.as_ref(), pr_summary_inner_width(frame_width))
    }

    /// Index of the popup's first content row containing `needle`, expressed
    /// in the same units as `PrTarget::line` (0 = first line under the top
    /// border, with the popup unscrolled).
    fn content_row_of(popup: &PopupBox, needle: &str) -> usize {
        popup
            .rows
            .iter()
            .skip(1) // row 0 is the top border
            .position(|r| r.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not found in popup:\n{}", popup.text()))
    }

    /// With a short body — no line wide enough to wrap — every navigable
    /// target's `line` lands on the row that actually draws it.
    #[test]
    fn golden_pr_summary_targets_match_drawn_rows_short_body() {
        let app = pr_summary_app("short body line\nanother short line");
        let popup = popup_box(&render_app(&app, 100, 30));
        let targets = app_pr_targets(&app, 100);

        // PR header, 2 checks, 1 PR run, 1 default-branch run.
        assert_eq!(targets.len(), 5, "target count");

        for (i, needle) in [
            (0usize, "o/r#7"),
            (1, "check-one"),
            (2, "check-two"),
            (3, "pr-workflow"),
            (4, "main-workflow"),
        ] {
            assert_eq!(
                targets[i].line as usize,
                content_row_of(&popup, needle),
                "target {i} ({needle}) points at the wrong row:\n{}",
                popup.text()
            );
        }
    }

    /// The selected row is the one that gets the highlight background, and
    /// selection moves the scroll to that row.
    #[test]
    fn golden_pr_summary_selection_highlights_its_own_row() {
        let mut app = pr_summary_app("short body");
        let targets = app_pr_targets(&app, 100);
        app.pr.select(1, &targets); // PR header → first check
        assert_eq!(app.pr.sel, 1);
        assert_eq!(app.pr.scroll, targets[1].line, "scroll snaps to the target");
    }

    /// Regression for the issue #87 defect: targets used to be computed from
    /// the body's *unwrapped* source line count while the popup rendered it
    /// wrapped, so a body line wider than the popup pushed every check and
    /// run target out of step with the row that drew it. Both now come from
    /// one row model, so a wrapping body cannot shift them.
    #[test]
    fn golden_pr_summary_targets_survive_a_wrapping_body() {
        // Popup is 76 wide, so 74 columns of content: this wraps to 2 rows.
        let long = "w".repeat(100);
        let app = pr_summary_app(&long);
        let popup = popup_box(&render_app(&app, 100, 30));
        assert_eq!(popup.width, 76, "PR summary popup width");

        let targets = app_pr_targets(&app, 100);
        for (i, needle) in [
            (0usize, "o/r#7"),
            (1, "check-one"),
            (2, "check-two"),
            (3, "pr-workflow"),
            (4, "main-workflow"),
        ] {
            assert_eq!(
                targets[i].line as usize,
                content_row_of(&popup, needle),
                "target {i} ({needle}) drifted when the body wrapped:\n{}",
                popup.text()
            );
        }
    }

    /// The row model orders the navigable rows as the popup lists them: the
    /// PR header, each check, each PR run, then each default-branch run.
    #[test]
    fn golden_pr_targets_order_header_checks_runs_and_default_branch_runs() {
        let app = pr_summary_app("short body");
        let targets = app_pr_targets(&app, 100);

        assert_eq!(targets.len(), 5); // header + 2 checks + 1 pr run + 1 branch run
        assert_eq!(targets[0].url, "https://github.com/o/r/pull/7");
        assert_eq!(targets[0].line, 0);
        assert_eq!(targets[1].url, "https://example.test/check/1");
        assert_eq!(targets[2].url, "https://example.test/check/2");
        assert_eq!(targets[3].url, "https://example.test/run/11");
        assert_eq!(targets[4].url, "https://example.test/run/22");

        // Rows strictly increase — blank and heading rows sit between the
        // sections without targets of their own.
        for w in targets.windows(2) {
            assert!(w[1].line > w[0].line, "targets must be in row order");
        }
    }

    /// A wrapped logical line contributes several rows, but only its first
    /// carries the URL — selecting it scrolls to where the item starts.
    #[test]
    fn golden_wrapped_rows_tag_only_their_first_row() {
        let app = pr_summary_app("short body");
        let rows = pr_summary_rows(app.pr.summary.as_ref(), &Theme::default(), 20);
        let tagged = rows.iter().filter(|r| r.url.is_some()).count();
        assert_eq!(
            tagged, 5,
            "a narrow width wraps rows but must not multiply targets"
        );
    }
}
