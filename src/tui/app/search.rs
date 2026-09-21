//! Session state for semantic search (#158).
//!
//! Shaped like `ReadinessState`: an idempotent "is there anything to ask?"
//! check, called after every key and event, plus a failure latch. It re-searches
//! when the candidate set gains issues that were **not judged** for the current
//! query — so relaxing a filter, or a refresh bringing new issues, is covered —
//! and never when the set only shrinks, since every remaining issue was judged.

use super::prelude::*;
use crate::typesafe::search::{Candidate, worth_searching};

#[derive(Debug, Default)]
pub struct SearchState {
    /// The query `judged` belongs to.
    query: String,
    /// Ids already sent for `query`, whether the answer has landed or not.
    judged: HashSet<String>,
    /// Bumped whenever the query changes or a new search starts. A response
    /// carrying an older generation is stale and dropped.
    generation: u64,
    /// Asking failed. Off for the rest of the session: no retry storm on a dead
    /// endpoint. Reset only by switching org.
    failed: bool,
    /// Why, in a few words — what the info bar shows beside `off` (#184).
    failure: Option<String>,
    /// A request for the current query is out.
    in_flight: bool,
    /// An answer for the current query has landed. Cleared when a new search
    /// starts or the query changes, so the info bar never reports a count that
    /// belongs to an earlier query.
    answered: bool,
}

impl SearchState {
    pub fn has_failed(&self) -> bool {
        self.failed
    }
}

/// What the info bar says about semantic search (#184). Only ever produced
/// while a query that would be sent is typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticIndicator {
    /// A request is out.
    Searching,
    /// The answer has landed and added this many issues the substring match did
    /// not already show. Zero is a real answer: "nothing else is about this".
    Added(usize),
    /// Asking failed; the short reason may be empty.
    Off(String),
}

/// Longest reason the info bar shows beside `off`.
const REASON_CHARS: usize = 30;

/// First line of `error`, cut to fit the info bar.
fn short_reason(error: &str) -> String {
    let line = error.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= REASON_CHARS {
        return line.to_string();
    }
    let cut: String = line.chars().take(REASON_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

impl App {
    /// Set the text filter. The single path both `/` and the filter editor's
    /// text field take, so semantic hits can never outlive the query they were
    /// judged for — even when the new query is never sent (a bare number, or
    /// no consent), stale hits would otherwise silently widen it.
    pub fn set_text_filter(&mut self, text: String) {
        if text != self.filters.text {
            self.filters.semantic_hits.clear();
            self.invalidate_search();
        }
        self.filters.text = text;
    }

    /// Forget what has been judged and make any in-flight response stale.
    ///
    /// The generation only ever **increases**. Resetting it would let a new
    /// search reuse a number an old in-flight response still carries, and that
    /// response would then be accepted as current.
    pub fn invalidate_search(&mut self) {
        self.search.query.clear();
        self.search.judged.clear();
        self.search.generation += 1;
        self.search.in_flight = false;
        self.search.answered = false;
    }

    /// What the info bar should say about semantic search, if anything.
    ///
    /// `None` without the consent and key, and for a query that is never sent
    /// (empty, or a bare number): a user who has not opted in sees nothing new,
    /// and a query semantic search does not handle does not claim it did.
    pub fn semantic_indicator(&self) -> Option<SemanticIndicator> {
        if !self.typesafe.issue_text_on() || !worth_searching(&self.filters.text) {
            return None;
        }
        if self.search.failed {
            return Some(SemanticIndicator::Off(
                self.search.failure.clone().unwrap_or_default(),
            ));
        }
        if self.search.in_flight {
            return Some(SemanticIndicator::Searching);
        }
        self.search
            .answered
            .then(|| SemanticIndicator::Added(self.semantic_added()))
    }

    /// Issues shown only because semantic search added them: everything the
    /// filters admit that the substring match alone would not.
    fn semantic_added(&self) -> usize {
        let exact = self.repo_filter_exact();
        self.repos
            .iter()
            .filter(|r| self.filters.repo_matches(&r.repo, exact))
            .flat_map(|r| r.issues.iter())
            .filter(|i| {
                self.filters.matches(i, self.state_filter) && !self.filters.substring_match(i)
            })
            .count()
    }

    /// Issues every filter except the text admits: what a search judges.
    ///
    /// The repo filter is **not** part of `Filters::matches` — `rebuild_rows`
    /// applies it per repo, with its exact-when-exact rule — so it is applied
    /// here the same way. Without it, a search while filtered to one repo would
    /// send every repo's issues.
    fn search_candidates(&self) -> Vec<Candidate> {
        let base = self.filters.without_text();
        let exact = self.repo_filter_exact();
        self.repos
            .iter()
            .filter(|r| self.filters.repo_matches(&r.repo, exact))
            .flat_map(|r| {
                r.issues
                    .iter()
                    .filter(|i| base.matches(i, self.state_filter))
                    .map(move |i| Candidate {
                        id: i.id.clone(),
                        title: i.title.clone(),
                        body: i.body.clone(),
                    })
            })
            .collect()
    }

    /// The search to send now, if any: `(generation, query, candidates)`.
    ///
    /// `None` when asking has failed this session, the query is empty or a bare
    /// issue number, there are no candidates, or every candidate has already
    /// been judged for this query.
    pub fn begin_semantic_search(&mut self) -> Option<(u64, String, Vec<Candidate>)> {
        if self.search.failed || !worth_searching(&self.filters.text) {
            return None;
        }
        let candidates = self.search_candidates();
        let all_judged = self.search.query == self.filters.text
            && candidates
                .iter()
                .all(|c| self.search.judged.contains(&c.id));
        if candidates.is_empty() || all_judged {
            return None;
        }
        self.search.generation += 1;
        self.search.in_flight = true;
        self.search.answered = false;
        self.search.query = self.filters.text.clone();
        self.search.judged = candidates.iter().map(|c| c.id.clone()).collect();
        Some((
            self.search.generation,
            self.search.query.clone(),
            candidates,
        ))
    }

    /// Record a search's answer. Dropped if a newer search or a text change has
    /// happened since it was sent.
    ///
    /// A landed answer covers the whole candidate set it was asked about, so it
    /// *replaces* the hits rather than adding to them.
    pub fn apply_semantic_search(
        &mut self,
        generation: u64,
        result: Result<HashSet<String>, String>,
    ) {
        if generation != self.search.generation {
            return;
        }
        self.search.in_flight = false;
        match result {
            Ok(hits) => {
                self.search.answered = true;
                self.filters.semantic_hits = hits;
                self.rebuild_rows();
                self.expand_single_visible();
            }
            Err(e) => {
                self.search.failed = true;
                self.search.failure = Some(short_reason(&e));
                self.search.judged.clear();
                self.status = Some(format!("semantic search off for this session: {e}"));
            }
        }
    }

    /// Forget everything, including the failure latch: a new org deserves a
    /// fresh attempt. The generation still advances — see `invalidate_search`.
    pub fn reset_search(&mut self) {
        self.invalidate_search();
        self.search.failed = false;
        self.search.failure = None;
    }
}
