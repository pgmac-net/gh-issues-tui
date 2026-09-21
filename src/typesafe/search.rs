//! Semantic issue search (#158).
//!
//! With the `send_issue_text` consent, `/` becomes a **union**: substring hits
//! appear instantly as before, and issues judged to be *about* the query are
//! added when this lands. Without the consent, `/` is exactly the old substring
//! match. See `docs/semantic-search.md`.
//!
//! **Shape: one isolated request per candidate** — state `{query, issue}`, one
//! Noul. This is the reranking cookbook's pattern ("one request per candidate ·
//! no request sees another"), and it was reached the hard way.
//!
//! The first shape put every candidate in one shared state and asked one Noul
//! per candidate, naming each by a tag. It measured well once, and that was one
//! lucky ordering. The model has to *locate* each candidate in a large state and
//! sometimes locates the wrong one: numeric tags were read as issue numbers
//! (`I085` → `#85`), letter tags matched inside words (`RL` in "URLs"), and path
//! references (`` `issues[17]` ``) moved with position. The test that exposed it
//! was order invariance — the same issues shuffled — which measures the
//! mechanism, not the answers. With one issue per request there is no position,
//! so the whole failure class is gone by construction.

use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};
use futures::stream::{self, StreamExt};
use serde_json::{Value, json};

use super::{Answer, Client, MODEL};

/// A candidate counts as a semantic hit above this.
///
/// **0.70 is an override, not a measurement result.** The rule fixed before
/// the measurement (commit `c3ce53c`) was: the midpoint of the gap between the
/// best irrelevant score and the worst genuine hit across all fourteen runs, and
/// an empty gap ships no threshold. The gap *was* empty — one outage report,
/// `incidents#85`, scored 0.84 on "choosing how urgent a ticket is" because it
/// opens with "Provisional severity: P2 … Confirmed P2", above the worst genuine
/// hit at 0.71. Shipping at 0.70 anyway was a product decision taken with that
/// data: every genuine hit clears it in both runs, and across ~1,250 judgements
/// the only false hit is that report. The feature only ever *adds* rows to a
/// union, so a stray related row costs little.
///
/// Fragile at the bottom: the worst genuine hit, `incidents#82`, scored 0.71 in
/// one run and 0.80 in the other, so it can flicker near the line. The guards in
/// `calibration` pin all of this, so a re-record that changes it fails loudly.
/// Not the readiness badge's 0.7 by coincidence of value — it answers a
/// different question and was set separately.
pub const SEARCH_YES: f64 = 0.70;

/// Most requests in flight at once. 141 parallel requests saw no rate limiting;
/// this caps a large candidate set (closed issues loaded) near that.
pub const MAX_IN_FLIGHT: usize = 150;

/// Characters of body sent per candidate, as measured.
pub const BODY_CHARS: usize = 200;

const INSTRUCTIONS: &str = "Is `issue` about what `query` is asking for?";
const TRUE: &str = "The issue is about the subject of the query, even if it uses different words";
const FALSE: &str = "The issue is about something else, or only shares a word with the query";

/// One issue a search may return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The app's issue id — what a hit is reported as.
    pub id: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub body: String,
}

/// Whether `query` should reach the API at all.
///
/// Empty sends nothing. A bare issue number (`123`, `#123`) sends nothing
/// either: substring already answers it exactly, and it is not worth shipping
/// issue text for a lookup the app can do alone.
pub fn worth_searching(query: &str) -> bool {
    let q = query.trim();
    let digits = q.strip_prefix('#').unwrap_or(q);
    !(q.is_empty() || (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())))
}

