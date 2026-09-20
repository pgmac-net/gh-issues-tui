use super::prelude::*;

/// Sorts after the four known ranks: a `priority:<value>` label the
/// convention does not recognise and inference has no opinion on.
const UNRECOGNISED_PRIORITY: u8 = 5;

/// One row of the priority filter picker, carrying the three sort keys by
/// name rather than by tuple position.
struct PriorityOption {
    text: String,
    rank: u8,
    /// Came from a `priority:<value>` label. Sorts ahead of an inferred label
    /// of the same rank.
    convention: bool,
}

/// Everything a list-of-choices popup needs, for every mode that shows one:
/// the option list, the type-ahead filter narrowing it, the highlighted
/// position, the multi-select working set, and the staleness guards for the
/// pickers whose options are fetched.
///
/// These reset together each time a picker opens, so they live together —
/// `start` is the single entry point and cannot leave a stale filter behind.
#[derive(Debug, Default)]
pub struct PickerState {
    /// Available options for the current picker.
    pub options: Vec<String>,
    /// Highlighted position within the *filtered* view, not `options`.
    pub idx: usize,
    /// Type-ahead filter narrowing the view; reset on open.
    pub filter: String,
    /// Toggled indices for a multi-select popup, in `options` terms
    /// (committed on Enter, discarded on Esc).
    pub multi_selected: std::collections::HashSet<usize>,
    /// Issue id the set-priority picker was requested for; guards against a
    /// stale options response landing after the selection moved on.
    pub priority_issue: Option<String>,
    /// Issue id the edit-labels picker was requested for; same guard.
    pub label_issue: Option<String>,
}

impl PickerState {
    /// Open a picker: set its options and initial highlight, and reset the
    /// type-ahead filter.
    pub fn start(&mut self, options: Vec<String>, idx: usize) {
        self.options = options;
        self.idx = idx;
        self.filter.clear();
    }

    /// The picker view under the type-ahead filter: `(original index,
    /// text)` pairs matching case-insensitively. An empty filter shows
    /// everything.
    pub fn filtered(&self) -> Vec<(usize, &str)> {
        let needle = self.filter.to_lowercase();
        self.options
            .iter()
            .enumerate()
            .filter(|(_, o)| needle.is_empty() || o.to_lowercase().contains(&needle))
            .map(|(i, o)| (i, o.as_str()))
            .collect()
    }

    /// Index into `options` of the highlighted row, `None` when the filter
    /// matches nothing.
    pub fn selected_original(&self) -> Option<usize> {
        self.filtered().get(self.idx).map(|(i, _)| *i)
    }

    /// Append a type-ahead character; the highlight jumps to the first match.
    pub fn filter_push(&mut self, c: char) {
        self.filter.push(c);
        self.idx = 0;
    }

    pub fn filter_backspace(&mut self) {
        self.filter.pop();
        self.clamp_idx();
    }

    pub fn filter_clear(&mut self) {
        self.filter.clear();
        self.clamp_idx();
    }

    /// Toggle `orig` in the multi-select set.
    pub fn toggle_multi(&mut self, orig: usize) {
        if !self.multi_selected.remove(&orig) {
            self.multi_selected.insert(orig);
        }
    }

    fn clamp_idx(&mut self) {
        let len = self.filtered().len();
        if self.idx >= len {
            self.idx = len.saturating_sub(1);
        }
    }
}

impl App {
    /// Open the new-issue form targeting `repo`. Options arrive later via
    /// `set_form_options`; the caller spawns that fetch.
    pub fn open_issue_form(&mut self, repo: String) {
        self.issue_form = Some(IssueForm::new(repo));
        self.mode = Mode::IssueForm;
    }

    /// Discard the form and return to Normal mode.
    pub fn cancel_issue_form(&mut self) {
        self.issue_form = None;
        self.mode = Mode::Normal;
    }

    /// Repo names available as a move target for the selected issue: every
    /// loaded repo except its own. Empty when nothing is selected or the
    /// issue's repo is the only one loaded.
    pub fn move_targets(&self) -> Vec<String> {
        let Some(current) = self.selected_repo().map(|r| r.repo.clone()) else {
            return Vec::new();
        };
        self.repos
            .iter()
            .map(|r| r.repo.clone())
            .filter(|r| *r != current)
            .collect()
    }

