pub mod auth;
pub mod client;

pub use client::Client;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

/// A Jira priority name → the `priority:<value>` label value the rest of the
/// app understands. Jira has five levels; they fold onto the app's four
/// (both `Low` and `Lowest` map to `low`).
pub fn priority_name_to_value(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "highest" => Some("urgent"),
        "high" => Some("high"),
        "medium" => Some("medium"),
        "low" | "lowest" => Some("low"),
        _ => None,
    }
}

/// Inverse of [`priority_name_to_value`] — a `priority:<value>` label value
/// back to the Jira priority name written on mutation. `None` for unknown
/// values (leaves priority untouched).
pub fn priority_value_to_name(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "urgent" => Some("Highest"),
        "high" => Some("High"),
        "medium" => Some("Medium"),
        "low" => Some("Low"),
        _ => None,
    }
}

/// Prefix marking a synthetic `priority:*` label id. Jira priority is a native
/// field, not a label; these fabricated ids let the app's priority picker and
/// new-issue form work in `priority:*` terms. The ids never reach Jira — the
/// create/update paths recognise the prefix and route the value to the native
/// priority field instead.
pub const SYNTHETIC_PRIORITY_PREFIX: &str = "jira-priority:";

/// The four synthetic `priority:*` labels, ordered urgent → low, as
/// `(id, name)` pairs. Injected into `repo_labels`/`repo_form_options`.
pub fn synthetic_priority_labels() -> Vec<(String, String)> {
    ["urgent", "high", "medium", "low"]
        .iter()
        .map(|v| {
            (
                format!("{SYNTHETIC_PRIORITY_PREFIX}{v}"),
                format!("priority:{v}"),
            )
        })
        .collect()
}

/// If `id` is a synthetic priority-label id, return the Jira priority name it
/// stands for; otherwise `None` (a real label — Jira labels are plain strings).
pub fn synthetic_priority_id_to_name(id: &str) -> Option<&'static str> {
    id.strip_prefix(SYNTHETIC_PRIORITY_PREFIX)
        .and_then(priority_value_to_name)
}

/// Recursively flatten an Atlassian Document Format value to plain text. ADF is
/// a rich JSON tree; `text` leaves carry the content, and block nodes
/// (paragraph, heading, list item) are separated by newlines. Rich features
/// (marks, tables, media) are intentionally dropped — the detail pane wants
/// readable text, not fidelity.
pub fn adf_to_text(adf: &Value) -> String {
    let mut out = String::new();
    walk_adf(adf, &mut out);
    out.trim_end().to_string()
}

fn walk_adf(node: &Value, out: &mut String) {
    match node {
        Value::Object(map) => {
            if let Some(Value::String(text)) = map.get("text") {
                out.push_str(text);
            }
            if let Some(Value::Array(content)) = map.get("content") {
                for child in content {
                    walk_adf(child, out);
                }
            }
            // Block-level nodes end with a newline so paragraphs/list items
            // don't run together.
            if let Some(Value::String(ty)) = map.get("type")
                && matches!(
                    ty.as_str(),
                    "paragraph" | "heading" | "listItem" | "blockquote" | "codeBlock" | "rule"
                )
            {
                out.push('\n');
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_adf(item, out);
            }
        }
        _ => {}
    }
}

/// Wrap plain text in a minimal ADF document (one paragraph per line). Used for
/// creating issues and comments, which require an ADF body on Jira Cloud.
pub fn text_to_adf(text: &str) -> Value {
    let content: Vec<Value> = if text.is_empty() {
        vec![json!({ "type": "paragraph", "content": [] })]
    } else {
        text.split('\n')
            .map(|line| {
                if line.is_empty() {
                    json!({ "type": "paragraph", "content": [] })
                } else {
                    json!({
                        "type": "paragraph",
                        "content": [{ "type": "text", "text": line }]
                    })
                }
            })
            .collect()
    };
    json!({ "type": "doc", "version": 1, "content": content })
}

