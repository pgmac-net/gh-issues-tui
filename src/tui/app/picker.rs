use super::prelude::*;

/// Picker state: the option list, the type-ahead filter, and the
/// multi-select working set. Shared by every mode that shows a list.
impl App {
    pub fn start_picker(&mut self, options: Vec<String>, idx: usize) {
        self.select_options = options;
        self.select_idx = idx;
        self.select_filter.clear();
    }

    /// The picker view under the type-ahead filter: `(original index,
    /// text)` pairs matching case-insensitively. An empty filter shows
    /// everything.
    pub fn filtered_select(&self) -> Vec<(usize, &str)> {
        let needle = self.select_filter.to_lowercase();
        self.select_options
            .iter()
            .enumerate()
            .filter(|(_, o)| needle.is_empty() || o.to_lowercase().contains(&needle))
            .map(|(i, o)| (i, o.as_str()))
            .collect()
    }

    /// Index into `select_options` of the highlighted picker row, `None`
    /// when the filter matches nothing.
    pub fn picker_selected_original(&self) -> Option<usize> {
        self.filtered_select().get(self.select_idx).map(|(i, _)| *i)
    }

    /// Append a type-ahead character; the highlight jumps to the first match.
    pub fn picker_filter_push(&mut self, c: char) {
        self.select_filter.push(c);
        self.select_idx = 0;
    }

    pub fn picker_filter_backspace(&mut self) {
        self.select_filter.pop();
        self.clamp_picker_idx();
    }

    pub fn picker_filter_clear(&mut self) {
        self.select_filter.clear();
        self.clamp_picker_idx();
    }

    fn clamp_picker_idx(&mut self) {
        let len = self.filtered_select().len();
        if self.select_idx >= len {
            self.select_idx = len.saturating_sub(1);
        }
    }

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
    /// picker); status values stay alphabetical.
    pub fn compute_multi_options(&self, idx: usize) -> Vec<String> {
        match idx {
            4 => {
                let rank = |v: &str| priority_value_rank(v).unwrap_or(5);
                let mut v = self.label_values("priority");
                v.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.cmp(b)));
                v
            }
            5 => self.label_values("status"),
            _ => vec![],
        }
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
