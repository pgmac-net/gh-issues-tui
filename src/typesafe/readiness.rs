//! Ticket-readiness judgement (#160): is this issue worth spending a real
//! coding-agent run on?
//!
//! Five independent Nouls over one state. Independent questions run in a
//! single request, so the extra four cost no extra round trip.
//!
//! **Advisory only.** The verdict reaches the detail pane and nothing else —
//! never the `A` key, `LaunchAction`, `expand_argv` or the PTY. Issue text is
//! attacker-controlled and the model is documented to be steerable by injected
//! instructions, so the worst case has to be a misleading line on screen. See
//! `docs/adr/0004-…`.
//!
//! **Consent.** Sending issue *text* is a larger disclosure than #156's label
//! names and takes its own flag (`send_issue_text`) as well as the key. ADR
//! 0004 supersedes ADR 0002 clause 3.

use std::collections::HashMap;

use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

use super::{Answer, Client, MODEL};

/// Above this, a Noul is read as yes.
const YES: f64 = 0.7;
/// Below this, a Noul is read as no.
///
/// The band between is reported as undecided rather than rounded. A Noul near
/// `0.5` means *equally likely either way* — there is no confidence value to
/// gate on as there is for a Score, so the probability itself has to carry it.
const NO: f64 = 0.3;

/// Comments sent, most recent first-cut last. The tail is the current state of
/// play, which is what "is it blocked now" depends on; shipping the whole
/// thread costs accuracy on every question, because "accuracy falls as the
/// state grows with content unrelated to the decision".
pub const COMMENTS_SENT: usize = 10;
/// Characters kept per comment.
pub const COMMENT_CHARS: usize = 500;
/// Characters kept of the body.
pub const BODY_CHARS: usize = 4000;

/// The signals that earn an `unsure` when they land between `NO` and `YES`.
///
/// **A rule, not a list:** a signal belongs here only if the verdict makes a
/// claim that rests on it being decisive. The verdict reads every signal
/// one-sidedly —
///
/// ```text
/// blocked    > YES  ->  Blocked          duplicate  > YES  ->  MaybeDuplicate
/// actionable < NO   ->  NotActionable    specifics/criteria < NO  ->  Thin
/// ```
///
/// — so:
///
/// * `specifics`, `criteria`: "ready — is specific and states an outcome" is a
///   positive claim. It must not be asserted on a coin flip.
/// * `blocked`, `duplicate`: missing one costs a whole agent run, so "might be"
///   is worth saying even though it is not enough to veto.
/// * **not `actionable`**: it is only ever consumed as `< NO`, so a middling
///   value just means "not vetoed" and hedging on it is noise. Real tickets sit
///   in its band routinely; treating that as undecided made 5 of 22 well-specified
///   bug reports read `unsure` (#169).
///
/// Add a signal to [`SIGNALS`] and this is where you must decide which direction
/// the verdict reads it.
const HEDGED: [Signal; 4] = [
    Signal::Specifics,
    Signal::Criteria,
    Signal::Blocked,
    Signal::Duplicate,
];

/// One judgement about a ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Specifics,
    Criteria,
    Actionable,
    Blocked,
    Duplicate,
}

impl Signal {
    /// Question id on the wire, and the name used in the badge.
    pub fn id(self) -> &'static str {
        match self {
            Signal::Specifics => "specifics",
            Signal::Criteria => "criteria",
            Signal::Actionable => "actionable",
            Signal::Blocked => "blocked",
            Signal::Duplicate => "duplicate",
        }
    }
}

/// Every signal, in the order a badge lists them.
pub const SIGNALS: [Signal; 5] = [
    Signal::Specifics,
    Signal::Criteria,
    Signal::Actionable,
    Signal::Blocked,
    Signal::Duplicate,
];

/// What to ask for each signal. `criteria` is spelled out on every question:
/// the model reads instructions literally, and the jaggedness notes call for
/// "explicit criteria aligned with instructions".
fn question(signal: Signal) -> Value {
    let (instructions, yes, no) = match signal {
        // Was `repro` ("could someone see the problem or the current behaviour
        // for themselves?") until #171. That wording asked about the *current*
        // state, so a precise spec for something not yet built scored 0.08 while
        // a record of finished work scored 0.73 — right for the question, wrong
        // for readiness. This asks the decision the badge serves instead.
        Signal::Specifics => (
            "Could someone begin work on this without having to ask what is meant?",
            "It names what to change, where to change it, or how to see the current \
             behaviour \u{2014} files, commands, inputs, steps, or a concrete description \
             of the wanted result",
            "It is in general terms only, so a reader would have to decide for \
             themselves what is being asked for",
        ),
        Signal::Criteria => (
            "Does this ticket state what finishing it would look like — an acceptance \
             criterion, an expected behaviour, or a definition of the fix?",
            "It states what the finished result should be, or lists criteria to meet",
            "It describes only the problem or the request, leaving what counts as \
             done unstated",
        ),
        Signal::Actionable => (
            "Does this ticket describe work for someone to carry out \u{2014} something to \
             build, change, fix, or investigate \u{2014} rather than being a question, a \
             discussion, or a record of work already finished?",
            "It describes a defect, a gap, or a change to make. A bug report counts, \
             whether or not it phrases itself as a request",
            "It only asks a question, opens a discussion, or records work that is \
             already complete",
        ),
        Signal::Blocked => (
            "As the thread currently stands, is this work stopped until something \
             outside it happens \u{2014} another ticket landing, an external dependency, or \
             an answer only someone else can give?",
            "The latest state of the thread says it cannot proceed yet, because \
             something it depends on has not happened",
            "Nothing outside it is outstanding, or something that was outstanding \
             has since been resolved. A ticket that is merely vague, not yet \
             designed, unscheduled, or still under investigation is NOT blocked",
        ),
        // REVERTED to the #167 wording after calibration (#168, round 3).
        //
        // The round-2 attempt — "has the work already been done … nothing left
        // to do here" — read as *true* for any finished ticket, because a closed
        // thread ends in "Work complete". Six false positives against one true
        // positive. This wording has one, measured in round 1.
        //
        // Neither separates. The question conflates "covered somewhere else"
        // with "this ticket is finished", and no rewording of a single Noul over
        // a thread that contains its own completion notice will fix that. It
        // needs redesign; see the calibration report and its follow-up.
        Signal::Duplicate => (
            "Does this thread state that the work described is already covered \
             somewhere else \u{2014} another issue, a pull request, or work already done?",
            "Someone says it duplicates, is covered by, or was already done \
             elsewhere",
            "No one says that, or it is only mentioned as related rather than as \
             the same work",
        ),
    };
    json!({
        "type": "noul",
        "instructions": instructions,
        "criteria": { "true": yes, "false": no },
    })
}

