//! HTTP plumbing shared by every backend.
//!
//! What lives here is the part that is genuinely the same across providers:
//! building the `reqwest::Client`, holding the last-seen rate-limit state and
//! phrasing its message, walking a JSON path, and joining a GraphQL `errors`
//! array. What stays with each backend is the part that differs — which
//! headers carry the rate limit, how that backend signals a rate limit, and
//! any retry strategy of its own.

use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value, json};

use super::error::{ProviderError, RATE_LIMIT_MSG_PREFIX, Result};
use super::types::RateLimitData;

/// Build the HTTP client every backend uses, given the value for its
/// `Authorization` header and any extra default headers it needs. The auth
/// header shape differs per backend (`Bearer …` for GitHub, a raw key for
/// Linear, `Basic …` for Jira) but everything around it — marking the
/// credential sensitive, the user agent, the connection setup — does not.
pub fn build_http_client(
    auth_value: &str,
    extra: &[(&str, &str)],
) -> anyhow::Result<reqwest::Client> {
    let mut headers = reqwest::header::HeaderMap::new();
    let mut auth = reqwest::header::HeaderValue::from_str(auth_value)?;
    auth.set_sensitive(true);
    headers.insert(reqwest::header::AUTHORIZATION, auth);
    for (name, value) in extra {
        headers.insert(
            reqwest::header::HeaderName::from_bytes(name.as_bytes())?,
            reqwest::header::HeaderValue::from_str(value)?,
        );
    }
    Ok(reqwest::Client::builder()
        .user_agent(concat!("gh-issues/", env!("CARGO_PKG_VERSION")))
        .default_headers(headers)
        .build()?)
}

/// A fetch is usually several requests (page 1..n, then hydration), so cost is
/// accumulated between [`RateLimitStore::begin_fetch`] and
/// [`RateLimitStore::end_fetch`] and only published as a whole. Reporting the
/// last *request* instead would show the cost of the final hydration batch,
/// which is the least interesting number in the sequence.
#[derive(Default)]
struct Inner {
    data: Option<RateLimitData>,
    /// Cost accrued by the fetch currently in flight.
    fetch_cost: u64,
    /// Cost of the last fetch that ran to completion.
    last_fetch_cost: Option<u64>,
}

/// The last-seen rate-limit state, shared by every clone of a client.
///
/// Backends read their own headers and call [`RateLimitStore::set`]; the
/// storage and the user-facing message live here so the format string exists
/// in exactly one place. The event loop classifies errors by
/// [`RATE_LIMIT_MSG_PREFIX`], so that prefix must lead the message.
#[derive(Clone, Default)]
pub struct RateLimitStore(Arc<Mutex<Inner>>);

impl RateLimitStore {
    pub fn get(&self) -> Option<RateLimitData> {
        let inner = self.0.lock().unwrap();
        inner.data.map(|mut data| {
            data.last_cost = inner.last_fetch_cost;
            data
        })
    }

    pub fn set(&self, data: RateLimitData) {
        self.0.lock().unwrap().data = Some(data);
    }

    /// Start accumulating cost for a new multi-request fetch.
    pub fn begin_fetch(&self) {
        self.0.lock().unwrap().fetch_cost = 0;
    }

    /// Add one request's reported `rateLimit.cost`.
    pub fn add_cost(&self, cost: u64) {
        self.0.lock().unwrap().fetch_cost += cost;
    }

    /// Publish the accumulated cost as the last completed fetch's cost.
    pub fn end_fetch(&self) {
        let mut inner = self.0.lock().unwrap();
        inner.last_fetch_cost = Some(inner.fetch_cost);
    }

    /// The rate-limit message for the currently stored state, falling back to
    /// the bare prefix (optionally qualified by `context`) when nothing has
    /// been observed yet.
    pub fn message(&self, context: Option<&str>) -> String {
        match self.get() {
            Some(data) => format!(
                "{RATE_LIMIT_MSG_PREFIX} — {}/{} used, resets {}",
                data.remaining,
                data.limit,
                data.reset_time()
            ),
            None => match context {
                Some(c) => format!("{RATE_LIMIT_MSG_PREFIX} ({c})"),
                None => RATE_LIMIT_MSG_PREFIX.to_string(),
            },
        }
    }
}

/// Read an integer header, returning `None` when it is absent or unparseable.
pub fn header_i64(headers: &reqwest::header::HeaderMap, name: &str) -> Option<i64> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
}

/// Join a GraphQL `errors` array into one message, falling back to the raw
/// JSON when no entry carries a `message` field.
pub fn join_error_messages(errors: &Value) -> String {
    errors
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.get("message").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("; ")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| errors.to_string())
}

/// Deserialize the value at a nested JSON path, reporting a `Shape` error
/// naming the full path when a segment is missing.
pub fn parse_at<T: for<'de> Deserialize<'de>>(data: &Value, path: &[&str]) -> Result<T> {
    let mut cur = data;
    for seg in path {
        cur = cur
            .get(seg)
            .ok_or_else(|| ProviderError::Shape(format!("missing {}", path.join("."))))?;
    }
    serde_json::from_value(cur.clone()).map_err(|e| ProviderError::Shape(e.to_string()))
}

/// The GraphQL body shared by the GraphQL backends.
pub fn graphql_body(query: &str, variables: Value) -> Value {
    json!({ "query": query, "variables": variables })
}