    /// Open the move-target picker. Callers gate on `move_targets()` being
    /// non-empty and the provider supporting moves before calling this.
    pub fn open_move_picker(&mut self, targets: Vec<String>) {
        self.picker.start(targets, 0);
        self.mode = Mode::MovePicker;
    }

    /// Deliver a per-repo options fetch. Dropped when the form has been
    /// closed or retargeted since the fetch was spawned (stale response).
    pub fn set_form_options(&mut self, repo: &str, options: FormOptions) {
        if let Some(form) = &mut self.issue_form
            && form.repo == repo
        {
            form.options = Some(options);
        }
    }

    /// Build the list of options shown when the user presses Enter on a
    /// select-style filter field (repo, assignee, author).
    /// The first entry is always `"—"` which means "no filter".
    pub fn compute_select_options(&self, idx: usize) -> Vec<String> {
        let mut opts: Vec<String> = match idx {
            1 => self.repos.iter().map(|r| r.repo.clone()).collect(),
            2 => {
                let mut v: Vec<String> = self
                    .repos
                    .iter()
                    .flat_map(|r| r.issues.iter())
                    .flat_map(|i| i.assignees.iter().cloned())
                    .collect();
                v.sort();
                v.dedup();
                v
            }
            3 => {
                let mut v: Vec<String> = self
                    .repos
                    .iter()
                    .flat_map(|r| r.issues.iter())
                    .map(|i| i.author.clone())
                    .collect();
                v.sort();
                v.dedup();
                v
            }
            _ => vec![],
        };
        opts.insert(0, "\u{2014}".to_string());
        opts
    }

    /// Options for a multi-select filter field (priority, status). No "—"
    /// row — clearing is deselecting everything. Priority values are
    /// ordered low → urgent with unknown values last (like the set-priority
    /// picker) and include labels whose rank was inferred (#164); status
    /// values stay alphabetical.
    pub fn compute_multi_options(&self, idx: usize) -> Vec<String> {
        match idx {
            4 => self.priority_filter_options(),
            5 => self.label_values("status"),
            _ => vec![],
        }
    }

    /// The priority filter picker's options: `priority:<value>` values and
    /// labels whose rank was inferred (#156), on one low → urgent scale.
    ///
    /// Deliberately *not* gated on the repo using the `priority:*` convention,
    /// unlike the set-priority picker (#162, ADR 0003). That gate guards a
    /// write; this list is read-only and spans every loaded repo, so gating it
    /// would let one convention repo hide `P0` from every other repo in the
    /// org — the very thing this exists to fix.
    ///
    /// One entry per option text. `label_filter_matches` makes a filter of
    /// `P1` match both `P1` and `priority:P1`, so two entries for the same
    /// text would look identical and select identical issues. The rank the
    /// entry sorts by is the convention's when it recognises the value, and an
    /// inferred rank only fills a gap: the fallback `5` means "unrecognised",
    /// not a position, so a declared `low` is never reordered by a model's
    /// opinion of a bare `low` elsewhere, while `priority:P1` still sorts
    /// with the inferred `P1` rather than dropping to the end.
    ///
    /// With no ranks resolved every entry is a convention entry and the sort
    /// key collapses to `(rank, text)` — the list as it was before inference
    /// existed.
    fn priority_filter_options(&self) -> Vec<String> {
        let mut options: Vec<PriorityOption> = self
            .label_values("priority")
            .into_iter()
            .map(|text| PriorityOption {
                rank: priority_value_rank(&text)
                    .or_else(|| self.label_rank.rank_of(&text))
                    .unwrap_or(UNRECOGNISED_PRIORITY),
                convention: true,
                text,
            })
            .collect();

        let mut inferred: Vec<PriorityOption> = self
            .repos
            .iter()
            .flat_map(|r| r.issues.iter())
            .flat_map(|i| i.ranked_labels())
            .filter(|l| priority_value(&l.name).is_none())
            .filter(|l| !options.iter().any(|o| o.text.eq_ignore_ascii_case(&l.name)))
            .filter_map(|l| {
                Some(PriorityOption {
                    text: l.name.clone(),
                    rank: l.rank?,
                    convention: false,
                })
            })
            .collect();
        inferred.sort_by(|a, b| a.text.cmp(&b.text));
        inferred.dedup_by(|a, b| a.text.eq_ignore_ascii_case(&b.text));
        options.extend(inferred);

        // Convention first on an equal rank, so every entry that exists today
        // keeps its position relative to the others.
        options.sort_by(|a, b| {
            (a.rank, !a.convention, &a.text).cmp(&(b.rank, !b.convention, &b.text))
        });
        options.into_iter().map(|o| o.text).collect()
    }

