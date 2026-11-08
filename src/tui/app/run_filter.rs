//! The repo workflows view's filter menu (#266): one popup listing the run
//! filter fields, and a picker of values for each. One value per field, which
//! is what a CI backend can push down; only the fields the backend can push
//! down are offered.

use std::time::Instant;

use super::prelude::*;
use crate::ci::{RunFilter, RunStatus, Workflow};

/// A field of the run filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunFilterField {
    Status,
    Workflow,
    Event,
    Branch,
    Actor,
}

impl RunFilterField {
    const ALL: [RunFilterField; 5] = [
        RunFilterField::Status,
        RunFilterField::Workflow,
        RunFilterField::Event,
        RunFilterField::Branch,
        RunFilterField::Actor,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RunFilterField::Status => "status",
            RunFilterField::Workflow => "workflow",
            RunFilterField::Event => "event",
            RunFilterField::Branch => "branch",
            RunFilterField::Actor => "actor",
        }
    }
}

/// The picker's first entry: no value for this field.
pub const ANY: &str = "\u{2014} any \u{2014}";
/// The menu's last entry.
pub const CLEAR_ALL: &str = "clear all filters";

/// One value a field can take, as shown and as applied.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Value {
    Status(RunStatus),
    Workflow(Workflow),
    Text(String),
}

impl App {
    /// The fields the menu offers, in order: those the backend can push down.
    pub fn run_filter_fields(&self) -> Vec<RunFilterField> {
        let c = self.repo_runs.capabilities;
        RunFilterField::ALL
            .into_iter()
            .filter(|f| match f {
                RunFilterField::Status => c.status,
                RunFilterField::Workflow => c.workflow,
                RunFilterField::Event => c.event,
                RunFilterField::Branch => c.branch,
                RunFilterField::Actor => c.actor,
            })
            .collect()
    }

    fn run_filter_value(f: &RunFilter, field: RunFilterField) -> Option<String> {
        match field {
            RunFilterField::Status => f.status.map(|s| s.word().to_string()),
            RunFilterField::Workflow => f.workflow.as_ref().map(|w| w.name.clone()),
            RunFilterField::Event => f.event.clone(),
            RunFilterField::Branch => f.branch.clone(),
            RunFilterField::Actor => f.actor.clone(),
        }
    }

    /// `f` in the view: the menu, each field with its current value.
    pub fn open_run_filter_menu(&mut self) {
        let f = self.repo_runs.filter().run;
        let mut options: Vec<String> = self
            .run_filter_fields()
            .into_iter()
            .map(|field| {
                let value = Self::run_filter_value(&f, field).unwrap_or_else(|| "any".into());
                format!("{}: {value}", field.label())
            })
            .collect();
        options.push(CLEAR_ALL.into());
        self.picker.start(options, 0);
        self.mode = Mode::RunFilterMenu;
    }

    /// `Enter` in the menu: a field's picker, or clear everything.
    pub fn run_filter_menu_pick(&mut self, now: Instant) {
        let fields = self.run_filter_fields();
        let Some(orig) = self.picker.selected_original() else {
            return;
        };
        match fields.get(orig) {
            Some(&field) => self.open_run_filter_pick(field),
            None => {
                self.mode = Mode::Normal;
                self.set_repo_run_filter(RunFilter::default(), now);
            }
        }
    }

    fn run_filter_values(&self, field: RunFilterField) -> Vec<Value> {
        let repo = self.repo_runs.target.as_deref().unwrap_or_default();
        let vocab = self.repo_runs.vocab.get(repo);
        let texts = |pick: fn(&super::repo_runs::RunVocab) -> &Vec<String>| {
            vocab
                .map(|v| pick(v).iter().cloned().map(Value::Text).collect())
                .unwrap_or_default()
        };
        match field {
            RunFilterField::Status => RunStatus::ALL.into_iter().map(Value::Status).collect(),
            RunFilterField::Workflow => match self.repo_runs.workflows.get(repo) {
                Some(Ok(ws)) => ws.iter().cloned().map(Value::Workflow).collect(),
                _ => Vec::new(),
            },
            RunFilterField::Event => texts(|v| &v.events),
            RunFilterField::Branch => texts(|v| &v.branches),
            RunFilterField::Actor => texts(|v| &v.actors),
        }
    }

    fn value_label(v: &Value) -> String {
        match v {
            Value::Status(s) => s.word().to_string(),
            Value::Workflow(w) => w.name.clone(),
            Value::Text(t) => t.clone(),
        }
    }

    pub fn open_run_filter_pick(&mut self, field: RunFilterField) {
        let values = self.run_filter_values(field);
        let current = Self::run_filter_value(&self.repo_runs.filter().run, field);
        let mut options = vec![ANY.to_string()];
        options.extend(values.iter().map(Self::value_label));
        let at = current
            .and_then(|c| options.iter().position(|o| *o == c))
            .unwrap_or(0);
        self.picker.start(options, at);
        self.mode = Mode::RunFilterPick(field);
    }

