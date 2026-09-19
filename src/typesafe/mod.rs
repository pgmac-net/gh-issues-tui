//! Priority-rank inference for labels outside the `priority:<value>`
//! convention (#156).
//!
//! `Issue::priority_rank` only understands `priority:low|medium|high|urgent`.
//! A repo that labels priority `P0`, `sev1` or `blocker` ranks every issue 0,
//! so `SortKey::Priority` silently does nothing. This module asks TypeSafe's
//! System One model to rate each such label name on the same 1..=4 scale, and
//! the answers are stamped onto `Label::rank`.
//!
//! **Read-only by design.** A rank only affects sorting and title colour. It
//! never reaches the write path (`priority_set_options`,
//! `priority_label_set`), so a wrong judgement can mis-sort a row but can
//! never change a label on a backend. See `docs/adr/0002-…`.
//!
//! **Two-key consent.** Inference needs `infer_priority_ranks = true` in
//! config *and* `TYPESAFE_API_KEY` in the environment. A key exported for
//! other tools is not consent for this app to send an organisation's label
//! names to a third party. Only label *names* leave the machine — never
//! titles, bodies, numbers, URLs or the org name.
//!
//! The judgement is never load-bearing: with either key missing, offline, or
//! on any error, behaviour is exactly what it was before this module existed.

pub mod cache;

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::provider::http::build_http_client;
use cache::RankCache;

/// Environment variable holding the API key. Never a config field — tokens
/// are not stored in `config.toml`.
pub const API_KEY_ENV: &str = "TYPESAFE_API_KEY";

const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";

/// Pinned to a versioned id rather than `jev-latest` so the cache means
/// something: an entry records the model that produced it, and a different
/// model is a miss.
pub const MODEL: &str = "jev-1.13.0";

/// Bump when [`LEVELS`] or the question wording changes, so every cached rank
/// produced under the old wording is discarded rather than trusted.
pub const PROMPT_VERSION: u32 = 1;

/// Below this the answer is treated as "no opinion". Deliberately
/// conservative: a label wrongly ranked mis-sorts issues, while a label
/// wrongly left unranked merely behaves as it did before. **A starting value,
/// to be tuned against real label sets** — not yet a config knob because
/// there is no evidence yet about the right number.
const MIN_CONFIDENCE: f64 = 0.7;

/// Labels per request. Every question repeats the level descriptions, so this
/// keeps a request comfortably inside the 64k-token budget.
const BATCH: usize = 100;

const TIMEOUT: Duration = Duration::from_secs(30);

/// Score levels, indexed by the rank they map to. Level 0 is "not about
/// urgency at all" and becomes `None`, which is what an unranked label is
/// today.
const LEVELS: [&str; 5] = [
    "Says nothing about urgency or priority: it names a topic, area, kind of \
     work or workflow state, such as `bug`, `docs`, `frontend` or `wontfix`",
    "Low priority, can wait indefinitely, such as `low`, `P3`, `minor`, \
     `nice-to-have` or `sev4`",
    "Medium priority, normal queue with no special hurry, such as `medium`, \
     `P2`, `normal` or `sev3`",
    "High priority, should be picked up next, such as `high`, `P1`, \
     `important` or `sev2`",
    "Urgent, drop everything, such as `urgent`, `P0`, `critical`, `blocker` \
     or `sev1`",
];

/// Convert one Score answer to a rank.
///
/// Takes the most probable level rather than the weighted `score`: a split
/// distribution (say 45% "no urgency" and 45% "urgent") averages to a middle
/// level nobody voted for, whereas here it just yields low confidence and is
/// gated out. Level 0 and anything outside 1..=4 mean "not a priority label".
pub fn rank_from_answer(probabilities: &HashMap<String, f64>, confidence: f64) -> Option<u8> {
    let mut levels: Vec<(u8, f64)> = probabilities
        .iter()
        .filter_map(|(level, p)| Some((level.parse::<u8>().ok()?, *p)))
        .collect();
    // Ascending, and only a strictly greater probability replaces the leader,
    // so an exact tie resolves to the lower (more conservative) level.
    levels.sort_by_key(|(level, _)| *level);
    let mut best: Option<(u8, f64)> = None;
    for (level, p) in levels {
        if best.is_none_or(|(_, top)| p > top) {
            best = Some((level, p));
        }
    }
    let (level, _) = best?;
    if confidence < MIN_CONFIDENCE || !(1..=4).contains(&level) {
        return None;
    }
    Some(level)
}

/// The request body for one batch. The label goes in each question's
/// `instructions`, not its id: question ids are for the caller and are never
/// shown to the model.
fn request_body(labels: &[String]) -> Value {
    let questions: Map<String, Value> = labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            (
                format!("q{i}"),
                json!({
                    "type": "score",
                    "instructions": format!(
                        "How much urgency does the issue-tracker label `{label}` signal, \
                         judging by its name?"
                    ),
                    "criteria": LEVELS,
                }),
            )
        })
        .collect();
    json!({
        // The whole batch as context: a set like P0/P1/P2 reads as a scheme
        // where a lone `P1` might not.
        "state": { "labels": labels },
        "model": MODEL,
        "questions": questions,
    })
}

