use super::prelude::*;

/// Session state for label-rank inference (#156): what has been resolved for
/// the current org, and whether a request is out or has already failed.
///
/// Reset as a unit by `App::switch_org` — ranks are per-org, and a fresh org
/// deserves a fresh attempt even if the last one failed.
#[derive(Debug, Default)]
pub struct RankState {
    /// Every answer received this session: label name → rank. A `None` value
    /// is a real answer ("not a priority label") and stops the label being
    /// asked about again.
    ranks: HashMap<String, Option<u8>>,
    /// A request is out. Refreshes land every few minutes; without this one
    /// landing mid-request would ask for the same labels twice.
    in_flight: bool,
    /// The request failed. Inference stays off for the rest of the session:
    /// no retry storm, and no repeating the status message on every refresh.
    /// Failures are not cached, so the next launch tries again.
    failed: bool,
}

impl App {
    /// The label names on loaded issues that still need a rank, marking a
    /// request as in flight — or `None` when there is nothing to ask, a
    /// request is already out, or inference has failed this session.
    ///
    /// `priority:*` labels are excluded: the convention already ranks them,
    /// and it always wins over an inferred rank.
    pub fn begin_rank_inference(&mut self) -> Option<Vec<String>> {
        if self.label_rank.in_flight || self.label_rank.failed {
            return None;
        }
        let mut names: Vec<String> = self
            .repos
            .iter()
            .flat_map(|r| &r.issues)
            .flat_map(|i| &i.labels)
            .map(|l| l.name.as_str())
            .filter(|n| priority_value(n).is_none() && !self.label_rank.ranks.contains_key(*n))
            .map(str::to_string)
            .collect();
        names.sort_unstable();
        names.dedup();
        if names.is_empty() {
            return None;
        }
        self.label_rank.in_flight = true;
        Some(names)
    }

    /// Deliver a finished inference. Dropped when `org` is no longer the
    /// current org — the list was pointed elsewhere while it was in flight —
    /// and that drop deliberately leaves `in_flight` alone, since it may now
    /// describe a newer request.
    pub fn apply_label_ranks(
        &mut self,
        org: &str,
        result: Result<HashMap<String, Option<u8>>, String>,
    ) {
        if org != self.org {
            return;
        }
        self.label_rank.in_flight = false;
        match result {
            Ok(ranks) => {
                let prev = self.selected_issue().map(|i| i.id.clone());
                self.label_rank.ranks.extend(ranks);
                self.stamp_label_ranks();
                // A new rank can reorder the list when sorted by priority, so
                // follow the issue rather than the index.
                self.rebuild_rows();
                self.reselect(prev);
            }
            Err(e) => {
                self.label_rank.failed = true;
                self.status = Some(format!(
                    "priority ranks unavailable ({e}) — sorting by label convention"
                ));
            }
        }
    }

    /// Copy the resolved ranks onto every loaded label. Called on every data
    /// load as well: a refresh replaces the issues with fresh ones whose
    /// labels carry no rank.
    pub(super) fn stamp_label_ranks(&mut self) {
        if self.label_rank.ranks.is_empty() {
            return;
        }
        for issue in self.repos.iter_mut().flat_map(|r| r.issues.iter_mut()) {
            for label in &mut issue.labels {
                if let Some(rank) = self.label_rank.ranks.get(&label.name) {
                    label.rank = *rank;
                }
            }
        }
    }
}