/// Extract `data` from a GraphQL response body, turning a non-empty `errors`
/// array into a `ProviderError::Api`.
///
/// `rate_limited` lets a backend claim an errors array as a rate limit before
/// it is reported as a generic API error — GitHub and Linear each signal it
/// differently, so the detection stays with them while the surrounding
/// extraction is shared.
pub fn graphql_data(
    body: &Value,
    rate_limited: impl Fn(&Value) -> Option<ProviderError>,
) -> Result<Value> {
    if let Some(errors) = body
        .get("errors")
        .filter(|e| !e.as_array().is_none_or(|a| a.is_empty()))
    {
        if let Some(err) = rate_limited(errors) {
            return Err(err);
        }
        return Err(ProviderError::Api(join_error_messages(errors)));
    }
    body.get("data")
        .cloned()
        .ok_or_else(|| ProviderError::Shape("missing data".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                reqwest::header::HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn store_starts_empty_and_records_state() {
        let store = RateLimitStore::default();
        assert!(store.get().is_none());
        store.set(RateLimitData {
            remaining: 10,
            limit: 100,
            reset: 1_800_000_000,
            last_cost: None,
        });
        assert_eq!(store.get().unwrap().remaining, 10);
    }

    #[test]
    fn fetch_cost_accumulates_across_requests_and_publishes_once() {
        let store = RateLimitStore::default();
        store.set(RateLimitData {
            remaining: 4000,
            limit: 5000,
            reset: 1_800_000_000,
            last_cost: None,
        });
        // Nothing published until a fetch completes.
        assert_eq!(store.get().unwrap().last_cost, None);

        store.begin_fetch();
        store.add_cost(1); // bulk page
        store.add_cost(2); // hydration batch
        assert_eq!(store.get().unwrap().last_cost, None);
        store.end_fetch();
        assert_eq!(store.get().unwrap().last_cost, Some(3));

        // The next fetch reports its own cost, not a running total.
        store.begin_fetch();
        store.add_cost(1);
        store.end_fetch();
        assert_eq!(store.get().unwrap().last_cost, Some(1));
    }

    #[test]
    fn header_updates_do_not_clear_the_last_fetch_cost() {
        let store = RateLimitStore::default();
        store.begin_fetch();
        store.add_cost(5);
        store.end_fetch();
        // A later response's headers refresh remaining/limit/reset only.
        store.set(RateLimitData {
            remaining: 10,
            limit: 5000,
            reset: 1_800_000_000,
            last_cost: None,
        });
        let data = store.get().unwrap();
        assert_eq!(data.remaining, 10);
        assert_eq!(data.last_cost, Some(5));
    }

    #[test]
    fn store_clones_share_one_state() {
        let store = RateLimitStore::default();
        let clone = store.clone();
        clone.set(RateLimitData {
            remaining: 1,
            limit: 2,
            reset: 3,
            last_cost: None,
        });
        assert_eq!(store.get().unwrap().limit, 2, "clone must share the state");
    }

    #[test]
    fn message_leads_with_the_prefix_and_carries_counts() {
        let store = RateLimitStore::default();
        store.set(RateLimitData {
            remaining: 0,
            limit: 5000,
            reset: 1_800_000_000,
            last_cost: None,
        });
        let msg = store.message(None);
        assert!(msg.starts_with(RATE_LIMIT_MSG_PREFIX), "{msg}");
        assert!(msg.contains("0/5000 used"), "{msg}");
    }

    #[test]
    fn message_falls_back_to_the_prefix_when_nothing_observed() {
        let store = RateLimitStore::default();
        assert_eq!(store.message(None), RATE_LIMIT_MSG_PREFIX);
        assert_eq!(
            store.message(Some("GraphQL")),
            format!("{RATE_LIMIT_MSG_PREFIX} (GraphQL)")
        );
    }

    #[test]
    fn header_i64_reads_absent_and_unparseable_as_none() {
        let h = headers(&[("a", "42"), ("b", "nope")]);
        assert_eq!(header_i64(&h, "a"), Some(42));
        assert_eq!(header_i64(&h, "b"), None);
        assert_eq!(header_i64(&h, "missing"), None);
    }

    #[test]
    fn join_error_messages_concatenates_and_falls_back() {
        let errors = json!([{"message": "first"}, {"message": "second"}]);
        assert_eq!(join_error_messages(&errors), "first; second");

        let no_messages = json!([{"type": "SOMETHING"}]);
        assert_eq!(join_error_messages(&no_messages), no_messages.to_string());
    }

    #[test]
    fn parse_at_walks_a_path_and_names_a_missing_segment() {
        let data = json!({"a": {"b": {"c": 41}}});
        assert_eq!(parse_at::<u64>(&data, &["a", "b", "c"]).unwrap(), 41);

        let err = parse_at::<u64>(&json!({"a": {}}), &["a", "b", "c"]).unwrap_err();
        assert!(err.to_string().contains("a.b.c"), "{err}");
        assert!(matches!(err, ProviderError::Shape(_)));
    }

    #[test]
    fn graphql_data_extracts_data_and_reports_errors() {
        let ok = json!({"data": {"x": 1}});
        assert_eq!(graphql_data(&ok, |_| None).unwrap(), json!({"x": 1}));

        // An empty errors array is not an error.
        let empty = json!({"data": {"x": 1}, "errors": []});
        assert!(graphql_data(&empty, |_| None).is_ok());

        let failed = json!({"errors": [{"message": "boom"}]});
        let err = graphql_data(&failed, |_| None).unwrap_err();
        assert!(matches!(err, ProviderError::Api(m) if m == "boom"));

        let missing = json!({});
        assert!(matches!(
            graphql_data(&missing, |_| None).unwrap_err(),
            ProviderError::Shape(_)
        ));
    }

    #[test]
    fn graphql_data_lets_a_backend_claim_errors_as_a_rate_limit() {
        let body = json!({"errors": [{"message": "slow down"}]});
        let err =
            graphql_data(&body, |_| Some(ProviderError::RateLimited("mine".into()))).unwrap_err();
        assert!(matches!(err, ProviderError::RateLimited(m) if m == "mine"));
    }
}