    /// `Enter` in a field's picker: apply it and go back to the view. A value
    /// typed into the type-ahead that is not offered is taken as typed, for
    /// the text fields: a branch not seen this session is still a branch.
    pub fn run_filter_pick(&mut self, field: RunFilterField, now: Instant) {
        let values = self.run_filter_values(field);
        let chosen = match self.picker.selected_original() {
            Some(0) => None,
            Some(i) => values.get(i - 1).cloned(),
            None => {
                let typed = self.picker.filter.trim().to_string();
                let free = matches!(
                    field,
                    RunFilterField::Event | RunFilterField::Branch | RunFilterField::Actor
                );
                if !free || typed.is_empty() {
                    return;
                }
                Some(Value::Text(typed))
            }
        };
        let mut f = self.repo_runs.filter().run;
        match (field, chosen) {
            (RunFilterField::Status, Some(Value::Status(s))) => f.status = Some(s),
            (RunFilterField::Status, _) => f.status = None,
            (RunFilterField::Workflow, Some(Value::Workflow(w))) => f.workflow = Some(w),
            (RunFilterField::Workflow, _) => f.workflow = None,
            (RunFilterField::Event, v) => f.event = v.map(|v| Self::value_label(&v)),
            (RunFilterField::Branch, v) => f.branch = v.map(|v| Self::value_label(&v)),
            (RunFilterField::Actor, v) => f.actor = v.map(|v| Self::value_label(&v)),
        }
        self.mode = Mode::Normal;
        self.set_repo_run_filter(f, now);
    }

    /// `/` in the view.
    pub fn open_run_search(&mut self) {
        let text = self.repo_runs.filter().text;
        self.input.start(&text);
        self.mode = Mode::Input(InputKind::RunSearch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ci::CiCapabilities;

    fn app() -> App {
        let mut app = App::new("o".into(), None, false, false, "{number}".into());
        app.set_data(vec![RepoIssues {
            repo: "r".into(),
            repo_url: "u".into(),
            issues: vec![Issue::fixture("I_1", 1)],
        }]);
        app.runs.supported = true;
        app.repo_runs.capabilities = CiCapabilities {
            status: true,
            workflow: true,
            event: true,
            branch: true,
            actor: true,
        };
        app.selected = 0;
        app.open_detail();
        app.repo_runs_follow(Instant::now());
        app
    }

    #[test]
    fn the_menu_offers_only_what_the_backend_can_push_down() {
        let mut app = app();
        app.repo_runs.capabilities.actor = false;
        app.open_run_filter_menu();
        assert_eq!(
            app.picker.options,
            vec![
                "status: any",
                "workflow: any",
                "event: any",
                "branch: any",
                CLEAR_ALL
            ]
        );
    }

    #[test]
    fn picking_a_status_filters_and_any_clears_it() {
        let mut app = app();
        let t = Instant::now();
        app.open_run_filter_pick(RunFilterField::Status);
        let at = app
            .picker
            .options
            .iter()
            .position(|o| o == "failure")
            .unwrap();
        app.picker.idx = at;
        app.run_filter_pick(RunFilterField::Status, t);
        assert_eq!(app.repo_runs.filter().run.status, Some(RunStatus::Failure));
        assert_eq!(app.mode, Mode::Normal);
        app.open_run_filter_pick(RunFilterField::Status);
        assert_eq!(app.picker.idx, at, "opens on the current value");
        app.picker.idx = 0;
        app.run_filter_pick(RunFilterField::Status, t);
        assert_eq!(app.repo_runs.filter().run.status, None);
    }

    #[test]
    fn a_branch_never_seen_can_still_be_typed() {
        let mut app = app();
        app.open_run_filter_pick(RunFilterField::Branch);
        app.picker.filter = "release/1.2".into();
        app.run_filter_pick(RunFilterField::Branch, Instant::now());
        assert_eq!(
            app.repo_runs.filter().run.branch.as_deref(),
            Some("release/1.2")
        );
    }

    #[test]
    fn a_workflow_is_picked_from_the_repos_list() {
        let mut app = app();
        app.repo_runs.workflows.insert(
            "r".into(),
            Ok(vec![
                Workflow {
                    id: 7,
                    name: "CI".into(),
                },
                Workflow {
                    id: 9,
                    name: "Release".into(),
                },
            ]),
        );
        app.open_run_filter_pick(RunFilterField::Workflow);
        assert_eq!(app.picker.options, vec![ANY, "CI", "Release"]);
        app.picker.idx = 2;
        app.run_filter_pick(RunFilterField::Workflow, Instant::now());
        assert_eq!(app.repo_runs.filter().run.workflow.map(|w| w.id), Some(9));
    }

    #[test]
    fn clear_all_drops_every_field() {
        let mut app = app();
        let t = Instant::now();
        app.set_repo_run_filter(
            RunFilter {
                branch: Some("main".into()),
                status: Some(RunStatus::Success),
                ..RunFilter::default()
            },
            t,
        );
        app.open_run_filter_menu();
        app.picker.idx = app.picker.options.len() - 1;
        app.run_filter_menu_pick(t);
        assert!(app.repo_runs.filter().run.is_empty());
    }
}
