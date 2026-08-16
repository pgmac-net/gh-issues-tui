use super::harness::SessionId;

/// A visible row in the main list: repo header or issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    RepoHeader { repo_idx: usize },
    Issue { repo_idx: usize, issue_idx: usize },
}

/// The filter-editor fields, in display order.
pub const FILTER_FIELDS: &[&str] = &[
    "text",
    "repo",
    "assignee",
    "author",
    "priority",
    "status",
    "created after (YYYY-MM-DD)",
    "created before",
    "updated after",
    "updated before",
    "closed after",
    "closed before",
    "hide empty repos",
];

/// Index of the "hide empty repos" toggle row in `FILTER_FIELDS` — it is
/// flipped in place on Enter rather than opening an input or picker.
pub const FILTER_HIDE_EMPTY_IDX: usize = FILTER_FIELDS.len() - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Single-line text input; `kind` says what the submitted text does.
    Input(InputKind),
    /// Filter editor list.
    FilterMenu,
    /// Picking from a list of values for a filter field.
    SelectField(usize),
    /// Multi-select picker (Space toggles) for a filter field.
    SelectFieldMulti(usize),
    /// Calendar date picker.
    Calendar(usize),
    /// Confirmation popup for close/reopen.
    ConfirmState,
    /// New-issue form: single inline form, `Tab`/`Shift+Tab` moves between
    /// fields and the Create/Cancel buttons; text fields (title, body) edit
    /// in place, choice fields open a picker popup.
    IssueForm,
    /// Single-select popup for a new-issue form field.
    IssueFormSelect(usize),
    /// Multi-select popup (Space toggles) for a new-issue form field.
    IssueFormMulti(usize),
    /// Multi-line editor for adding a comment to the selected issue.
    CommentEditor,
    /// Single-select popup choosing a priority label for the selected issue.
    PrioritySet,
    /// Multi-select popup editing the full label set of the selected issue.
    LabelsSet,
    /// Picker choosing which linked PR to summarise, when more than one link
    /// was found.
    PrPicker,
    /// Popup showing a linked PR's summary.
    PrSummary,
    /// A coding-harness session is on screen (#23). Every key is forwarded
    /// to the child except the `F12` prefix chord — see
    /// `event::keys::harness`.
    Harness,
    /// Picker choosing which harness to launch for the selected issue.
    HarnessPicker,
    /// Picker choosing which running/exited session to attach to.
    SessionPicker,
    /// Confirmation popup whose `Yes` performs a harness action.
    ConfirmHarness(HarnessConfirm),
    Help,
}

/// What a `Mode::ConfirmHarness` popup will do if confirmed. Each carries
/// everything the action needs, so the popup cannot act on a selection that
/// moved while it was open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessConfirm {
    /// Terminate a live session's child.
    Kill(SessionId),
    /// Discard an exited session's screen and start its harness again.
    Relaunch(SessionId),
    /// Quit the app while sessions are still running.
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Search,
    FilterField(usize),
    Assignees,
    Title,
    /// Switch the org/owner being browsed.
    Org,
    /// Jump the selection to a loaded issue by its number (does not filter).
    GotoNumber,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Detail,
}

/// What the detail pane's keyboard focus is on: the issue body region, or one
/// of the comment cards (0-indexed). Tab/Shift+Tab cycle through
/// `Body → Comment(0) → … → Comment(n-1) → Body`; the selected region is the
/// one `j/k` scroll and `e` edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailSel {
    /// The pane opens on the body, and an empty thread falls back to it.
    #[default]
    Body,
    Comment(usize),
}

/// One open-able row in the PR summary popup (`Mode::PrSummary`): the PR
/// header, a check, or a workflow run. `line` is the row's position in the
/// popup's drawn rows, read straight off the row model by `ui::pr_targets`
/// rather than derived a second time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrTarget {
    pub url: String,
    pub line: u16,
}

/// Which element of the inline comment section (`Mode::CommentEditor`) has
/// keys: the multi-line editor itself, or one of its two buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommentFocus {
    /// The editor has keys when the widget opens.
    #[default]
    Editor,
    Save,
    Cancel,
}

/// What the inline editor (`Mode::CommentEditor`) writes on save. All three
/// share the same multi-line-editor + Save/Cancel widget; only the mutation
/// and the header text differ.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum EditorTarget {
    /// Add a new comment to the selected issue (`c`).
    #[default]
    NewComment,
    /// Edit an existing comment by its backend id (`e` on a comment card).
    EditComment { comment_id: String },
    /// Edit the selected issue's description (`e` on the body card).
    EditBody,
}

/// Which button has keys in the `Mode::ConfirmState` popup. Reset to `No`
/// each time the popup opens — the safe default if Enter is pressed without
/// looking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmChoice {
    Yes,
    No,
}