/// Parse a Jira datetime (`2026-01-02T03:04:05.678+0000` — a numeric offset
/// with no colon, so not RFC 3339). `None` on any parse failure.
pub fn parse_jira_dt(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.3f%z")
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// The numeric suffix of a Jira issue key (`PROJ-123` → `123`); `0` when the
/// key has no trailing number.
pub fn key_to_number(key: &str) -> u64 {
    key.rsplit('-')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_maps_five_levels_to_four() {
        assert_eq!(priority_name_to_value("Highest"), Some("urgent"));
        assert_eq!(priority_name_to_value("High"), Some("high"));
        assert_eq!(priority_name_to_value("Medium"), Some("medium"));
        assert_eq!(priority_name_to_value("Low"), Some("low"));
        assert_eq!(priority_name_to_value("Lowest"), Some("low"));
        assert_eq!(priority_name_to_value("Whatever"), None);
    }

    #[test]
    fn priority_value_back_to_jira_name() {
        assert_eq!(priority_value_to_name("urgent"), Some("Highest"));
        assert_eq!(priority_value_to_name("high"), Some("High"));
        assert_eq!(priority_value_to_name("medium"), Some("Medium"));
        assert_eq!(priority_value_to_name("low"), Some("Low"));
        assert_eq!(priority_value_to_name("p1"), None);
    }

    #[test]
    fn synthetic_priority_labels_well_formed() {
        let labels = synthetic_priority_labels();
        assert_eq!(labels.len(), 4);
        assert_eq!(
            labels[0],
            ("jira-priority:urgent".into(), "priority:urgent".into())
        );
        for (id, _name) in &labels {
            assert!(synthetic_priority_id_to_name(id).is_some(), "{id}");
        }
        assert_eq!(
            synthetic_priority_id_to_name("jira-priority:urgent"),
            Some("Highest")
        );
        assert_eq!(synthetic_priority_id_to_name("real-string-label"), None);
    }

    #[test]
    fn adf_flattens_nested_text() {
        let adf = json!({
            "type": "doc", "version": 1,
            "content": [
                { "type": "paragraph", "content": [
                    { "type": "text", "text": "Hello " },
                    { "type": "text", "text": "world" }
                ]},
                { "type": "heading", "content": [{ "type": "text", "text": "Section" }] },
                { "type": "bulletList", "content": [
                    { "type": "listItem", "content": [
                        { "type": "paragraph", "content": [{ "type": "text", "text": "item" }] }
                    ]}
                ]}
            ]
        });
        let text = adf_to_text(&adf);
        assert!(text.contains("Hello world"), "{text:?}");
        assert!(text.contains("Section"), "{text:?}");
        assert!(text.contains("item"), "{text:?}");
    }

    #[test]
    fn text_to_adf_round_trips_through_flatten() {
        let adf = text_to_adf("line one\nline two");
        assert_eq!(adf["type"], "doc");
        let text = adf_to_text(&adf);
        assert_eq!(text, "line one\nline two");
    }

    #[test]
    fn empty_text_makes_empty_paragraph() {
        let adf = text_to_adf("");
        assert_eq!(adf["content"].as_array().unwrap().len(), 1);
        assert_eq!(adf_to_text(&adf), "");
    }

    #[test]
    fn parses_jira_datetime() {
        let dt = parse_jira_dt("2026-01-02T03:04:05.678+0000").unwrap();
        assert_eq!(dt.to_rfc3339(), "2026-01-02T03:04:05.678+00:00");
        assert!(parse_jira_dt("nonsense").is_none());
    }

    #[test]
    fn key_number_suffix() {
        assert_eq!(key_to_number("PROJ-123"), 123);
        assert_eq!(key_to_number("ABC-1"), 1);
        assert_eq!(key_to_number("NODASH"), 0);
        assert_eq!(key_to_number("PROJ-"), 0);
    }
}
