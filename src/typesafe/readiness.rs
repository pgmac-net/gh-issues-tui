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

/// One judgement about a ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Repro,
    Criteria,
    Actionable,
    Blocked,
    Duplicate,
}

impl Signal {
    /// Question id on the wire, and the name used in the badge.
    pub fn id(self) -> &'static str {
        match self {
            Signal::Repro => "repro",
            Signal::Criteria => "criteria",
            Signal::Actionable => "actionable",
            Signal::Blocked => "blocked",
            Signal::Duplicate => "duplicate",
        }
    }
}

/// Every signal, in the order a badge lists them.
pub const SIGNALS: [Signal; 5] = [
    Signal::Repro,
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
        Signal::Repro => (
            "Does this ticket describe concrete steps, inputs, or conditions that \
             would let someone reproduce or directly observe the problem?",
            "It gives specific steps, inputs, commands, logs, or conditions under \
             which the problem appears",
            "It describes the problem only in general terms, or is not about a \
             problem that can be observed",
        ),
        Signal::Criteria => (
            "Does this ticket state what finishing it would look like — an acceptance \
             criterion, an expected behaviour, or a definition of the fix?",
            "It states what the finished result should be, or lists criteria to meet",
            "It describes only the problem or the request, leaving what counts as \
             done unstated",
        ),
        Signal::Actionable => (
            "Is this ticket a piece of work for someone to carry out, as opposed to a \
             question, a discussion, or a status update?",
            "It asks for something to be built, changed, fixed, or investigated",
            "It is a question, a discussion, an announcement, or a status update",
        ),
        Signal::Blocked => (
            "As the thread currently stands, is this work waiting on something \
             unresolved — another ticket, a decision, or an external dependency?",
            "The most recent state of the thread says it is still waiting on \
             something that has not happened yet",
            "Nothing is outstanding, or something that was outstanding has since \
             been resolved",
        ),
        Signal::Duplicate => (
            "Does this thread state that the work described is already covered \
             somewhere else — another issue, a pull request, or work already done?",
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
    pub repro: f64,
    pub criteria: f64,
    pub actionable: f64,
    pub blocked: f64,
    pub duplicate: f64,
}

impl Readiness {
    fn get(&self, signal: Signal) -> f64 {
        match signal {
            Signal::Repro => self.repro,
            Signal::Criteria => self.criteria,
            Signal::Actionable => self.actionable,
            Signal::Blocked => self.blocked,
            Signal::Duplicate => self.duplicate,
        }
    }

    /// What to show. Vetoes are checked first and named individually: they do
    /// not compensate, so averaging them into one score would let a blocked
    /// ticket with an excellent repro read as ready.
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
        // Anything the model could not call leaves the verdict undecided
        // rather than rounded into a confident one.
        let undecided: Vec<Signal> = SIGNALS
            .iter()
            .copied()
            .filter(|s| (NO..=YES).contains(&self.get(*s)))
            .collect();
        if !undecided.is_empty() {
            return Verdict::Unsure(undecided);
        }
        let missing: Vec<Signal> = [Signal::Repro, Signal::Criteria]
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
            repro: at(Signal::Repro)?,
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
    /// A repro and a stated outcome.
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
            Verdict::Ready => "ready \u{2014} has a repro and a stated outcome".into(),
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
            repro: 0.95,
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
        // excellent repro and clear criteria is still blocked.
        let r = Readiness {
            repro: 1.0,
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
        let thin_repro = Readiness {
            repro: 0.04,
            ..ready()
        };
        assert_eq!(
            thin_repro.verdict(),
            Verdict::Thin(vec![Signal::Repro]),
            "{thin_repro:?}"
        );
        assert_eq!(thin_repro.verdict().line(), "thin \u{2014} no repro");

        let thin_both = Readiness {
            repro: 0.04,
            criteria: 0.08,
            ..ready()
        };
        assert_eq!(
            thin_both.verdict(),
            Verdict::Thin(vec![Signal::Repro, Signal::Criteria])
        );
        assert_eq!(
            thin_both.verdict().line(),
            "thin \u{2014} no repro, criteria"
        );
    }

    #[test]
    fn a_signal_in_the_middle_band_is_undecided_not_rounded() {
        // A noul near 0.5 means equally likely either way. There is no
        // confidence value to gate on, so the probability has to carry it, and
        // guessing a verdict from it would be inventing an answer.
        for p in [NO, 0.5, YES] {
            let r = Readiness {
                repro: p,
                ..ready()
            };
            assert_eq!(
                r.verdict(),
                Verdict::Unsure(vec![Signal::Repro]),
                "repro = {p} should not resolve either way"
            );
        }
        // Just outside the band it resolves again.
        assert_eq!(
            Readiness {
                repro: YES + 0.01,
                ..ready()
            }
            .verdict(),
            Verdict::Ready
        );
        assert_eq!(
            Readiness {
                repro: NO - 0.01,
                ..ready()
            }
            .verdict(),
            Verdict::Thin(vec![Signal::Repro])
        );
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
            repro: 0.5,
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
            ("repro", 0.9),
            ("criteria", 0.8),
            ("actionable", 0.7),
            ("blocked", 0.1),
            ("duplicate", 0.2),
        ]))
        .expect("all five present");
        assert_eq!(r.repro, 0.9);
        assert_eq!(r.duplicate, 0.2);
    }

    #[test]
    fn a_missing_answer_fails_rather_than_defaulting_to_a_verdict() {
        let err = Readiness::from_answers(&answers(&[("repro", 0.9)]))
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
            "repro".into(),
            Answer::Score {
                probabilities: HashMap::new(),
                confidence: 0.9,
            },
        );
        assert!(Readiness::from_answers(&a).is_err());
    }
}