/// One candidate's request: the query and that issue alone. The org is never
/// included — ADR 0004 still keeps it off the wire.
fn request(query: &str, c: &Candidate) -> Value {
    let body: String = c.body.chars().take(BODY_CHARS).collect();
    json!({
        "state": {
            "query": query,
            "issue": {
                "ref": format!("{}#{}", c.repo, c.number),
                "title": c.title,
                "body": body.split_whitespace().collect::<Vec<_>>().join(" "),
            },
        },
        "model": MODEL,
        "questions": {
            "about": {
                "type": "noul",
                "instructions": INSTRUCTIONS,
                "criteria": { "true": TRUE, "false": FALSE },
            },
        },
    })
}

/// Probability, per candidate id, that the candidate is about `query`.
///
/// Requests run concurrently, at most [`MAX_IN_FLIGHT`] at a time.
/// **All-or-nothing**: if any request fails the whole search fails, because a
/// list searched in part would be silently misleading — a missing hit would look
/// like "not relevant" rather than "not asked".
pub async fn score(
    client: &Client,
    query: &str,
    candidates: &[Candidate],
) -> Result<HashMap<String, f64>> {
    // Each future owns its body and id and borrows only the client, and they are
    // collected before streaming: a lazy `iter().map(..)` carries a closure that
    // is generic over the borrow's lifetime, which `tokio::spawn` rejects.
    let requests: Vec<_> = candidates
        .iter()
        .map(|c| {
            let body = request(query, c);
            let id = c.id.clone();
            async move {
                let answers = client.ask(&body).await?;
                match answers.get("about").and_then(Answer::noul) {
                    Some(p) => Ok((id, p)),
                    None => bail!("TypeSafe response is missing its answer for {id}"),
                }
            }
        })
        .collect();
    let results: Vec<Result<(String, f64)>> = stream::iter(requests)
        .buffer_unordered(MAX_IN_FLIGHT)
        .collect()
        .await;
    results.into_iter().collect()
}

