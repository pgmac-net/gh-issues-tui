use super::prelude::*;

/// The new-issue form fields, in display order. The row after the last
/// field is the `[Create issue]` action (`ISSUE_FORM_CREATE_ROW`).
pub const ISSUE_FORM_FIELDS: &[&str] = &[
    "title",
    "description",
    "assignees",
    "labels",
    "type",
    "priority",
    "project",
    "milestone",
];

/// Index of the `[Create issue]` row in the form.
pub const ISSUE_FORM_CREATE_ROW: usize = ISSUE_FORM_FIELDS.len();

/// Index of the `[Cancel]` row in the form, one past Create.
pub const ISSUE_FORM_CANCEL_ROW: usize = ISSUE_FORM_CREATE_ROW + 1;

/// Width of the label column in the new-issue form; the value column gets
/// the rest of `issue_form_width`.
pub const ISSUE_FORM_LABEL_WIDTH: usize = 14;

/// The new-issue form's outer width; inner text width mirrors the other
/// popups' clamp-minus-borders pattern.
pub const ISSUE_FORM_WIDTH: u16 = 78;

pub fn issue_form_width(frame_width: u16) -> u16 {
    ISSUE_FORM_WIDTH.min(frame_width).saturating_sub(2)
}

/// Visual rows reserved for the inline description box, independent of
/// content length — scrolls to keep the cursor visible, mirroring the
/// inline comment editor.
pub const ISSUE_FORM_DESC_HEIGHT: usize = 4;

/// State of the new-issue form. Selections index into the corresponding
/// `FormOptions` list (not the "—"-prefixed popup display list).
pub struct IssueForm {
    /// Repo the issue will be created in, captured when the form opened.
    pub repo: String,
    pub title: InputState,
    pub body: BodyEditor,
    pub assignees: std::collections::HashSet<usize>,
    pub labels: std::collections::HashSet<usize>,
    pub issue_type: Option<usize>,
    pub priority: Option<usize>,
    pub project: Option<usize>,
    pub milestone: Option<usize>,
    /// `None` while the per-repo options fetch is still in flight.
    pub options: Option<FormOptions>,
    pub field_idx: usize,
}

impl IssueForm {
    pub fn new(repo: String) -> Self {
        Self {
            repo,
            title: InputState::default(),
            body: BodyEditor::default(),
            assignees: Default::default(),
            labels: Default::default(),
            issue_type: None,
            priority: None,
            project: None,
            milestone: None,
            options: None,
            field_idx: 0,
        }
    }

    /// True for fields edited with the multi-select popup.
    pub fn is_multi_field(idx: usize) -> bool {
        matches!(idx, 2 | 3)
    }

    /// True for fields edited with the single-select popup.
    pub fn is_select_field(idx: usize) -> bool {
        matches!(idx, 4..=7)
    }

    /// Labels acting as priorities under the `priority:<value>` convention.
    pub fn priority_options(&self) -> Vec<&IdName> {
        self.options
            .as_ref()
            .map(|o| {
                o.labels
                    .iter()
                    .filter(|l| l.name.to_lowercase().starts_with("priority:"))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The option list backing a select/multi field, as display names.
    pub fn field_options(&self, idx: usize) -> Vec<String> {
        let Some(o) = &self.options else {
            return Vec::new();
        };
        let names = |v: &[IdName]| v.iter().map(|x| x.name.clone()).collect::<Vec<_>>();
        match idx {
            2 => names(&o.users),
            3 => names(&o.labels),
            4 => names(&o.issue_types),
            5 => self
                .priority_options()
                .iter()
                .map(|l| l.name.clone())
                .collect(),
            6 => names(&o.projects),
            7 => names(&o.milestones),
            _ => Vec::new(),
        }
    }

    /// Current selection(s) of a field as display text for the form row.
    pub fn field_display(&self, idx: usize) -> String {
        let opts = self.field_options(idx);
        let pick = |sel: Option<usize>| sel.and_then(|i| opts.get(i).cloned()).unwrap_or_default();
        match idx {
            0 => self.title.buffer.clone(),
            1 => self.body.summary(),
            2 | 3 => {
                let set = if idx == 2 {
                    &self.assignees
                } else {
                    &self.labels
                };
                let mut picked: Vec<usize> = set.iter().copied().collect();
                picked.sort_unstable();
                picked
                    .into_iter()
                    .filter_map(|i| opts.get(i).cloned())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
            4 => pick(self.issue_type),
            5 => pick(self.priority),
            6 => pick(self.project),
            7 => pick(self.milestone),
            _ => String::new(),
        }
    }

    /// Set a single-select field; `None` clears it.
    pub fn set_single(&mut self, idx: usize, choice: Option<usize>) {
        match idx {
            4 => self.issue_type = choice,
            5 => self.priority = choice,
            6 => self.project = choice,
            7 => self.milestone = choice,
            _ => {}
        }
    }

    pub fn get_single(&self, idx: usize) -> Option<usize> {
        match idx {
            4 => self.issue_type,
            5 => self.priority,
            6 => self.project,
            7 => self.milestone,
            _ => None,
        }
    }

    pub fn multi_set(&self, idx: usize) -> &std::collections::HashSet<usize> {
        if idx == 2 {
            &self.assignees
        } else {
            &self.labels
        }
    }

    pub fn multi_set_mut(&mut self, idx: usize) -> &mut std::collections::HashSet<usize> {
        if idx == 2 {
            &mut self.assignees
        } else {
            &mut self.labels
        }
    }

    /// Assemble the create parameters. `None` until the title is non-empty
    /// and the options fetch has landed (repo id comes from it).
    pub fn build_params(&self) -> Option<NewIssueParams> {
        let o = self.options.as_ref()?;
        let title = self.title.buffer.trim();
        if title.is_empty() {
            return None;
        }
        let ids = |set: &std::collections::HashSet<usize>, from: &[IdName]| {
            let mut picked: Vec<usize> = set.iter().copied().collect();
            picked.sort_unstable();
            picked
                .into_iter()
                .filter_map(|i| from.get(i).map(|x| x.id.clone()))
                .collect::<Vec<String>>()
        };
        let mut label_ids = ids(&self.labels, &o.labels);
        if let Some(p) = self.priority
            && let Some(label) = self.priority_options().get(p).map(|l| l.id.clone())
            && !label_ids.contains(&label)
        {
            label_ids.push(label);
        }
        Some(NewIssueParams {
            repo_id: o.repo_id.clone(),
            title: title.to_string(),
            body: self.body.text().trim_end().to_string(),
            assignee_ids: ids(&self.assignees, &o.users),
            label_ids,
            milestone_id: self
                .milestone
                .and_then(|i| o.milestones.get(i))
                .map(|m| m.id.clone()),
            issue_type_id: self
                .issue_type
                .and_then(|i| o.issue_types.get(i))
                .map(|t| t.id.clone()),
            project_id: self
                .project
                .and_then(|i| o.projects.get(i))
                .map(|p| p.id.clone()),
        })
    }
}
