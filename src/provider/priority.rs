//! Synthetic `priority:*` labels for backends whose priority is a native
//! field rather than a label.
//!
//! The app's priority picker, sort, colouring and new-issue form all work in
//! terms of `priority:<value>` labels. Backends that model priority natively
//! (Linear, Jira) fabricate label entries whose ids carry a per-backend
//! prefix, so nothing upstream needs to know the difference. Those ids never
//! reach the backend: the create and update paths recognise the prefix and
//! route the value to the native field instead.
//!
//! Only the prefix and the native representation differ per backend, so the
//! prefix is a parameter here and the value mapping stays with the backend.

/// The four priority values, ordered urgent → low.
pub const PRIORITY_VALUES: [&str; 4] = ["urgent", "high", "medium", "low"];

/// The synthetic `(id, name)` label pairs for `prefix`, ordered urgent → low.
/// Injected into a backend's label list so the picker and form see them.
pub fn synthetic_priority_labels(prefix: &str) -> Vec<(String, String)> {
    PRIORITY_VALUES
        .iter()
        .map(|v| (format!("{prefix}{v}"), format!("priority:{v}")))
        .collect()
}

/// The priority value behind a synthetic label id, or `None` when `id` is a
/// real backend label id.
pub fn strip_synthetic_prefix<'a>(prefix: &str, id: &'a str) -> Option<&'a str> {
    id.strip_prefix(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_ordered_and_well_formed() {
        let labels = synthetic_priority_labels("test-priority:");
        assert_eq!(labels.len(), 4);
        assert_eq!(
            labels[0],
            ("test-priority:urgent".into(), "priority:urgent".into())
        );
        assert_eq!(
            labels[3],
            ("test-priority:low".into(), "priority:low".into())
        );
    }

    #[test]
    fn every_synthetic_id_strips_back_to_its_value() {
        for (id, name) in synthetic_priority_labels("x:") {
            let value = strip_synthetic_prefix("x:", &id).expect("synthetic id should strip");
            assert_eq!(format!("priority:{value}"), name);
        }
    }

    #[test]
    fn real_ids_and_foreign_prefixes_do_not_strip() {
        assert_eq!(strip_synthetic_prefix("x:", "real-label-id"), None);
        // A different backend's prefix must not match.
        assert_eq!(
            strip_synthetic_prefix("jira-priority:", "linear-priority:high"),
            None
        );
    }
}