/// The state for one ticket: named fields, truncated.
///
/// `comment_count` is the real total even when fewer comments are sent, so the
/// model is told it is seeing a tail rather than the whole thread. The org name
/// is never included — ADR 0002 clause 3 still holds for that.
pub fn state(title: &str, body: &str, comments: &[String], comment_count: u64) -> Value {
    let recent: Vec<Value> = comments
        .iter()
        .rev()
        .take(COMMENTS_SENT)
        .rev()
        .map(|c| Value::String(truncate(c, COMMENT_CHARS)))
        .collect();
    json!({
        "title": title,
        "body": truncate(body, BODY_CHARS),
        "recent_comments": recent,
        "total_comments": comment_count,
    })
}

/// Cut to `max` characters on a char boundary, marking that it was cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max).collect();
    format!("{kept}… [truncated]")
}

fn request_body(state: Value) -> Value {
    let questions: Map<String, Value> = SIGNALS
        .iter()
        .map(|s| (s.id().to_string(), question(*s)))
        .collect();
    json!({ "state": state, "model": MODEL, "questions": questions })
}

/// The five probabilities, as answered. Raw on purpose: the policy in
/// [`Readiness::verdict`] is a pure function over them, so changing a
/// threshold or the badge wording never needs a new request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Readiness {
    pub specifics: f64,
    pub criteria: f64,
    pub actionable: f64,
    pub blocked: f64,
    pub duplicate: f64,
}

impl Readiness {
    fn get(&self, signal: Signal) -> f64 {
        match signal {
            Signal::Specifics => self.specifics,
            Signal::Criteria => self.criteria,
            Signal::Actionable => self.actionable,
            Signal::Blocked => self.blocked,
            Signal::Duplicate => self.duplicate,
        }
    }

    /// What to show. Vetoes are checked first and named individually: they do
    /// not compensate, so averaging them into one score would let a blocked
    /// ticket with an excellent specifics read as ready.
    pub fn verdict(&self) -> Verdict {
        if self.blocked > YES {
            return Verdict::Blocked;
        }
        if self.duplicate > YES {
            return Verdict::MaybeDuplicate;
        }
        if self.actionable < NO {
            return Verdict::NotActionable;
        }
        // A signal the model could not call leaves the verdict undecided rather
        // than rounded into a confident one — but only for the signals whose
        // direction the verdict is about to rely on, listed in [`HEDGED`].
        let undecided: Vec<Signal> = HEDGED
            .iter()
            .copied()
            .filter(|s| (NO..=YES).contains(&self.get(*s)))
            .collect();
        if !undecided.is_empty() {
            return Verdict::Unsure(undecided);
        }
        let missing: Vec<Signal> = [Signal::Specifics, Signal::Criteria]
            .into_iter()
            .filter(|s| self.get(*s) < NO)
            .collect();
        if missing.is_empty() {
            Verdict::Ready
        } else {
            Verdict::Thin(missing)
        }
    }

    /// Collapse the answers for the five questions, by question id.
    fn from_answers(answers: &HashMap<String, Answer>) -> Result<Self> {
        let at = |signal: Signal| -> Result<f64> {
            match answers.get(signal.id()).and_then(Answer::noul) {
                Some(p) => Ok(p),
                None => bail!("TypeSafe response is missing `{}`", signal.id()),
            }
        };
        Ok(Self {
            specifics: at(Signal::Specifics)?,
            criteria: at(Signal::Criteria)?,
            actionable: at(Signal::Actionable)?,
            blocked: at(Signal::Blocked)?,
            duplicate: at(Signal::Duplicate)?,
        })
    }
}

/// What the badge says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Waiting on something unresolved.
    Blocked,
    /// The thread says this is already covered elsewhere.
    MaybeDuplicate,
    /// Not a piece of work.
    NotActionable,
    /// These signals landed between the thresholds.
    Unsure(Vec<Signal>),
    /// A specifics and a stated outcome.
    Ready,
    /// Workable, but these are absent.
    Thin(Vec<Signal>),
}

impl Verdict {
    /// The badge, as one line.
    pub fn line(&self) -> String {
        let names = |s: &[Signal]| s.iter().map(|s| s.id()).collect::<Vec<_>>().join(", ");
        match self {
            Verdict::Blocked => "blocked \u{2014} waiting on something unresolved".into(),
            Verdict::MaybeDuplicate => {
                "may be a duplicate \u{2014} the thread says this is covered elsewhere".into()
            }
            Verdict::NotActionable => {
                "not a work item \u{2014} reads as a question or update".into()
            }
            Verdict::Unsure(s) => format!("unsure \u{2014} cannot judge {}", names(s)),
            Verdict::Ready => "ready \u{2014} is specific and states an outcome".into(),
            Verdict::Thin(s) => format!("thin \u{2014} no {}", names(s)),
        }
    }
}

