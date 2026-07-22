pub mod auth;
pub mod client;

pub use client::Client;

/// Prefix marking a synthetic `priority:*` label id. Linear priority is a
/// native field, not a label, but the app's priority picker and new-issue
/// form work in terms of `priority:*` labels — so the Linear provider
/// fabricates label entries whose ids carry this prefix. These ids are never
/// sent to Linear: the create/update paths recognise the prefix and route the
/// value to the native priority field instead.
pub const SYNTHETIC_PRIORITY_PREFIX: &str = "linear-priority:";

/// The four synthetic `priority:*` labels, ordered urgent → low, as
/// `(id, name)` pairs. Injected into `repo_labels` and `repo_form_options` so
/// the picker and form see them.
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

/// If `id` is a synthetic priority-label id, return its Linear native
/// priority integer; otherwise `None` (a real Linear label id).
pub fn synthetic_priority_id_to_int(id: &str) -> Option<u8> {
    id.strip_prefix(SYNTHETIC_PRIORITY_PREFIX)
        .and_then(priority_value_to_int)
}

/// Linear's native priority integer → the `priority:<value>` label value the
/// rest of the app understands. Linear uses `0 = none, 1 = urgent, 2 = high,
/// 3 = medium, 4 = low` (note: not the same ordering as the app's sort rank).
pub fn priority_int_to_value(priority: u8) -> Option<&'static str> {
    match priority {
        1 => Some("urgent"),
        2 => Some("high"),
        3 => Some("medium"),
        4 => Some("low"),
        _ => None,
    }
}

/// Inverse of [`priority_int_to_value`] — a `priority:<value>` label value back
/// to Linear's native integer. `None` for unknown values (leaves priority unset).
pub fn priority_value_to_int(value: &str) -> Option<u8> {
    match value.to_ascii_lowercase().as_str() {
        "urgent" => Some(1),
        "high" => Some(2),
        "medium" => Some(3),
        "low" => Some(4),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_round_trips() {
        for (int, value) in [(1, "urgent"), (2, "high"), (3, "medium"), (4, "low")] {
            assert_eq!(priority_int_to_value(int), Some(value));
            assert_eq!(priority_value_to_int(value), Some(int));
        }
    }

    #[test]
    fn priority_none_and_unknown() {
        assert_eq!(priority_int_to_value(0), None);
        assert_eq!(priority_int_to_value(9), None);
        assert_eq!(priority_value_to_int("p1"), None);
        assert_eq!(priority_value_to_int("URGENT"), Some(1));
    }

    #[test]
    fn synthetic_priority_labels_are_well_formed() {
        let labels = synthetic_priority_labels();
        assert_eq!(labels.len(), 4);
        assert_eq!(
            labels[0],
            ("linear-priority:urgent".into(), "priority:urgent".into())
        );
        // Every synthetic id maps back to a native int; real ids do not.
        for (id, _name) in &labels {
            assert!(synthetic_priority_id_to_int(id).is_some(), "{id}");
        }
        assert_eq!(
            synthetic_priority_id_to_int("linear-priority:urgent"),
            Some(1)
        );
        assert_eq!(synthetic_priority_id_to_int("real-label-id"), None);
    }
}