#[derive(Deserialize)]
struct Response {
    answers: HashMap<String, Answer>,
}

#[derive(Deserialize)]
struct Answer {
    probabilities: HashMap<String, f64>,
    confidence: f64,
}

/// A TypeSafe API client. Only ever constructed when inference is enabled and
/// a key is present — see [`Client::from_settings`].
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    endpoint: String,
}

impl Client {
    /// `Some` only when config opted in **and** `TYPESAFE_API_KEY` is set and
    /// non-empty. Either missing leaves the feature dormant.
    pub fn from_settings(enabled: bool) -> Option<Self> {
        Self::new(enabled, std::env::var(API_KEY_ENV).ok())
    }

    fn new(enabled: bool, key: Option<String>) -> Option<Self> {
        if !enabled {
            return None;
        }
        let key = key
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())?;
        let http = build_http_client(&format!("Bearer {key}"), &[]).ok()?;
        Some(Self {
            http,
            endpoint: ENDPOINT.to_string(),
        })
    }

    /// Rate each label. All-or-nothing: any failed batch fails the call, so
    /// nothing half-inferred is ever returned (or cached).
    pub async fn rank_labels(&self, labels: &[String]) -> Result<HashMap<String, Option<u8>>> {
        let mut out = HashMap::with_capacity(labels.len());
        for batch in labels.chunks(BATCH) {
            let resp = self
                .http
                .post(&self.endpoint)
                .timeout(TIMEOUT)
                .json(&request_body(batch))
                .send()
                .await
                .context("TypeSafe request failed")?;
            let status = resp.status();
            if !status.is_success() {
                bail!("TypeSafe returned {status}");
            }
            let parsed: Response = resp.json().await.context("unexpected TypeSafe response")?;
            for (i, label) in batch.iter().enumerate() {
                let Some(answer) = parsed.answers.get(&format!("q{i}")) else {
                    bail!("TypeSafe response is missing an answer for `{label}`");
                };
                out.insert(
                    label.clone(),
                    rank_from_answer(&answer.probabilities, answer.confidence),
                );
            }
        }
        Ok(out)
    }
}