/// Judge one ticket. Any failure is the caller's to swallow: the badge is
/// advisory, so not having one behaves exactly as before this existed.
pub async fn assess(client: &Client, state: Value) -> Result<Readiness> {
    let answers = client.ask(&request_body(state)).await?;
    Readiness::from_answers(&answers)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clean, decisive answer: workable and nothing wrong with it.
    fn ready() -> Readiness {
        Readiness {
            specifics: 0.95,
            criteria: 0.92,
            actionable: 0.98,
            blocked: 0.02,
            duplicate: 0.03,
        }
    }

    #[test]
    fn a_decisive_clean_answer_is_ready() {
        assert_eq!(ready().verdict(), Verdict::Ready);
        assert!(ready().verdict().line().starts_with("ready"));
    }

    /// The badge must not claim a reproduction (#171). A precise spec for
    /// something not yet built has none, and telling the reader it "has a repro"
    /// — or that it lacks one — was the bug the `repro` -> `specifics` rename fixed.
    #[test]
    fn the_badge_never_talks_about_a_reproduction() {
        for verdict in [
            Verdict::Ready,
            Verdict::Thin(vec![Signal::Specifics]),
            Verdict::Thin(vec![Signal::Specifics, Signal::Criteria]),
            Verdict::Unsure(vec![Signal::Specifics]),
        ] {
            let line = verdict.line();
            assert!(
                !line.to_lowercase().contains("repro"),
                "`{line}` still talks about a reproduction"
            );
        }
        assert_eq!(
            Verdict::Ready.line(),
            "ready \u{2014} is specific and states an outcome"
        );
        assert_eq!(
            Verdict::Thin(vec![Signal::Specifics]).line(),
            "thin \u{2014} no specifics"
        );
    }

    #[test]
    fn each_veto_fires_on_its_own_and_names_itself() {
        for (mutate, expected) in [
            (
                (|r: &mut Readiness| r.blocked = 0.91) as fn(&mut Readiness),
                Verdict::Blocked,
            ),
            (
                |r: &mut Readiness| r.duplicate = 0.88,
                Verdict::MaybeDuplicate,
            ),
            (
                |r: &mut Readiness| r.actionable = 0.05,
                Verdict::NotActionable,
            ),
        ] {
            let mut r = ready();
            mutate(&mut r);
            assert_eq!(r.verdict(), expected, "from {r:?}");
        }
    }

    #[test]
    fn a_veto_beats_strong_quality_signals() {
        // The whole reason vetoes are not averaged in: a blocked ticket with an
        // excellent specifics and clear criteria is still blocked.
        let r = Readiness {
            specifics: 1.0,
            criteria: 1.0,
            blocked: 0.95,
            ..ready()
        };
        assert_eq!(r.verdict(), Verdict::Blocked);
    }

    #[test]
    fn vetoes_are_reported_in_a_fixed_order_when_several_fire() {
        let r = Readiness {
            blocked: 0.9,
            duplicate: 0.9,
            actionable: 0.1,
            ..ready()
        };
        assert_eq!(r.verdict(), Verdict::Blocked, "blocked is reported first");
    }

    #[test]
    fn a_missing_quality_signal_is_named() {
        let thin_specifics = Readiness {
            specifics: 0.04,
            ..ready()
        };
        assert_eq!(
            thin_specifics.verdict(),
            Verdict::Thin(vec![Signal::Specifics]),
            "{thin_specifics:?}"
        );
        assert_eq!(
            thin_specifics.verdict().line(),
            "thin \u{2014} no specifics"
        );

        let thin_both = Readiness {
            specifics: 0.04,
            criteria: 0.08,
            ..ready()
        };
        assert_eq!(
            thin_both.verdict(),
            Verdict::Thin(vec![Signal::Specifics, Signal::Criteria])
        );
        assert_eq!(
            thin_both.verdict().line(),
            "thin \u{2014} no specifics, criteria"
        );
    }

    #[test]
    fn a_signal_in_the_middle_band_is_undecided_not_rounded() {
        // A noul near 0.5 means equally likely either way. There is no
        // confidence value to gate on, so the probability has to carry it, and
        // guessing a verdict from it would be inventing an answer.
        for p in [NO, 0.5, YES] {
            let r = Readiness {
                specifics: p,
                ..ready()
            };
            assert_eq!(
                r.verdict(),
                Verdict::Unsure(vec![Signal::Specifics]),
                "specifics = {p} should not resolve either way"
            );
        }
        // Just outside the band it resolves again.
        assert_eq!(
            Readiness {
                specifics: YES + 0.01,
                ..ready()
            }
            .verdict(),
            Verdict::Ready
        );
        assert_eq!(
            Readiness {
                specifics: NO - 0.01,
                ..ready()
            }
            .verdict(),
            Verdict::Thin(vec![Signal::Specifics])
        );
    }

    /// The verdict reads `actionable` only as `< NO`, so a middling value means
    /// "not vetoed" and must not force `unsure`. This is the bug #169 fixed:
    /// five well-specified bug reports read `unsure` because `actionable` sat in
    /// the band, though nothing depends on it being high.
    #[test]
    fn a_middling_actionable_is_not_vetoed_and_does_not_hedge() {
        for p in [NO, 0.37, 0.5, 0.65, YES] {
            let r = Readiness {
                actionable: p,
                ..ready()
            };
            assert_eq!(
                r.verdict(),
                Verdict::Ready,
                "actionable = {p} must not make a ready ticket unsure"
            );
        }
    }

    /// ...and the asymmetry is real: the same middling value on a signal a
    /// positive claim rests on *does* hedge, for every signal in `HEDGED`.
    #[test]
    fn every_hedged_signal_still_hedges_when_undecided() {
        for signal in HEDGED {
            let mut r = ready();
            match signal {
                Signal::Specifics => r.specifics = 0.5,
                Signal::Criteria => r.criteria = 0.5,
                Signal::Blocked => r.blocked = 0.5,
                Signal::Duplicate => r.duplicate = 0.5,
                Signal::Actionable => unreachable!("not hedged"),
            }
            assert_eq!(r.verdict(), Verdict::Unsure(vec![signal]), "{signal:?}");
        }
    }

    /// `actionable` is the only signal outside `HEDGED`, so adding a sixth to
    /// `SIGNALS` without deciding its direction fails here rather than silently
    /// inheriting one.
    #[test]
    fn hedged_is_every_signal_except_actionable() {
        let unhedged: Vec<Signal> = SIGNALS
            .iter()
            .copied()
            .filter(|s| !HEDGED.contains(s))
            .collect();
        assert_eq!(unhedged, vec![Signal::Actionable]);
    }

    /// The veto is untouched: a low `actionable` still fires, however the
    /// undecided rule is scoped.
    #[test]
    fn a_low_actionable_still_vetoes() {
        let r = Readiness {
            actionable: NO - 0.01,
            ..ready()
        };
        assert_eq!(r.verdict(), Verdict::NotActionable);
    }

    #[test]
    fn an_undecided_veto_signal_does_not_veto_but_is_reported() {
        let r = Readiness {
            blocked: 0.5,
            ..ready()
        };
        assert_eq!(r.verdict(), Verdict::Unsure(vec![Signal::Blocked]));
        assert_eq!(r.verdict().line(), "unsure \u{2014} cannot judge blocked");
    }

    #[test]
    fn a_decisive_veto_wins_over_another_signal_being_undecided() {
        let r = Readiness {
            blocked: 0.95,
            specifics: 0.5,
            ..ready()
        };
        assert_eq!(r.verdict(), Verdict::Blocked);
    }

    // ---- state ----

    #[test]
    fn only_the_most_recent_comments_are_sent_with_the_true_total() {
        let comments: Vec<String> = (1..=47).map(|n| format!("comment {n}")).collect();
        let s = state("t", "b", &comments, 47);
        let sent = s["recent_comments"].as_array().unwrap();
        assert_eq!(sent.len(), COMMENTS_SENT);
        // The tail, in order: the current state of play is what the questions
        // are worded against.
        assert_eq!(sent[0], "comment 38");
        assert_eq!(sent[COMMENTS_SENT - 1], "comment 47");
        // The model is told it is seeing a tail, not the whole thread.
        assert_eq!(s["total_comments"], 47);
    }

    #[test]
    fn a_short_thread_is_sent_whole_and_in_order() {
        let comments = vec!["first".to_string(), "second".to_string()];
        let s = state("t", "b", &comments, 2);
        assert_eq!(s["recent_comments"].as_array().unwrap().len(), 2);
        assert_eq!(s["recent_comments"][0], "first");
        assert_eq!(s["recent_comments"][1], "second");
    }

    #[test]
    fn long_text_is_cut_and_says_so() {
        let long = "x".repeat(BODY_CHARS + 500);
        let s = state("t", &long, &["y".repeat(COMMENT_CHARS + 50)], 1);
        let body = s["body"].as_str().unwrap();
        assert!(body.ends_with("… [truncated]"), "body not marked as cut");
        assert_eq!(
            body.chars().count(),
            BODY_CHARS + "… [truncated]".chars().count()
        );
        assert!(
            s["recent_comments"][0]
                .as_str()
                .unwrap()
                .ends_with("… [truncated]")
        );
    }

    #[test]
    fn truncation_never_splits_a_char() {
        // Multi-byte throughout: a byte-slicing implementation would panic.
        let s = "é".repeat(BODY_CHARS + 10);
        assert_eq!(
            truncate(&s, BODY_CHARS).chars().count(),
            BODY_CHARS + "… [truncated]".chars().count()
        );
    }

    // ---- the request ----

    #[test]
    fn the_request_asks_all_five_as_nouls_with_explicit_criteria() {
        let body = request_body(state("t", "b", &[], 0));
        let questions = body["questions"].as_object().unwrap();
        assert_eq!(questions.len(), 5);
        for signal in SIGNALS {
            let q = &questions[signal.id()];
            assert_eq!(q["type"], "noul", "{}", signal.id());
            assert!(
                q["instructions"].as_str().is_some_and(|s| s.len() > 20),
                "{} has no instructions",
                signal.id()
            );
            // The jaggedness notes call for criteria aligned with the
            // instructions; both sides are spelled out.
            assert!(q["criteria"]["true"].is_string(), "{}", signal.id());
            assert!(q["criteria"]["false"].is_string(), "{}", signal.id());
        }
        assert_eq!(body["model"], MODEL);
    }

    #[test]
    fn the_request_carries_issue_text_but_never_the_org_or_repo() {
        // ADR 0004 widened this to titles, bodies and comments. It did not
        // widen it to the org name, which ADR 0002 clause 3 still covers.
        let body = request_body(state(
            "Upgrade Calico",
            "steps to reproduce",
            &["blocked on #184".to_string()],
            1,
        ));
        let wire = body.to_string();
        assert!(wire.contains("Upgrade Calico"));
        assert!(wire.contains("steps to reproduce"));
        assert!(wire.contains("blocked on #184"));
        // Nothing in the builder can carry them: it takes no org or repo.
        assert!(!wire.contains("pgmac-net"));
    }

    // ---- collapsing answers ----

    fn answers(pairs: &[(&str, f64)]) -> HashMap<String, Answer> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), Answer::Noul { noul: *v }))
            .collect()
    }

    #[test]
    fn answers_are_read_by_question_id() {
        let r = Readiness::from_answers(&answers(&[
            ("specifics", 0.9),
            ("criteria", 0.8),
            ("actionable", 0.7),
            ("blocked", 0.1),
            ("duplicate", 0.2),
        ]))
        .expect("all five present");
        assert_eq!(r.specifics, 0.9);
        assert_eq!(r.duplicate, 0.2);
    }

    #[test]
    fn a_missing_answer_fails_rather_than_defaulting_to_a_verdict() {
        let err = Readiness::from_answers(&answers(&[("specifics", 0.9)]))
            .expect_err("four answers are missing");
        assert!(err.to_string().contains("criteria"), "{err}");
    }

    #[test]
    fn a_score_where_a_noul_was_asked_for_fails() {
        // Guards the enum: reading a Score's distribution as a probability
        // would silently produce a verdict from the wrong number.
        let mut a = answers(&[
            ("criteria", 0.8),
            ("actionable", 0.7),
            ("blocked", 0.1),
            ("duplicate", 0.2),
        ]);
        a.insert(
            "specifics".into(),
            Answer::Score {
                probabilities: HashMap::new(),
                confidence: 0.9,
            },
        );
        assert!(Readiness::from_answers(&a).is_err());
    }
}

