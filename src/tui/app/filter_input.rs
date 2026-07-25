use super::prelude::*;

/// Applying edits from the filter editor to `Filters`, and the toggles
/// that clear or restore them.
impl App {
    /// Apply the submitted input buffer according to the active input kind.
    /// Returns the action the event loop must run, if any.
    pub fn apply_filter_input(&mut self, kind: InputKind, value: &str) {
        match kind {
            InputKind::Search => self.filters.text = value.to_string(),
            InputKind::FilterField(idx) => {
                let v = value.trim().to_string();
                match idx {
                    0 => self.filters.text = v,
                    1 => self.filters.repo = v,
                    2 => self.filters.assignee = v,
                    3 => self.filters.author = v,
                    4 => self.filters.priority = parse_filter_list(&v),
                    5 => self.filters.status = parse_filter_list(&v),
                    6 => self.filters.created_after = parse_date(&v),
                    7 => self.filters.created_before = parse_date(&v),
                    8 => self.filters.updated_after = parse_date(&v),
                    9 => self.filters.updated_before = parse_date(&v),
                    10 => self.filters.closed_after = parse_date(&v),
                    11 => self.filters.closed_before = parse_date(&v),
                    // "hide empty repos" toggles in place, never via input.
                    _ => {}
                }
            }
            _ => {}
        }
        self.rebuild_rows();
        self.expand_single_visible();
    }

    /// Commit a multi-select filter field (priority, status) and recompute
    /// the visible rows. An empty `values` clears the filter.
    pub fn apply_multi_filter(&mut self, idx: usize, values: Vec<String>) {
        match idx {
            4 => self.filters.priority = values,
            5 => self.filters.status = values,
            _ => return,
        }
        self.rebuild_rows();
        self.expand_single_visible();
    }

    pub fn current_filter_value(&self, idx: usize) -> String {
        let d = |o: Option<NaiveDate>| o.map(|d| d.to_string()).unwrap_or_default();
        match idx {
            0 => self.filters.text.clone(),
            1 => self.filters.repo.clone(),
            2 => self.filters.assignee.clone(),
            3 => self.filters.author.clone(),
            4 => self.filters.priority.join(", "),
            5 => self.filters.status.join(", "),
            6 => d(self.filters.created_after),
            7 => d(self.filters.created_before),
            8 => d(self.filters.updated_after),
            9 => d(self.filters.updated_before),
            10 => d(self.filters.closed_after),
            11 => d(self.filters.closed_before),
            _ => if self.filters.hide_empty { "yes" } else { "no" }.to_string(),
        }
    }

    /// Flip the hide-empty-repos filter and recompute the rows.
    pub fn toggle_hide_empty(&mut self) {
        self.filters.hide_empty = !self.filters.hide_empty;
        self.rebuild_rows();
        self.expand_single_visible();
    }

    /// Clear the filter editor back to its defaults — the hide-empty
    /// toggle returns to the *config* default, not blanket false.
    pub fn clear_filters(&mut self) {
        self.filters.clear();
        self.filters.hide_empty = self.hide_empty_default;
    }

    /// Whether the filters-active indicator should show: any text/date
    /// filter set, or the hide-empty toggle moved off its config default.
    pub fn filters_active(&self) -> bool {
        self.filters.is_active() || self.filters.hide_empty != self.hide_empty_default
    }

    /// Set the config-derived default for the hide-empty filter, applying
    /// it to the live filter too (called once at startup).
    pub fn set_hide_empty_default(&mut self, hide: bool) {
        self.hide_empty_default = hide;
        self.filters.hide_empty = hide;
    }
}