/// Resolve ranks for `labels` in `org`: cache hits are free, only the misses
/// go to the API, and fresh answers are written back. A cache that cannot be
/// written is not an error — it only costs a repeat request next launch.
pub async fn resolve(
    client: &Client,
    cache_path: &Path,
    org: &str,
    labels: Vec<String>,
) -> Result<HashMap<String, Option<u8>>> {
    let mut cache = RankCache::load(cache_path);
    let mut out = HashMap::with_capacity(labels.len());
    let mut misses = Vec::new();
    for label in labels {
        match cache.get(org, &label) {
            Some(rank) => {
                out.insert(label, rank);
            }
            None => misses.push(label),
        }
    }
    if !misses.is_empty() {
        for (label, rank) in client.rank_labels(&misses).await? {
            cache.insert(org, &label, rank);
            out.insert(label, rank);
        }
        let _ = cache.save();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn probs(pairs: &[(&str, f64)]) -> HashMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn a_confident_level_becomes_its_rank() {
        let p = probs(&[("0", 0.01), ("3", 0.04), ("4", 0.95)]);
        assert_eq!(rank_from_answer(&p, 0.93), Some(4));
    }

    #[test]
    fn a_split_distribution_is_gated_out_not_averaged_into_a_middle_rank() {
        // 45% "no urgency" / 45% "urgent": the weighted score would be ~2
        // ("medium"), a level nobody voted for.
        let p = probs(&[
            ("0", 0.45),
            ("1", 0.03),
            ("2", 0.04),
            ("3", 0.03),
            ("4", 0.45),
        ]);
        assert_eq!(rank_from_answer(&p, 0.2), None);
    }

    #[test]
    fn level_zero_is_not_a_priority_label_even_when_confident() {
        let p = probs(&[
            ("0", 0.97),
            ("1", 0.01),
            ("2", 0.01),
            ("3", 0.005),
            ("4", 0.005),
        ]);
        assert_eq!(rank_from_answer(&p, 0.95), None);
    }

    #[test]
    fn the_confidence_gate_is_inclusive_at_the_threshold() {
        let p = probs(&[("2", 1.0)]);
        assert_eq!(rank_from_answer(&p, MIN_CONFIDENCE), Some(2));
        assert_eq!(rank_from_answer(&p, MIN_CONFIDENCE - 0.01), None);
    }

    #[test]
    fn an_exact_tie_resolves_to_the_lower_level() {
        let p = probs(&[("2", 0.5), ("1", 0.5)]);
        assert_eq!(rank_from_answer(&p, 0.9), Some(1));
    }

    #[test]
    fn malformed_levels_never_yield_a_rank() {
        assert_eq!(rank_from_answer(&HashMap::new(), 0.99), None);
        assert_eq!(rank_from_answer(&probs(&[("x", 1.0)]), 0.99), None);
        // Out of the 1..=4 range the sort understands.
        assert_eq!(rank_from_answer(&probs(&[("7", 1.0)]), 0.99), None);
    }

    #[test]
    fn the_label_is_named_in_the_question_and_only_label_names_are_sent() {
        let body = request_body(&["P0".to_string(), "chore".to_string()]);
        assert_eq!(body["model"], MODEL);
        assert_eq!(body["state"], json!({ "labels": ["P0", "chore"] }));
        let q0 = &body["questions"]["q0"];
        assert_eq!(q0["type"], "score");
        // Question ids are never shown to the model, so the label has to be
        // in the instructions or the model cannot know which one it is rating.
        assert!(q0["instructions"].as_str().unwrap().contains("`P0`"));
        assert!(
            body["questions"]["q1"]["instructions"]
                .as_str()
                .unwrap()
                .contains("`chore`")
        );
        assert_eq!(q0["criteria"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn inference_needs_both_the_config_flag_and_a_key() {
        assert!(Client::new(false, Some("k".into())).is_none(), "flag off");
        assert!(Client::new(true, None).is_none(), "no key");
        assert!(Client::new(true, Some("  ".into())).is_none(), "blank key");
        assert!(Client::new(true, Some("k".into())).is_some());
    }

    /// Serve exactly one canned response and hand back the raw request text.
    /// Once the task finishes the listener is dropped, so a second connection
    /// attempt is refused — which is how a test proves a call was *not* made.
    async fn one_shot_server(
        status: &'static str,
        body: String,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = sock.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..end]).to_lowercase();
                    let len = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if buf.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            let resp = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&buf).into_owned()
        });
        (format!("http://{addr}/v1/systemone"), handle)
    }

    fn client_at(endpoint: String) -> Client {
        let mut c = Client::new(true, Some("test-key".into())).unwrap();
        c.endpoint = endpoint;
        c
    }

    fn canned_answers() -> String {
        json!({
            "model": MODEL,
            "answers": {
                "q0": { "type": "score", "score": 3.9,
                        "legend": {},
                        "probabilities": { "0": 0.01, "1": 0.01, "2": 0.01, "3": 0.02, "4": 0.95 },
                        "confidence": 0.93 },
                "q1": { "type": "score", "score": 0.1,
                        "legend": {},
                        "probabilities": { "0": 0.9, "1": 0.04, "2": 0.02, "3": 0.02, "4": 0.02 },
                        "confidence": 0.9 },
            },
            "usage": { "input_tokens": 100, "output_tokens": 10 },
        })
        .to_string()
    }

    #[tokio::test]
    async fn resolve_sends_the_documented_request_fills_the_cache_and_then_stays_offline() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("ranks.json");
        let (url, server) = one_shot_server("200 OK", canned_answers()).await;
        let client = client_at(url);
        let labels = vec!["P0".to_string(), "chore".to_string()];

        let got = resolve(&client, &cache_path, "acme-private", labels.clone())
            .await
            .unwrap();
        assert_eq!(got["P0"], Some(4));
        assert_eq!(got["chore"], None);

        let raw = server.await.unwrap();
        let lower = raw.to_lowercase();
        assert!(lower.starts_with("post /v1/systemone"), "{raw}");
        assert!(lower.contains("authorization: bearer test-key"), "{raw}");
        let body: Value = serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["state"], json!({ "labels": ["P0", "chore"] }));
        assert!(
            !raw.contains("acme-private"),
            "the org name must not leave the machine"
        );

        // The server is gone. A second call can only succeed from the cache.
        let again = resolve(&client, &cache_path, "acme-private", labels)
            .await
            .expect("a warm cache must not touch the network");
        assert_eq!(again["P0"], Some(4));
        assert_eq!(again["chore"], None);
    }

    #[tokio::test]
    async fn an_http_error_fails_the_call_and_caches_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("ranks.json");
        let (url, _server) = one_shot_server("401 Unauthorized", "{}".into()).await;
        let err = resolve(&client_at(url), &cache_path, "acme", vec!["P0".into()])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("401"), "{err}");
        assert!(!cache_path.exists(), "a failure must not be cached");
    }

    #[tokio::test]
    async fn a_response_missing_an_answer_fails_rather_than_caching_a_guess() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("ranks.json");
        let (url, _server) = one_shot_server("200 OK", r#"{"answers":{}}"#.into()).await;
        let err = resolve(&client_at(url), &cache_path, "acme", vec!["P0".into()])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("`P0`"), "{err}");
        assert!(!cache_path.exists());
    }
}