/// Calibration of the five questions and the two thresholds against real
/// tickets (#168).
///
/// #156's calibration could commit its corpus: 55 label *names*. This one
/// cannot — a readiness corpus is issue bodies and comment threads, the text
/// ADR 0004 singles out as the most sensitive in a repository, and this repo is
/// public. So the corpus is a list of **references** with hand-written
/// expectations, and the harness fetches each ticket live.
///
/// Public `pgmac-net` repos only, so any reviewer can open any case and
/// disagree with the expectation recorded against it.
///
/// **The rule that makes this worth anything:** expectations are written from
/// reading the ticket, *before* any request, and are never edited to match a
/// returned number. Only question wordings may change, only where reading says
/// the question is wrong, and every change is disclosed.
#[cfg(test)]
mod calibration {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;

    /// What a reader expects of one signal on one ticket.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    enum Expect {
        /// Must come back above `YES`.
        Yes,
        /// Must come back below `NO`.
        No,
        /// Must not fall below `NO`, and nothing more.
        ///
        /// For a signal the verdict reads only as `< NO` — `actionable` — this
        /// is the whole of what "yes" can mean. `Yes` would assert it exceeds
        /// `YES`, a property nothing consumes: #168 asserted exactly that on 18
        /// cases and reported the resulting mismatch as a defect in the question
        /// (#169).
        NotVetoed,
        /// Genuinely arguable. Recorded, asserted on neither side — #163's
        /// "ambiguous band", which is what stops a threshold being fitted to a
        /// case a reader could not call either.
        Unasserted,
    }

    use Expect::{No as N, NotVetoed as V, Unasserted as U, Yes as Y};

    /// One corpus case. `expect` is in [`SIGNALS`] order.
    struct Case {
        repo: &'static str,
        number: u64,
        expect: [Expect; 5],
        /// Why this case is in the corpus — the thing it is meant to probe.
        note: &'static str,
    }

    /// Expectations written from reading each ticket, before any request.
    ///
    /// The `specifics` column was **re-derived for #171** against "could someone
    /// begin work on this without having to ask what is meant?", from the ticket
    /// text, in the commit *before* the one that recorded any measurement — so
    /// `git log` proves the order. Seven changed from the `repro` column it
    /// replaces; the reasons are in `docs/development-log.md`.
    ///
    /// `blocked` has **no `Yes` case**: no open issue in any public `pgmac-net`
    /// repo is waiting on something unresolved as its thread currently stands.
    /// That side is therefore unmeasured, and the harness says so rather than
    /// resting a threshold on a manufactured example.
    //                                    specifics criteria actionable blocked duplicate
    const CORPUS: &[Case] = &[
        // ---- well-specified bug reports: both quality signals present ----
        Case {
            repo: "nagios-public-status-page",
            number: 69,
            expect: [Y, Y, V, N, N],
            note: "route pattern plus the exact URL that fails to match",
        },
        Case {
            repo: "nagios-public-status-page",
            number: 60,
            expect: [Y, Y, V, N, N],
            note: "observed JSON from the live deployment",
        },
        Case {
            repo: "nagios-public-status-page",
            number: 67,
            expect: [Y, Y, V, N, N],
            note: "has its own Measured section",
        },
        Case {
            repo: "nagios-public-status-page",
            number: 71,
            // duplicate CORRECTED No -> Yes (#170). Its only comment reads
            // "Already fixed by 5e6e6e9 (PR #70, merged 2026-07-29)", and #67's
            // merge comment independently says it fixed #71 along the way. I
            // marked it `No` in #168 from the body alone and never read the
            // thread; the model read it and was right at 0.97. This is a fact I
            // can quote, not a re-judgement after seeing a number — which is the
            // only kind of change to an expectation this corpus allows.
            expect: [Y, Y, V, N, Y],
            note: "comment says 'Already fixed by 5e6e6e9 (PR #70, merged)'; body names the offending fixtures",
        },
        Case {
            repo: "incidents",
            number: 48,
            expect: [Y, Y, V, N, N],
            note: "incident date, observed restart counts, explicit thresholds",
        },
        Case {
            repo: "docker-registry-walk",
            number: 59,
            expect: [Y, Y, V, N, N],
            note: "cites another repo's code as the model to copy, which is \
                      not a duplicate claim",
        },
        // ---- the Jev trap: a blocker raised mid-thread and later resolved ----
        // "Blocked on Step 0 (the gate measurement)" appears verbatim in a
        // comment, and a later comment closes it out. Jev "reads dates as text,
        // not as ordered quantities", so this is the documented failure mode.
        Case {
            repo: "docker-registry-walk",
            number: 96,
            expect: [Y, Y, V, N, N],
            note: "THE BLOCKED TRAP: 'Blocked on Step 0' mid-thread, \
                      resolved by the last comment",
        },
        // ---- the one real duplicate in the public pool ----
        // Open, and its only comment is "Addressed in incidents#87 (merged)".
        // Exactly the case the wording was written for: someone said it is
        // already covered and nobody closed the ticket.
        Case {
            repo: "incidents",
            number: 86,
            expect: [Y, U, V, N, Y],
            note: "THE DUPLICATE: 'Addressed in #87 (merged)', still open",
        },
        // ---- observable problem, no stated outcome ----
        Case {
            repo: "incidents",
            number: 49,
            expect: [Y, N, V, N, N],
            note: "investigation with times and symptoms but no definition of done",
        },
        // ---- thin: vague or speculative ----
        Case {
            repo: "Docker-Nagios",
            number: 1,
            expect: [N, N, V, N, N],
            note: "speculative throughout, ends 'Maybe not that'",
        },
        Case {
            repo: "incidents",
            number: 72,
            expect: [N, U, V, N, N],
            note: "says it needs brainstorming, yet names concrete wants",
        },
        Case {
            repo: "gh-issues-tui",
            number: 60,
            expect: [N, N, U, N, N],
            note: "'We need to brainstorm these' — actionable is arguable",
        },
        Case {
            repo: "Docker-Nagios",
            number: 3,
            expect: [N, N, U, N, U],
            note: "115 chars and a Slack link; comment says 'Nothing to see here'",
        },
        // ---- outcome stated, nothing to reproduce ----
        Case {
            repo: "Docker-Nagios",
            number: 4,
            expect: [Y, Y, V, N, N],
            note: "small precise spec, but no problem to observe",
        },
        Case {
            repo: "metasearch",
            number: 22,
            expect: [U, Y, V, N, N],
            note: "two-bullet outcome, plus Linear migration metadata as noise",
        },
        Case {
            repo: "metasearch",
            number: 19,
            expect: [U, U, V, N, N],
            note: "body is one sentence; the Current State and Gaps to Fix detail is in a comment",
        },
        Case {
            repo: "gh-issues-tui",
            number: 129,
            expect: [Y, Y, V, N, N],
            note: "gives the input format and the triggering keypress",
        },
        // ---- not a work item ----
        Case {
            repo: "gh-issues-tui",
            number: 130,
            expect: [N, N, N, N, N],
            note: "THE QUESTION: 51 chars, 'Is it possible to...'",
        },
        Case {
            repo: "tremendous-cve",
            number: 10,
            expect: [U, U, N, N, U],
            note: "open, but the body is a record of work already DONE & MERGED",
        },
        // ---- references to other issues that are not duplicate claims ----
        Case {
            repo: "incidents",
            number: 75,
            expect: [Y, Y, V, N, N],
            note: "'Follow-up to #63 / #12' — a lineage, not a duplicate",
        },
        Case {
            repo: "gh-issues-tui",
            number: 160,
            expect: [Y, Y, V, N, N],
            note: "feature proposal citing code locations; last comment says \
                      a follow-up was filed as #168",
        },
        Case {
            repo: "gh-issues-tui",
            number: 168,
            expect: [Y, Y, V, N, N],
            note: "LITERAL-MINDEDNESS: discusses duplicate detection at \
                      length without being a duplicate",
        },
    ];

    /// FNV-1a over the five serialised questions.
    ///
    /// Readiness has no `PROMPT_VERSION` — nothing is cached, so there was never
    /// anything to invalidate — so this digest is what stops a reworded question
    /// silently inheriting thresholds tuned against the old wording.
    ///
    /// Hand-rolled rather than `DefaultHasher`, whose output is explicitly not
    /// stable across Rust releases and so cannot be committed, and rather than a
    /// new dependency for one guard. `serde_json` orders object keys, so the
    /// serialisation is deterministic.
    fn questions_digest() -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for signal in SIGNALS {
            for b in question(signal).to_string().as_bytes() {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        format!("{h:016x}")
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct RecordedCase {
        r#ref: String,
        note: String,
        expect: BTreeMap<String, Expect>,
        probabilities: BTreeMap<String, f64>,
        /// Recorded for a reader, never asserted: a verdict is five signals
        /// through two thresholds, so asserting it would make every legitimate
        /// re-tune a corpus edit, and the composition has its own tests.
        verdict: String,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct Recording {
        model: String,
        questions_digest: String,
        yes: f64,
        no: f64,
        cases: Vec<RecordedCase>,
    }

    impl Case {
        fn reference(&self) -> String {
            format!("pgmac-net/{}#{}", self.repo, self.number)
        }
        fn expect_of(&self, signal: Signal) -> Expect {
            let i = SIGNALS
                .iter()
                .position(|s| *s == signal)
                .expect("in SIGNALS");
            self.expect[i]
        }
    }

    const RECORDING_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/typesafe/readiness-calibration.json"
    );

    /// One issue with the comment thread **as production fetches it**.
    ///
    /// `comments(first: 100)` deliberately, not `last: 10`: the app fetches the
    /// first hundred and `state` then takes the tail of those. Querying the last
    /// ten directly would send something the app never sends, and a harness has
    /// to measure the real path.
    const ONE_ISSUE: &str = "
query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    issue(number: $number) {
      title
      body
      comments(first: 100) { totalCount nodes { body } }
    }
  }
}";

    /// Fetch one case and build exactly the state the app would send.
    ///
    /// Its own GraphQL call rather than a provider method: nothing in the app
    /// fetches an issue by `owner/repo#N`, and adding that would mean a trait
    /// method on GitHub, Linear and Jira whose only caller is an ignored test.
    async fn fetch_state(http: &reqwest::Client, case: &Case) -> Value {
        let resp = http
            .post("https://api.github.com/graphql")
            .json(&json!({
                "query": ONE_ISSUE,
                "variables": {
                    "owner": "pgmac-net",
                    "name": case.repo,
                    "number": case.number,
                },
            }))
            .send()
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", case.reference()));
        assert!(
            resp.status().is_success(),
            "{}: HTTP {}",
            case.reference(),
            resp.status()
        );
        let body: Value = resp.json().await.expect("GitHub response shape");
        let issue = body
            .pointer("/data/repository/issue")
            .unwrap_or_else(|| panic!("{}: not found ({body})", case.reference()));
        let comments: Vec<String> = issue["comments"]["nodes"]
            .as_array()
            .expect("comment nodes")
            .iter()
            .map(|n| n["body"].as_str().unwrap_or_default().to_string())
            .collect();
        state(
            issue["title"].as_str().unwrap_or_default(),
            issue["body"].as_str().unwrap_or_default(),
            &comments,
            issue["comments"]["totalCount"].as_u64().unwrap_or(0),
        )
    }

    /// Ask the live API about every case, print the table, write the recording.
    ///
    /// `cargo test calibrate_readiness_against_live_api -- --ignored --nocapture`
    ///
    /// Needs `TYPESAFE_API_KEY` and a GitHub token. Reports only — asserts
    /// nothing, so model drift can never fail CI. Each case is its own `state`
    /// and so its own request; unlike #156's 55 labels these cannot be batched.
    #[tokio::test]
    #[ignore = "live APIs; needs TYPESAFE_API_KEY and a GitHub token"]
    async fn calibrate_readiness_against_live_api() {
        let client = Client::from_settings(true).expect("set TYPESAFE_API_KEY");
        let token = crate::github::auth::resolve_token(None).expect("GitHub token");
        let http = crate::provider::http::build_http_client(
            &format!("Bearer {token}"),
            &[("User-Agent", "gh-issues-calibration")],
        )
        .expect("http client");

        let mut recorded = Vec::new();
        for case in CORPUS {
            let state = fetch_state(&http, case).await;
            let readiness = assess(&client, state)
                .await
                .unwrap_or_else(|e| panic!("{}: {e}", case.reference()));
            recorded.push(RecordedCase {
                r#ref: case.reference(),
                note: case.note.to_string(),
                expect: SIGNALS
                    .iter()
                    .map(|s| (s.id().to_string(), case.expect_of(*s)))
                    .collect(),
                probabilities: SIGNALS
                    .iter()
                    .map(|s| (s.id().to_string(), readiness.get(*s)))
                    .collect(),
                verdict: readiness.verdict().line(),
            });
        }

        print_table(&recorded);
        report_gaps(&recorded);

        let recording = Recording {
            model: MODEL.to_string(),
            questions_digest: questions_digest(),
            yes: YES,
            no: NO,
            cases: recorded,
        };
        std::fs::write(
            RECORDING_PATH,
            format!("{}\n", serde_json::to_string_pretty(&recording).unwrap()),
        )
        .expect("write recording");
        println!("\nwrote {RECORDING_PATH}");
    }

    /// Per case: expectation against measurement, and the verdict.
    fn print_table(cases: &[RecordedCase]) {
        println!("\nYES = {YES}   NO = {NO}   model = {MODEL}\n");
        println!(
            "{:<42} {:>10} {:>10} {:>11} {:>9} {:>10}   verdict",
            "ref", "specifics", "criteria", "actionable", "blocked", "duplicate"
        );
        for c in cases {
            let cell = |s: Signal| {
                let p = c.probabilities[s.id()];
                let mark = match c.expect[s.id()] {
                    Expect::Yes if p > YES => " ",
                    Expect::NotVetoed if p >= NO => " ",
                    Expect::No if p < NO => " ",
                    Expect::Unasserted => "\u{00b7}",
                    _ => "!",
                };
                format!("{p:.2}{mark}")
            };
            println!(
                "{:<42} {:>10} {:>10} {:>11} {:>9} {:>10}   {}",
                c.r#ref,
                cell(Signal::Specifics),
                cell(Signal::Criteria),
                cell(Signal::Actionable),
                cell(Signal::Blocked),
                cell(Signal::Duplicate),
                c.verdict
            );
        }
        println!("\n  ! = disagrees with the expectation   \u{00b7} = unasserted\n");
    }

    // ----------------------------------------------------------------------
    // Offline guards over the committed recording. Run in CI with no key.
    //
    // These deliberately do **not** assert that every expectation holds. It
    // does not: `specifics` and `duplicate` do not separate at all, and
    // `actionable` separates only well below `YES`. Asserting the intended
    // behaviour would mean a permanently red suite, so instead these pin the
    // measured state — characterisation tests, like the `#87` screen goldens —
    // so the redesign cannot land without updating what is claimed here.

    const RECORDED: &str = include_str!("readiness-calibration.json");

    const RECALIBRATE: &str = "re-run `cargo test calibrate_readiness_against_live_api \
         -- --ignored --nocapture`, read the table, and update the claims in \
         `docs/ticket-readiness.md`";

    fn recording() -> Recording {
        serde_json::from_str(RECORDED).expect("readiness-calibration.json must parse")
    }

    /// Worst-case separation for one signal: `(worst no, worst yes)`, or `None`
    /// when either side has no asserted case.
    fn gap(rec: &Recording, signal: Signal) -> Option<(f64, f64)> {
        let side = |e: Expect| -> Vec<f64> {
            rec.cases
                .iter()
                .filter(|c| c.expect[signal.id()] == e)
                .map(|c| c.probabilities[signal.id()])
                .collect()
        };
        let (yes, no) = (side(Expect::Yes), side(Expect::No));
        if yes.is_empty() || no.is_empty() {
            return None;
        }
        Some((
            no.iter().copied().fold(f64::MIN, f64::max),
            yes.iter().copied().fold(f64::MAX, f64::min),
        ))
    }

    /// For a signal the verdict reads only as `< NO` — `actionable` — the range
    /// `NO` must fall inside: above the worst asserted `no` (so those are
    /// vetoed) and at or below the worst asserted `not_vetoed` (so none of those
    /// is). `None` when either side has no case.
    ///
    /// This is a constraint on `NO` alone. It says nothing about `YES`, which is
    /// the point: #168 folded `actionable` into the two-sided gap and reported
    /// its low yes-side as a defect.
    fn veto_range(rec: &Recording, signal: Signal) -> Option<(f64, f64)> {
        let side = |e: Expect| -> Vec<f64> {
            rec.cases
                .iter()
                .filter(|c| c.expect[signal.id()] == e)
                .map(|c| c.probabilities[signal.id()])
                .collect()
        };
        let (kept, vetoed) = (side(Expect::NotVetoed), side(Expect::No));
        if kept.is_empty() || vetoed.is_empty() {
            return None;
        }
        Some((
            vetoed.iter().copied().fold(f64::MIN, f64::max),
            kept.iter().copied().fold(f64::MAX, f64::min),
        ))
    }

    /// The recording and the corpus must describe the same cases, or the
    /// committed numbers are about tickets nobody listed.
    #[test]
    fn the_recording_covers_exactly_the_corpus() {
        let rec = recording();
        let recorded: Vec<&str> = rec.cases.iter().map(|c| c.r#ref.as_str()).collect();
        assert_eq!(rec.cases.len(), CORPUS.len(), "{RECALIBRATE}");
        for case in CORPUS {
            assert!(
                recorded.contains(&case.reference().as_str()),
                "`{}` is in the corpus but not the recording \u{2014} {RECALIBRATE}",
                case.reference()
            );
        }
    }

    /// The drift guard. Readiness has no `PROMPT_VERSION` and caches nothing, so
    /// this digest is the only thing stopping a reworded question from silently
    /// inheriting a measurement taken against the old wording.
    #[test]
    fn the_recording_was_made_with_the_current_questions_and_model() {
        let rec = recording();
        assert_eq!(
            rec.questions_digest,
            questions_digest(),
            "a question wording changed since the recording \u{2014} {RECALIBRATE}"
        );
        assert_eq!(rec.model, MODEL, "the model changed \u{2014} {RECALIBRATE}");
    }

    /// The recorded thresholds are the ones the code uses, so the table in the
    /// docs describes the running behaviour.
    #[test]
    fn the_recording_carries_the_thresholds_in_force() {
        let rec = recording();
        assert_eq!(rec.yes, YES);
        assert_eq!(rec.no, NO);
    }

    /// `duplicate` separates, and its veto is right on both real duplicates.
    ///
    /// #168 reported the opposite — worst asserted `no` 0.97 above worst asserted
    /// `yes` 0.87 — and #170 was filed as "needs redesign". Both rested on one
    /// wrong expectation: `nagios-public-status-page#71` was marked `no` from its
    /// body, but its only comment says "Already fixed by 5e6e6e9 (PR #70,
    /// merged)". The model read the comment and was right. Corrected in the commit
    /// before this one was recorded.
    ///
    /// The veto reads `duplicate` as `> YES`, so what has to hold is that it fires
    /// on the asserted `yes` cases and on none of the asserted `no` ones — not a
    /// two-sided gap. It also has to hold that `YES` sits inside the measured gap,
    /// or moving it would change that.
    #[test]
    fn duplicate_separates_and_its_veto_is_right() {
        let rec = recording();
        let (worst_no, worst_yes) = gap(&rec, Signal::Duplicate).expect("both sides asserted");
        assert!(
            worst_no < worst_yes,
            "`duplicate` stopped separating ({worst_no:.2} .. {worst_yes:.2}) \u{2014} {RECALIBRATE}"
        );
        assert!(
            worst_no < YES && YES <= worst_yes,
            "YES = {YES} is outside duplicate's gap ({worst_no:.2} .. {worst_yes:.2}]"
        );
        for c in &rec.cases {
            let p = c.probabilities["duplicate"];
            match c.expect["duplicate"] {
                Expect::Yes => assert!(
                    p > YES,
                    "{} is a real duplicate but scores {p:.2} and escapes the veto",
                    c.r#ref
                ),
                Expect::No => assert!(
                    p <= YES,
                    "{} is not a duplicate but scores {p:.2} and would be vetoed",
                    c.r#ref
                ),
                _ => {}
            }
        }
    }

    /// **Pinned as measured (#171).** `specifics` replaced `repro`, whose gap was
    /// inverted by 0.33. This one is inverted by 0.09 — better, but still not a
    /// signal a threshold can split — and the inversion rests **entirely on two
    /// named cases**, both flagged as hard before any measurement was taken:
    ///
    /// * `incidents#72` (asserted `no`): its body names concrete wants while also
    ///   saying the design needs brainstorming. The one expectation I hesitated on.
    /// * `Docker-Nagios#4` (asserted `yes`): the anchor case #171 was filed about.
    ///
    /// Neither was re-marked after the result was seen — doing so would be the
    /// fitting the commit-before-measurement ordering exists to prevent. This
    /// pins that they are the extremes, so the claim in the docs stays checkable.
    #[test]
    fn specifics_is_inverted_only_because_of_two_contested_cases() {
        let rec = recording();
        let of = |e: Expect| -> Vec<(&str, f64)> {
            rec.cases
                .iter()
                .filter(|c| c.expect["specifics"] == e)
                .map(|c| (c.r#ref.as_str(), c.probabilities["specifics"]))
                .collect()
        };
        let extreme = |v: Vec<(&str, f64)>, worst_is_max: bool| -> (String, f64) {
            let pick = if worst_is_max {
                v.into_iter().max_by(|a, b| a.1.total_cmp(&b.1))
            } else {
                v.into_iter().min_by(|a, b| a.1.total_cmp(&b.1))
            };
            let (r, p) = pick.expect("both sides asserted");
            (r.to_string(), p)
        };
        let (worst_no_ref, worst_no) = extreme(of(Expect::No), true);
        let (worst_yes_ref, worst_yes) = extreme(of(Expect::Yes), false);

        assert!(
            worst_no > worst_yes,
            "`specifics` now separates ({worst_no:.2} .. {worst_yes:.2}) \u{2014} good news, \
             but the docs and this test must be updated"
        );
        assert_eq!(worst_no_ref, "pgmac-net/incidents#72");
        assert_eq!(worst_yes_ref, "pgmac-net/Docker-Nagios#4");

        // Set those two aside and the rest separate widely — which is what makes
        // this an honest finding about two contested tickets, not a broken signal.
        let rest = |e: Expect, keep_max: bool| {
            let xs = of(e)
                .into_iter()
                .filter(|(r, _)| *r != worst_no_ref && *r != worst_yes_ref)
                .map(|(_, p)| p);
            if keep_max {
                xs.fold(f64::MIN, f64::max)
            } else {
                xs.fold(f64::MAX, f64::min)
            }
        };
        let (rest_no, rest_yes) = (rest(Expect::No, true), rest(Expect::Yes, false));
        assert!(
            rest_yes - rest_no > 0.3,
            "the other cases no longer separate widely ({rest_no:.2} .. {rest_yes:.2})"
        );
    }

    /// The `actionable` veto is right: it fires on the cases a reader expects and
    /// never on a real work item.
    ///
    /// #168 reported `actionable` as under-reading because its asserted-`yes`
    /// range sat below `YES`. That measured a property nothing consumes — the
    /// verdict reads `actionable` only as `< NO` — so the expectation is
    /// `NotVetoed`, and this asserts what the verdict actually does (#169).
    #[test]
    fn the_actionable_veto_never_fires_on_a_real_work_item() {
        let rec = recording();
        for c in &rec.cases {
            let p = c.probabilities["actionable"];
            match c.expect["actionable"] {
                Expect::NotVetoed => assert!(
                    p >= NO,
                    "{} is a real work item but scores {p:.2} < NO ({NO}) and would be \
                     vetoed \u{2014} {RECALIBRATE}",
                    c.r#ref
                ),
                Expect::No => assert!(
                    p < NO,
                    "{} is not a work item but scores {p:.2} and escapes the veto",
                    c.r#ref
                ),
                _ => {}
            }
        }
        // And `NO` sits inside the one-sided range the recording measured.
        let (worst_vetoed, worst_kept) =
            veto_range(&rec, Signal::Actionable).expect("both sides asserted");
        assert!(
            worst_vetoed < NO && NO <= worst_kept,
            "NO = {NO} is outside ({worst_vetoed:.2} .. {worst_kept:.2}]"
        );
    }

    /// The recorded verdict line is what the current code says about the
    /// recorded probabilities. A change to the verdict logic that is not
    /// followed by a re-record would otherwise leave the committed table
    /// describing behaviour the code no longer has — which is exactly how
    /// #169's fix could have gone unrecorded.
    #[test]
    fn the_recorded_verdicts_are_what_the_current_code_says() {
        let rec = recording();
        for c in &rec.cases {
            let p = &c.probabilities;
            let readiness = Readiness {
                specifics: p["specifics"],
                criteria: p["criteria"],
                actionable: p["actionable"],
                blocked: p["blocked"],
                duplicate: p["duplicate"],
            };
            assert_eq!(
                c.verdict,
                readiness.verdict().line(),
                "{} \u{2014} the verdict logic changed since the recording; {RECALIBRATE}",
                c.r#ref
            );
        }
    }

    /// An in-band `actionable` must not be what makes a ticket `unsure` (#169).
    /// Every recorded `unsure` has a hedged signal in the band.
    #[test]
    fn no_recorded_unsure_rests_on_actionable_alone() {
        let rec = recording();
        for c in rec.cases.iter().filter(|c| c.verdict.starts_with("unsure")) {
            assert!(
                !c.verdict.contains("actionable"),
                "{}: `{}` \u{2014} actionable must not be reported as undecided",
                c.r#ref,
                c.verdict
            );
        }
    }

    /// `criteria` is the one signal that both separates and straddles the
    /// threshold correctly.
    #[test]
    fn criteria_separates_across_the_threshold() {
        let rec = recording();
        let (worst_no, worst_yes) = gap(&rec, Signal::Criteria).expect("both sides");
        assert!(
            worst_no < worst_yes,
            "criteria stopped separating ({worst_no:.2} .. {worst_yes:.2}) \u{2014} {RECALIBRATE}"
        );
    }

    /// `blocked` has no asserted `yes` case: no open issue in any public
    /// `pgmac-net` repo is waiting on something unresolved. Its `no` side is
    /// clean, so the half that could be measured was.
    #[test]
    fn blocked_has_no_yes_case_in_the_public_pool() {
        let rec = recording();
        assert!(
            gap(&rec, Signal::Blocked).is_none(),
            "a `blocked = yes` case now exists \u{2014} measure it and update the docs"
        );
        for case in &rec.cases {
            let p = case.probabilities["blocked"];
            assert!(
                p < YES,
                "{} scores {p:.2} on blocked, but nothing in the corpus is blocked",
                case.r#ref
            );
        }
    }

    /// No single pair of thresholds fits every signal, which is why this work
    /// did **not** move `YES` or `NO`. Moving them would hide it.
    #[test]
    fn no_single_pair_of_thresholds_fits_every_signal() {
        let rec = recording();
        let measured: Vec<(f64, f64)> = SIGNALS.iter().filter_map(|s| gap(&rec, *s)).collect();
        let worst_no = measured.iter().map(|g| g.0).fold(f64::MIN, f64::max);
        let worst_yes = measured.iter().map(|g| g.1).fold(f64::MAX, f64::min);
        assert!(
            worst_no >= worst_yes,
            "the gaps now intersect ({worst_no:.2} .. {worst_yes:.2}) \u{2014} thresholds can \
             finally be justified from the recording; do that and update the docs"
        );
    }

    /// Per-signal separation, the intersection, and what is left unmeasured.
    fn report_gaps(cases: &[RecordedCase]) {
        let mut worst_no_all = f64::MIN;
        let mut worst_yes_all = f64::MAX;
        // The most `NO` may be while still not vetoing a not-vetoed case.
        let mut veto_ceiling = f64::MAX;
        println!(
            "{:<12} {:>7} {:>7} {:>22} {:>7}",
            "signal", "n(yes)", "n(no)", "gap (worst no..worst yes)", "width"
        );
        for s in SIGNALS {
            let p = |e: Expect| -> Vec<f64> {
                cases
                    .iter()
                    .filter(|c| c.expect[s.id()] == e)
                    .map(|c| c.probabilities[s.id()])
                    .collect()
            };
            let yes = p(Expect::Yes);
            let no = p(Expect::No);
            let kept = p(Expect::NotVetoed);
            let worst_yes = yes.iter().copied().fold(f64::MAX, f64::min);
            let worst_no = no.iter().copied().fold(f64::MIN, f64::max);
            if !kept.is_empty() {
                // One-sided: constrains NO alone and says nothing about YES.
                let worst_kept = kept.iter().copied().fold(f64::MAX, f64::min);
                worst_no_all = worst_no_all.max(worst_no);
                veto_ceiling = veto_ceiling.min(worst_kept);
                println!(
                    "{:<12} {:>7} {:>7} {:>22} {:>7}",
                    s.id(),
                    format!("{}*", kept.len()),
                    no.len(),
                    format!("NO in ({worst_no:.2} .. {worst_kept:.2}]"),
                    "1-sided"
                );
                continue;
            }
            let gap = if yes.is_empty() || no.is_empty() {
                "  (one side unmeasured)".to_string()
            } else {
                worst_no_all = worst_no_all.max(worst_no);
                worst_yes_all = worst_yes_all.min(worst_yes);
                format!("{worst_no:.2} .. {worst_yes:.2}")
            };
            let width = if yes.is_empty() || no.is_empty() {
                "-".to_string()
            } else {
                format!("{:.2}", worst_yes - worst_no)
            };
            println!(
                "{:<12} {:>7} {:>7} {:>22} {:>7}",
                s.id(),
                yes.len(),
                no.len(),
                gap,
                width
            );
        }
        println!("\nintersection of measured gaps: {worst_no_all:.2} .. {worst_yes_all:.2}");
        if worst_no_all >= worst_yes_all {
            println!(
                "  EMPTY \u{2014} no single pair of thresholds fits every signal. \
                 Reword the outlier and re-record; do not fit the expectations."
            );
        } else {
            println!("  NO must be > {worst_no_all:.2}; YES must be < {worst_yes_all:.2}");
        }
        if veto_ceiling < f64::MAX {
            println!(
                "  * one-sided signal: NO must also be <= {veto_ceiling:.2}, or a real \
                 work item is vetoed"
            );
        }
        let unsure = cases
            .iter()
            .filter(|c| c.verdict.starts_with("unsure"))
            .count();
        println!(
            "\nverdict is `unsure` for {unsure}/{} cases{}",
            cases.len(),
            if unsure * 2 > cases.len() {
                " \u{2014} dominant, so a band is wrong or a question is ambiguous"
            } else {
                ""
            }
        );
    }
}