    /// Distinct sorted values of `<prefix>:<value>` labels across all issues.
    /// Splits on `:` rather than byte-slicing so mixed-case or non-ASCII
    /// label names can never panic on a char boundary.
    fn label_values(&self, prefix: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .repos
            .iter()
            .flat_map(|r| r.issues.iter())
            .flat_map(|i| i.labels.iter())
            .filter_map(|l| {
                l.name
                    .split_once(':')
                    .filter(|(p, _)| p.eq_ignore_ascii_case(prefix))
                    .map(|(_, value)| value.to_string())
            })
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// Returns `true` when the field at `idx` should show a single-select
    /// list instead of a free-text input.
    pub fn is_select_field(idx: usize) -> bool {
        matches!(idx, 1..=3)
    }

    /// Returns `true` when the field at `idx` should show a multi-select
    /// list (priority, status — several values OR together).
    pub fn is_multi_select_field(idx: usize) -> bool {
        matches!(idx, 4 | 5)
    }

    /// Prepares the calendar cursor from the current filter value or today.
    pub fn calendar_init(&mut self, idx: usize) {
        let current = self.current_filter_value(idx);
        self.calendar_cursor = parse_date(&current).unwrap_or_else(|| Utc::now().date_naive());
    }

    /// Returns `true` when the field at `idx` uses the calendar date picker.
    pub fn is_calendar_field(idx: usize) -> bool {
        matches!(idx, 6..=11)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker(options: &[&str]) -> PickerState {
        let mut p = PickerState::default();
        p.start(options.iter().map(|s| s.to_string()).collect(), 0);
        p
    }

    #[test]
    fn start_resets_the_filter_left_by_a_previous_picker() {
        let mut p = picker(&["alpha", "beta"]);
        p.filter_push('z');
        p.start(vec!["gamma".into()], 0);
        assert!(p.filter.is_empty(), "a stale filter must not survive open");
        assert_eq!(p.idx, 0);
    }

    #[test]
    fn filtered_narrows_case_insensitively_and_keeps_original_indices() {
        let mut p = picker(&["alpha", "Beta", "gamma"]);
        p.filter_push('B');
        assert_eq!(p.filtered(), vec![(1, "Beta")]);
        // The reported index is into `options`, not into the filtered view.
        assert_eq!(p.selected_original(), Some(1));
    }

    #[test]
    fn empty_filter_shows_everything() {
        let p = picker(&["alpha", "beta"]);
        assert_eq!(p.filtered().len(), 2);
    }

    #[test]
    fn typing_moves_the_highlight_to_the_first_match() {
        let mut p = picker(&["alpha", "beta", "gamma"]);
        p.idx = 2;
        p.filter_push('a');
        assert_eq!(p.idx, 0, "a new filter re-homes the highlight");
    }

    #[test]
    fn backspace_clamps_the_highlight_into_the_widened_view() {
        let mut p = picker(&["alpha", "beta", "gamma"]);
        for c in "gam".chars() {
            p.filter_push(c);
        }
        p.idx = 0;
        p.filter_backspace();
        assert!(p.idx < p.filtered().len());
    }

    #[test]
    fn clearing_a_filter_that_matched_nothing_leaves_a_valid_index() {
        let mut p = picker(&["alpha", "beta"]);
        p.filter_push('z');
        assert!(p.filtered().is_empty());
        assert_eq!(p.selected_original(), None, "no match, nothing selected");
        p.filter_clear();
        assert!(p.idx < p.filtered().len(), "index must be back in range");
    }

    #[test]
    fn selected_original_is_none_when_there_are_no_options() {
        assert_eq!(PickerState::default().selected_original(), None);
    }

    #[test]
    fn toggle_multi_adds_then_removes() {
        let mut p = picker(&["alpha", "beta"]);
        p.toggle_multi(1);
        assert!(p.multi_selected.contains(&1));
        p.toggle_multi(1);
        assert!(!p.multi_selected.contains(&1));
    }
}