/// The ids of the candidates that are about `query`.
pub async fn search(
    client: &Client,
    query: &str,
    candidates: &[Candidate],
) -> Result<HashSet<String>> {
    Ok(score(client, query, candidates)
        .await?
        .into_iter()
        .filter(|(_, p)| *p > SEARCH_YES)
        .map(|(id, _)| id)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(n: u64) -> Candidate {
        Candidate {
            id: format!("id{n}"),
            repo: "r".into(),
            number: n,
            title: format!("title {n}"),
            body: format!("body {n}"),
        }
    }

    #[test]
    fn empty_and_bare_numbers_send_nothing() {
        for q in ["", "   ", "123", "#123", " #7 "] {
            assert!(!worth_searching(q), "{q:?} should not be sent");
        }
        for q in ["login token", "#123 crash", "v2", "#", "a"] {
            assert!(worth_searching(q), "{q:?} should be sent");
        }
    }

    /// One issue per request: there is no list in the state, so there is no
    /// position for the model to mis-locate — the bug the shared-state shape had.
    #[test]
    fn a_request_holds_exactly_one_issue_and_one_question() {
        let body = request("disk trouble", &cand(85));
        assert_eq!(body["state"]["query"], "disk trouble");
        assert_eq!(body["state"]["issue"]["ref"], "r#85");
        assert!(body["state"]["issue"].is_object(), "not a list");
        let qs = body["questions"].as_object().unwrap();
        assert_eq!(qs.len(), 1);
        assert_eq!(qs["about"]["type"], "noul");
        assert_eq!(qs["about"]["instructions"], INSTRUCTIONS);
        assert_eq!(body["model"], MODEL);
    }

    #[test]
    fn the_body_is_cut_to_200_chars_before_whitespace_is_collapsed() {
        let c = Candidate {
            body: format!("a\r\n\n  b   {}", "é".repeat(500)),
            ..cand(1)
        };
        let body = request("q", &c)["state"]["issue"]["body"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(body.starts_with("a b "), "whitespace collapsed: {body:?}");
        // Multi-byte throughout, so byte slicing would panic or mis-cut.
        assert_eq!(
            body.chars().filter(|&c| c == 'é').count(),
            BODY_CHARS - "a\r\n\n  b   ".chars().count()
        );
    }

    #[test]
    fn the_org_is_never_sent() {
        let body = request("q", &cand(1)).to_string();
        assert!(body.contains("r#1"), "repo and number are sent");
        assert!(!body.contains("pgmac-net"), "the org never is");
    }
}

/// Measurement of [`SEARCH_YES`] (#158), and the guards that hold the code to it.
///
/// The corpus is every issue in nine public `pgmac-net` repos as it stood when
/// the threshold was first measured — public only, so no private issue text
/// leaves the machine. Seven queries each avoid their targets' literal words;
/// their expected hits were **written and hashed before any request**. One has
/// no answer, to test abstention.
///
/// Two runs per query: the **full** corpus (141 issues) and the corpus **scoped
/// to one repo** (9–63 issues), mirroring a search while filtered to one repo.
/// With one issue per request the candidate set cannot change a score, so the
/// scoped run is also a second, independent scoring of those issues: any
/// difference between the two is model noise, which the recording keeps.
#[cfg(test)]
mod calibration {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;

    const PUBLIC_REPOS: &[&str] = &[
        "Docker-Nagios",
        "claude-plugins",
        "docker-registry-walk",
        "gh-issues-tui",
        "incidents",
        "metasearch",
        "nagios-public-status-page",
        "pg-actions",
        "tremendous-cve",
    ];

    /// The corpus is the issues created before the expectations were written, so
    /// a later issue can never quietly join it.
    const CORPUS_CUTOFF: &str = "2026-09-21T11:48:11Z";
    const CORPUS_SIZE: usize = 141;

    struct Query {
        q: &'static str,
        must: &'static [&'static str],
        ok: &'static [&'static str],
        /// The repo the scoped run searches within.
        repo: &'static str,
    }

    /// Verbatim from the pre-registered expectations (sha256 20748a48…),
    /// translated from positional tags to refs.
    const QUERIES: &[Query] = &[
        Query {
            q: "persistent disks becoming unwritable in kubernetes",
            must: &["incidents#85", "incidents#82"],
            ok: &[
                "incidents#81",
                "incidents#79",
                "incidents#49",
                "incidents#48",
                "incidents#47",
            ],
            repo: "incidents",
        },
        Query {
            q: "traffic blackholed by a leftover routing entry",
            must: &["incidents#86", "incidents#79"],
            ok: &[],
            repo: "incidents",
        },
        Query {
            q: "keeping secrets out of version control",
            must: &["nagios-public-status-page#63"],
            ok: &["nagios-public-status-page#59"],
            repo: "nagios-public-status-page",
        },
        Query {
            q: "opening web addresses by pointing at them",
            must: &["gh-issues-tui#80"],
            ok: &["gh-issues-tui#130"],
            repo: "gh-issues-tui",
        },
        Query {
            q: "choosing how urgent a ticket is",
            must: &["gh-issues-tui#30", "gh-issues-tui#156"],
            ok: &[
                "gh-issues-tui#28",
                "gh-issues-tui#26",
                "gh-issues-tui#164",
                "gh-issues-tui#162",
                "gh-issues-tui#34",
                "gh-issues-tui#163",
            ],
            repo: "gh-issues-tui",
        },
        Query {
            q: "putting text somewhere it can be pasted later",
            must: &[
                "docker-registry-walk#75",
                "docker-registry-walk#40",
                "gh-issues-tui#46",
            ],
            ok: &["docker-registry-walk#97"],
            repo: "docker-registry-walk",
        },
        Query {
            q: "a cooking recipe for sourdough bread",
            must: &[],
            ok: &[],
            repo: "gh-issues-tui",
        },
    ];

    /// FNV-1a over the request shape: the question template and state keys. A
    /// reworded question fails the offline guard rather than silently inheriting
    /// a threshold measured against the old wording. Hand-rolled because
    /// `DefaultHasher` is not stable across Rust releases.
    fn request_digest() -> String {
        let probe = Candidate {
            id: String::new(),
            repo: "r".into(),
            number: 1,
            title: "t".into(),
            body: "b".into(),
        };
        let shape = request("q", &probe).to_string();
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in shape.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{h:016x}")
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct Run {
        candidates: usize,
        must: BTreeMap<String, f64>,
        ok: BTreeMap<String, f64>,
        /// The highest-scoring issues that are neither `must` nor `ok`.
        top_irrelevant: Vec<(String, f64)>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct Recorded {
        q: String,
        full: Run,
        scoped: Run,
        /// The same issues scored twice (full and scoped run): the largest and
        /// the median absolute difference. Model noise, since nothing else
        /// differs between the two requests.
        noise_max: f64,
        noise_median: f64,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct Recording {
        model: String,
        request_digest: String,
        search_yes: f64,
        max_in_flight: usize,
        body_chars: usize,
        queries: Vec<Recorded>,
    }

    const RECORDING_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/typesafe/search-calibration.json"
    );

    const RECALIBRATE: &str = "re-run `cargo test calibrate_semantic_search_against_live_api \
         -- --ignored --nocapture` and read the table before touching SEARCH_YES";

    async fn fetch_corpus(http: &reqwest::Client) -> Vec<Candidate> {
        let mut out = Vec::new();
        for repo in PUBLIC_REPOS {
            for page in 1.. {
                let url = format!(
                    "https://api.github.com/repos/pgmac-net/{repo}/issues?state=all&per_page=100&page={page}"
                );
                let items: Vec<Value> = http
                    .get(&url)
                    .send()
                    .await
                    .unwrap_or_else(|e| panic!("{repo}: {e}"))
                    .json()
                    .await
                    .expect("GitHub response shape");
                let n = items.len();
                for i in items {
                    let created = i["created_at"].as_str().unwrap_or_default();
                    if i.get("pull_request").is_some() || created >= CORPUS_CUTOFF {
                        continue;
                    }
                    let number = i["number"].as_u64().unwrap_or_default();
                    out.push(Candidate {
                        id: format!("{repo}#{number}"),
                        repo: (*repo).to_string(),
                        number,
                        title: i["title"].as_str().unwrap_or_default().to_string(),
                        body: i["body"].as_str().unwrap_or_default().to_string(),
                    });
                }
                if n < 100 {
                    break;
                }
            }
        }
        out
    }

    fn run(q: &Query, scores: &HashMap<String, f64>) -> Run {
        let pick = |refs: &[&str]| -> BTreeMap<String, f64> {
            refs.iter()
                .filter_map(|r| scores.get(*r).map(|p| ((*r).to_string(), *p)))
                .collect()
        };
        let mut irrelevant: Vec<(String, f64)> = scores
            .iter()
            .filter(|(r, _)| !q.must.contains(&r.as_str()) && !q.ok.contains(&r.as_str()))
            .map(|(r, p)| (r.clone(), *p))
            .collect();
        irrelevant.sort_by(|a, b| b.1.total_cmp(&a.1));
        irrelevant.truncate(5);
        Run {
            candidates: scores.len(),
            must: pick(q.must),
            ok: pick(q.ok),
            top_irrelevant: irrelevant,
        }
    }

    /// Send every query to the live API over the full and the scoped corpus,
    /// print the separation, and write the recording.
    ///
    /// `cargo test calibrate_semantic_search_against_live_api -- --ignored --nocapture`
    ///
    /// Needs `TYPESAFE_API_KEY` and a GitHub token. Public repos only. Reports,
    /// asserts nothing — model drift can never fail CI.
    #[tokio::test]
    #[ignore = "live APIs; needs TYPESAFE_API_KEY and a GitHub token"]
    async fn calibrate_semantic_search_against_live_api() {
        let client = Client::from_settings(true).expect("set TYPESAFE_API_KEY");
        let token = crate::github::auth::resolve_token(None).expect("GitHub token");
        let http = crate::provider::http::build_http_client(
            &format!("Bearer {token}"),
            &[("User-Agent", "gh-issues-calibration")],
        )
        .expect("http client");
        let corpus = fetch_corpus(&http).await;
        assert_eq!(
            corpus.len(),
            CORPUS_SIZE,
            "the measured corpus has changed \u{2014} an issue was transferred, \
             deleted or made private; the recording no longer describes it"
        );

        let mut queries = Vec::new();
        for q in QUERIES {
            let full = score(&client, q.q, &corpus).await.expect("full run");
            let scoped_corpus: Vec<Candidate> = corpus
                .iter()
                .filter(|c| c.repo == q.repo)
                .cloned()
                .collect();
            let scoped = score(&client, q.q, &scoped_corpus)
                .await
                .expect("scoped run");
            let mut diffs: Vec<f64> = scoped.iter().map(|(id, p)| (p - full[id]).abs()).collect();
            diffs.sort_by(f64::total_cmp);
            queries.push(Recorded {
                q: q.q.to_string(),
                full: run(q, &full),
                scoped: run(q, &scoped),
                noise_max: diffs.last().copied().unwrap_or(0.0),
                noise_median: diffs.get(diffs.len() / 2).copied().unwrap_or(0.0),
            });
        }

        for r in &queries {
            println!(
                "{:<52} noise: median {:.3}  max {:.3}",
                r.q, r.noise_median, r.noise_max
            );
            for (name, run) in [("full", &r.full), ("scoped", &r.scoped)] {
                let worst_must = run.must.values().copied().fold(f64::NAN, f64::min);
                let best_bad = run.top_irrelevant.first().map_or(0.0, |x| x.1);
                println!(
                    "{:<52} {name:6} n={:3}  worst must {:>5.2}  best irrelevant {:.2}",
                    r.q, run.candidates, worst_must, best_bad
                );
            }
        }

        let recording = Recording {
            model: MODEL.to_string(),
            request_digest: request_digest(),
            search_yes: SEARCH_YES,
            max_in_flight: MAX_IN_FLIGHT,
            body_chars: BODY_CHARS,
            queries,
        };
        std::fs::write(
            RECORDING_PATH,
            format!("{}\n", serde_json::to_string_pretty(&recording).unwrap()),
        )
        .expect("write recording");
        println!("\nwrote {RECORDING_PATH}");
    }

    // ------------------------------------------------------------------------
    // Offline guards over the committed recording. Run in CI with no key.

    const RECORDED: &str = include_str!("search-calibration.json");

    fn recording() -> Recording {
        serde_json::from_str(RECORDED).expect("search-calibration.json must parse")
    }

    /// `(worst genuine hit, best irrelevant)` across every recorded run.
    fn gap(rec: &Recording) -> (f64, f64) {
        let runs = || rec.queries.iter().flat_map(|q| [&q.full, &q.scoped]);
        let worst_must = runs()
            .flat_map(|r| r.must.values().copied())
            .fold(f64::MAX, f64::min);
        let best_irrelevant = runs()
            .flat_map(|r| r.top_irrelevant.iter().map(|(_, p)| *p))
            .fold(f64::MIN, f64::max);
        (worst_must, best_irrelevant)
    }

    /// The pre-registered rule, applied to the recording.
    fn rule(worst_must: f64, best_irrelevant: f64) -> f64 {
        let mid = (worst_must + best_irrelevant) / 2.0;
        let coarse = (mid * 20.0).round() / 20.0;
        if best_irrelevant < coarse && coarse < worst_must {
            coarse
        } else {
            (mid * 100.0).round() / 100.0
        }
    }

    #[test]
    fn the_recording_covers_exactly_the_queries() {
        let rec = recording();
        let recorded: Vec<&str> = rec.queries.iter().map(|q| q.q.as_str()).collect();
        let expected: Vec<&str> = QUERIES.iter().map(|q| q.q).collect();
        assert_eq!(recorded, expected, "{RECALIBRATE}");
    }

    /// A reworded question, a changed state shape or a new model must not
    /// inherit a threshold measured against the old one.
    #[test]
    fn the_recording_was_made_with_the_current_request_and_model() {
        let rec = recording();
        assert_eq!(
            rec.request_digest,
            request_digest(),
            "request changed \u{2014} {RECALIBRATE}"
        );
        assert_eq!(rec.model, MODEL, "model changed \u{2014} {RECALIBRATE}");
        assert_eq!(rec.body_chars, BODY_CHARS, "{RECALIBRATE}");
        assert_eq!(rec.max_in_flight, MAX_IN_FLIGHT, "{RECALIBRATE}");
    }

    /// The pre-registered rule could not produce a threshold: the recorded gap
    /// is empty. That is *why* `SEARCH_YES` is an override. If a future
    /// re-record opens a clean gap, this fails, and the rule should be used
    /// instead of the override.
    #[test]
    fn the_rule_found_no_clean_threshold_so_search_yes_is_an_override() {
        let rec = recording();
        let (worst_must, best_irrelevant) = gap(&rec);
        assert!(
            best_irrelevant >= worst_must,
            "the gap is now clean ({best_irrelevant:.2} .. {worst_must:.2}): drop the \
             override and set SEARCH_YES = {:.2} by the rule",
            rule(worst_must, best_irrelevant)
        );
        assert_eq!(rec.search_yes, SEARCH_YES, "{RECALIBRATE}");
    }

    #[test]
    fn at_the_override_every_genuine_hit_clears_in_both_runs() {
        let rec = recording();
        for q in &rec.queries {
            for (name, run) in [("full", &q.full), ("scoped", &q.scoped)] {
                for (id, p) in &run.must {
                    assert!(*p > SEARCH_YES, "{}: {name}: {id} at {p:.2}", q.q);
                }
            }
        }
    }

    /// The one known false hit, pinned exactly: any new one in a re-record fails.
    #[test]
    fn at_the_override_the_only_false_hit_is_the_known_one() {
        let rec = recording();
        let mut false_hits: Vec<(&str, &str)> = Vec::new();
        for q in &rec.queries {
            for run in [&q.full, &q.scoped] {
                // The recording keeps the top five; the fifth must miss, or hits
                // beyond it would go unseen.
                if let Some((_, p)) = run.top_irrelevant.last() {
                    assert!(
                        *p < SEARCH_YES,
                        "{}: false hits may extend past the top five",
                        q.q
                    );
                }
                for (id, p) in &run.top_irrelevant {
                    if *p > SEARCH_YES {
                        false_hits.push((q.q.as_str(), id.as_str()));
                    }
                }
            }
        }
        false_hits.sort_unstable();
        false_hits.dedup();
        assert_eq!(
            false_hits,
            vec![("choosing how urgent a ticket is", "incidents#85")],
            "{RECALIBRATE}"
        );
    }

    /// The query with no answer must find nothing, in either run.
    #[test]
    fn a_query_with_no_answer_finds_nothing() {
        let rec = recording();
        let q = rec
            .queries
            .iter()
            .find(|q| q.q.contains("sourdough"))
            .expect("the abstention query is recorded");
        for run in [&q.full, &q.scoped] {
            assert!(run.top_irrelevant.iter().all(|(_, p)| *p < SEARCH_YES));
        }
    }

    #[test]
    fn the_rule_stays_inside_the_gap() {
        // Coarse rounding inside the gap is taken...
        assert_eq!(rule(0.93, 0.49), 0.7);
        // ...and falls back to 0.01 when 0.05 would leave a narrow gap.
        assert_eq!(rule(0.79, 0.76), 0.78);
    }
}
